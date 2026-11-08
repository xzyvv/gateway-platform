//! Web Server 共享类型
//!
//! 包含 AppState、共享请求/响应类型。
//! 鉴权中间件见 `super::middleware`，错误类型见 `super::error`。

/// 嵌入式 Web UI 兜底页（单文件 HTML，编译进二进制）
///
/// 当 `GATEWAY_WEB_DIR` 指向的目录不存在 `index.html` 时，返回此兜底页面。
/// 完整调试控制台由 CI 把 `gateway-web/dist/` 复制到 `gateway-core/ui-dist/` 后提供。
pub(crate) const WEB_UI_HTML: &str = include_str!("builtin_ui.html");
use std::sync::Arc;

use arc_swap::ArcSwap;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use gateway_core::config::ConfigVersionStore;
use gateway_core::config::TemplateManager;
use gateway_core::config::{ConfigLoader, GatewayConfig, WebConfig};
use gateway_core::engine::Diagnostics;
use gateway_core::north::server::Server;
use gateway_core::north::sim_consumer::SimConsumer;
use gateway_core::platform::GpioController;
use gateway_core::platform::NtpManager;

// 兼容 re-export：原有 `crate::web::types::ApiError` / `require_auth` 引用无需改动
pub use super::error::ApiError;
pub(crate) use super::middleware::{
    require_admin_or_exec_scope, require_auth, require_auth_if_anon_disabled,
};
// F-11：调度时序响应体定义在 core 采集层，但 `web/api` handler 层只允许引用
// 白名单模块（见 `gateway-api/tests/arch_deps.rs`），故在 web 契约层统一透出，
// handler 不必直引 `gateway_core::south`。
pub(crate) use gateway_core::south::scheduler::ScheduleTimeline;
// 点位运行态快照（强类型）同 F-11 处理：定义在 core 采集层，此处统一透出，
// `web/api` handler 层不必直引 `gateway_core::south`。
pub(crate) use gateway_core::south::scheduler::DeviceStatus;
// RealtimeBus 的消费端类别与 Lagged 快照定义在 core 编排层（`engine::bus`），
// `web/api` handler 层允许直引 `gateway_core::engine`，此处统一透出契约类型。
pub(crate) use gateway_core::engine::bus::{BusConsumerKind, LaggedCounts};

/// 配置相关依赖聚合：动态配置、加载器、reload 锁、版本库、模板、上行测试历史
///
/// `Clone` 必须**手写**——`ArcSwap<T>` 在其默认 `HybridStrategy` 下不实现
/// `Clone`（`Clone for ArcSwapAny<T, S>` 要求 `S: LockStorage`，而 `DefaultStrategy`
/// 的内部 strategy 未必实现），其它字段全部 `Clone`，手工 clone 仍能收敛到
/// 8 行内，避免每加一个字段都得多处同步。
pub struct ConfigRefs {
    pub config: ArcSwap<GatewayConfig>,
    pub config_loader: ConfigLoader,
    pub config_reload_lock: Arc<tokio::sync::Mutex<()>>,
    pub version_store: Arc<ConfigVersionStore>,
    pub templates: Arc<TemplateManager>,
    pub uplink_test_history: Arc<gateway_core::config::UplinkTestHistoryStore>,
}

impl Clone for ConfigRefs {
    fn clone(&self) -> Self {
        // `ArcSwap::from(load_full())`：克隆内部 Arc，引用计数 +1，语义与
        // `arc_swap::ArcSwap::from(arc_swap::ArcSwap::load_full(...))` 一致。
        Self {
            config: arc_swap::ArcSwap::from(self.config.load_full()),
            config_loader: self.config_loader.clone(),
            config_reload_lock: self.config_reload_lock.clone(),
            version_store: self.version_store.clone(),
            templates: self.templates.clone(),
            uplink_test_history: self.uplink_test_history.clone(),
        }
    }
}

