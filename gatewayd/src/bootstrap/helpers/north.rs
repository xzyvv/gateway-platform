//! 北向服务辅助函数（Step 8-11：队列/证书/MQTT/Pipeline + 北向 Server 启动）
//!
//! 对应 bootstrap 三段拆分中的 NorthProcessing 阶段，以及 Step 13 中的北向服务启动。

use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::{info, warn};

use gateway_core::config::{GatewayConfig, PointConfig};
use gateway_core::crypto::cert::{CertManager, CertPair};
use gateway_core::engine::supervision::{spawn_catch_unwind, spawn_supervised};
use gateway_core::north::mqtt::MqttClient;
use gateway_core::north::server::Server;
use gateway_core::north::sim_consumer::SimConsumer;
use gateway_core::pipeline::{EdgeProcessor, PipelineSpawnInputs, ProcessingPipeline, RealtimeBus};
use gateway_core::queue::DataQueue;
use gateway_core::support::app::Gateway;

use gateway_core::support::{default_cert_dir, default_db_path, default_outbox_db};

use super::super::{CollectionChain, NorthProcessing};
use super::NorthChannel;

/// 启动所有北向 Server（OPC UA / Modbus / 透传）并返回句柄列表供优雅停机。
///
/// Server 启动通过 [`gateway_core::north::server::ServerRegistry`] 插件化管理，
/// 新增北向通道只需在 registry 中注册，无需修改本函数。
///
/// r66-SD1：同步返回 `ConversionManager` 的 run 循环 JoinHandle。
/// 旧实现 `start_conversion_manager` 调 `spawn_supervised` 但丢弃了返回值——
/// 停机序只调 `conversion_mgr.shutdown_token().shutdown()`，不知其 fan-out
/// task 是否真退出（与 r64 webhook 同型问题）。把句柄透传给停机序
/// 让其在 `shutdown()` 之后 `await` 其真正退出。
pub(crate) fn start_north_servers(
    config: &GatewayConfig,
    conversion_mgr: &Arc<gateway_core::north::ConversionManager>,
    cert_pair: &Option<CertPair>,
    cert_manager: &CertManager,
    gateway: &Arc<Gateway>,
    audit: &Arc<gateway_core::engine::audit::AuditLog>,
    expanded_points: &[PointConfig],
) -> (Vec<Arc<dyn Server>>, tokio::task::JoinHandle<()>) {
    use gateway_core::north::server::{ServerContext, ServerRegistry};

    let ctx = ServerContext {
        config,
        conversion_mgr,
        cert_pair,
        cert_manager,
        gateway,
        audit,
        expanded_points,
    };

    // 插件化启动所有已注册的 Server
    let registry = ServerRegistry::new();
    let north_servers = registry.start_all(&ctx);

    // Modbus Passthrough 已通过 ServerRegistry 注册并启动
    let conversion_run = start_conversion_manager(conversion_mgr);

    (north_servers, conversion_run)
}

/// 启动转换层分发循环。
///
/// r66-SD1：返回 `spawn_supervised` 的 JoinHandle（旧实现丢弃了）。
fn start_conversion_manager(
    conversion_mgr: &Arc<gateway_core::north::ConversionManager>,
) -> tokio::task::JoinHandle<()> {
    let mgr = conversion_mgr.clone();
    spawn_supervised("conversion_manager", move || {
        let mgr = mgr.clone();
        async move {
            mgr.run().await;
        }
    })
}

// ============================================================================
// 阶段函数：Step 8-11（北向与处理链初始化）
// ============================================================================

