//! OTel metrics 桥接：将自定义 Metrics struct 的原子计数器/仪表
//! 通过 OTel SDK PeriodicReader 周期性导出到 OTLP collector。
//!
//! 设计原则（稳定性优先）：
//! - 不替换现有 Metrics struct，保持 Prometheus /api/metrics 端点不变
//! - 新增 OTLP 导出路径（feature-gated），作为可选的集中式监控通道
//! - 回调模式：PeriodicReader 触发时读取 Metrics 原子值并 observe 到 OTel 仪器
//! - 失败不阻断启动（与 trace pipeline 一致的降级策略）
#![cfg(feature = "tracing-otlp")]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use once_cell::sync::Lazy;

use crate::engine::metrics::Metrics;

/// OTel metrics provider guard（保持存活防止 PeriodicReader 退出）
static OTEL_METRICS_GUARD: Lazy<Mutex<Option<opentelemetry_sdk::metrics::SdkMeterProvider>>> =
    Lazy::new(|| Mutex::new(None));

/// Bug r19-OTEL2 (fix): OTel 初始化失败次数。
///
/// 旧实现失败时仅 `eprintln!`，运维无法量化"OTel 故障频率"——
///
/// 散落的 eprintln 在容器化部署中易被丢弃，监控不可见。
/// 改为模块级 AtomicU64 累加，与 r15-KAF4 同模式。配 Grafana
/// `rate(otel_init_failures_total[5m]) > 0` 即可实时感知。
static OTEL_INIT_FAILURES: AtomicU64 = AtomicU64::new(0);

/// 初始化 OTel metrics 桥接。
///
/// 在启动时调用一次，持有 provider 直到进程退出。
/// 失败不阻断启动（log + 降级到仅 Prometheus 模式）。
pub fn init_otel_metrics(metrics: Arc<Metrics>, telemetry: &crate::config::TelemetryConfig) {
    match build_metrics_pipeline(metrics, telemetry) {
        Ok(provider) => {
            if let Ok(mut g) = OTEL_METRICS_GUARD.lock() {
                *g = Some(provider);
            }
            tracing::info!("OTel metrics pipeline initialized");
        }
        Err(e) => {
            let total = OTEL_INIT_FAILURES.fetch_add(1, Ordering::Relaxed) + 1;
            eprintln!(
                "OTel metrics init failed (total={}): {e}; using Prometheus-only",
                total
            );
        }
    }
}