/// 北向依赖聚合：MQTT/OTA/证书/网关组件管理器
///
/// 不持有 MQTT 客户端快照：热重载会替换/移除客户端，快照必然过期。
/// 需要 MQTT 客户端时一律走 [`NorthRefs::has_north_consumer`] 所依赖的
/// `gateway.mqtt()`（实时值）。
///
/// 注：`gateway` 字段是 `Arc<dyn GatewayTrait>` —— `Arc<T: ?Sized>` 实现 `Clone`
/// 但**不会**复制底层 trait 对象实例，而只是把引用计数 `+1`，与原手写 Clone 行为一致。
#[derive(Clone)]
pub struct NorthRefs {
    pub ota: Arc<dyn crate::web::traits::OtaTrait>,
    pub cert_manager: Arc<gateway_core::crypto::cert::CertManager>,
    pub gateway: Arc<dyn crate::web::traits::GatewayTrait>,
    pub north_servers: Vec<Arc<dyn Server>>,
    /// N-7：北向 fan-out 管理器——状态页从这里读各 connector 的真实
    /// `health()` 与实时广播丢帧计数，而不是按配置猜连通性。
    pub conversion: Arc<gateway_core::north::ConversionManager>,
    /// 内置模拟消费者（SimConsumer）：无 MQTT 通道时的兜底消费者。
    ///
    /// **恒为 Some**（即便配置未启用）——它是一个可运行时启停的组件，
    /// 「是否存在消费者」以 `is_running()` 的实际运行态为准，而不是配置意图。
    pub sim_consumer: Arc<SimConsumer>,
    /// 北向专用队列发送端（第 5 层）：`/apin` 从这里读积压/背压计数。
    ///
    /// `None` 仅出现在测试或降级装配（无处理管道）场景，此时不输出相关指标。
    pub north_feed: Option<gateway_core::north::NorthSender>,
    /// 死信队列（Webhook 不可投递批次）。
    ///
    /// 打不开 outbox 库时用 [`crate::web::traits::NoopDeadLetter`] 占位，
    /// 让北向故障不至于把整个 Web 面拖挂——死信查询是运维辅助功能，
    /// 不该成为网关启动的硬依赖。
    pub dead_letter: Arc<dyn crate::web::traits::DeadLetterTrait>,
}

impl NorthRefs {
    /// **「是否存在会消费数据队列的北向消费者」的唯一口径**。
    ///
    /// 队列健康判定（`queue_health`）的 `has_north_consumer` 参数必须取自这里，
    /// 否则同一队列长度会在 `/health/ready`、`/api/health/report`、
    /// `/api/watchdog/services` 与后台积压告警监控之间给出互斥结论。
    ///
    /// 两个消费者，都会真的 dequeue → 处理 → ack：
    /// - MQTT publish 循环：取 `gateway.mqtt()` 的**实时**值（热重载替换客户端后
    ///   立即跟随变化），任何 handler 需要 MQTT 客户端时也必须走这条路径；
    /// - 内置模拟消费者：取 `is_running()` 的**实际运行态**（可运行时启停）。
    ///
    /// 注：MQTT 客户端「存在但未连接」也算有消费者——publish 循环仍在运行，
    /// 只是暂时无法送达；这属于「上行故障」，由 `MQTT_DISCONNECTED` 单独告警，
    /// 不应在这里被误判成「无消费者」。
    pub fn has_north_consumer(&self) -> bool {
        self.gateway.mqtt().is_some() || self.sim_consumer.is_running()
    }
}

/// 南向依赖聚合：调度器、队列、实时总线、透传
#[derive(Clone)]
pub struct SouthRefs {
    pub scheduler: Arc<dyn crate::web::traits::SchedulerTrait>,
    pub queue: Arc<dyn crate::web::traits::DataQueueTrait>,
    pub realtime_bus: Arc<gateway_core::pipeline::RealtimeBus>,
    pub passthrough: Option<Arc<gateway_core::north::server::MqttPacketPassthrough>>,
}

impl ConfigRefs {
    pub fn load_full(&self) -> std::sync::Arc<GatewayConfig> {
        self.config.load_full()
    }
    pub fn load(&self) -> std::sync::Arc<GatewayConfig> {
        self.config.load_full()
    }
    pub fn store(&self, new_cfg: std::sync::Arc<GatewayConfig>) {
        self.config.store(new_cfg);
    }
}