/// Step 8-11：初始化北向与处理链（队列/证书/MQTT/Pipeline/实时总线/webhook）。
///
/// 当前阶段不直接消费 `chain` 中的资源（仅用 `config`），保留 `chain` 参数
/// 以与拆分方案的依赖传递结构一致；以 `_` 前缀避免 unused 警告。
pub(crate) async fn init_north_and_processing(
    config: &GatewayConfig,
    _chain: &CollectionChain,
    metrics: Arc<gateway_core::engine::metrics::Metrics>,
) -> Result<NorthProcessing> {
    // ===== Step 8: SQLite 队列 =====
    // 行数上限来自配置项 `queue.max_rows`（已通过 `GatewayConfig::validate_all`
    // 的范围与水位顺序校验），这里注入存储层的淘汰线。
    let queue = Arc::new(
        DataQueue::new_with_limit(default_db_path(), metrics.clone(), config.max_queue_rows())
            .with_context(|| format!("打开 SQLite 队列失败: {}", default_db_path()))?,
    );
    info!(
        "data queue initialized ({}), max_rows={}",
        default_db_path(),
        config.max_queue_rows()
    );

    // ===== Step 8b: X.509 设备证书（供 OPC UA + MQTT 双向 TLS 使用）=====
    // S1: 跨平台路径，避免硬编码 Linux 路径
    let cert_dir = Path::new(default_cert_dir());
    let cert_manager = gateway_core::crypto::cert::CertManager::new(
        cert_dir.join("gateway.pem").as_path(),
        cert_dir.join("gateway_key.pem").as_path(),
    )
    .with_cn("Gateway Device");
    let cert_pair = match cert_manager.get_or_create() {
        Ok(p) => Some(p),
        Err(e) => {
            degrade_cert_or_fail(
                e,
                gateway_core::crypto::cert::require_external_cert(),
                cert_dir,
            )?;
            None
        }
    };

    // ===== Step 9: MQTT 客户端 =====
    let mqtt: Option<Arc<MqttClient>> = if config.mqtt.enabled {
        match MqttClient::new(
            config.mqtt.clone(),
            queue.clone(),
            Some(default_outbox_db()),
            cert_pair.clone(),
            metrics.clone(),
        ) {
            Ok(c) => {
                let arc = Arc::new(c);
                arc.start();
                info!("mqtt client connected to broker");
                Some(arc)
            }
            Err(e) => {
                warn!("mqtt client init failed: {}", e);
                None
            }
        }
    } else {
        info!("mqtt disabled by config");
        None
    };

    // ===== Step 9b: 内置模拟消费者（无 MQTT 时的兜底消费者）=====
    //
    // 用途：现场/开发环境没有可用 MQTT 时，队列数据仍走完「出队 → 处理 → ack」
    // 完整链路，消除「队列有积压却没有消费者」的告警歧义（has_consumer=false）。
    //
    // 与 MQTT 互斥：两者都是队列的消费者，同时开会分流同一批数据——
    // 被 MQTT 分走的那部分正常上报，被模拟消费者分走的那部分则被「消费掉但不上报」，
    // 现场表现为静默丢数。故 MQTT 已启用时只告警、不启动模拟消费者。
    //
    // **恒创建实例**：配置里的 `enabled` 只表示「启动时是否自动拉起」，
    // 运行期开关由 API 控制（POST /api/sim_consumer/start|stop），
    // 故实例必须始终存在，状态接口也才有东西可报。
    let sim_consumer = SimConsumer::new(
        queue.clone(),
        config.sim_consumer.clone(),
        Some(metrics.clone()),
    );
    if config.sim_consumer.enabled {
        if mqtt.is_some() {
            warn!(
                "sim_consumer.enabled=true 但 mqtt 已启用：两个消费者会分流同一批数据导致静默丢数，未自动启动 sim_consumer（可通过 API 在停用 MQTT 后手动启动）"
            );
        } else {
            sim_consumer.start();
            info!(
                "sim consumer started (batch={}, interval={}ms)",
                config.sim_consumer.batch_size, config.sim_consumer.interval_ms
            );
        }
    }

    // ===== Step 10: EdgeProcessor + ProcessingPipeline =====
    let edge = Arc::new(EdgeProcessor::new());
    let realtime_bus = RealtimeBus::new(
        config
            .web
            .as_ref()
            .map(|w| w.realtime_buffer)
            .unwrap_or(1024),
    );

    // ===== 1.2: ConversionManager（统一北向连接器注册）=====
    //
    // RealtimeBus 第 5 层：北向单独一条无损 mpsc 队列（容量 1024 批）。
    // 处理管道与广播并列投递同一个 Arc；队列满时背压回传，不静默丢上报。
    let (north_tx, north_rx) =
        gateway_core::north::NorthFeed::channel(gateway_core::north::DEFAULT_NORTH_FEED_CAPACITY);
    let conversion_mgr = gateway_core::north::ConversionManager::new(north_rx);

    // 注册 MQTT connector（若启用）
    if let Some(mqtt_client) = &mqtt {
        conversion_mgr.register_connector(
            mqtt_client.clone() as std::sync::Arc<dyn gateway_core::north::NorthConnector>
        );
    }

    // Webhook 北向上报（若启用）
    //
    // r64-SD1：捕获 `spawn_catch_unwind` 返回的 JoinHandle（旧实现丢弃了返回值），
    // 供 graceful_shutdown 在 stop() 之后 `await` 其退出——真正等到 webhook run
    // 循环结束，而不是仅"发信号不等待"。句柄经 NorthProcessing 透传到停机序。
    let (webhook_client, webhook_run): (
        Option<std::sync::Arc<gateway_core::north::webhook::WebhookClient>>,
        Option<tokio::task::JoinHandle<()>>,
    ) = if let Some(wh_cfg) = config.webhook.as_ref().filter(|w| w.enabled) {
        match gateway_core::north::mqtt::outbox::OutboxStore::open(Path::new(default_outbox_db())) {
            Ok(wh_outbox) => {
                // Bug-N1 (fix)：outbox 文件 open 失败时不再禁用整个 webhook 通道，
                // 而是从 sqlite 退化到 in-memory 模式继续工作，并递增 metrics 告警。
                if wh_outbox.is_degraded() {
                    metrics
                        .queue_persist
                        .outbox_degraded
                        .fetch_add(1, Ordering::Relaxed);
                    warn!(
                        "webhook outbox 持久化模式 open 失败，已降级到内存模式 — 数据上报在本进程内可用，重启后丢失"
                    );
                }
                // r79-OBX1：注入全局 metrics，使 webhook give-up / permanent 失败
                // 路径能递增 webhook_data_lost_total / webhook_giveup_total，
                // 运维可通过 Grafana 告警发现「数据丢失」事件。
                let wh = std::sync::Arc::new(gateway_core::north::webhook::WebhookClient::new(
                    wh_cfg.clone(),
                    wh_outbox,
                    metrics.clone(),
                ));
                let wh_for_run = wh.clone();
                let handle = spawn_catch_unwind("webhook", async move {
                    wh_for_run.run().await;
                });
                info!("webhook enabled, url={}", wh_cfg.url);
                (Some(wh), Some(handle))
            }
            Err(e) => {
                // Bug-N1：open 仍可能完全失败（in_memory 也失败）——保留 fail-closed
                warn!("webhook outbox 完全 open 失败（持久化与内存模式都不行），disabling webhook: {}", e);
                (None, None)
            }
        }
    } else {
        (None, None)
    };

    // 注册 Webhook connector
    if let Some(wh) = &webhook_client {
        conversion_mgr.register_connector(
            wh.clone() as std::sync::Arc<dyn gateway_core::north::NorthConnector>
        );
    }

    // Server Sinks (OPC UA / Modbus) 由 start_north_servers() 通过 ServerRegistry 注册。

    // Bug 216: 原实现此处新建独立 Metrics 实例，与启动期注入的共享实例不是同一个，
    // 导致处理链/OTel 写入的指标经 /api/metrics（读 AppState 侧实例）永远读不到、恒为 0。
    // 改为复用上游注入的共享实例，保证采集/处理/导出全链路指标口径一致。
    let pipeline_metrics = metrics.clone();

    // 2.3.4: OTel metrics 桥接（tracing-otlp feature + telemetry.enabled）
    #[cfg(feature = "tracing-otlp")]
    if let Some(telemetry) = config.telemetry.as_ref().filter(|t| t.enabled) {
        gateway_core::engine::monitor::otel::init_otel_metrics(pipeline_metrics.clone(), telemetry);
    }

    // C1: 协议转换模板引擎——从配置编译规则集（空配置 = 空引擎，apply 早退零开销）。
    // 热重载由 Gateway::reload_config 检测 convert_rules 差异后 swap，无需重建 pipeline。
    let convert = gateway_core::pipeline::ConvertEngine::new(
        config.convert_rules.clone().unwrap_or_default(),
    );
    if convert.rule_count() > 0 {
        info!(
            "convert engine started with {} rule(s)",
            convert.rule_count()
        );
    }

    // 批次 B：把转换引擎注入 MQTT 客户端——payload 构建期回查 enum 规则补回
    // 枚举文本；共享 Arc，热重载即生效。
    if let Some(mqtt_client) = &mqtt {
        mqtt_client.set_convert_engine(convert.clone());
    }

    // 虚拟点位引擎（公式派生量）——从配置编译规则集（空配置 = 空引擎，求值零开销）。
    // 热重载由 Gateway::reload_config 检测 virtual_points 差异后 swap，无需重建 pipeline。
    let virtual_points = gateway_core::pipeline::VirtualPointEngine::new(
        config.virtual_points.clone().unwrap_or_default(),
    );
    if virtual_points.rule_count() > 0 {
        info!(
            "virtual point engine started with {} point(s)",
            virtual_points.rule_count()
        );
    }

    let pipeline = ProcessingPipeline::spawn(PipelineSpawnInputs {
        edge: edge.clone(),
        convert: convert.clone(),
        virtual_points: virtual_points.clone(),
        queue: queue.clone(),
        realtime_bus: Some(realtime_bus.clone()),
        north_tx: Some(north_tx),
        buffer: 4096,
        metrics: pipeline_metrics,
    });
    info!("processing pipeline started");

    // P3：webhook 装入统一北向通道表（顺序即停机序，与旧实现一致）。
    let mut channels: Vec<NorthChannel> = Vec::new();
    if let Some(ch) = NorthChannel::new("webhook", webhook_client, webhook_run) {
        channels.push(ch);
    }
    // 模拟消费者装入统一北向通道表：停机序按表顺序「先发停止信号、再 await 退出」，
    // 与 MQTT 走同一条停机通路，不会留下孤儿 task。
    //
    // run 句柄传 None：run 循环的 JoinHandle 由 SimConsumer 内部持有，
    // 其 `stop()` 本身就会等待 run 循环真正退出（上限 5s），停机序无需再
    // 额外 await 句柄（否则会等同一个句柄两次）。
    if let Some(ch) = NorthChannel::new("sim consumer", Some(sim_consumer.clone()), None) {
        channels.push(ch);
    }

    Ok(NorthProcessing {
        queue,
        cert_manager,
        cert_pair,
        mqtt,
        sim_consumer,
        pipeline,
        convert,
        virtual_points,
        realtime_bus,
        conversion_mgr,
        channels,
    })
}

