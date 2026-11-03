//! MQTT 事件循环主体（从 client.rs 抽出）
//!
//! 处理 ConnAck / PubAck / Disconnect / Publish / 错误重连等事件。
//! 通过 Arc 共享状态字段给外部 publish 路径和健康检查。
//!
//! 同时承载与事件循环紧密耦合的发布 / outbox 周期方法：
//! publish_loop、replay_outbox、cleanup_outbox、publish_raw、publish_now。

use super::*;
use crate::config::ReconnectBackoffConfig;

/// 入站下行报文发送端槽位：Some = 订阅者已挂接可投递，None = 尚未挂接
type IncomingTxSlot = Option<mpsc::Sender<(String, Vec<u8>)>>;

/// `run_event_loop` 的入参集合：所有共享状态通过 Arc 传入，装配点唯一，
/// 用结构体承载以避免超长参数列表（clippy::too_many_arguments）。
pub(super) struct EventLoopInputs {
    pub(super) eventloop: EventLoop,
    pub(super) shutdown: Arc<Notify>,
    /// r48-MQTT1：关停标志（与 shutdown 配合，见 mod.rs::wait_shutdown）
    pub(super) stopping: Arc<std::sync::atomic::AtomicBool>,
    pub(super) pending_ack: PendingAckQueue,
    pub(super) incoming_tx: Arc<parking_lot::Mutex<IncomingTxSlot>>,
    /// f5: 期望订阅表，ConnAck 成功后据此重新订阅（clean_session=true 会清空订阅）
    pub(super) desired_subscriptions: Arc<parking_lot::Mutex<Vec<String>>>,
    pub(super) connected: Arc<std::sync::atomic::AtomicBool>,
    pub(super) outbox: Option<Arc<AsyncMutex<OutboxStore>>>,
    pub(super) replay_in_progress: Arc<std::sync::atomic::AtomicBool>,
    /// P1-4：孤儿 PubAck 计数（与 MqttClient 字段共享同一原子）
    pub(super) orphan_acks: Arc<std::sync::atomic::AtomicU64>,
    /// P1-4：槽位 ack_id 生成器（replay 路径共用）
    pub(super) ack_id_gen: Arc<std::sync::atomic::AtomicU64>,
    /// P1-4：publish 串行信号量（replay 路径同样遵守）
    pub(super) publish_semaphore: Arc<tokio::sync::Semaphore>,
    /// P1-5：数据队列句柄（replay 成功路径需同步 ack 队列行，防双通道重复投递）
    pub(super) queue: Arc<DataQueue>,
    pub(super) config: MqttConfig,
    pub(super) client: AsyncClient,
    /// R2：指标实例，用于统计 ConnAck 被拒次数（与 MqttClient 共享同一 Arc）
    pub(super) metrics: Arc<crate::engine::metrics::Metrics>,
}

