//! 可观测性/遥测配置（§2.3 OTel 全栈接入）
//!
//! 配置 OTLP trace + metrics 远程导出。
//! 所有字段可选，不启用时零开销——编译期 feature gate 控制是否引入 OTel SDK。
//! 环境变量 OTEL_* 仍可覆盖 config 值，优先级：env > config > default。

use serde::{Deserialize, Serialize};
use validator::Validate;

/// 遥测配置（§2.3 OTel 全栈接入）
///
/// 不启用时（None 或 enabled=false）仅走本地 JSON 日志 + Prometheus /api/metrics，
/// 不影响现有行为。启用后叠加 OTLP 远程导出（trace + metrics）。
///
/// 配置示例：
/// ```json
/// "telemetry": {
///   "enabled": true,
///   "otlp_endpoint": "http://collector:4317",
///   "service_name": "gateway-01",
///   "sample_ratio": 0.5,
///   "metrics_interval_secs": 15
/// }
/// ```
#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, Validate, schemars::JsonSchema, ts_rs::TS,
)]
#[ts(export)]
pub struct TelemetryConfig {
    /// 是否启用 OTLP 远程导出
    #[serde(default)]
    pub enabled: bool,

    /// OTLP gRPC 端点（如 "http://localhost:4317"）
    #[serde(default = "default_otlp_endpoint")]
    pub otlp_endpoint: String,

    /// 服务名（出现在 Jaeger/Tempo 的 service 列）
    #[serde(default = "default_service_name")]
    pub service_name: String,

    /// 采样率（0.0~1.0，1.0=全量采样）
    #[serde(default = "default_sample_ratio")]
    #[ts(type = "number")]
    pub sample_ratio: f64,

    /// OTLP metrics 导出间隔（秒）
    #[serde(default = "default_metrics_interval")]
    #[ts(type = "number")]
    pub metrics_interval_secs: u64,
}

fn default_otlp_endpoint() -> String {
    "http://localhost:4317".into()
}

fn default_service_name() -> String {
    "gateway-core".into()
}

fn default_sample_ratio() -> f64 {
    1.0
}

fn default_metrics_interval() -> u64 {
    15
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            otlp_endpoint: default_otlp_endpoint(),
            service_name: default_service_name(),
            sample_ratio: default_sample_ratio(),
            metrics_interval_secs: default_metrics_interval(),
        }
    }
}
