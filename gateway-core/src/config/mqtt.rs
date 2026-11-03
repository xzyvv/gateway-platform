//! MQTT 上传配置

use serde::{Deserialize, Serialize};
use validator::Validate;

use super::{
    validate_mqtt_topic, validate_non_zero_interval, validate_qos, ReconnectBackoffConfig,
};

/// MQTT 配置（§3.3）
/// Bug 2.4 修复：MQTT TLS 客户端配置（企业内网自建 broker 支持）
///
/// 旧实现：BrokerScheme::Tls + 无 client_cert 路径走 rumqttc::Transport::tls_with_default_config()，
///         仅信任 WebPKI + 系统 CA 证书池；内网自签或私有 CA 签发的 broker 必失败。
/// 新增：ca_cert_path 显式指定 PEM/DER 格式 CA bundle；insecure_skip_verify 留给
///         测试场景（生产警告）。
#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, Validate, schemars::JsonSchema, ts_rs::TS,
)]
#[ts(export)]
pub struct MqttTlsConfig {
    /// 自定义 CA 证书路径（PEM 格式，可含多个证书）
    #[serde(default)]
    pub ca_cert_path: Option<String>,

    /// 跳过服务器证书验证（仅测试/排错使用；生产启用会在启动时记 error 日志）
    #[serde(default)]
    pub insecure_skip_verify: bool,
}

#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, Validate, schemars::JsonSchema, ts_rs::TS,
)]
#[ts(export)]
pub struct MqttConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default = "default_broker")]
    pub broker: String,

    #[serde(default = "default_topic")]
    #[validate(custom(function = "validate_mqtt_topic"))]
    pub topic: String,

    /// 批量上传周期（秒，>=1）
    #[serde(default = "default_publish_interval")]
    #[validate(custom(function = "validate_non_zero_interval"))]
    #[ts(type = "number")]
    pub publish_interval: u64,

    /// QoS 等级 0/1/2
    #[serde(default = "default_qos")]
    #[validate(custom(function = "validate_qos"))]
    pub qos: u8,

    /// 心跳间隔（秒）
    #[serde(default = "default_keep_alive")]
    #[ts(type = "number")]
    pub keep_alive: Option<u64>,

    /// PubAck 等待超时（秒）：QoS1 消息发布后等待 broker 确认的最长时间，
    /// 超时后释放 in_flight 让数据可重传。None=使用默认值 10 秒。
    #[serde(default)]
    #[ts(type = "number")]
    pub ack_timeout_secs: Option<u64>,

    /// MQTT broker 登录用户名（可选）。为空表示匿名连接。
    /// 支持以 `enc:` 前缀的密文形式落盘，启动时由 decrypt_config_secrets 解密。
    #[serde(default)]
    pub username: Option<String>,

    /// MQTT broker 登录密码（可选）。
    /// 支持以 `enc:` 前缀的密文形式落盘，避免明文密码写入 config.json。
    /// 启动时由 decrypt_config_secrets 解密；解密失败保留原值并告警。
    #[serde(default)]
    pub password: Option<String>,

    #[serde(default)]
    pub compression: Option<String>,

    #[serde(default)]
    pub compression_threshold: Option<usize>,

    /// 断点续传限速（条/秒）：网络恢复后补发积压数据时的最大速率。
    /// None=不限速（按 publish_interval 正常补发）。
    /// 建议 200-1000，防止断网恢复后打爆 broker。
    #[serde(default)]
    pub resume_rate_limit: Option<usize>,

    /// 自定义上行 payload 模板（可选）：含 ${var} 占位符的 JSON 模板。
    /// - None / 空 = 使用默认 JSON 格式（按设备分组嵌套，与历史 schema 兼容）
    /// - Some("...") = 按点位循环渲染模板，每点输出一行（JSON Lines 风格）
    ///
    /// 模板语法见 `north::template`。
    #[serde(default)]
    pub payload_template: Option<String>,

    /// TLS 客户端配置（企业内网自建 broker 需配置 ca_cert_path 指向自签/私有 CA 证书）
    #[serde(default)]
    pub tls: Option<MqttTlsConfig>,

    /// 重连退避参数（默认序列 2/4/8/16/32/60 秒 + 40% 抖动，与历史硬编码一致）。
    #[serde(default = "default_reconnect_backoff")]
    #[validate(nested)]
    pub reconnect_backoff: ReconnectBackoffConfig,
}

impl Default for MqttConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            broker: "tcp://localhost:1883".to_string(),
            topic: "factory/line1".to_string(),
            publish_interval: 5,
            qos: 1,
            keep_alive: Some(30),
            ack_timeout_secs: None,
            username: None,
            password: None,
            compression: None,
            compression_threshold: None,
            resume_rate_limit: None,
            payload_template: None,
            tls: None,
            reconnect_backoff: default_reconnect_backoff(),
        }
    }
}

