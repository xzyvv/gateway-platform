//! 网关启动流程（gatewayd 二进制：main.rs 仅保留入口，启动逻辑在此）
//!
//! 启动编排整体位于 gatewayd；gateway-core 引擎侧仅保留启动期共享支撑
//! （`gateway_core::runtime::GatewayRuntime` 与 `gateway_core::shared::*`），避免引擎反向依赖二进制。
//!
//! 13 步启动（§11）：
//!  1. 加载配置（ConfigLoader + validate_all）
//!  2. 初始化日志（JSON + 日志轮转）
//!  3. 启动看门狗（软件 + 可选硬件）
//!  4. 初始化 NTP 同步
//!  5. 初始化 GPIO（Linux）
//!  6. 打开串口总线 + BusManager
//!  7. 初始化 Collector（ProtocolRegistry 创建）
//!  8. 初始化 SQLite 队列（DataQueue）
//!  9. 初始化 MQTT 客户端（可选）

//! 11. 初始化 EdgeProcessor + ProcessingPipeline
//! 12. 启动 Scheduler（按协议 + 总线分组）
//! 13. 启动 Web Server（带 api_token 校验）
//!
//! god 函数拆分（保留所有 spawn 时机与行为）：
//! - `init_collection_chain`：Step 2-7（helpers/collection.rs）
//! - `init_north_and_processing`：Step 8-11（helpers/north.rs）
//! - `assemble_and_start_services`：Step 12-13（helpers/assemble.rs）

use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::info;

use gateway_core::config::ConfigLoader;
use gateway_core::engine::shutdown::wait_for_shutdown;
use gateway_core::shared::default_config_path;

mod helpers;
use helpers::*;

//  ============================================================================
// 3 个结构体承载子函数之间的依赖传递
// ============================================================================

/// 采集链（Step 2-7 产物）：logging/watchdog/NTP/GPIO/串口总线/采集器初始化结果。
pub(crate) struct CollectionChain {
    pub(crate) ntp: Arc<gateway_core::platform::NtpManager>,
    pub(crate) gpio: Arc<parking_lot::Mutex<gateway_core::platform::GpioController>>,
    pub(crate) bus_managers:
        std::collections::HashMap<String, Arc<gateway_core::south::bus::BusManager>>,
    pub(crate) collectors:
        std::collections::HashMap<String, Arc<dyn gateway_core::south::protocol::Collector>>,
    pub(crate) expanded_points: Vec<gateway_core::config::PointConfig>,
    pub(crate) templates: Arc<gateway_core::config::TemplateManager>,
}

/// 北向与处理链（Step 8-11 产物）：队列/证书/MQTT/Pipeline/实时总线。
pub(crate) struct NorthProcessing {
    pub(crate) queue: Arc<gateway_core::queue::DataQueue>,
    pub(crate) cert_manager: gateway_core::crypto::cert::CertManager,
    pub(crate) cert_pair: Option<gateway_core::crypto::cert::CertPair>,
    pub(crate) mqtt: Option<Arc<gateway_core::north::mqtt::MqttClient>>,
    /// 内置模拟消费者（**恒为 Some**）。
    ///
    /// 配置里的 `enabled` 只表示「启动时是否自动拉起」，运行期开关由 API
    /// 控制，故实例始终存在。「队列是否有消费者」以 `is_running()` 的实际
    /// 运行态判定——启动时未拉起（配置未启用 / MQTT 已启用）就不算消费者。
    pub(crate) sim_consumer: Arc<gateway_core::north::sim_consumer::SimConsumer>,
    pub(crate) pipeline: Arc<gateway_core::pipeline::ProcessingPipeline>,
    /// C1: 协议转换模板引擎（与 pipeline 共享同一实例；GatewayRuntime 持有用于热重载 swap）
    pub(crate) convert: Arc<gateway_core::pipeline::ConvertEngine>,
    /// 虚拟点位引擎（与 pipeline 共享同一实例；GatewayRuntime 持有用于热重载 swap 与 API 查询）
    pub(crate) virtual_points: Arc<gateway_core::pipeline::VirtualPointEngine>,
    pub(crate) realtime_bus: Arc<gateway_core::pipeline::RealtimeBus>,
    pub(crate) north_dispatcher: Arc<gateway_core::north::NorthDispatcher>,
    /// P3：北向 run 循环通道表（webhook 等，非 Clone 的 JoinHandle）；
    /// 装配阶段补充报警/透传后交给 graceful_shutdown 统一停机。
    pub(crate) channels: Vec<NorthChannel>,
}

