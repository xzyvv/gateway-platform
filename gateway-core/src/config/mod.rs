//! 配置管理（§3 + §4）
//!
//! 顶层配置 `GatewayConfig` 从 `/etc/gateway/config.json` 加载，
//! 启动时 serde 反序列化 + validator 校验，失败则备份并生成默认配置。

use serde::{Deserialize, Serialize};
use validator::Validate;

mod backoff;
mod bus;
mod cellular;
mod convert;
mod loader;
mod migrate;
mod modbus_passthrough;
mod modbus_server;
mod mqtt;
mod ntp;
mod opcua;
mod point;
mod reload_scope;
mod schema;
mod self_heal;
mod shard;
mod sim_consumer;
mod store;
mod telemetry;
mod template;
mod validate;
mod web;
mod webhook;

mod groups_assets;
mod license;
mod virtual_point;

// S6: 配置向后兼容回归（crate 内单测，随 `cargo test --lib` 门禁）
#[cfg(test)]
mod compat;

pub use backoff::ReconnectBackoffConfig;
pub use bus::{BusConfig, OpcUaClientConfig};
pub use cellular::CellularConfig;
pub use convert::{ConvertEnumItemConfig, ConvertRuleConfig};
pub use groups_assets::{DeviceAsset, DeviceGroup};
pub use license::{LicenseConfig, TierQuotaEntry};
pub use loader::ConfigLoader;
pub use modbus_passthrough::ModbusPassthroughConfig;
pub use modbus_server::ModbusServerConfig;
pub use mqtt::MqttConfig;
pub use ntp::NtpConfig;
pub use opcua::{OpcuaConfig, OpcuaUserConfig};
pub use point::{AggregationConfig, DeviceConfig, PointConfig, PointTrashEntry};
pub use reload_scope::{reload_scope, ReloadScope};
pub use self_heal::SelfHealConfig;
pub use shard::expand_includes;
pub use sim_consumer::SimConsumerConfig;
pub use store::{
    ApiAccessLog, ApiKeyRecord, ApiKeyStore, ConfigVersion, ConfigVersionStore, QualityRule,
    QualityRuleStore, RateLimitStats, RateRule, RateRuleStore, ScheduledTask, ScheduledTaskStore,
    TaskRunStatus, TaskType,
};
pub use telemetry::TelemetryConfig;
pub use template::{DeviceTemplate, TemplateManager};
pub(crate) use validate::validate_protocol;
pub use virtual_point::VirtualPoint;
pub use web::{UserRole, WebConfig, WebUser};
pub use webhook::WebhookConfig;
// 校验函数移至 validate.rs，重新导出以兼容子模块 `use super::{validate_*}` 引用
pub use schema::{export_to_file, validate_value};
pub(crate) use validate::*;
// exposed for gateway-api (web) pre-submit collection validation
pub use validate::validate_collection_config;

/// 敏感配置字段相对路径清单（相对配置根，JSON Pointer 形式，含前导 `/`）。
///
/// **唯一来源**：GET 响应脱敏（gateway-api `redact_config_secrets`）与保存回写还原
/// （gateway-api `restore_redacted_secrets`）都必须以此为准。历史上两处各维护一份
/// 字段表，新增含密字段时漏改一边，会让前端回传的 `"********"` 占位串被当作真实
/// 密文落盘，永久覆盖密钥。
///
/// 匹配语义是**路径完全相等**（无前缀匹配 / 通配 / 数组下标）：调用方逐项按路径
/// 精确取值。`webhook.headers` 是动态键的 Map，无法用静态路径表达，另见
/// [`SENSITIVE_SECRET_HEADERS_PATH`]。
pub fn sensitive_field_paths() -> &'static [&'static str] {
    &[
        "/web/api_token",
        "/web/password_hash",
        "/web/ota_signing_key",
        "/mqtt/username",
        "/mqtt/password",
        "/cellular/username",
        "/cellular/password",
        "/cellular/pin",
    ]
}

/// 动态键敏感 Map 的路径（相对配置根）：`webhook.headers` 的值常装 Authorization
/// 等凭据，需逐键脱敏/还原，不能作为静态字段路径列出。
pub const SENSITIVE_SECRET_HEADERS_PATH: &str = "/webhook/headers";

/// 当前配置 Schema 版本号
///
/// 每次结构性变更（新增必填字段、语义变更）时递增，
/// 旧配置通过 `migrate()` 逐步升级到当前版本。
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// 顶层配置（§3.1）
#[derive(Debug, Clone, Serialize, Deserialize, Validate, schemars::JsonSchema, ts_rs::TS)]
#[ts(export)]
pub struct GatewayConfig {
    /// 配置 Schema 版本号，用于版本迁移。
    /// 旧配置（无此字段）反序列化为 0，由 migrate() 升级。
    #[serde(default)]
    pub schema_version: u32,

