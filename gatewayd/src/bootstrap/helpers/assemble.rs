//! 装配阶段辅助函数（Step 12-13：Scheduler/GatewayRuntime/AppState/Web/北向/报警/透传 + 优雅停机）
//!
//! 对应 bootstrap 三段拆分中的 GatewayHandles 阶段，以及停机序列。

use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::{error, info, warn};

use gateway_api::web::AppState;
use gateway_core::config::{ConfigLoader, GatewayConfig, WebConfig};
use gateway_core::config::{ConfigVersionStore, UplinkTestHistoryStore};
use gateway_core::engine::metrics::Metrics;
use gateway_core::engine::supervision::spawn_supervised;
use gateway_core::engine::Diagnostics;
use gateway_core::north::server::{MqttPacketPassthrough, NorthServer};
use gateway_core::platform::ota::OtaManager;
use gateway_core::runtime::{GatewayComponents, GatewayRuntime};
use gateway_core::south::bus::SerialManager;
use gateway_core::south::scheduler::Scheduler;

use super::super::{CollectionChain, GatewayHandles, NorthProcessing};
use super::*;
use gateway_core::shared::{
    default_audit_log, default_diag_log, default_license_clock_db, default_ota_staging,
    default_outbox_db, default_version_db,
};

// ============================================================================
// AppState 依赖构造
// ============================================================================

/// AppState 依赖聚合：版本库/上行测试历史/OTA/诊断/指标/磁盘历史。
///
/// 这些对象构造较重（打开数据库、初始化 OTA 分区管理器、打开磁盘历史存储），
/// 但相互之间无依赖，集中构造便于在 god 函数拆分中作为单一返回值传递。
pub(crate) struct AppDeps {
    pub(crate) version_store: Arc<ConfigVersionStore>,
    pub(crate) uplink_test_history: Arc<UplinkTestHistoryStore>,
    pub(crate) rate_rules: Arc<gateway_core::config::RateRuleStore>,
    pub(crate) quality_rule_store: Arc<gateway_core::config::QualityRuleStore>,
    pub(crate) api_keys: Arc<gateway_core::config::ApiKeyStore>,
    pub(crate) sched_tasks: Arc<gateway_core::config::ScheduledTaskStore>,
    pub(crate) ota: Arc<OtaManager>,
    pub(crate) diagnostics: Arc<Diagnostics>,
    pub(crate) metrics: Arc<Metrics>,
    pub(crate) disk_history: Arc<gateway_core::engine::monitor::health::DiskHistoryStore>,
}

/// 构造 AppState 所需的重依赖：版本库/上行测试历史/OTA/诊断/指标/磁盘历史。
///
/// `ota_signing_key`：由 `web.ota_signing_key`（base64）解码后的 HMAC 密钥。
/// None = 未配置（CRC32-only 向后兼容）；Some = 强制验签（见 Bug C8）。
pub(crate) fn build_app_state_dependencies(
    ota_signing_key: Option<Vec<u8>>,
    metrics: Arc<Metrics>,
) -> Result<AppDeps> {
    let version_store = Arc::new(
        ConfigVersionStore::new(default_version_db())
            .with_context(|| format!("打开版本库失败: {}", default_version_db()))?,
    );
    let uplink_test_history = Arc::new(
        UplinkTestHistoryStore::new(default_version_db())
            .with_context(|| format!("打开上行测试历史库失败: {}", default_version_db()))?,
    );
    // 访问控制域存储（限流规则 / API 密钥 / 计划任务）：与版本库共用同一 SQLite 文件
    let rate_rules = Arc::new(
        gateway_core::config::RateRuleStore::new(default_version_db())
            .with_context(|| format!("打开限流规则库失败: {}", default_version_db()))?,
    );
    // 数据质量规则：与版本库共用同一 SQLite 文件。启用规则在此安装到处理管道快照——
    // 读取失败只记 error 日志、不阻断启动（网关仍可按既有质量码运行）。
    let quality_rule_store = Arc::new(
        gateway_core::config::QualityRuleStore::new(default_version_db())
            .with_context(|| format!("打开质量规则库失败: {}", default_version_db()))?,
    );
    match quality_rule_store.list_enabled_rules() {
        Ok(rules) => {
            let count = rules.len();
            gateway_core::pipeline::quality_rules::install_rules(rules);
            info!("quality rules installed at startup: {}", count);
        }
        Err(e) => {
            error!("failed to load quality rules at startup: {}", e);
        }
    }
    let api_keys = Arc::new(
        gateway_core::config::ApiKeyStore::new(default_version_db())
            .with_context(|| format!("打开 API 密钥库失败: {}", default_version_db()))?,
    );
    let sched_tasks = Arc::new(
        gateway_core::config::ScheduledTaskStore::new(default_version_db())
            .with_context(|| format!("打开计划任务库失败: {}", default_version_db()))?,
    );
    // Bug 216: 原实现此处新建独立 Metrics 实例，导致 Web /api/metrics 暴露的实例
    // 与调度器/处理链实际写入的实例不是同一个（所有运行期指标恒为 0）。
    // 改为使用调用方注入的共享实例。
    // OTA A/B 分区：走 AbPartitionConfig::default()，与 gateway-core 保持单一来源。
    // 分区路径可由 GATEWAY_OTA_SLOT_A / GATEWAY_OTA_SLOT_B 覆盖（现场介质不一定是
    // eMMC）；此处不再写死 /dev/mmcblk0p2/p3，避免代码与 systemd DeviceAllow 漂移。
    let ota = OtaManager::new(
        gateway_core::platform::ota::AbPartitionConfig::default(),
        default_ota_staging(),
        ota_signing_key,
        metrics.clone(),
    );
    // Bug 155: OtaManager::new 已按 bootloader 环境校正 active_slot；
    // 此处再执行启动自检——bootcount >= 3 时触发 A/B 自动回滚（非 Linux 为 no-op）。
    ota.boot_rollback_check();
    let diagnostics = Diagnostics::new(default_diag_log());
    let disk_history = Arc::new(
        gateway_core::engine::monitor::health::DiskHistoryStore::new(1440, default_version_db())?,
    );
    Ok(AppDeps {
        version_store,
        uplink_test_history,
        rate_rules,
        quality_rule_store,
        api_keys,
        sched_tasks,
        ota,
        diagnostics,
        metrics,
        disk_history,
    })
}