/// 网关服务句柄（Step 12-13 产物）：主函数等待退出与优雅停机时使用。
pub(crate) struct GatewayHandles {
    pub(crate) scheduler: Arc<gateway_core::south::scheduler::Scheduler>,
    pub(crate) north_servers: Vec<Arc<dyn gateway_core::north::server::NorthServer>>,
    pub(crate) health_handles: Vec<tokio::task::JoinHandle<()>>,
    /// 蜂窝模块实例（仅配置 enabled 时 Some），停机时交给 graceful_shutdown。
    pub(crate) cellular: Option<Arc<gateway_core::platform::cellular::CellularManager>>,
    /// 自愈执行器停机句柄（仅 [self_heal] opt-in 启用时 Some），停机时交给 graceful_shutdown。
    pub(crate) self_heal: Option<SelfHealShutdown>,
    /// r66-SD1：NorthDispatcher run 循环 JoinHandle，停机时在 shutdown_token
    /// 触发后 await 其退出——确保 fan-out task 真正停掉再推进停机序。
    pub(crate) north_dispatcher_run: Option<tokio::task::JoinHandle<()>>,
    /// P3：北向 run 循环通道表（webhook / 报警 / 透传），
    /// 停机序按表顺序统一 stop + await。
    pub(crate) north_channels: Vec<NorthChannel>,
    /// r74-SD1：Web 优雅停机信号（step 13a 必 Some），停机序**最前**触发，
    /// 阻止停机窗口内继续接受写请求。
    pub(crate) web_shutdown: Option<gateway_api::web::ShutdownSignal>,
    /// r74-SD1：Web run 循环 JoinHandle，触发停机信号后 await 其真正退出。
    pub(crate) web_run: Option<tokio::task::JoinHandle<()>>,
}

// ============================================================================
// 入口
// ============================================================================

/// 网关入口：完整启动流程 + 等待停机
pub async fn run() -> Result<()> {
    run_with_config_path(default_config_path()).await
}

