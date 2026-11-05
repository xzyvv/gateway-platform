//! MqttClient 连接初始化与生命周期（从 client.rs 拆出）
//!
//! 包含：构造函数 new、订阅、incoming handler 注入、启动后台 task、停止。
//! 事件循环主体见 `event_loop::run_event_loop`。

use super::*;
use crate::north::mqtt::event_loop::{run_event_loop, EventLoopInputs};
use crate::north::mqtt::outbox::OutboxStore;

impl MqttClient {
    /// 创建 MQTT 客户端
    pub fn new(
        config: MqttConfig,
        queue: Arc<DataQueue>,
        outbox_path: Option<&str>,
        client_cert: Option<CertPair>,
        metrics: Arc<crate::engine::metrics::Metrics>,
    ) -> GatewayResult<Self> {
        // client_id 必须全局唯一：hostname + pid + 时间戳（毫秒）
        let pid = std::process::id();
        let hostname = std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("COMPUTERNAME"))
            .unwrap_or_else(|_| "unknown".to_string());
        let client_id = format!(
            "gateway_{}_{}_{}",
            hostname,
            pid,
            chrono::Utc::now().timestamp_millis()
        );

        let (host, port, scheme) = parse_broker(&config.broker)?;
        let mut options = MqttOptions::new(client_id, host, port);

        match scheme {
            // TLS 与 WSS 共用同一套证书材料：自定义 CA + 系统 CA 合并，
            // 有客户端证书即启用 mTLS。
            //
            // 旧实现 `Wss` 直接走 `wss_with_default_config()`，导致 wss:// 下
            // 配置的 `ca_cert_path` 与客户端证书被静默忽略——私有 CA 场景连不上，
            // 且日志看起来「配置已生效」，属于配置撒谎。现改为显式构造
            // `TlsConfiguration::Simple` 后交给 `wss_with_config`。
            BrokerScheme::Tls | BrokerScheme::Wss => {
                let custom_ca = config.tls.as_ref().and_then(|t| t.ca_cert_path.as_deref());
                let has_client_cert = client_cert.is_some();
                // 什么都没配时保留原生默认信任库，避免把系统根证书重编码成 PEM
                // 反而引入兼容性风险（wss 的既有行为也维持不变）。
                if matches!(scheme, BrokerScheme::Wss) && custom_ca.is_none() && !has_client_cert {
                    options.set_transport(Transport::wss_with_default_config());
                    tracing::info!(
                        "mqtt: wss enabled with system CA only (no custom CA configured)"
                    );
                } else {
                    use rumqttc::TlsConfiguration;
                    let mut ca_pem = load_custom_ca_pem(custom_ca);
                    ca_pem.extend_from_slice(&load_native_ca_pem());
                    let client_auth = client_cert.as_ref().map(|c| {
                        (
                            c.cert_pem.as_bytes().to_vec(),
                            c.key_pem.as_bytes().to_vec(),
                        )
                    });
                    let tls_config = TlsConfiguration::Simple {
                        ca: ca_pem,
                        alpn: None,
                        client_auth,
                    };
                    if matches!(scheme, BrokerScheme::Wss) {
                        options.set_transport(Transport::wss_with_config(tls_config));
                    } else {
                        options.set_transport(Transport::Tls(tls_config));
                    }
                    let kind = if matches!(scheme, BrokerScheme::Wss) {
                        "wss"
                    } else {
                        "tls"
                    };
                    tracing::info!(
                        "mqtt: {} enabled (client_cert={}, custom_ca={:?}, system CA merged)",
                        kind,
                        has_client_cert,
                        custom_ca
                    );
                }
            }
            BrokerScheme::Ws => {
                options.set_transport(Transport::ws());
            }
            BrokerScheme::Tcp => {}
        }