// ===== 默认值函数 =====

fn default_broker() -> String {
    "tcp://localhost:1883".to_string()
}
fn default_topic() -> String {
    "factory/line1".to_string()
}
fn default_publish_interval() -> u64 {
    5
}
fn default_qos() -> u8 {
    1
}
fn default_keep_alive() -> Option<u64> {
    Some(30)
}

/// MQTT 重连退避默认值：精确复刻历史硬编码序列 2/4/8/16/32/60 秒 + 40% 抖动。
fn default_reconnect_backoff() -> ReconnectBackoffConfig {
    ReconnectBackoffConfig {
        seconds: vec![2, 4, 8, 16, 32, 60],
        jitter_ratio: 0.4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bug 2.4: MqttTlsConfig 字段在 JSON 中可选（None=系统 CA only）
    #[test]
    fn test_mqtt_config_without_tls_field() {
        let json = r#"{
            "enabled": true,
            "broker": "mqtt://localhost:1883",
            "topic": "test/topic",
            "publish_interval": 5,
            "qos": 1
        }"#;
        let cfg: MqttConfig = serde_json::from_str(json).expect("parse without tls field");
        assert!(cfg.tls.is_none(), "未配置 tls → None（君道可走系统 CA）");
    }

    /// Bug 2.4: MqttTlsConfig 含 ca_cert_path + insecure_skip_verify 双字段
    #[test]
    fn test_mqtt_config_with_tls_custom_ca() {
        let json = r#"{
            "enabled": true,
            "broker": "tls://broker.internal:8883",
            "topic": "factory/line1",
            "publish_interval": 5,
            "qos": 1,
            "tls": {
                "ca_cert_path": "/etc/gateway/ca.pem",
                "insecure_skip_verify": false
            }
        }"#;
        let cfg: MqttConfig = serde_json::from_str(json).expect("parse with tls");
        let tls = cfg.tls.expect("tls is Some");
        assert_eq!(tls.ca_cert_path.as_deref(), Some("/etc/gateway/ca.pem"));
        assert!(!tls.insecure_skip_verify);
    }

    /// Bug 2.4: 默认值与历史一致，未填 tls 字段无破坏性变化
    #[test]
    fn test_mqtt_config_default_has_no_tls() {
        let cfg = MqttConfig::default();
        assert!(cfg.tls.is_none());
    }

    /// r25-VLD1：合法 topic（无通配符 / 无 `$` 前缀）必须通过校验。
    #[test]
    fn validate_mqtt_topic_accepts_normal_topics() {
        for ok in &[
            "factory/line1",
            "data/point/x",
            "a",                          // 单字符
            "tenant-1/zone/2/device/abc", // 多级
            "site$internal/zone/a",       // 内部含 `$` 但非首位
        ] {
            assert!(
                validate_mqtt_topic(ok).is_ok(),
                "合法 topic '{}' 应通过校验",
                ok
            );
        }
    }

    /// r25-VLD1：`+`（单级通配符）在 publish topic 中必须被拒绝——subscribe-only 字面。
    #[test]
    fn validate_mqtt_topic_rejects_plus_wildcard() {
        assert!(validate_mqtt_topic("factory/+/line1").is_err());
        assert!(validate_mqtt_topic("+").is_err());
        // 含 `+` 字符无论位置
        assert!(validate_mqtt_topic("a+/b").is_err());
    }

    /// r25-VLD1：`#`（多级通配符）在 publish topic 中必须被拒绝。
    #[test]
    fn validate_mqtt_topic_rejects_hash_wildcard() {
        assert!(validate_mqtt_topic("factory/#").is_err());
        assert!(validate_mqtt_topic("#").is_err());
        assert!(validate_mqtt_topic("a/b/#").is_err());
    }

    /// r25-VLD1：`$` 开头（broker 系统保留前缀 `$SYS/` 等）必须被拒绝。
    #[test]
    fn validate_mqtt_topic_rejects_system_reserved_prefix() {
        assert!(validate_mqtt_topic("$SYS/broker/info").is_err());
        assert!(validate_mqtt_topic("$share/group/topic").is_err());
        assert!(validate_mqtt_topic("$").is_err());
    }

    /// r25-VLD1：旧有非法字符（空格 / NUL / `\r` / `\n`）仍应被拒绝（回归）。
    #[test]
    fn validate_mqtt_topic_still_rejects_forbidden_chars() {
        assert!(validate_mqtt_topic("").is_err());
        assert!(validate_mqtt_topic("a b").is_err());
        assert!(validate_mqtt_topic("a\0b").is_err());
        assert!(validate_mqtt_topic("a\nb").is_err());
    }
}