/// 运行 MQTT 事件循环（ConnAck / PubAck / Disconnect / Publish / 错误重连）
///
/// 在 `MqttClient::new` 中由 `spawn_catch_unwind("mqtt_eventloop", ...)` 启动。
/// 所有共享状态通过 Arc 传入，与 publish 路径 / 健康检查共享同一份原子。
pub(super) async fn run_event_loop(inputs: EventLoopInputs) {
    let EventLoopInputs {
        mut eventloop,
        shutdown,
        stopping,
        pending_ack,
        incoming_tx,
        desired_subscriptions,
        connected,
        outbox,
        replay_in_progress,
        orphan_acks,
        ack_id_gen,
        publish_semaphore,
        queue,
        config,
        client,
        metrics,
    } = inputs;
    let mut reconnect_failures: u32 = 0;
    loop {
        tokio::select! {
            _ = wait_shutdown(&shutdown, &stopping) => {
                break;
            }
            event = eventloop.poll() => {
                match event {
                    Ok(Event::Incoming(Incoming::ConnAck(ack))) => {
                        // S3: 校验 ConnAck code，鉴权失败/服务不可用时不能标记 connected
                        use rumqttc::ConnectReturnCode;
                        if ack.code != ConnectReturnCode::Success {
                            // r46-MQTT1：与 Disconnect / poll 出错统一收敛到 mark_disconnected。
                            // 旧实现只手写 connected=false，不清理 pending 槽位与 orphan 基线，
                            // 导致故障尝试期间登记/超时的槽位留下陈旧 orphan 计数（故障期没有
                            // Disconnect 事件来清零），重连成功后每个真实 PubAck 都会被误当作
                            // 孤儿消化 → 头部槽位永不确认 → 每条消息永久重复投递。
                            mark_disconnected(&pending_ack, &orphan_acks, connected.as_ref());
                            // R2: ConnAck 被拒（鉴权失败/服务不可用/标识被拒）单独计数 +
                            // error 级日志，与普通断网重连区分，便于运维告警与排障。
                            metrics.inc_mqtt_connack_refused();
                            tracing::error!(
                                "mqtt connack refused: code={:?}（鉴权失败/权限不足/服务不可用，请检查 broker 凭据与配置）",
                                ack.code
                            );
                            // 不重放 outbox，应用退避。r45-MQTT1：与普通重连失败分支
                            // 统一走 reconnect_delay_secs（叠加配置的 jitter_ratio，MQTT
                            // 默认 0.4）；旧实现单独用无抖动的 base_delay_secs，忽略运维
                            // 配置，多网关对同一 broker 同步重试会形成惊群。
                            reconnect_failures = reconnect_failures.saturating_add(1);
                            let delay = reconnect_delay_secs(
                                &config.reconnect_backoff,
                                reconnect_failures,
                                ReconnectBackoffConfig::time_rand01(),
                            );
                            tokio::select! {
                                _ = tokio::time::sleep(Duration::from_secs_f64(delay)) => {}
                                _ = wait_shutdown(&shutdown, &stopping) => { return; }
                            }
                            continue;
                        }
                        reconnect_failures = 0; // F1: 连接成功，重置退避
                        // Bug 54: 更新 connected 状态供健康检查 / publish_raw 使用。
                        // r46-MQTT1：clean_session 新建会话，须一并重置孤儿对账基线，
                        // 否则故障期遗留的陈旧 orphan 会让重连后所有 PubAck 被误消化
                        // （头部槽位永不确认 → 永久重复投递）。见 mark_connected_fresh_session。
                        mark_connected_fresh_session(&pending_ack, &orphan_acks, connected.as_ref());
                        tracing::info!("mqtt connected");
                        // f5 修复：clean_session=true 时 broker 在每次重连后清空该
                        // client 的订阅关系，rumqttc 也不会自动重订阅。若不在此重新
                        // 订阅，透传等下行通道会一次断线后永久静默失效（表现为
                        // "手命令下发没有任何反应"）。先取出快照再逐条订阅，避免跨
                        // await 持锁。
                        let resub: Vec<String> = desired_subscriptions.lock().clone();
                        for topic in resub {
                            match client.subscribe(&topic, QoS::AtLeastOnce).await {
                                Ok(()) => {
                                    tracing::info!("mqtt re-subscribed {} after reconnect", topic)
                                }
                                Err(e) => tracing::warn!(
                                    "mqtt re-subscribe {} after reconnect failed: {}",
                                    topic,
                                    e
                                ),
                            }
                        }
                        if let Some(o) = outbox.clone() {
                            let pa = pending_ack.clone();
                            let rp = replay_in_progress.clone();
                            let oa = orphan_acks.clone();
                            let ag = ack_id_gen.clone();
                            let ps = publish_semaphore.clone();
                            let q = queue.clone();
                            let cfg = config.clone();
                            let c = client.clone();
                            crate::engine::supervision::spawn_catch_unwind("mqtt_replay_outbox", async move {
                                replay_pending_outbox(ReplayHandles {
                                    outbox: &o,
                                    client: &c,
                                    pending_ack: &pa,
                                    orphan_acks: &oa,
                                    ack_id_gen: &ag,
                                    publish_semaphore: &ps,
                                    queue: &q,
                                    replay_in_progress: rp.as_ref(),
                                }, cfg.ack_timeout_secs).await;
                            });
                        }
                    }
                    Ok(Event::Incoming(Incoming::PubAck(_))) => {
                        // P1-4 修复：reconcile_puback 先对账孤儿计数——超时/
                        // 失败路径按 ack_id 移除槽位时递增 orphan_acks（publish
                        // 可能已发出、PubAck 迟到）。orphan>0 时本次 PubAck 属于
                        // 已被移除的槽位：减一并跳过 pop_front，防止偷走 FIFO
                        // 头部（后续批次的真实 ack 槽位）造成假确认 → 丢数据。
                        match reconcile_puback(&pending_ack, &orphan_acks) {
                            Some(slot) => {
                                tracing::trace!(data_id = slot.data_id, "mqtt PubAck received");
                                let _ = slot.tx.send(());
                            }
                            None => {
                                tracing::debug!("mqtt PubAck consumed as orphan (late ack)");
                            }
                        }
                    }
                    Ok(Event::Incoming(Incoming::Disconnect)) => {
                        // Bug 54: 断连时立即更新 connected 状态，
                        // 防止 publish_raw / publish_bypass_queue 在重连窗口期内误判。
                        // P0-2: clean_session=true 下断线时 rumqttc 丢弃 inflight，
                        // broker 端 PubAck 不再到达；清空 pending_ack 防僵尸槽位
                        // 无界增长 + FIFO 头错位导致重连后重复投递。真实未送达
                        // 数据由 outbox 持久化，ConnAck 成功路径会 replay。
                        // r41-MQTT1：与 Err 分支统一收敛到 mark_disconnected，
                        // 避免"置 connected=false + 清槽"两处重复实现再次漂移。
                        let dropped =
                            mark_disconnected(&pending_ack, &orphan_acks, connected.as_ref());
                        if dropped > 0 {
                            tracing::warn!(
                                "mqtt disconnected: cleared {} pending ack slots (will replay outbox on reconnect)",
                                dropped
                            );
                        } else {
                            tracing::warn!("mqtt disconnected");
                        }
                    }
                    Ok(Event::Incoming(Incoming::Publish(p))) => {
                        // 透传模式：把订阅到的下行指令转发给 incoming handler
                        // 先 clone 出 tx，避免 MutexGuard 跨 await 持有
                        let tx_opt = incoming_tx.lock().clone();
                        if let Some(tx) = tx_opt {
                            let topic = p.topic.clone();
                            let payload = p.payload.to_vec();
                            if tx.send((topic, payload)).await.is_err() {
                                tracing::debug!("mqtt incoming handler closed");
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        // F1: 指数退避，避免 broker 不可用时每秒重连
                        reconnect_failures = reconnect_failures.saturating_add(1);
                        // S5: 加抖动（默认 ±20%），避免多台网关同时断网后同步重连导致 broker 雪崩
                        let delay_secs = reconnect_delay_secs(
                            &config.reconnect_backoff,
                            reconnect_failures,
                            ReconnectBackoffConfig::time_rand01(),
                        );
                        // P0-2: 事件循环出错通常意味着底层连接已不可用，pending
                        // 槽位与 Disconnect 同样会变成僵尸。清空 + 清零 orphan。
                        // r41-MQTT1：网络掉线走的是本分支（未必有 Disconnect 事件），
                        // 必须同样置 connected=false，否则整段重连期间 is_connected()
                        // 持续误报在线。统一收敛到 mark_disconnected。
                        let dropped =
                            mark_disconnected(&pending_ack, &orphan_acks, connected.as_ref());
                        if dropped > 0 {
                            tracing::warn!(
                                "mqtt eventloop error: {}, cleared {} pending slots; retrying in {:.1}s",
                                e, dropped, delay_secs
                            );
                        } else {
                            tracing::warn!(
                                "mqtt eventloop error: {}, retrying in {:.1}s",
                                e, delay_secs
                            );
                        }
                        // Bug 33: 退避期间也监听 shutdown，避免停机延迟最长 60s
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_secs_f64(delay_secs)) => {}
                            _ = wait_shutdown(&shutdown, &stopping) => { return; }
                        }
                    }
                }
            }
        }
    }
    tracing::info!("mqtt eventloop stopped");
}