/// 构建 OTLP metrics 导出 pipeline。
///
/// 创建 SdkMeterProvider + PeriodicReader，注入 observable instruments
/// 通过回调读取 Metrics struct 的原子值。调用方持有返回的 provider
/// 以保持周期导出活跃。失败时返回 Err，调用方降级到仅 Prometheus 模式。
fn build_metrics_pipeline(
    metrics: Arc<Metrics>,
    telemetry: &crate::config::TelemetryConfig,
) -> Result<opentelemetry_sdk::metrics::SdkMeterProvider, Box<dyn std::error::Error + Send + Sync>>
{
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_otlp::{MetricExporter, WithExportConfig};
    use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};

    // 端点优先级：env > config
    let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .ok()
        .unwrap_or_else(|| telemetry.otlp_endpoint.clone());

    let exporter = MetricExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()?;

    // Bug r19-OTEL1 (fix): metrics_interval_secs 硬上限 1s-3600s。
    // 配置错（如 0 / u64::MAX）：
    //   - 0: 持续轰炸 OTLP collector（DoS）
    //   - u64::MAX: 实际不导出（监控盲区）
    // 上限 1h 足够密集监控，下限 1s 防止刷爆 collector。
    let interval_secs = telemetry.metrics_interval_secs.clamp(1, 3600);
    let reader = PeriodicReader::builder(exporter)
        .with_interval(Duration::from_secs(interval_secs))
        .build();

    let service_name =
        std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| telemetry.service_name.clone());

    let provider = SdkMeterProvider::builder()
        .with_reader(reader)
        .with_resource(
            opentelemetry_sdk::Resource::builder()
                .with_service_name(service_name)
                .build(),
        )
        .build();

    let meter = provider.meter("gateway-core");

    // ===== 计数器（Counter，单调递增）====
    let m = metrics.clone();
    meter
        .u64_observable_counter("gateway_mqtt_publish_success")
        .with_description("MQTT publish success count")
        .with_callback(move |observer| {
            observer.observe(m.mqtt.publish_success.load(Ordering::Relaxed), &[]);
        })
        .build();

    let m = metrics.clone();
    meter
        .u64_observable_counter("gateway_mqtt_publish_failure")
        .with_description("MQTT publish failure count")
        .with_callback(move |observer| {
            observer.observe(m.mqtt.publish_failure.load(Ordering::Relaxed), &[]);
        })
        .build();

    let m = metrics.clone();
    meter
        .u64_observable_counter("gateway_serial_errors_total")
        .with_description("Serial IO errors")
        .with_callback(move |observer| {
            observer.observe(m.collect.serial_errors.load(Ordering::Relaxed), &[]);
        })
        .build();

    let m = metrics.clone();
    meter
        .u64_observable_counter("gateway_collect_total")
        .with_description("Collection attempts")
        .with_callback(move |observer| {
            observer.observe(m.collect.collect_total.load(Ordering::Relaxed), &[]);
        })
        .build();

    let m = metrics.clone();
    meter
        .u64_observable_counter("gateway_collect_failures")
        .with_description("Collection failures")
        .with_callback(move |observer| {
            observer.observe(m.collect.collect_failures.load(Ordering::Relaxed), &[]);
        })
        .build();

    let m = metrics.clone();
    meter
        .u64_observable_counter("gateway_task_panics_total")
        .with_description("Background task panic count")
        .with_callback(move |observer| {
            observer.observe(m.lifecyle.task_panics.load(Ordering::Relaxed), &[]);
        })
        .build();

    let m = metrics.clone();
    meter
        .u64_observable_counter("gateway_config_reload_total")
        .with_description("Config reload count")
        .with_callback(move |observer| {
            observer.observe(m.lifecyle.config_reload_total.load(Ordering::Relaxed), &[]);
        })
        .build();

    let m = metrics.clone();
    meter
        .u64_observable_counter("gateway_protocol_decode_errors_total")
        .with_description("Protocol decode errors")
        .with_callback(move |observer| {
            observer.observe(
                m.collect.protocol_decode_errors.load(Ordering::Relaxed),
                &[],
            );
        })
        .build();

    // ===== 仪表盘（Gauge，可增可减）=====
    let m = metrics.clone();
    meter
        .i64_observable_gauge("gateway_queue_depth")
        .with_description("Current data queue depth")
        .with_callback(move |observer| {
            observer.observe(m.queue_depth.load(Ordering::Relaxed), &[]);
        })
        .build();

    let m = metrics.clone();
    meter
        .i64_observable_gauge("gateway_points_online")
        .with_description("Online points")
        .with_callback(move |observer| {
            observer.observe(m.points_online.load(Ordering::Relaxed), &[]);
        })
        .build();

    let m = metrics.clone();
    meter
        .i64_observable_gauge("gateway_points_offline")
        .with_description("Offline points")
        .with_callback(move |observer| {
            observer.observe(m.points_offline.load(Ordering::Relaxed), &[]);
        })
        .build();

    let m = metrics.clone();
    meter
        .i64_observable_gauge("gateway_mqtt_in_flight")
        .with_description("MQTT in-flight records (dequeued, not acked)")
        .with_callback(move |observer| {
            observer.observe(m.mqtt_in_flight.load(Ordering::Relaxed), &[]);
        })
        .build();

    let m = metrics.clone();
    meter
        .i64_observable_gauge("gateway_outbox_pending")
        .with_description("Outbox pending messages")
        .with_callback(move |observer| {
            observer.observe(m.outbox_pending.load(Ordering::Relaxed), &[]);
        })
        .build();

    Ok(provider)
}