        // B11: keep_alive 范围校验。MqttConfig 的 keep_alive 是 Option<u64>，
        // 缺失/为 0/超过 1 小时 / 小于 5 秒 都不合理（broker 频繁断线 / 长时间无心跳假在线）。
        // 这里在使用点 clamp 到合法范围 [5, 3600]，避免修改 schema 触发迁移。
        let keep_alive = match config.keep_alive {
            Some(v) if (5..=3600).contains(&v) => v,
            Some(v) => {
                tracing::warn!(
                    "mqtt keep_alive={} out of range [5, 3600], fallback to 30s",
                    v
                );
                30
            }
            None => 30,
        };
        options.set_keep_alive(Duration::from_secs(keep_alive));
        options.set_clean_session(true);
        options.set_max_packet_size(256 * 1024, 256 * 1024);

        // #5: MQTT broker 鉴权（用户名/密码）。配置中可密文落盘，启动前已由
        // decrypt_config_secrets 解密为明文。两者都给定时才设置凭据。
        if let (Some(u), Some(p)) = (&config.username, &config.password) {
            if !u.is_empty() {
                options.set_credentials(u.clone(), p.clone());
            }
        }

        let (client, eventloop) = AsyncClient::new(options, 50);

        let shutdown = Arc::new(Notify::new());
        // r48-MQTT1：关停标志（与 shutdown Notify 配合，消除 notify_waiters 的
        // 丢失唤醒竞态，见 mod.rs::wait_shutdown）。
        let stopping = Arc::new(std::sync::atomic::AtomicBool::new(false));
        // P1-4：pending_ack 元素为 PendingAckSlot（ack_id + data_id + sender），
        // 超时/失败路径按 ack_id 定位移除，杜绝盲 pop_front 误删其它批次槽位。
        let pending_ack: PendingAckQueue = Arc::new(parking_lot::Mutex::new(VecDeque::new()));
        let orphan_acks = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let ack_id_gen = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let incoming_tx: IncomingTxChannel = Arc::new(parking_lot::Mutex::new(None));
        // f5: 期望订阅表，供事件循环重连后重新订阅（clean_session=true 会清空订阅）
        let desired_subscriptions: Arc<parking_lot::Mutex<Vec<String>>> =
            Arc::new(parking_lot::Mutex::new(Vec::new()));
        // Bug 54: connected 用于在事件循环中维护连接状态
        let connected = Arc::new(std::sync::atomic::AtomicBool::new(false));

