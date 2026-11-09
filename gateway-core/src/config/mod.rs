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
pub(crate) mod point;
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
mod queue;
mod virtual_point;

// S6: 配置向后兼容回归（crate 内单测，随 `cargo test --lib` 门禁）
#[cfg(test)]
mod compat;

pub use backoff::ReconnectBackoffConfig;
pub use bus::{BusConfig, OpcUaClientConfig};
pub use cellular::CellularConfig;
pub use convert::{ConvertEnumItemConfig, ConvertRuleConfig};
pub use groups_assets::{DeviceAsset, DeviceGroup};
pub use license::{default_quota_for_tier, LicenseConfig, TierQuotaEntry, DEFAULT_TIER_QUOTAS};
pub use loader::ConfigLoader;
pub use modbus_passthrough::ModbusPassthroughConfig;
pub use modbus_server::ModbusServerConfig;
pub use mqtt::MqttConfig;
pub use ntp::NtpConfig;
pub use opcua::{OpcuaConfig, OpcuaUserConfig};
pub use point::{
    allowed_function_codes, AggregationConfig, DeviceConfig, PointConfig, PointTrashEntry,
};
pub use queue::QueueConfig;
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
pub use web::{
    default_realtime_max_subscribers, UserRole, WebConfig, WebUser, REALTIME_MAX_SUBSCRIBERS_MAX,
    REALTIME_MAX_SUBSCRIBERS_MIN,
};
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

/// 持有凭据的配置结构体的 `Debug` 实现：先序列化为 JSON，再把敏感键的值替换为
/// `<redacted>` 后输出。
///
/// `derive(Debug)` 会把 `api_token` / `password` 等字段原样打印，任何
/// `tracing::debug!("{:?}", cfg)` 或 `anyhow!("{cfg:?}")` 都会把明文凭据写进日志。
/// 日志排查需要看配置时，用本函数代替默认实现。
///
/// 说明：落盘序列化（`serde_json::to_string(&cfg)`）仍保留明文/密文原值，
/// 加密由 `crypto::encrypt_config_secrets` 负责，与本函数无关。
pub(crate) fn fmt_redacted_debug<T: serde::Serialize>(
    value: &T,
    f: &mut std::fmt::Formatter<'_>,
) -> std::fmt::Result {
    match serde_json::to_value(value) {
        Ok(mut v) => {
            redact_debug_value(&mut v);
            match serde_json::to_string(&v) {
                Ok(s) => f.write_str(&s),
                Err(_) => f.write_str("<redacted>"),
            }
        }
        // 序列化失败时宁可不输出，也不能回退到明文 Debug
        Err(_) => f.write_str("<redacted>"),
    }
}

/// 递归脱敏：按**键名**识别凭据字段，兼容 `webhook.headers` 这类动态键。
fn redact_debug_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                let key_lc = key.to_ascii_lowercase();
                let is_secret = matches!(
                    key_lc.as_str(),
                    "password"
                        | "password_hash"
                        | "api_token"
                        | "ota_signing_key"
                        | "pin"
                        | "secret"
                        | "token"
                        // 与 `sensitive_field_paths()` 保持一致：mqtt/cellular 的
                        // username 常是账号/邮箱，Debug 里一并隐去（宁多勿漏）。
                        | "username"
                );
                if is_secret && v.is_string() {
                    *v = serde_json::Value::String("<redacted>".to_string());
                } else if key_lc == "headers" {
                    // HTTP 头键名不可枚举（Authorization / X-Api-Key / Cookie …），整体脱敏值
                    if let serde_json::Value::Object(headers) = v {
                        for hv in headers.values_mut() {
                            if hv.is_string() {
                                *hv = serde_json::Value::String("<redacted>".to_string());
                            }
                        }
                    }
                } else {
                    redact_debug_value(v);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                redact_debug_value(item);
            }
        }
        _ => {}
    }
}

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

    /// 队列容量配置（缺省即默认上限；`#[serde(default)]` 保证旧配置零迁移）
    #[serde(default)]
    pub queue: QueueConfig,
}

impl GatewayConfig {
    /// 队列表行数硬上限（换算成队列内核使用的 `i64` 口径）。
    pub fn max_queue_rows(&self) -> i64 {
        self.queue.max_rows as i64
    }
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
            queue: QueueConfig::default(),
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
            realtime_max_subscribers: web::default_realtime_max_subscribers(),
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

#[cfg(test)]
mod debug_redaction_tests {
    use super::*;

    /// 断言渲染结果既不含任何明文凭据，也确实执行了脱敏（而非靠字段缺失蒙混过关）。
    fn assert_redacted(rendered: &str, secrets: &[&str]) {
        for secret in secrets {
            assert!(
                !rendered.contains(secret),
                "Debug 输出泄露明文凭据 {secret:?}: {rendered}"
            );
        }
        assert!(
            rendered.contains("<redacted>"),
            "应至少出现一个脱敏占位符: {rendered}"
        );
    }

    #[test]
    fn config_debug_never_leaks_plaintext_secrets() {
        const SECRETS: &[&str] = &[
            "TOKEN-PLAINTEXT-123",
            "HASH-PLAINTEXT-456",
            "OTA-KEY-PLAINTEXT-789",
            "MQTT-USER-PLAINTEXT",
            "MQTT-PASS-PLAINTEXT",
            "CELL-PASS-PLAINTEXT",
            "CELL-PIN-9876",
            "OPCUA-PASS-PLAINTEXT",
            "WEBHOOK-AUTH-PLAINTEXT",
        ];

        let cfg = GatewayConfig {
            web: Some(WebConfig {
                api_token: "TOKEN-PLAINTEXT-123".into(),
                password_hash: "HASH-PLAINTEXT-456".into(),
                ota_signing_key: Some("OTA-KEY-PLAINTEXT-789".into()),
                ..Default::default()
            }),
            mqtt: MqttConfig {
                enabled: true,
                username: Some("MQTT-USER-PLAINTEXT".into()),
                password: Some("MQTT-PASS-PLAINTEXT".into()),
                ..Default::default()
            },
            cellular: Some(CellularConfig {
                password: "CELL-PASS-PLAINTEXT".into(),
                pin: Some("CELL-PIN-9876".into()),
                ..Default::default()
            }),
            opcua: Some(OpcuaConfig {
                users: vec![opcua::OpcuaUserConfig {
                    username: "operator".into(),
                    password: "OPCUA-PASS-PLAINTEXT".into(),
                }],
                ..Default::default()
            }),
            webhook: Some(WebhookConfig {
                url: "https://example.invalid/hook".into(),
                headers: [(
                    "Authorization".to_string(),
                    "WEBHOOK-AUTH-PLAINTEXT".to_string(),
                )]
                .into_iter()
                .collect(),
                ..Default::default()
            }),
            ..Default::default()
        };

        // 顶层整体打印（最常见的排查姿势）
        assert_redacted(&format!("{cfg:?}"), SECRETS);
        // 单拎叶子结构体打印也必须安全
        assert_redacted(&format!("{:?}", cfg.web.as_ref().unwrap()), SECRETS);
        assert_redacted(&format!("{:?}", cfg.mqtt), SECRETS);
        assert_redacted(&format!("{:?}", cfg.cellular.as_ref().unwrap()), SECRETS);
        assert_redacted(&format!("{:?}", cfg.webhook.as_ref().unwrap()), SECRETS);
    }
}