impl MqttClient {
    /// 发布循环
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub(crate) async fn publish_loop(&self) {
        let interval = Duration::from_secs(self.config.publish_interval.max(1));
        let mut cleanup_counter: u32 = 0;
        // W3: MQTT 发布循环心跳
        let hb = crate::engine::monitor::heartbeat::get()
            .register("mqtt-publish", 60_000)
            .await;
        loop {
            hb.kick();
            tokio::select! {
                _ = wait_shutdown(&self.shutdown, &self.stopping) => {
                    break;
                }
                _ = tokio::time::sleep(interval) => {
                    self.publish_batch().await;
                    // 每 10 个发布周期清理一次已确认的 outbox 记录（>1小时）
                    cleanup_counter += 1;
                    if cleanup_counter >= 10 {
                        cleanup_counter = 0;
                        self.cleanup_outbox(3_600_000).await;
                        // F2: 限制未 ACK 消息数量，防止断网期间 outbox 无限增长
                        // Bug 169: COUNT+DELETE 是同步 SQLite 操作，直接在 tokio worker 上
                        // 执行会阻塞该 worker（与上方 A3 的 cleanup_outbox 处理方式对齐，
                        // 改走 spawn_blocking）。try_lock 拿不到锁说明另有任务在写，跳过即可。
                        // r71-OBX1：传 `Some(60_000)` 1 分钟宽限窗口——broker 已收到但
                        // PubAck 在途的行（`last_attempt_at` 较新）暂时保留，避免被立即
                        // 删后 reconcile 永久 orphan。60s 是 ack_timeout_secs=10s 的 6 倍
                        // 余量，足够覆盖网络最坏抖动。
                        if let Some(outbox) = &self.outbox {
                            let outbox_clone = outbox.clone();
                            let joined = tokio::task::spawn_blocking(move || {
                                if let Ok(mut store) = outbox_clone.try_lock() {
                                    let _ = store.enforce_pending_limit("mqtt", 100_000, Some(60_000));
                                }
                            })
                            .await;
                            if let Err(e) = joined {
                                tracing::warn!("mqtt: enforce_pending_limit task failed: {}", e);
                            }
                        }
                    }
                }
            }
        }
        tracing::info!("mqtt publish loop stopped");
    }