        // outbox 必须在事件循环前创建，以便 ConnAck 时触发 replay
        let outbox = match outbox_path {
            Some(p) => match OutboxStore::open(std::path::Path::new(p)) {
                Ok(store) => Some(Arc::new(AsyncMutex::new(store))),
                Err(e) => {
                    tracing::warn!("outbox init failed, continuing without outbox: {}", e);
                    None
                }
            },
            None => None,
        };
        let replay_in_progress = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let publish_semaphore = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_PUBLISH));

        // 启动事件循环（实现见 event_loop::run_event_loop）
        let eventloop_handle = crate::engine::supervision::spawn_catch_unwind(
            "mqtt_eventloop",
            run_event_loop(EventLoopInputs {
                eventloop,
                shutdown: shutdown.clone(),
                stopping: stopping.clone(),
                pending_ack: pending_ack.clone(),
                incoming_tx: incoming_tx.clone(),
                desired_subscriptions: desired_subscriptions.clone(),
                connected: connected.clone(),
                outbox: outbox.clone(),
                replay_in_progress: replay_in_progress.clone(),
                orphan_acks: orphan_acks.clone(),
                ack_id_gen: ack_id_gen.clone(),
                publish_semaphore: publish_semaphore.clone(),
                queue: queue.clone(),
                config: config.clone(),
                client: client.clone(),
                metrics: metrics.clone(),
            }),
        );

        Ok(Self {
            client,
            queue,
            config,
            shutdown,
            stopping,
            pending_ack,
            orphan_acks,
            ack_id_gen,
            eventloop_handle: parking_lot::Mutex::new(Some(eventloop_handle)),
            publish_handle: parking_lot::Mutex::new(None),
            // Bug 54: connected 字段包装为 Arc，让事件循环和外部共享同一 AtomicBool
            connected,
            rate_tokens: std::sync::atomic::AtomicU64::new(0),
            rate_last_refill: std::sync::atomic::AtomicU64::new(0),
            outbox,
            circuit_failures: std::sync::atomic::AtomicU32::new(0),
            circuit_open: std::sync::atomic::AtomicBool::new(false),
            circuit_opened_at: std::sync::atomic::AtomicU64::new(0),
            publish_semaphore,
            outbox_cmd_tx: arc_swap::ArcSwapOption::new(None),
            outbox_writer_handle: parking_lot::Mutex::new(None),
            incoming_tx,
            desired_subscriptions,
            // 模板编译：延迟到首次使用（payload_template 是 Option<String>，可能运行时热更新）
            payload_template_engine: once_cell::sync::OnceCell::new(),
            point_configs: Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new())),
            // R3: 模板未解析变量只在首次渲染时告警一次，避免每个发布周期刷屏
            unresolved_template_warned: std::sync::atomic::AtomicBool::new(false),
            // 批次 B：转换引擎由 bootstrap 在 ConvertEngine 创建后经 set_convert_engine 注入
            convert_engine: arc_swap::ArcSwapOption::new(None),
            metrics,
        })
    }

    /// 注入协议转换引擎（批次 B：enum 标签回查）。
    ///
    /// payload 构建期对每条 QueuedItem 调 `enum_label_for(point_id, raw_value)`
    /// 补回 value_text；引擎为共享 Arc，热重载后读当前规则集。
    pub fn set_convert_engine(&self, engine: Arc<crate::pipeline::ConvertEngine>) {
        self.convert_engine.store(Some(engine));
    }

    /// 订阅一个 MQTT topic（用于透传模式接收下行指令）
    pub async fn subscribe(&self, topic: &str) -> GatewayResult<()> {
        // f5: 记录期望订阅，供事件循环在重连（ConnAck 成功）后重新订阅
        {
            let mut subs = self.desired_subscriptions.lock();
            if !subs.iter().any(|t| t == topic) {
                subs.push(topic.to_string());
            }
        }
        self.client
            .subscribe(topic, QoS::AtLeastOnce)
            .await
            .map_err(|e| {
                crate::engine::error::GatewayError::Mqtt(format!(
                    "mqtt subscribe {} failed: {}",
                    topic, e
                ))
            })
    }

    /// 设置订阅消息处理器：收到 Publish 消息时通过此通道转发
    ///
    /// r30c-PS6 (修复)：旧实现无任何提示地覆盖前一个 handler。`MqttClient` 同一
    /// 时刻只允许一个 incoming handler，最后设置的胜出——如果 passthrough 与其他
    /// 模块（如 south 反向）先后调用，前者收消息的能力被静默废弃且无任何告警。
    /// 现版本：覆盖前先 `tracing::warn!` 记录前一个 handler 被替换的事实。
    pub fn set_incoming_handler(&self, tx: mpsc::Sender<(String, Vec<u8>)>) {
        let mut slot = self.incoming_tx.lock();
        if slot.is_some() {
            tracing::warn!(
                "MqttClient::set_incoming_handler: replacing an existing incoming handler. \
                 The previous subscriber (e.g. passthrough) will silently stop receiving \
                 inbound messages. Ensure only one consumer is registered per MqttClient."
            );
        }
        *slot = Some(tx);
    }

    /// 启动上传循环
    pub fn start(self: &Arc<Self>) {
        let client = self.clone();
        let handle = crate::engine::supervision::spawn_supervised("mqtt_publish", move || {
            let client = client.clone();
            async move {
                client.publish_loop().await;
            }
        });
        *self.publish_handle.lock() = Some(handle);

        // P3: 启动 outbox 异步写入器（若 outbox 已配置）
        if let Some(outbox) = &self.outbox {
            let (tx, rx) = mpsc::channel::<OutboxCmd>(256);
            self.outbox_cmd_tx.store(Some(Arc::new(tx)));
            let outbox = outbox.clone();
            let handle =
                crate::engine::supervision::spawn_catch_unwind("outbox_writer", async move {
                    outbox_writer_loop(outbox, rx).await;
                });
            *self.outbox_writer_handle.lock() = Some(handle);
        }
    }

    /// 停止客户端并等待后台 task 退出（最多 5 秒）
    pub async fn stop(&self) {
        // P3: 先关闭 outbox 写入器通道，触发 flush 并退出
        self.outbox_cmd_tx.store(None);
        // r48-MQTT1：先置位关停标志再唤醒。若此刻事件循环正阻塞在 poll()、
        // 发布循环正阻塞在 publish_batch()（PubAck 超时最长 10s），
        // notify_waiters() 会丢失通知 → 两任务漏停、stop() 等满 5s 后放弃。
        // 置位标志后，两个 waiter 无论何时注册都能经 wait_shutdown 复查到并退出。
        self.stopping
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.shutdown.notify_waiters();
        let eventloop_handle = self.eventloop_handle.lock().take();
        let publish_handle = self.publish_handle.lock().take();
        let outbox_handle = self.outbox_writer_handle.lock().take();
        let wait_one = |h: Option<JoinHandle<()>>| async move {
            if let Some(h) = h {
                let _ = tokio::time::timeout(Duration::from_secs(5), h).await;
            }
        };
        futures::future::join3(
            wait_one(eventloop_handle),
            wait_one(publish_handle),
            wait_one(outbox_handle),
        )
        .await;
    }
}