/// 证书初始化失败时的处置策略：`require_external` 为 true 时必须阻断启动。
///
/// 背景：`GATEWAY_REQUIRE_EXTERNAL_CERT=1` 的语义是「不许自签、必须用运维
/// 提供的证书」。旧实现只让 `get_or_create()` 拒绝自签，而 bootstrap 拿到
/// Err 后仅 `warn` 并以「无证书」继续，结果是：OPC UA 退化为临时自签 + 明文
/// `None` 端点，MQTT 退化为无客户端证书的单向 TLS——开关形同虚设，合规场景
/// 下属于「静默降级」，比直接启动失败危险得多。故此处改为 fail-closed。
///
/// `require_external` 由调用方传入而非在函数内读环境变量，便于单测覆盖两条
/// 分支（环境变量是进程级全局状态，测试间会互相污染）。
fn degrade_cert_or_fail(
    err: gateway_core::engine::error::GatewayError,
    require_external: bool,
    cert_dir: &Path,
) -> Result<()> {
    if require_external {
        return Err(anyhow::anyhow!(
            "证书初始化失败且 GATEWAY_REQUIRE_EXTERNAL_CERT=1，拒绝以降级模式启动。\
             请检查 {} 下的 gateway.pem 与 gateway_key.pem 是否同时存在且配对；\
             原始错误: {}",
            cert_dir.display(),
            err
        ));
    }
    warn!("cert init failed, continuing without cert: {}", err);
    Ok(())
}