// ============================================================================
// Web 配置准备
// ============================================================================

/// ConfigLoader 不是 Clone（path 是 owned），但 AppState 想要持有。
/// 用一个轻包装把 path 存进去，每次调用都新建 loader。
pub(crate) fn config_loader_arc(loader: &ConfigLoader) -> ConfigLoader {
    ConfigLoader::new(loader.path())
}

/// 生成启动期默认 web api_token（32 字符十六进制）。
///
/// 工业网关通常无强随机源硬件，这里用本机指纹 + 启动时间纳秒 + PID 作为熵源，
/// 经手写 SHA256 摘要后取前 32 个十六进制字符。
/// 注意：这不是密码学安全的随机数生成，但远好于硬编码默认值。
pub(crate) fn generate_default_web_token() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let mut entropy: Vec<u8> = Vec::with_capacity(128);

    if let Ok(id) = std::fs::read_to_string("/etc/machine-id") {
        entropy.extend_from_slice(id.trim().as_bytes());
    }
    if let Ok(cpu) = std::fs::read_to_string("/proc/cpuinfo") {
        for line in cpu.lines() {
            if line.starts_with("Serial") || line.starts_with("Hardware") {
                entropy.extend_from_slice(line.as_bytes());
                break;
            }
        }
    }
    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "gateway-default".to_string());
    entropy.extend_from_slice(host.as_bytes());

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    entropy.extend_from_slice(&now.as_nanos().to_le_bytes());
    entropy.extend_from_slice(&std::process::id().to_le_bytes());

    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let c = COUNTER.fetch_add(1, Ordering::Relaxed);
    entropy.extend_from_slice(&c.to_le_bytes());

    if entropy.is_empty() {
        entropy.extend_from_slice(b"gateway-fallback-entropy-pool");
    }
    let hash = gateway_core::crypto::sha256_bytes(&entropy);
    let mut out = String::with_capacity(32);
    for b in &hash[..16] {
        use std::fmt::Write as _;
        let _ = write!(out, "{:02x}", b);
    }
    out
}

/// 准备 WebConfig：未配置时给安全默认值，token/password_hash 缺失时自动生成或拒绝启动。
pub(crate) fn prepare_web_config(config: &GatewayConfig) -> Result<WebConfig> {
    let mut web_cfg: WebConfig = config.web.clone().unwrap_or_else(|| {
        let token = generate_default_web_token();
        // Bug 172: 默认收敛——不再默认监听所有网卡（旧值为 0.0.0.0:8080），改为回环地址，
        // 避免出厂即把控制接口暴露给整段车间网。需要「插网线直连调试」的现场，
        // 请在 config.json 显式设置 web.listen_addr = "0.0.0.0:8080" 并配合 web.password_hash。
        warn!("web config missing: using safe defaults: listen=127.0.0.1:8080 (loopback only), generated api_token (set web.api_token in config.json to override)");
        eprintln!("[bootstrap] generated api_token={} (shown once, save it now)", token);
        WebConfig {
            api_token: token,
            username: "admin".to_string(),
            password_hash: String::new(),
            listen_addr: "127.0.0.1:8080".to_string(),
            allow_anon_read: false,
            realtime_buffer: 1024,
            realtime_max_subscribers: gateway_core::config::default_realtime_max_subscribers(),
            ota_signing_key: None,
            rate_limit_per_sec: None,
            cors_origins: None,
            allow_default_token: true,
            users: Vec::new(),
            webhook_test_allow_hosts: Vec::new(),
            webhook_test_block_private: false,
            // P0/R1：预置演示会话默认关闭，只有演示 / 培训环境显式配置才注入。
            demo_replay_sessions: false,
        }
    });
    if web_cfg.api_token.is_empty() {
        let token = generate_default_web_token();
        warn!("web.api_token is empty, auto-generated token (set web.api_token in config.json to override)");
        eprintln!(
            "[bootstrap] generated api_token={} (shown once, save it now)",
            token
        );
        web_cfg.api_token = token;
    }

    if web_cfg.password_hash.is_empty() && !web_cfg.allow_default_token {
        anyhow::bail!("web.password_hash is empty and web.allow_default_token=false. Refusing to start Web service. Set web.password_hash (PBKDF2) or web.allow_default_token=true in config.json.");
    }

    // Bug 172: 硬性安全违规 fail-closed（区别于下方 security_warnings 的软告警）。
    // 典型违规：监听所有网卡 + 未设置 password_hash（默认凭据 admin/admin）——
    // 同网段任意主机都能登录控制接口。此时拒绝启动，强制运维显式收敛或设置密码。
    if let Some(first) = web_cfg.security_violations().first() {
        anyhow::bail!("[security] refusing to start Web service: {}", first);
    }

    // 统一输出安全风险项（默认凭据 / 匿名只读 / 开放 CORS / 全网卡监听）：
    // 与前端 whoami 的 security_warnings 共用同一判定，避免两处口径漂移。
    for msg in web_cfg.security_warnings() {
        warn!("[security] {}", msg);
    }

    Ok(web_cfg)
}