/// S4: 加载系统 CA 证书为 PEM 字节，供 mTLS 的 TlsConfiguration::Simple.ca 使用。
///
/// 原实现 ca: Vec::new() 会导致 rustls root store 为空，服务器证书验证全部失败。
/// 本函数从系统信任库（Windows: 证书存储；Linux: /etc/ssl/certs）加载 CA，
/// 用 pem crate 转为 PEM 格式后拼接为单一字节向量。
fn load_native_ca_pem() -> Vec<u8> {
    let mut pem_bytes = Vec::new();
    match rustls_native_certs::load_native_certs() {
        Ok(certs) => {
            let cert_count = certs.len();
            for cert in certs {
                // cert 是 rustls::Certificate(Vec<u8>)，.0 取 DER 字节
                let pem_block = pem::Pem::new("CERTIFICATE", cert.0);
                let pem_str = pem::encode(&pem_block);
                pem_bytes.extend_from_slice(pem_str.as_bytes());
            }
            tracing::debug!("loaded {} native CA certs for mTLS", cert_count);
        }
        Err(e) => {
            tracing::warn!(
                "failed to load native CA certs: {}, mTLS server verification may fail",
                e
            );
        }
    }
    pem_bytes
}

/// Bug 2.4: 加载自定义 CA 证书（PEM 格式）
///
/// 读取路径指定的文件并原样返回字节。多个证书可序连接在同一 PEM 文件中。
/// 失败旵 warn 日志不中断连接（附加系统 CA 依然可用）。
fn load_custom_ca_pem(ca_path: Option<&str>) -> Vec<u8> {
    let mut pem_bytes = Vec::new();
    let Some(path) = ca_path else {
        return pem_bytes;
    };
    match std::fs::read(path) {
        Ok(bytes) => {
            // 序连接 PEM 内容：rumqttc::TlsConfiguration::Simple.ca 期望是 PEM 字节。
            // 不区分 PEM/DER：用户提供 PEM 文件，直接原样传递；
            // 若供应是 DER，补充一个 BEGIN/END 包裹中后调用方报错。该路径限 PEM，避免隐藏识别 bug。
            pem_bytes.extend_from_slice(&bytes);
            tracing::info!(
                "loaded custom CA bundle from {} ({} bytes)",
                path,
                bytes.len()
            );
        }
        Err(e) => {
            tracing::warn!(
                "failed to read custom CA bundle from {}: {} — falling back to system CA only",
                path,
                e
            );
        }
    }
    pem_bytes
}