#[cfg(test)]
mod cert_degrade_tests {
    use super::degrade_cert_or_fail;
    use std::path::Path;

    fn sample_err() -> gateway_core::engine::error::GatewayError {
        gateway_core::engine::error::GatewayError::Config(
            "cert files not found and self-signed generation is disabled".to_string(),
        )
    }

    /// 强制外部证书时，证书失败必须让启动失败（fail-closed），且错误信息要
    /// 同时带上证书目录与原始原因，供现场直接定位。
    #[test]
    fn require_external_cert_makes_cert_failure_fatal() {
        let err = degrade_cert_or_fail(sample_err(), true, Path::new("C:/data/certs"))
            .expect_err("REQUIRE_EXTERNAL_CERT=1 时不得降级启动");
        let msg = err.to_string();
        assert!(msg.contains("GATEWAY_REQUIRE_EXTERNAL_CERT=1"), "{}", msg);
        assert!(msg.contains("C:/data/certs"), "{}", msg);
        assert!(msg.contains("self-signed"), "应保留原始错误: {}", msg);
    }

    /// 未开启该开关时保持原行为：告警并继续（兼容实验室/调试场景）。
    #[test]
    fn without_require_external_cert_failure_is_degraded() {
        degrade_cert_or_fail(sample_err(), false, Path::new("C:/data/certs"))
            .expect("未开启强制外部证书时应允许降级继续");
    }
}