// ============================================================================
// AppState 构造 + 服务启动
// ============================================================================

/// `build_app_state` 的装配参数：bootstrap 装配点唯一，
/// 用结构体承载以避免超长参数列表（clippy::too_many_arguments）。
pub(crate) struct AppStateInputs<'a> {
    pub(crate) config: &'a GatewayConfig,
    pub(crate) config_loader: &'a ConfigLoader,
    pub(crate) scheduler: &'a Arc<Scheduler>,
    pub(crate) chain: &'a CollectionChain,
    pub(crate) north: &'a NorthProcessing,
    pub(crate) deps: AppDeps,
    pub(crate) web_cfg: &'a WebConfig,
    pub(crate) gateway: &'a Arc<GatewayRuntime>,
    pub(crate) passthrough_manager: &'a Option<Arc<MqttPacketPassthrough>>,
    pub(crate) north_servers: Vec<Arc<dyn NorthServer>>,
    pub(crate) license: Arc<gateway_core::license::LicenseManager>,
    pub(crate) cellular: Option<Arc<gateway_core::platform::cellular::CellularManager>>,
    /// 审计日志实例（由 bootstrap 提前创建，同时注入北向 Server 与 AppState）
    pub(crate) audit: Arc<gateway_core::engine::audit::AuditLog>,
}

/// 构造死信门面。
///
/// 独立打开一次 outbox 库（与 webhook/MQTT 各持一个连接，WAL + busy_timeout
/// 下写锁冲突可控）。失败时降级为 `NoopDeadLetter`：死信查询/重投是运维辅助
/// 能力，若因库打不开就让整个 Web 面起不来，属于「辅助功能拖挂主功能」。
fn open_dead_letter() -> Arc<dyn gateway_api::web::traits::DeadLetterTrait> {
    let path = std::path::Path::new(default_outbox_db());
    match gateway_api::web::traits::SqliteDeadLetter::open(path) {
        Ok(dl) => Arc::new(dl),
        Err(e) => {
            warn!(
                outbox = %path.display(),
                error = %e,
                "死信库打开失败，死信查询/重投接口将返回 503（北向上报不受影响）"
            );
            Arc::new(gateway_api::web::traits::NoopDeadLetter)
        }
    }
}

/// 构造 AppState：把 chain / north / deps / web_cfg / gateway / passthrough 聚合到
/// 单一共享状态中。
pub(crate) fn build_app_state(inputs: AppStateInputs<'_>) -> Arc<AppState> {
    let AppStateInputs {
        config,
        config_loader,
        scheduler,
        chain,
        north,
        deps,
        web_cfg,
        gateway,
        passthrough_manager,
        north_servers,
        license,
        cellular,
        audit,
    } = inputs;
    use gateway_api::web::types::{ConfigRefs, NorthRefs, PlatformRefs, SouthRefs};
    Arc::new(AppState {
        config: ConfigRefs {
            config: arc_swap::ArcSwap::new(std::sync::Arc::new(config.clone())),
            config_loader: config_loader_arc(config_loader),
            config_reload_lock: Arc::new(tokio::sync::Mutex::new(())),
            version_store: deps.version_store.clone(),
            templates: chain.templates.clone(),
            uplink_test_history: deps.uplink_test_history.clone(),
        },
        north: NorthRefs {
            ota: deps.ota.clone(),
            cert_manager: Arc::new(north.cert_manager.clone()),
            gateway: gateway.clone(),
            north_servers: north_servers.clone(),
            north_dispatcher: north.north_dispatcher.clone(),
            // 模拟消费者实例（恒存在）：Web 侧的状态接口与启停接口操作的就是它。
            sim_consumer: north.sim_consumer.clone(),
            // 北向队列观测句柄：/apin 从这里读积压/背压，不新增后台采样任务。
            north_feed: north.pipeline.north_sender(),
            // 死信查询/重投门面：独立打开 outbox 库（同文件第二连接，WAL 下安全）。
            // 打不开时降级为 Noop——死信是运维辅助能力，不该阻断网关启动。
            dead_letter: open_dead_letter(),
        },
        south: SouthRefs {
            scheduler: scheduler.clone(),
            queue: north.queue.clone(),
            realtime_bus: north.realtime_bus.clone(),
            passthrough: passthrough_manager.clone(),
        },
        platform: PlatformRefs {
            gpio: chain.gpio.clone(),
            ntp: chain.ntp.clone(),
            cellular,
            diagnostics: deps.diagnostics.clone(),
            metrics: deps.metrics.clone(),
            disk_history: deps.disk_history.clone(),
            heal_events: Arc::new(gateway_api::web::heal::HealEventStore::new()),
            license,
            // S2: 跨平台审计日志路径（与北向 Server 写回共用同一实例）
            audit,
            rate_rules: deps.rate_rules.clone(),
            quality_rule_store: deps.quality_rule_store.clone(),
            api_keys: deps.api_keys.clone(),
            sched_tasks: deps.sched_tasks.clone(),
        },
        web: web_cfg.clone(),
        rate_limiter: Arc::new(gateway_api::web::middleware::RateLimiter::default()),
    })
}