/// 平台基础设施依赖聚合：GPIO/NTP/蜂窝/诊断/指标/磁盘历史/审计/license
#[derive(Clone)]
pub struct PlatformRefs {
    pub gpio: Arc<Mutex<GpioController>>,
    pub ntp: Arc<NtpManager>,
    pub cellular: Option<Arc<gateway_core::platform::cellular::CellularManager>>,
    pub diagnostics: Arc<Diagnostics>,
    pub metrics: Arc<gateway_core::engine::metrics::Metrics>,
    pub disk_history: Arc<gateway_core::engine::monitor::health::DiskHistoryStore>,
    /// 自愈事件存储（真实执行记录，供 /api/watchdog/events 读取）
    pub heal_events: Arc<crate::web::heal::HealEventStore>,
    /// License 校验器（启动期创建，挂在平台 refs 而非 config refs，
    /// 因为它的存在与配置节独立——未配置 license 节时也照常构造）。
    pub license: Arc<gateway_core::license::LicenseManager>,
    /// 审计日志（跨域横切：config/devices/history/uplink/system 等域均写入，
    /// 故归平台基础设施 refs，而非任一功能域 refs）。
    pub audit: Arc<gateway_core::engine::audit::AuditLog>,
    /// API 限流规则 + 访问日志（SQLite 持久化，中间件在请求路径上执行规则）
    pub rate_rules: Arc<gateway_core::config::RateRuleStore>,
    /// 数据质量规则（SQLite 持久化；启用规则安装到处理管道快照后真实执行）
    pub quality_rule_store: Arc<gateway_core::config::QualityRuleStore>,
    /// API 密钥（SQLite，仅存哈希与预览；鉴权中间件查表）
    pub api_keys: Arc<gateway_core::config::ApiKeyStore>,
    /// 计划任务（SQLite；调度执行器轮询到期任务并回写执行结果）
    pub sched_tasks: Arc<gateway_core::config::ScheduledTaskStore>,
}

/// 共享应用状态（按子系统分组：config / north / south / platform）
///
/// Clone 由字段 derive 自动派生：所有字段都是 `Arc<T>` / `ArcSwap<T>` /
/// `Clone` 子结构，对底层状态仅做引用计数 `+1`，语义与原手写 Clone 完全一致。
/// 注意：当字段存在 `WebConfig`（不含 Arc），需要 `WebConfig: Clone`（已具备）。
#[derive(Clone)]
pub struct AppState {
    pub config: ConfigRefs,
    pub north: NorthRefs,
    pub south: SouthRefs,
    pub platform: PlatformRefs,
    pub web: WebConfig,
    /// 全局速率限制状态（每实例独立，避免多 Router 串扰）
    pub rate_limiter: Arc<crate::web::middleware::RateLimiter>,
}

/// 版本信息
#[derive(Serialize, ts_rs::TS)]
#[ts(export)]
pub(crate) struct VersionInfo {
    pub(crate) version: &'static str,
    pub(crate) build_time: &'static str,
    pub(crate) git_hash: &'static str,
}

/// 队列健康阈值（行数）。
///
/// BUG-L：前端原本自己写死 `queueWarning=100 / queueError=1000`，而后端唯一口径是
/// `gateway_core::queue::limit` 的 `BACKLOG_WARN_ROWS=10_000` /
/// `BACKPRESSURE_ROWS=100_000`。同一队列长度在「总览告警」与「健康报告」里
/// 会得到完全相反的结论。这里把后端阈值随 `/api/status` 一起下发，前端只消费不猜测。
#[derive(Serialize, ts_rs::TS)]
#[ts(export)]
pub(crate) struct QueueThresholds {
    /// 积压预警阈值（行）：超过即提示「观察是否继续增长」
    pub(crate) warning: i64,
    /// 背压/告警阈值（行）：超过即判定上行消费不及，持续丢数风险
    pub(crate) error: i64,
}

