//! HTTP Webhook 北向上报配置
//!
//! 工业网关除 MQTT 外，支持通过 HTTP POST 将采集数据推送到第三方平台。
//! 适用于无 MQTT broker、直接对接企业 API 的场景。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// HTTP Webhook 上报配置
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, ts_rs::TS)]
#[ts(export)]
pub struct WebhookConfig {
    /// 是否启用 Webhook 上报
    #[serde(default)]
    pub enabled: bool,

    /// 上报目标 URL（支持 http/https）
    pub url: String,

    /// 自定义请求头（如 Authorization、X-Gateway-Id 等）
    #[serde(default)]
    pub headers: HashMap<String, String>,

    /// 请求超时时间（秒），默认 10
    #[serde(default = "default_timeout")]
    #[ts(type = "number")]
    pub timeout_secs: u64,

    /// 单次上报最大数据条数，默认 100
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,

    /// 上报间隔（毫秒），默认 1000
    #[serde(default = "default_interval")]
    #[ts(type = "number")]
    pub interval_ms: u64,

    /// 失败重试次数，默认 3
    #[serde(default = "default_retries")]
    pub max_retries: u32,
}

fn default_timeout() -> u64 {
    10
}
fn default_batch_size() -> usize {
    100
}
fn default_interval() -> u64 {
    1000
}
fn default_retries() -> u32 {
    3
}

impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: String::new(),
            headers: HashMap::new(),
            timeout_secs: default_timeout(),
            batch_size: default_batch_size(),
            interval_ms: default_interval(),
            max_retries: default_retries(),
        }
    }
}