/// 启动 Web Server（Step 13a）：解析 listen_addr -> spawn_supervised。
///
/// r74-SD1：返回 (ShutdownSignal, run 循环 JoinHandle)，供 graceful_shutdown
/// 在**停机序最前**触发优雅停机——旧实现句柄被丢弃且无关闭原语，整个停机窗口
/// （最坏 80s）内 HTTP 仍接受写请求，与 queue drain 及陆续退出的子系统并发。
pub(crate) fn start_web_server(
    app_state: &Arc<AppState>,
    web_cfg: &WebConfig,
) -> Result<(
    gateway_api::web::ShutdownSignal,
    tokio::task::JoinHandle<()>,
)> {
    let web_addr: std::net::SocketAddr = web_cfg
        .listen_addr
        .parse()
        .with_context(|| format!("invalid web listen_addr: {}", web_cfg.listen_addr))?;
    let state_for_web = app_state.clone();
    let shutdown = gateway_api::web::ShutdownSignal::new();
    let shutdown_for_web = shutdown.clone();
    let handle = spawn_supervised("web_server", move || {
        let state = state_for_web.clone();
        let shutdown = shutdown_for_web.clone();
        async move {
            if let Err(e) = gateway_api::web::start_with_shutdown(state, web_addr, shutdown).await {
                // P2 修复（控制面永久失效且无自动恢复手段）：原实现此处仅 `error!` 记日志后
                // 正常返回，而 `spawn_supervised` 把「future 正常结束」视为任务自然完成、
                // **不再重启**（见 engine/supervision.rs 的 `Ok(()) => return`）。于是
                // Web/API 一旦启动失败（端口被占、api_token 缺失或弱 token 被 fail-closed
                // 拒绝）或运行中 serve 出错，控制面就永久失效——现场只能重启整个进程恢复，
                // 无人值守电柜场景下远程调试通道彻底断掉。
                // 改为 panic：由 `spawn_supervised` 按 1s→30s 退避自动重启，并计入
                // gateway_task_panics_total，使该故障在监控上可见。
                panic!("web server exited with error: {}", e);
            }
        }
    });
    info!("web server spawned on {}", web_addr);
    Ok((shutdown, handle))
}

/// 启动报警引擎（Step 13d）：限值报警 + MQTT 上报。
///
/// 仅当至少一个点位配置了 alarm_high/alarm_low 时才 spawn；否则跳过。
///
/// r73-SD1：返回 (AlarmEngine, run 循环 JoinHandle)，供 graceful_shutdown
/// 先 stop 再 await —— 旧实现句柄被丢弃且无停机接口，报警引擎在整个停机窗口
/// 内仍持续评估并向已停的 MQTT 上报。
pub(crate) fn start_alarm_engine(
    chain: &CollectionChain,
    north: &NorthProcessing,
    mqtt_topic: &str,
    metrics: Arc<Metrics>,
) -> (
    Option<Arc<gateway_core::engine::alarm::AlarmEngine>>,
    Option<tokio::task::JoinHandle<()>>,
) {
    use gateway_core::engine::alarm::AlarmEngine;
    let alarm_points: Vec<_> = chain
        .expanded_points
        .iter()
        .filter(|p| p.alarm_high.is_some() || p.alarm_low.is_some())
        .cloned()
        .collect();
    let alarm_count = alarm_points.len();
    if alarm_count > 0 {
        // 依赖倒置：报警引擎只认 `AlarmPublisher` 契约，MQTT 是它当前的实现方。
        // 这里是唯一的"实现 → 契约"装配点，引擎侧不感知 MQTT。
        let alarm_publisher: Option<
            std::sync::Arc<dyn gateway_core::engine::ports::AlarmPublisher>,
        > = north
            .mqtt
            .clone()
            .map(|m| m as std::sync::Arc<dyn gateway_core::engine::ports::AlarmPublisher>);
        let alarm_engine = std::sync::Arc::new(AlarmEngine::new(
            alarm_points,
            alarm_publisher,
            mqtt_topic.to_string(),
            Some(metrics),
        ));
        let bus = north.realtime_bus.clone();
        let engine_for_run = alarm_engine.clone();
        let handle = spawn_supervised("alarm_engine", move || {
            let engine = engine_for_run.clone();
            let bus = bus.clone();
            async move {
                engine.run(bus).await;
            }
        });
        info!(
            "alarm engine started with {} configured point(s)",
            alarm_count
        );
        (Some(alarm_engine), Some(handle))
    } else {
        info!("alarm engine skipped: no point has alarm_high/alarm_low configured");
        (None, None)
    }
}