/// 运行状态
///
/// Dashboard 一次性返回的汇总字段：避免前端 N+1 轮询
/// （device 列表 / bus 列表 / 指标猜测表）。
/// `today_alerts` / `collect_today` 由后端 Metrics 的当日滚动计数器提供
/// （UTC 跨日自动清零：告警在 AlarmEngine 触发时累加、采集在 record_collect 时累加）。
#[derive(Serialize, ts_rs::TS)]
#[ts(export)]
pub(crate) struct RunStatus {
    pub(crate) online_devices: usize,
    pub(crate) total_devices: usize,
    pub(crate) total_points: usize,
    pub(crate) bus_count: usize,
    pub(crate) queue_length: usize,
    /// 队列阈值（后端单一来源）；旧版本服务端不带该字段时前端回落到内置默认值
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) queue_thresholds: Option<QueueThresholds>,
    pub(crate) cpu_usage: f32,
    pub(crate) mem_usage: f32,
    pub(crate) today_alerts: u32,
    pub(crate) collect_today: u32,
    /// **点位粒度**的运行态行（一行 = 一个点位，不是一台设备）。
    ///
    /// 来源 `state.south.scheduler.device_status()`：模板展开后的每个点位都有一行，
    /// 因此 `point_rows.len()` ≥ 设备数（1 台 4 点位的设备会产出 4 行）。
    /// 设备级在线数请看 `online_devices` / `total_devices`（已按 device_id 去重）。
    /// 字段名从 `devices` 更名而来：旧名让消费方误以为是设备粒度
    /// （实测 4 行 / `total_devices`=1）。
    pub(crate) point_rows: Vec<DeviceStatus>,
}

/// IO 控制请求
#[derive(Deserialize)]
pub(crate) struct IoControlRequest {
    pub(crate) action: String,
}

/// OTA 状态
#[derive(Serialize, Default, ts_rs::TS)]
#[ts(export)]
pub(crate) struct OtaStatus {
    pub(crate) progress: u8,
    pub(crate) status: String,
    pub(crate) error: Option<String>,
}

/// 内置模拟消费者（SimConsumer）状态快照
///
/// 字段口径：
///
/// - `enabled` = **配置意图**（`[sim_consumer].enabled`，启动时是否自动拉起）；
/// - `running` = **实际运行态**（run 循环是否在跑）。
///
/// 两者必须同时下发：只看 `enabled` 无法区分「配置开了但被 MQTT 互斥挡住」
/// 与「根本没配」，只看 `running` 又无法解释「为什么没跑」。
#[derive(Serialize, ts_rs::TS)]
#[ts(export)]
pub(crate) struct SimConsumerStatus {
    /// 是否正在运行（实际运行态）。
    pub(crate) running: bool,
    /// 配置意图：启动时是否自动拉起。
    pub(crate) enabled: bool,
    /// 累计成功消费（ack）的数据条数。
    pub(crate) consumed: u64,
    /// 累计成功消费的批次数。
    pub(crate) batches: u64,
    /// 当前队列积压条数（实时读取队列长度）。
    pub(crate) queue_len: usize,
    /// 最近一次消费成功时刻（epoch 毫秒）；尚未消费过时为 null。
    pub(crate) last_consumed_at_ms: Option<u64>,
    /// 本次运行启动时刻（epoch 毫秒）；未运行时为 null。
    pub(crate) started_at_ms: Option<u64>,
    /// 本次运行已持续时长（毫秒）；未运行时为 null。
    pub(crate) uptime_ms: Option<u64>,
    /// 单批出队条数。
    pub(crate) batch_size: usize,
    /// 队列为空时的轮询间隔（毫秒）。
    pub(crate) interval_ms: u64,
    /// 每消费满多少条打一条 INFO 摘要日志（0 = 不打）。
    pub(crate) log_every: usize,
    /// 无法启动的可读原因（如 `mqtt_enabled`）；无阻塞时为 null。
    pub(crate) blocked_reason: Option<String>,
}

/// 历史查询参数
#[derive(Deserialize)]
pub(crate) struct HistoryQuery {
    pub(crate) point_id: String,
    /// 起始时间戳（毫秒）。缺省为 `end` 往前 24 小时
    #[serde(default)]
    pub(crate) start: Option<i64>,
    /// 结束时间戳（毫秒）。缺省为当前时间
    #[serde(default)]
    pub(crate) end: Option<i64>,
    #[serde(default = "default_limit")]
    pub(crate) limit: usize,
    /// 降采样时间桶宽度（秒）。缺省 / 0 = 不降采样（返回原始行）。
    ///
    /// P2-1：桶宽 ≥ 60 且窗口跨过 24 小时时，旧段直接读分钟聚合表
    /// （`data_queue_aggregated`），避免长窗口查询扫原始大表。
    #[serde(default)]
    pub(crate) downsample: Option<u32>,
}

fn default_limit() -> usize {
    1000
}

// ===== 统一响应体 =====