    /// T2.2: 清理已确认且过期的 outbox 记录（周期调用）
    pub async fn cleanup_outbox(&self, older_than_ms: i64) {
        if let Some(outbox) = &self.outbox {
            // A3: cleanup_acked 放到 spawn_blocking，避免大量 DELETE 阻塞 tokio runtime
            let outbox_clone = outbox.clone();
            let join_handle = tokio::task::spawn_blocking(move || {
                outbox_clone.blocking_lock().cleanup_acked(older_than_ms)
            });
            match join_handle.await {
                Ok(Ok(n)) if n > 0 => tracing::info!("outbox cleaned {} acked records", n),
                Ok(Ok(_)) => {}
                Ok(Err(e)) => tracing::warn!("outbox cleanup failed: {}", e),
                Err(e) => tracing::error!("outbox cleanup spawn_blocking join failed: {}", e),
            }
        }
    }

    /// 直接发布原始消息到指定 topic（供报警引擎等非数据流场景使用）
    ///
    /// QoS=AtLeastOnce，不保留。失败返回 false，不重试（报警允许偶尔丢失）。
    ///
    /// tracing: 原始消息发布 span（topic 通过 Debug 自动记录），tracing feature off 时 cfg_attr 剥离为零成本。
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, payload), level = "debug")
    )]
    pub async fn publish_raw(&self, topic: &str, payload: Vec<u8>) -> bool {
        if !self.is_connected() {
            tracing::debug!("mqtt publish_raw skipped: not connected");
            return false;
        }
        // Bug 132: 检查断路器——熔断期间拒绝发布，避免 burst 流量绕过
        // publish_batch 的冷却保护。原实现 publish_raw 总是尝试发布，
        // 与 publish_batch 行为不一致（后者有断路器检查）。
        if self.is_circuit_open() {
            tracing::debug!("mqtt publish_raw skipped: circuit open");
            return false;
        }
        // P1-4 修复：publish_raw 也走 publish_semaphore + 登记 pending_ack 槽位。
        // 原实现绕过登记，其 PubAck 到达时会被事件循环 pop_front 偷走——
        // 误 signal FIFO 头部（其它批次的真实 ack 槽位），导致 publish_batch
        // 以为数据已确认而从队列删除，造成数据丢失。try_acquire 保持报警
        // 路径非阻塞：常规发布在飞时本次报警直接丢弃（报警语义允许丢失）。
        let _permit = match self.publish_semaphore.try_acquire() {
            Ok(p) => p,
            Err(_) => {
                tracing::debug!("mqtt publish_raw skipped: publish in flight");
                return false;
            }
        };
        let ack_id = next_ack_id(&self.ack_id_gen);
        let (tx, _rx) = oneshot::channel();
        self.pending_ack.lock().push_back(PendingAckSlot {
            ack_id,
            data_id: 0, // 非队列来源
            tx,
        });
        match self
            .client
            .publish(topic, QoS::AtLeastOnce, false, payload)
            .await
        {
            Ok(_) => true,
            Err(e) => {
                // publish 调用本身失败：消息未入 rumqttc 队列，不会有
                // PubAck 到达——只移除槽位，不递增孤儿计数。
                remove_slot_by_ack_id(&self.pending_ack, ack_id);
                // r50-MQTT1 修复：原实现注释承诺"递增失败计数（由 eventloop 统计）"
                // 但代码只打 warn、未调 record_circuit_failure / inc_mqtt_publish(false)。
                // 这会导致：
                //   1) publish_raw 连续失败永不触发断路器——故障 broker 下报警/透传
                //      流量绕开发布冷却保护，反复对故障端点施压；
                //   2) mqtt_publish{ok=false} 指标对 publish_raw 路径全盲，
                //      运维告警 / 仪表盘失真。
                // 修复：与 publish_batch 失败路径（publish.rs:234 / 220）一致，
                // 失败时调 record_circuit_failure + inc_mqtt_publish(false)。
                // 注：reset_circuit_failures 由 publish_batch 半开成功路径管理，
                // 本路径不重置——避免透传侧频繁成功/失败掩盖 publish_batch 失败
                // 累计，与 publish_bypass_queue（publish.rs:396-398）行为一致。
                tracing::warn!("mqtt publish_raw to {} failed: {}", topic, e);
                self.metrics.inc_mqtt_publish(false);
                self.record_circuit_failure();
                false
            }
        }
    }
}