/// 启动 MQTT 透传模式（Step 13e）：服务器通过 MQTT 下发原始报文，网关转发到设备并回传响应。
///
/// r73-SD1：返回 (MqttPacketPassthrough, run 循环 JoinHandle)，供 graceful_shutdown
/// 先 stop 再 await。
pub(crate) fn start_passthrough_mode(
    passthrough_manager: &Option<Arc<MqttPacketPassthrough>>,
    mqtt_topic: &str,
) -> (
    Option<Arc<MqttPacketPassthrough>>,
    Option<tokio::task::JoinHandle<()>>,
) {
    if let Some(passthrough) = passthrough_manager.clone() {
        let pt_for_run = passthrough.clone();
        let handle = spawn_supervised("passthrough_manager", move || {
            let pt = pt_for_run.clone();
            async move {
                pt.run().await;
            }
        });
        info!(
            "passthrough mode enabled (in={}/passthrough/in, out={}/passthrough/out)",
            mqtt_topic, mqtt_topic
        );
        (Some(passthrough), Some(handle))
    } else {
        info!("passthrough mode skipped: mqtt disabled");
        (None, None)
    }
}

// ============================================================================
// License 上下文（§12 / §7）
// ============================================================================

/// License 加载器返回的 helper 视图：已加载的管理器
pub(crate) struct LicenseInit {
    pub manager: Arc<gateway_core::license::LicenseManager>,
}

/// 解析 license 路径：环境变量 > config 节 > 默认 `./license.lic`。
/// 不存在文件不报错（开源版/未授权版可用，§7 一视同仁）。
pub(crate) fn init_license(config: &GatewayConfig) -> LicenseInit {
    let env_path = std::env::var("GATEWAY_LICENSE_PATH").ok();
    let cfg_path = config.license.as_ref().and_then(|l| l.path.clone());
    let resolved = env_path
        .or(cfg_path)
        .unwrap_or_else(|| "./license.lic".to_string());
    let path = std::path::PathBuf::from(&resolved);
    let manager = Arc::new(gateway_core::license::LicenseManager::new(
        std::path::Path::new(default_license_clock_db()),
    ));
    // v2.0 注入 tier_quotas 配置：bootstrap 阶段已有 GatewayConfig 完整加载，直接拿 license 节
    // 缺省时 None → LicenseManager.current() 对"quota 全空"的 license 兜底为 DEFAULT_ABSENT_QUOTA（不再等于无限）
    if let Some(lic_cfg) = config.license.clone() {
        manager.set_quota_config(lic_cfg);
    }
    if let Err(e) = manager.load(&path) {
        // 启动期校验失败仅 warn：缺 license 文件也是合法起步态
        tracing::warn!("license load rejected: {} (continuing in degraded mode)", e);
    } else {
        match manager.current().status {
            gateway_core::license::LicenseStatus::Ok => tracing::info!(
                "license loaded from {} (tier={:?})",
                path.display(),
                manager.current().tier
            ),
            _ => tracing::warn!(
                "license loaded but invalid (continuing in degraded mode): {}",
                path.display()
            ),
        }
    }
    // v2.0 启动告警：tier_quotas 没写全四档时，这些档位会回落到代码内置默认表
    // （该档应有数值，不会退到开源版 absent 兜底），但运维无法在 config.json 里
    // 调整这几档的配额。配置缺失在启动时是唯一能提前发现它的时机。
    if let Some(lic_cfg) = config.license.as_ref() {
        let missing = lic_cfg.missing_tiers();
        if !missing.is_empty() {
            tracing::warn!(
                "license.tier_quotas missing tiers: {:?} — quotas for these tiers fall back to the built-in default table; write all four tiers in config.json to tune them per deployment",
                missing
            );
        }
        let s = manager.current();
        if s.present && s.quota_source == gateway_core::license::QuotaSource::TierMissing {
            tracing::warn!(
                "license tier {:?} has no tier_quotas mapping — quota taken from the built-in default table (max_points={:?}); add this tier to license.tier_quotas in config.json to customize it",
                s.tier,
                s.quota.max_points
            );
        }
    }
    LicenseInit { manager }
}

/// 启动 License 每日重校验后台任务（§7 refresh_loop）。
/// LicenseManager::apply_ntp_resync 在 NTP 校时成功时调用即可（§9）。
pub(crate) fn spawn_license_refresh_loop(manager: Arc<gateway_core::license::LicenseManager>) {
    spawn_supervised("license_refresh", move || {
        let m = manager.clone();
        async move {
            m.refresh_loop().await;
        }
    });
}