/// 指定配置文件路径启动（便于测试和回滚）
pub async fn run_with_config_path(config_path: &str) -> Result<()> {
    // P3-5：在任何初始化之前记录进程启动时刻，供 /api/diagnostics/gateway
    // 与 /api/watchdog/services 区分「进程运行时长」与「主机开机时长」
    gateway_api::web::uptime::mark_process_start();

    // ===== Step 1: 加载配置（保留在主函数：config_loader 需供 AppState 使用）=====
    let config_loader = ConfigLoader::new(config_path);
    let config = config_loader
        .load()
        .with_context(|| format!("加载配置失败: {}", config_path))?;
    info!("config loaded from {}", config_path);

    // ===== 创建 metrics 实例（1.2：全局单例改为注入）=====
    use gateway_core::engine::metrics::Metrics;
    use std::sync::Arc;
    let metrics = Arc::new(Metrics::new());
    // 1.2: 初始化 supervision 基础设施层 metrics（供 spawn_supervised 等内部读取）
    gateway_core::engine::supervision::init(metrics.clone());

    // 配置域共享库（config_versions.db）承载 config_versions / api_access_logs /
    // disk_history / uplink_test_history 四张表，都有行数 retention，但 SQLite
    // 默认不会把 DELETE 释放的页还给文件系统 → 文件只增不减。
    // 在创建任何 Store 长驻连接之前做一次性 auto_vacuum 转换
    // （已有库需要跑一次 VACUUM），之后由 storage_maintenance 周期回收。
    gateway_core::engine::monitor::db_maintenance::ensure_incremental_auto_vacuum(
        std::path::Path::new(gateway_core::shared::default_version_db()),
    );

    // ===== Step 2-7: 采集链初始化（logging/watchdog/NTP/GPIO/串口/采集器）=====
    let chain = init_collection_chain(&config, metrics.clone()).await?;
    // ===== Step 8-11: 北向与处理链初始化（队列/证书/MQTT/Pipeline）=====
    // P3：mut 供 assemble 取走北向通道表（含非 Clone 的 JoinHandle）。
    // （非 Clone，只能 move 给 ShutdownInputs）。
    let mut north = init_north_and_processing(&config, &chain, metrics.clone()).await?;

    // ===== Step 12-13: 组装并启动服务（Scheduler/GatewayRuntime/AppState/Web/北向/报警/透传）=====
    let (app_state, handles) =
        assemble_and_start_services(&config, &chain, &mut north, &config_loader, metrics.clone())
            .await?;

    // ===== 后台监控任务 =====
    // API Key 调用统计批量化落盘：鉴权热路径只做内存自增，这里每 5s 单事务
    // 批量写回 SQLite。任务随 runtime 退出；正常停机路径在 graceful_shutdown
    // 中还会强制 flush 一次，确保最后一个周期的增量不丢。
    {
        let api_keys = app_state.platform.api_keys.clone();
        gateway_core::engine::supervision::spawn_catch_unwind("api_key_usage_flush", async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(5));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                if let Err(e) = api_keys.flush() {
                    tracing::warn!("api key usage flush failed: {}", e);
                }
            }
        });
    }

    // spawn_disk_monitor 内部使用 spawn_supervised（self-recovery），
    // 不返回 JoinHandle，因此 health_handles 始终为空；graceful_shutdown 中
    // 的 abort() 在空 Vec 上是 no-op。
    //
    // 分级告警（25%/15%/10%）：档位变化时主动外呼一条系统级告警到
    // `<mqtt_topic>/system/alarm`，把"磁盘快满了"从写满后的静默丢数据
    // 提前到写满前几天可见。未配置 MQTT 时 notifier 为 None —— 档位仍会
    // 记录并暴露到 /api/metrics 与 /health/report，只是没有外呼通道。
    let disk_alarm_publisher: Option<
        std::sync::Arc<dyn gateway_core::engine::ports::AlarmPublisher>,
    > = north
        .mqtt
        .clone()
        .map(|m| m as std::sync::Arc<dyn gateway_core::engine::ports::AlarmPublisher>);
    let disk_alarm = std::sync::Arc::new(
        gateway_core::engine::monitor::health::DiskAlarmNotifier::new(
            disk_alarm_publisher,
            &config.mqtt.topic,
        ),
    );
    gateway_core::engine::monitor::health::spawn_disk_monitor(
        north.queue.clone(),
        app_state.platform.disk_history.clone(),
        app_state.north.gateway.disk_status().clone(),
        metrics.clone(),
        Some(disk_alarm),
    );

    // r52-MEM1：进程 RSS 自监控。补齐 health.rs 模块文档承诺的"内存 RSS 监控"。
    // 30s 粒度采集，写入 `gateway_process_rss_bytes` gauge（Prometheus 暴露）。
    // 同样走 spawn_supervised：panic 自恢复，不返回 JoinHandle。
    gateway_core::engine::monitor::health::spawn_memory_monitor(metrics.clone());

    // 存储维护：配置库周期 incremental_vacuum + 两个业务库的占用/空闲页采样。
    // 暴露 gateway_db_size_bytes / gateway_db_freelist_pages / gateway_aggregated_rows，
    // 让「删除是否真的把空间还给了文件系统」在现场可见。
    gateway_core::engine::monitor::db_maintenance::spawn_storage_maintenance(
        gateway_core::shared::default_db_path().to_string(),
        gateway_core::shared::default_version_db().to_string(),
        metrics.clone(),
    );

    // P8：outbox 积压深度采样 → `gateway_outbox_pending` gauge。
    // 该 gauge 此前无任何写入点（恒 0），导致「上行通道堵死」在监控上完全不可见。
    // 采样任务自己持有一条指向共享 outbox 文件的连接，统计全协议合计待发数，
    // 与 MQTT / Webhook 的启用组合解耦（任一协议积压都能被看到）。
    gateway_core::north::outbox_monitor::spawn_outbox_pending_monitor(
        std::path::PathBuf::from(gateway_core::shared::default_outbox_db()),
        metrics.clone(),
    );

    // ===== P0-3: 队列健康 → 告警联动（无条件启动）=====
    // 旧实现只在 `mqtt.is_some()` 时启动，网线直连调试（未配 MQTT）时没有任何
    // 采样者 → 积压 14.9 万条、`today_alerts` 仍恒为 0，告警链路整体失明。
    // 现在无论有无消费者都启动；判定交给 core 的 `queue_health`：
    // 有消费者时才把「超过背压阈值」当故障，无消费者时队列兼作本地历史库，
    // 只在存量达到预警阈值（warn）与逼近硬上限（critical）时告警。
    // 内置模拟消费者同样是队列的消费者：它会真的把队列数据消费掉（出队 → 处理 → ack），
    // 不是「只进不出」的积压库。故 has_consumer 判定必须把它算进去，
    // 否则启用它反而会让「无消费者」告警继续误报。
    //
    // 判定口径与 Web API 完全同源：`NorthRefs::has_north_consumer()`（实时 MQTT
    // 客户端 + 模拟消费者实际运行态）。这里传**运行期探针**而不是启动期快照：
    // 模拟消费者可被 API 随时启停，MQTT 客户端也会被热重载替换，
    // 启动期求值一次会让后台告警与 `/health/ready`、`/api/health/report`、
    // `/api/watchdog/services` 给出互斥结论。
    let north_refs = app_state.north.clone();
    let has_north_consumer = Arc::new(move || north_refs.has_north_consumer());
    north.queue.spawn_backlog_alarm_monitor(has_north_consumer);
    info!(
        "queue health alarm monitor started (north_consumer={})",
        app_state.north.has_north_consumer()
    );

    // ===== 等待退出信号 =====
    wait_for_shutdown().await;
    info!("shutdown signal received, draining...");

    // ===== 优雅停机 =====
    let GatewayHandles {
        scheduler,
        north_servers,
        health_handles,
        cellular,
        self_heal,
        // r66-SD1：NorthDispatcher run 循环 JoinHandle（始终 Some）。
        north_dispatcher_run,
        // P3：北向 run 循环通道表。
        north_channels,
        // r74-SD1：Web 停机信号 + run 句柄（停机序最前触发）。
        web_shutdown,
        web_run,
    } = handles;
    graceful_shutdown(ShutdownInputs {
        scheduler,
        pipeline: north.pipeline.clone(),
        north_dispatcher: north.north_dispatcher.clone(),
        mqtt: north.mqtt.clone(),
        queue: north.queue.clone(),
        state: app_state.clone(),
        cellular,
        health_handles,
        north_servers,
        self_heal,
        // r66-SD1：NorthDispatcher run 循环 JoinHandle，
        // graceful_shutdown 在 north_dispatcher.shutdown_token() 后 await。
        north_dispatcher_run,
        // P3：北向 run 循环通道表（webhook / 报警 / 透传）。
        north_channels,
        // r74-SD1：Web 停机信号 + run 句柄——停机序最前触发优雅停机。
        web_shutdown,
        web_run,
        // S04: 注入 metrics 给 graceful_shutdown，超时后 +1 shutdown_forced_total
        metrics: metrics.clone(),
    })
    .await;

    info!("gateway stopped cleanly");
    // OP-5：停机序的最后一步——冲刷日志管道。
    //
    // 必须放在 `info!` 之后、`run()` 返回之前：
    // 1) `tracing_appender::non_blocking` 的 WorkerGuard 存在 gateway-core 的静态量里，
    //    而 Rust 不为静态量执行 Drop，不显式调用则 channel 中未落盘的 JSON 行
    //    （含上面这条 "gateway stopped cleanly"）会随进程退出丢失；
    // 2) `run()` 是启动编排的唯一出口（main.rs 只有 `#[tokio::main] fn main`），
    //    放此处可同时覆盖正常停机与启动失败提前 return 两条路径；
    // 3) 调用后不得再写日志——文件 channel 已关闭（本行之后唯一动作是 `return`）。
    gateway_core::engine::logging::shutdown_logging();
    Ok(())
}