/// 重连退避延迟（秒）——**ConnAck 被拒与普通断线重连共用同一口径**。
///
/// 统一走 [`ReconnectBackoffConfig::delay_secs`]（叠加配置的 `jitter_ratio`，
/// MQTT 默认 0.4），`jitter_ratio=0.0` 时结果仍等于基准，不改变既有重连节奏。
/// 历史缺陷：ConnAck 被拒分支单独用了无抖动的 `base_delay_secs`，把运维配置完全忽略，
/// 多网关对同一 broker 同步重试会形成惊群，违背本项目「退避重试须加随机抖动」的约定。
fn reconnect_delay_secs(cfg: &ReconnectBackoffConfig, attempt: u32, rand01: f64) -> f64 {
    cfg.delay_secs(attempt, rand01)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// r45-MQTT1 回归：退避必须叠加配置的抖动比例（MQTT 默认 0.4）。
    ///
    /// 旧实现（ConnAck 被拒分支用无抖动的 `base_delay_secs`）会忽略这里的
    /// `rand01`，使 lo/hi 恒等于基准；本测试的 `lo < base < hi` 断言即会失败。
    #[test]
    fn reconnect_delay_applies_configured_jitter() {
        let cfg = ReconnectBackoffConfig {
            seconds: vec![60],
            jitter_ratio: 0.4,
        };
        let base = cfg.base_delay_secs(1);
        assert_eq!(base, 60.0);

        let lo = reconnect_delay_secs(&cfg, 1, 0.0); // 下界 = base*(1 - 0.4/2)
        let hi = reconnect_delay_secs(&cfg, 1, 1.0); // 上界 = base*(1 + 0.4/2)

        assert!(lo < base, "rand01=0 应低于基准，抖动未生效: {}", lo);
        assert!(hi > base, "rand01=1 应高于基准，抖动未生效: {}", hi);
        assert!(
            (lo - base * 0.8).abs() < 1e-9 && (hi - base * 1.2).abs() < 1e-6,
            "抖动应落在基准 ±20% 内（lo={} hi={}）",
            lo,
            hi
        );
    }

    /// `jitter_ratio=0.0`（如 Modbus TCP 默认）时退避仍等于基准，保证不改变既有节奏。
    #[test]
    fn reconnect_delay_is_base_without_jitter() {
        let cfg = ReconnectBackoffConfig {
            seconds: vec![2, 4, 8],
            jitter_ratio: 0.0,
        };
        for attempt in 1..=5 {
            assert_eq!(
                reconnect_delay_secs(&cfg, attempt, 0.77),
                cfg.base_delay_secs(attempt),
                "attempt={} 无抖动时应等于基准",
                attempt
            );
        }
    }
}