/// 启动 License 时钟基准对账任务（§9：NTP 校时成功 → ClockGuard 重置基准）。
///
/// NtpManager 每次校时成功会刷新 `last_sync`（NTP 服务器时间，毫秒），且不修改系统时钟；
/// 本任务按 `last_sync` 对账：发现新的成功校时即调 `apply_ntp_resync`——把回拨基准重置为
/// NTP 真实时间并立即重校验。覆盖两类场景：
/// 1. 断电 RTC 漂移超 48h 阈值 → 启动时判 Tampered 降级 core → 校时后自动洗白，无需重启；
/// 2. 运行中修改 NTP 配置后重新校时成功。
///
/// 首轮立即对账（启动早期 Step 3 已做过一次同步），此后 60s 周期。
pub(crate) fn spawn_license_ntp_resync_watch(
    license: Arc<gateway_core::license::LicenseManager>,
    ntp: Arc<gateway_core::platform::ntp::NtpManager>,
) {
    spawn_supervised("license_ntp_resync", move || {
        let license = license.clone();
        let ntp = ntp.clone();
        async move {
            // 上次已对账的 NTP 校时时间戳（毫秒）；None = 尚未见过成功校时
            let mut last_processed: Option<i64> = None;
            loop {
                let st = ntp.status();
                // source=="NTP" 才代表最近一次同步成功；RTC 后备时 last_sync 是旧值，跳过
                if st.source == "NTP" {
                    match st.last_sync {
                        Some(ts) if Some(ts) != last_processed => {
                            // NTP 时间为毫秒，ClockGuard 基准为秒
                            license.apply_ntp_resync(ts.div_euclid(1000));
                            last_processed = Some(ts);
                        }
                        _ => {}
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            }
        }
    });
}

// ============================================================================
// 阶段函数：Step 12-13（组装并启动服务）
// ============================================================================

/// P2-1：配置版本库为空时，用当前生效配置落一条基线快照。
///
/// 说明：`state.config` 是启动时解密/模板展开后的生效配置；`save_version`
/// 内部会重新加密敏感字段，因此落库的是可回滚的真实快照（各版本的读取
/// 接口仍按 GET /api/config 的口径脱敏）。失败只告警不阻塞启动——版本库
/// 不可用不应让网关起不来，用户仍可在配置页手动保存生成版本。
fn seed_baseline_config_version(state: &AppState) {
    let store = state.config.version_store.as_ref();
    let cfg = state.config.load_full();
    seed_baseline_version_if_empty(store, &cfg);
}

/// 版本库为空则落基线；返回是否新写入（便于单测断言）。
fn seed_baseline_version_if_empty(
    store: &gateway_core::config::ConfigVersionStore,
    config: &GatewayConfig,
) -> bool {
    match store.list_versions() {
        Ok(list) if list.is_empty() => match store.save_version(config, "initial baseline") {
            Ok(id) => {
                info!("config baseline version v{} saved", id);
                true
            }
            Err(e) => {
                warn!("failed to save config baseline version: {}", e);
                false
            }
        },
        Ok(_) => false,
        Err(e) => {
            warn!("failed to inspect config version history: {}", e);
            false
        }
    }
}

/// Step 12-13：组装并启动所有服务（Scheduler/GatewayRuntime/AppState/Web/HA/北向/报警/透传）。
///
/// 接收 `config_loader`（主函数保留所有权）用于 `AppState.config_loader`
/// 字段——`config_loader_arc(&config_loader)` 重建等价实例。
pub(crate) async fn assemble_and_start_services(
    config: &GatewayConfig,
    chain: &CollectionChain,
    north: &mut NorthProcessing,
    config_loader: &ConfigLoader,
    metrics: Arc<Metrics>,
) -> Result<(Arc<AppState>, GatewayHandles)> {
    // ===== Step 12: Scheduler =====
    // 1.2: metrics 由调用方注入（替代全局 metrics::get()）
    let gateway_metrics = metrics.clone();
    let scheduler = Arc::new(Scheduler::new(
        chain.collectors.clone(),
        chain.expanded_points.clone(),
        north.pipeline.clone(),
        gateway_metrics.clone(),
    ));
    scheduler.start();
    info!(
        "scheduler started with {} point(s)",
        chain.expanded_points.len()
    );

    // ===== 步骤 13 之前：构造 GatewayRuntime / AppState =====
    let disk_status = Arc::new(gateway_core::engine::monitor::health::DiskStatus::new());
    let gateway = Arc::new(GatewayRuntime::new(GatewayComponents {
        bus_managers: chain.bus_managers.clone(),
        mqtt: north.mqtt.clone(),
        scheduler: scheduler.clone(),
        queue: north.queue.clone(),
        ntp: chain.ntp.clone(),
        serial_mgr: SerialManager::new(),
        metrics: gateway_metrics.clone(),
        disk_status: disk_status.clone(),
        convert: north.convert.clone(),
        virtual_points: north.virtual_points.clone(),
    }));

    // Bug C8: 把配置里的 OTA 签名密钥接入运行时的 OtaManager——原实现恒传 None，
    // 导致 web.ota_signing_key 配了也从不验签（强制签名形同虚设）。
    // 未配置 → None（保持向后兼容）；配置了但 base64 不合法 → fail-closed 阻断启动。
    let ota_signing_key = match config
        .web
        .as_ref()
        .and_then(|w| w.ota_signing_key.as_deref())
    {
        Some(s) if !s.is_empty() => Some(
            gateway_core::platform::ota::parse_signing_key(s)
                .context("解析 web.ota_signing_key 失败")?,
        ),
        _ => None,
    };
    let deps = build_app_state_dependencies(ota_signing_key, gateway_metrics.clone())?;

    // ===== P4：硬件看门狗 abort 前的 flush 回调接线 =====
    // 看门狗超时 = 进程卡死，此时唯一的补救是在 `std::process::abort()` 前把
    // 内存态落盘。此前 `set_flush_callback` 零调用 ⇒ 该分支直接 abort，
    // 内存 fallback 缓冲里的数据点与 OTA 进行中标记全部丢失——
    // 保护机制在自己最该生效的场景下反而丢数据。
    //
    // 回调体全部同步执行（卡死场景下 tokio 运行时可能无法调度，异步路径不可用），
    // 实现见 core 的 `platform::watchdog_flush`；这里只做装配。
    gateway_core::platform::watchdog_flush::install_watchdog_flush(
        north.queue.clone(),
        deps.ota.clone(),
        std::path::PathBuf::from(gateway_core::shared::default_watchdog_state()),
    );
    info!(
        "watchdog flush callback installed (state={})",
        gateway_core::shared::default_watchdog_state()
    );

    spawn_software_watchdog(gateway_metrics.clone());

    let web_cfg = prepare_web_config(config)?;

    let passthrough_manager: Option<Arc<MqttPacketPassthrough>> = if config.mqtt.enabled {
        north.mqtt.clone().map(|mqtt_client| {
            MqttPacketPassthrough::new(mqtt_client, gateway.clone(), &config.mqtt.topic)
        })
    } else {
        None
    };

    // License 初始化（§12 路径解析 + §7 启动校验 + load）
    let license = init_license(config);

    // ===== Step 13f: 蜂窝模块（仅配置 enabled 时构造并启动）=====
    // 此前 PlatformRefs.cellular 硬编码 None：cellular 配置可写、可校验、可计费，
    // 但运行时从不实例化，`/api/cellular/status` 永远为空。此处按 enabled 构造并启动
    // （非 Linux 平台只跑 monitor/failover），实例交由 graceful_shutdown 统一停机。
    let cellular: Option<Arc<gateway_core::platform::cellular::CellularManager>> =
        config.cellular.clone().filter(|c| c.enabled).map(|cfg| {
            let mgr = gateway_core::platform::cellular::CellularManager::new(cfg);
            mgr.clone().start();
            info!("cellular manager started");
            mgr
        });

    // ===== Step 13c: 协议转换层（南向采集 -> 北向服务）=====
    //
    // 审计实例提前到北向 Server 启动之前：OPC UA 写回需要与 AppState 共用
    // 同一个 AuditLog（同一个写线程/文件句柄），避免两个实例并发写同一文件。
    let audit = Arc::new(gateway_core::engine::audit::AuditLog::new(
        std::path::PathBuf::from(default_audit_log()),
    ));
    let (north_servers, north_dispatcher_run) = start_north_servers(
        config,
        &north.north_dispatcher,
        &north.cert_pair,
        &north.cert_manager,
        &gateway,
        &audit,
        &chain.expanded_points,
    );

    let app_state = build_app_state(AppStateInputs {
        config,
        config_loader,
        scheduler: &scheduler,
        chain,
        north: &*north,
        deps,
        web_cfg: &web_cfg,
        gateway: &gateway,
        passthrough_manager: &passthrough_manager,
        north_servers: north_servers.clone(),
        license: license.manager.clone(),
        cellular: cellular.clone(),
        audit: audit.clone(),
    });

    // ===== P2-1: 保证配置版本库有基线快照 =====
    // 版本快照此前只在 POST /api/config 保存时写入：全新安装/换机导入后从未通过
    // 网页保存过配置，版本列表就是空的，"回滚到出厂配置"入口形同虚设。启动时
    // 若版本库为空，就把当前生效配置落成基线（后续保存会自然追加新版本）。
    seed_baseline_config_version(&app_state);

    // 启动 license 每日重校验后台任务（§7 refresh_loop）
    spawn_license_refresh_loop(license.manager.clone());
    // 启动 NTP 校时 → 时钟基准对账任务（§9：断电恢复场景自动洗白，无需重启）
    spawn_license_ntp_resync_watch(license.manager.clone(), chain.ntp.clone());
    // 历史保留天数与 license 无关（v2.1：配额只管采集点数），队列启动即固定 30 天，
    // 由 maintenance 周期按 FIFO 截断执行；如需调整走运维配置，不再随 license 变化。

    // ===== Step 13i: 计划任务调度执行器 =====
    // 轮询 scheduled_tasks 表的到期任务并执行（读点位/备份配置/NTP 校时）。
    // panic 由 spawn_supervised 按退避重启，任务执行失败只回写记录不影响循环。
    {
        let st = app_state.clone();
        spawn_supervised("scheduled_tasks", move || {
            let st = st.clone();
            async move {
                gateway_api::web::api::access::executor::run_loop(st).await;
            }
        });
        info!("scheduled task executor started");
    }

    // ===== Step 13a: 启动 Web Server =====
    // r74-SD1：捕获 ShutdownSignal + run 句柄，透传到停机序（停机最前触发）。
    let (web_shutdown, web_run) = start_web_server(&app_state, &web_cfg)?;

    // ===== Step 13d: 报警引擎（限值报警 + MQTT 上报）=====
    // r73-SD1：捕获 AlarmEngine + run 句柄，透传到停机序。
    let (alarm_engine, alarm_engine_run) =
        start_alarm_engine(chain, north, &config.mqtt.topic, gateway_metrics.clone());

    // ===== Step 13e: 透传模式（透明传输）=====
    // r73-SD1：捕获 MqttPacketPassthrough + run 句柄，透传到停机序。
    let (passthrough, passthrough_run) =
        start_passthrough_mode(&passthrough_manager, &config.mqtt.topic);

    // ===== 后台监控任务 =====
    // Bug 85: spawn_disk_monitor 内部使用 spawn_supervised（self-recovery），
    // 不返回 JoinHandle，因此 health_handles 始终为空；graceful_shutdown 中
    // 的 abort() 在空 Vec 上是 no-op（符合预期）。
    let health_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    // ===== Step 13g: 自愈执行器（opt-in，默认关闭，存量部署零影响）=====
    // P4: 参数从 `[self_heal]` 配置节读取（缺省即 enabled=false）→ 默认不 spawn，
    // 运行时不产生任何调用者，行为与接入前一致。
    let self_heal = {
        let settings: gateway_api::web::heal::SelfHealSettings =
            config.self_heal.clone().map(Into::into).unwrap_or_default();
        if settings.enabled {
            let (cancel, handle) =
                gateway_api::web::heal::spawn_executor(app_state.clone(), settings);
            info!("self-heal executor started");
            Some(SelfHealShutdown { cancel, handle })
        } else {
            info!("self-heal executor disabled (opt-in; enable via [self_heal] config)");
            None
        }
    };

    // P3：统一北向通道表——顺序即停机序：webhook（NorthProcessing 已装入，
    // mem::take 保持原顺序）-> 报警引擎 -> 透传管理器。
    let mut north_channels = std::mem::take(&mut north.channels);
    north_channels.extend(NorthChannel::new(
        "alarm engine",
        alarm_engine,
        alarm_engine_run,
    ));
    north_channels.extend(NorthChannel::new(
        "passthrough",
        passthrough,
        passthrough_run,
    ));

    Ok((
        app_state,
        GatewayHandles {
            scheduler,
            north_servers,
            health_handles,
            cellular,
            self_heal,
            // r66-SD1：NorthDispatcher run 循环 JoinHandle（fan-out 退出等待）。
            north_dispatcher_run: Some(north_dispatcher_run),
            // P3：北向 run 循环通道表（顺序即停机序）。
            north_channels,
            // r74-SD1：Web 停机信号 + run 句柄，停机序最前触发优雅停机。
            web_shutdown: Some(web_shutdown),
            web_run: Some(web_run),
        },
    ))
}

#[cfg(test)]
mod baseline_version_tests {
    use super::*;

    /// P2-1 回归：空版本库落基线且幂等。
    ///
    /// 线上问题：版本库只在网页保存配置时写入，全新部署从未保存过配置，
    /// `GET /api/config/versions` 永远是空列表（"回滚到出厂配置"无从谈起）。
    #[test]
    fn seeds_baseline_only_when_history_is_empty() {
        // 用文件库而非 ":memory:"：与其他 Store 一样走长驻连接，
        // 内存库在 Store drop 后即消失，不利于断言落盘结果。
        let tmp = tempfile::tempdir().expect("create temp dir");
        let db_path = tmp.path().join("version-baseline.db");
        let store =
            gateway_core::config::ConfigVersionStore::new(db_path.to_str().unwrap()).unwrap();
        let cfg = GatewayConfig::default();

        assert!(
            seed_baseline_version_if_empty(&store, &cfg),
            "空版本库应落基线快照"
        );
        let versions = store.list_versions().unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].note, "initial baseline");

        // 幂等：已有历史时重启不再写第二条基线
        assert!(!seed_baseline_version_if_empty(&store, &cfg));
        assert_eq!(store.list_versions().unwrap().len(), 1);

        // 回滚输入可用：基线快照能按 id 读回，且是完整配置 JSON（含 schema_version）
        let baseline = store.get_version(1).unwrap().expect("baseline readable");
        assert!(
            baseline.config_json.contains("schema_version"),
            "基线快照应为可回滚的完整配置, got {}",
            baseline.config_json
        );
    }
}