    /// 总线配置列表（嵌套校验：每条 bus 的 baud_rate 必须合法）
    #[validate(nested)]
    pub buses: Vec<BusConfig>,

    /// 逐点模式点位列表（嵌套校验：每个 point 的 interval >= 1）
    #[validate(nested)]
    pub points: Vec<PointConfig>,

    /// 点位回收站（软删除）。为空时不落盘，存量配置零影响。
    /// `ts(optional)`：与 `skip_serializing_if` 的「None 时省略键」行为对齐，
    /// 让生成的 TS 类型把该字段标为可选（ts-rs 不解析 skip_serializing_if）。
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub point_trash: Option<Vec<PointTrashEntry>>,

    /// 协议转换规则列表（C1 通用值变换模板：raw/linear/enum）。
    /// 为空时不落盘，存量配置零影响；运行时由 pipeline::convert::ConvertEngine 执行。
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub convert_rules: Option<Vec<ConvertRuleConfig>>,

    /// 模板模式设备列表（与 points 二选一）
    #[serde(default)]
    pub devices: Option<Vec<DeviceConfig>>,

    /// MQTT 上传配置（嵌套校验：topic/qos/publish_interval）
    #[validate(nested)]
    pub mqtt: MqttConfig,

    /// 内置模拟消费者配置（默认不启用）。
    ///
    /// 没有真实北向通道（MQTT）时打开它：队列数据仍会按批「出队 → 处理 → ack」，
    /// 走完完整链路，消除「有队列积压却没有消费者」的告警歧义。
    /// 与 MQTT 同时开启会导致两个消费者分流同一批数据，故二者互斥使用。
    #[serde(default)]
    pub sim_consumer: SimConsumerConfig,

    /// NTP 配置，None=用默认值
    #[serde(default = "ntp::default_ntp")]
    pub ntp: Option<NtpConfig>,

    /// Web 服务配置（鉴权等）
    #[serde(default)]
    pub web: Option<WebConfig>,

    /// HTTP Webhook 北向上报配置（默认不启用）
    #[serde(default)]
    pub webhook: Option<WebhookConfig>,

    /// 4G 蜂窝网络配置（默认不启用）
    #[serde(default)]
    pub cellular: Option<CellularConfig>,

    /// OPC UA Server 配置（默认不启用，F2）
    #[serde(default)]
    pub opcua: Option<OpcuaConfig>,

    /// Modbus Server（北向）配置（默认不启用）
    #[serde(default)]
    pub modbus_server: Option<ModbusServerConfig>,

    /// Modbus TCP↔RTU 透传配置（默认不启用）
    #[serde(default)]
    pub modbus_passthrough: Option<ModbusPassthroughConfig>,
    /// 设备分组列表（持久化，用户配置）
    #[serde(default)]
    pub device_groups: Option<Vec<DeviceGroup>>,

    /// 设备资产标签列表（持久化，资产生命周期）
    #[serde(default)]
    pub device_assets: Option<Vec<DeviceAsset>>,

    /// 虚拟点位列表（持久化，公式派生）
    #[serde(default)]
    pub virtual_points: Option<Vec<VirtualPoint>>,

    /// OTel 遥测配置（默认不启用，§2.3）
    #[serde(default)]
    pub telemetry: Option<TelemetryConfig>,

    /// License 校验配置（默认不启用：未导入 license 时一切照常，tier=core）
    #[serde(default)]
    pub license: Option<LicenseConfig>,

    /// 自愈执行器配置（默认不启用：opt-in，存量部署零影响）
    #[serde(default)]
    #[validate(nested)]
    pub self_heal: Option<SelfHealConfig>,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            buses: Vec::new(),
            points: Vec::new(),
            point_trash: None,
            convert_rules: None,
            devices: None,
            mqtt: MqttConfig::default(),
            sim_consumer: SimConsumerConfig::default(),
            ntp: Some(NtpConfig::default()),
            web: None,
            cellular: None,
            opcua: None,
            modbus_server: None,
            modbus_passthrough: None,
            webhook: None,
            device_groups: None,
            device_assets: None,
            virtual_points: None,
            telemetry: None,
            license: None,
            self_heal: None,
        }
    }
}

pub use store::{NewUplinkTestHistory, UplinkTestHistory, UplinkTestHistoryStore};

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            api_token: String::new(),
            username: web::default_web_username(),
            password_hash: String::new(),
            listen_addr: web::default_listen_addr(),
            allow_anon_read: web::default_allow_anon_read(),
            realtime_buffer: web::default_realtime_buffer(),
            ota_signing_key: None,
            rate_limit_per_sec: None,
            cors_origins: None,
            allow_default_token: web::default_allow_default_token(),
            users: Vec::new(),
            webhook_test_allow_hosts: Vec::new(),
            webhook_test_block_private: false,
            demo_replay_sessions: false,
        }
    }
}