/// Vue axios 友好的统一响应格式
#[derive(Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub code: i32,
    pub message: String,
    pub data: Option<T>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn ok(data: T) -> Self {
        Self {
            code: 0,
            message: "success".into(),
            data: Some(data),
        }
    }
    pub fn err(code: i32, msg: impl Into<String>) -> Self {
        Self {
            code,
            message: msg.into(),
            data: None,
        }
    }
}

impl<T: Serialize> axum::response::IntoResponse for ApiResponse<T> {
    fn into_response(self) -> axum::response::Response {
        use axum::http::StatusCode;
        use axum::response::Json;
        let code = self.code;
        // 业务错误码按区间映射 HTTP 状态：
        // - 4xxx：参数/资源/校验类错误（4000 参数非法、4004 不存在、4005-4007 用户域、
        //   4002 测点校验、4009 id 冲突）→ 400
        // - 6xxx：设备分组等业务校验（6001 名称非法/重复、6002 分组不存在）→ 400
        // 这两类都是客户端可纠正的错误；此前只认 4xxx，6xxx 被误映射成 500。
        // 5xxx 保持 500：服务端执行失败（5101 写点位、证书解析失败）。
        let status = if code == 0 {
            StatusCode::OK
        } else if (4000..5000).contains(&code) || (6000..7000).contains(&code) {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        (
            status,
            Json(serde_json::to_value(&self).unwrap_or_default()),
        )
            .into_response()
    }
}

#[cfg(test)]
mod status_mapping_tests {
    use super::*;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    fn err_status(code: i32) -> StatusCode {
        ApiResponse::<()>::err(code, "x").into_response().status()
    }

    #[test]
    fn client_correctable_codes_map_to_400() {
        assert_eq!(err_status(4000), StatusCode::BAD_REQUEST);
        assert_eq!(err_status(4004), StatusCode::BAD_REQUEST);
        // 用户域校验码（原 5001/5002/5003，方案 B 后归入 4xxx 以得到 400）
        assert_eq!(err_status(4005), StatusCode::BAD_REQUEST);
        assert_eq!(err_status(4006), StatusCode::BAD_REQUEST);
        assert_eq!(err_status(4007), StatusCode::BAD_REQUEST);
        // 设备分组业务码：此前被误映射成 500
        assert_eq!(err_status(6001), StatusCode::BAD_REQUEST);
        assert_eq!(err_status(6002), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn server_side_codes_stay_500_and_ok_is_200() {
        assert_eq!(err_status(5101), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(err_status(5001), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(ApiResponse::ok(()).into_response().status(), StatusCode::OK);
    }
}

#[cfg(test)]
mod export_ts {
    use gateway_core::config::{ConfigVersion, DeviceConfig, GatewayConfig, PointConfig};
    use ts_rs::TS;

    #[test]
    fn export_ts_types() {
        let out = std::path::Path::new("../gateway-web/src/api/generated");
        std::fs::create_dir_all(out).expect("create generated dir");
        GatewayConfig::export_all_to(out).expect("GatewayConfig");
        PointConfig::export_all_to(out).expect("PointConfig");
        DeviceConfig::export_all_to(out).expect("DeviceConfig");
        ConfigVersion::export_all_to(out).expect("ConfigVersion");
        crate::web::types::VersionInfo::export_all_to(out).expect("VersionInfo");
        crate::web::types::QueueThresholds::export_all_to(out).expect("QueueThresholds");
        crate::web::types::RunStatus::export_all_to(out).expect("RunStatus");
        crate::web::types::OtaStatus::export_all_to(out).expect("OtaStatus");
        crate::web::types::SimConsumerStatus::export_all_to(out).expect("SimConsumerStatus");
        // D3：死信 DTO（列表响应里嵌 `DeadLetterDto[]`）
        crate::web::traits::DeadLetterDto::export_all_to(out).expect("DeadLetterDto");
        gateway_core::south::protocol::registry::ProtocolCapabilities::export_all_to(out)
            .expect("ProtocolCapabilities");
        // F-11：调度时序端点响应体（含嵌套 TimelineGroup/Sample/Overlap）
        gateway_core::south::scheduler::ScheduleTimeline::export_all_to(out)
            .expect("ScheduleTimeline");
        println!("TS types exported to {:?}", out);
    }
}
