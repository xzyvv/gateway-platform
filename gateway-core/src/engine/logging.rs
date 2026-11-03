//! 日志初始化与滚动清理
//!
//! # 运维环境变量
//!
//! ## 调日志级别（推荐 `gateway_core=info,gatewayd=info`）
//!
//! ```bash
//! RUST_LOG="info,gateway_core::south=debug,gateway_core::north=debug"
//! RUST_LOG="info,gateway_core::south::bus::tcp_task=trace"
//! RUST_LOG="info,gateway_core::south::protocol::modbus=debug"
//! ```
//!
//! ## 双输出层（OP-5 增强）
//!
//! ```bash
//! # 默认：JSON 文件 + 人类可读 stderr 双输出（生产推荐；便于现场调试）
//! # 关闭 stderr 端：仅写文件（适合容器化部署，由 Filebeat/Vector 接文件）
//! GATEWAY_LOG_STDERR=false
//!
//! # 文件路径不可写时，自动 fallback 到 stderr-only（已内置）
//! ```
//!
//! ## OTLP trace / metrics 远程导出（Loki/Tempo/Grafana）
//!
//! ```bash
//! OTEL_EXPORTER_OTLP_ENDPOINT="http://tempo-collector:4317"
//! OTEL_SERVICE_NAME="gateway-01"
//! OTEL_TRACES_SAMPLER="traceidratio"
//! OTEL_TRACES_SAMPLER_ARG="0.5"
//! ```
//!
//! ## 兜底策略
//!
//! - 文件路径不可写 → 自动 stderr-only 兜底（不丢日志）
//! - OTLP 端点不可达 → 不阻断启动，自动降级到 fmt 文件 + stderr 层
//!
//! # 设计要点（OP-5 双输出修复）
//!
//! 原实现只有**单层**（JSON 文件或 stdout 兜底）。工业现场常出现的"运维未接
//! 日志收集器但需要现场诊断"场景下，stderr 也没有被消费——操作员面对的是
//! 一个没人接的 JSON 文件。
//!
//! 新实现：
//! 1. 永远注册 stderr 层（人类可读格式、关闭 ANSI）；运维直接
//!    `journalctl -u gatewayd` 即可看，不依赖外接日志收集器；
//! 2. 同时尝试注册 JSON 文件层（生产机器消费的形态）；
//! 3. 文件层失败 → stderr 单层兜底（不阻断启动）；
//! 4. `GATEWAY_LOG_STDERR=false` 环境变量可关闭 stderr（容器化 daemon 场景）。
//!
//! 双层均受 `RUST_LOG` 控制，运维调日志级别对两端生效。

use once_cell::sync::Lazy;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;

/// 日志 non-blocking guard（保持存活防止 flush 线程退出）
pub(super) static LOG_GUARD: Lazy<Mutex<Option<tracing_appender::non_blocking::WorkerGuard>>> =
    Lazy::new(|| Mutex::new(None));

/// OP-5：OTLP provider 句柄（仅 `tracing-otlp` feature）。
///
/// `SdkTracerProvider` 是 `Arc` 包装的 Clone 句柄，`global::set_tracer_provider`
/// 会拿走所有权，故此处另存一份用于停机时 `force_flush` + `shutdown`
/// ——否则进程退出时 batch exporter 里待导出的 span 会整批丢失。
#[cfg(feature = "tracing-otlp")]
pub(super) static OTLP_PROVIDER: Lazy<Mutex<Option<opentelemetry_sdk::trace::SdkTracerProvider>>> =
    Lazy::new(|| Mutex::new(None));

/// OP-5 (修复)：stderr 双输出是否启用。
///
/// 默认 `true`：生产推荐 stderr + JSON 文件双输出，便于现场调试。
/// 容器化 / daemon 场景可通过 `GATEWAY_LOG_STDERR=false` 关闭（仅写文件）。
fn stderr_layer_enabled() -> bool {
    stderr_enabled_from(std::env::var("GATEWAY_LOG_STDERR").ok().as_deref())
}

/// 纯函数形式：由原始环境变量值判断是否启用 stderr 层（便于单测，不触碰进程环境）。
fn stderr_enabled_from(raw: Option<&str>) -> bool {
    raw.map(|v| {
        !matches!(
            v.trim().to_lowercase().as_str(),
            "false" | "0" | "no" | "off"
        )
    })
    .unwrap_or(true)
}

/// 构造 stderr 层（人类可读格式，关闭 ANSI 兼容 journald）。
///
/// 具体类型 `Registry` 是 tracing-subscriber 默认 subscriber，足够覆盖本项目
/// 使用场景（避免引入 generic S / LookupSpan 复杂约束）。
#[cfg(not(feature = "tracing"))]
fn make_stderr_layer(
) -> Box<dyn tracing_subscriber::Layer<tracing_subscriber::Registry> + Send + Sync> {
    use tracing_subscriber::fmt;
    Box::new(
        fmt::layer()
            .with_writer(std::io::stderr)
            .with_target(true)
            .with_thread_ids(false)
            .with_ansi(false),
    )
}

/// 构造 JSON 文件层。失败返回 Err。
#[cfg(not(feature = "tracing"))]
fn make_json_file_layer(
    log_dir: &std::path::Path,
) -> Result<
    (
        Box<dyn tracing_subscriber::Layer<tracing_subscriber::Registry> + Send + Sync>,
        tracing_appender::non_blocking::WorkerGuard,
    ),
    String,
> {
    use tracing_appender::rolling::{RollingFileAppender, Rotation};
    use tracing_subscriber::fmt;
    if std::fs::create_dir_all(log_dir).is_err() {
        return Err(format!("create_dir_all failed for {}", log_dir.display()));
    }
    let appender = RollingFileAppender::new(Rotation::DAILY, log_dir, "gateway.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(appender);
    let layer = fmt::layer()
        .json()
        .with_writer(non_blocking)
        .with_target(true);
    Ok((Box::new(layer) as _, guard))
}

/// 计算应被轮转删除的日志文件列表（纯函数，便于单元测试）
///
/// 算法（Bug 51 修复后）：
/// 1. 按 mtime 降序排序（最新在前）
/// 2. 跳过前 `keep_recent` 个（始终保留最新 N 个，无论大小）
/// 3. 在剩余文件中，仅删除 size > `size_threshold` 的
///
/// 关键不变量：
/// - 最新 N 个文件永不被删（即使超过阈值）
/// - 旧文件中小于阈值的保留
/// - 旧文件中超过阈值的才删除
///
/// # 参数
/// - `files`: `(路径, 大小字节, mtime)` 三元组切片
/// - `keep_recent`: 始终保留的最新文件数
/// - `size_threshold`: 触发删除的大小阈值（字节）
///
/// # 返回
/// 应删除的 `(路径, 大小)` 列表（按 mtime 降序，即最旧的待删文件排在最后）。
pub(super) fn select_files_for_rotation(
    files: &[(PathBuf, u64, SystemTime)],
    keep_recent: usize,
    size_threshold: u64,
) -> Vec<(PathBuf, u64)> {
    let mut indexed: Vec<&(PathBuf, u64, SystemTime)> = files.iter().collect();
    indexed.sort_by_key(|a| std::cmp::Reverse(a.2));
    indexed
        .iter()
        .skip(keep_recent)
        .filter(|f| f.1 > size_threshold)
        .map(|f| (f.0.clone(), f.1))
        .collect()
}

/// 本系统日志文件前缀：轮转只清理自己的日志，绝不动日志目录下的其它文件
const LOG_FILE_PREFIX: &str = "gateway.log";
/// 始终保留的最新日志文件数（无论大小）
const KEEP_RECENT_LOGS: usize = 7;
/// 单个旧日志超过此大小才纳入第一轮删除
const LOG_SIZE_THRESHOLD: u64 = 50 * 1024 * 1024;
/// #17: 日志目录内 `gateway.log*` 的总大小上限。
const LOG_TOTAL_CAP: u64 = 500 * 1024 * 1024;

/// 日志文件轮转：保留最近 7 个，删除超过 50MB 的旧日志，并保证总量不超过上限
fn spawn_log_rotation() {
    crate::engine::supervision::spawn_supervised("log_rotation", || async {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            let log_dir = crate::support::default_log_dir();
            let Ok(mut entries) = std::fs::read_dir(log_dir) else {
                continue;
            };
            let mut files: Vec<(std::path::PathBuf, u64, std::time::SystemTime)> = Vec::new();
            while let Some(Ok(entry)) = entries.next() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if !name.starts_with(LOG_FILE_PREFIX) {
                    continue;
                }
                if let Ok(meta) = entry.metadata() {
                    if !meta.is_file() {
                        continue;
                    }
                    let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                    files.push((entry.path(), meta.len(), mtime));
                }
            }

            let mut to_delete =
                select_files_for_rotation(&files, KEEP_RECENT_LOGS, LOG_SIZE_THRESHOLD);

            {
                let mut remaining: Vec<&(std::path::PathBuf, u64, std::time::SystemTime)> = files
                    .iter()
                    .filter(|f| !to_delete.iter().any(|(p, _)| *p == f.0))
                    .collect();
                remaining.sort_by_key(|a| a.2);
                let mut total: u64 = remaining.iter().map(|f| f.1).sum();
                let protected = remaining.len().min(KEEP_RECENT_LOGS);
                let deletable = remaining.len().saturating_sub(protected);
                for f in remaining.iter().take(deletable) {
                    if total <= LOG_TOTAL_CAP {
                        break;
                    }
                    to_delete.push((f.0.clone(), f.1));
                    total = total.saturating_sub(f.1);
                }
            }

            for (path, size) in to_delete {
                let _ = std::fs::remove_file(&path);
                tracing::info!("rotated log file: {} ({} bytes)", path.display(), size);
            }
        }
    });
}

/// 构造带 span 事件的 stderr 层（`tracing` feature 开启时使用）。
///
/// 注意：人类可读格式默认已把当前 span 的字段拼进事件行，无需（也无法）
/// 调用 `with_current_span`——该 API 只存在于 JSON formatter。
#[cfg(feature = "tracing")]
fn make_stderr_layer_with_span(
) -> Box<dyn tracing_subscriber::Layer<tracing_subscriber::Registry> + Send + Sync> {
    use tracing_subscriber::fmt;
    use tracing_subscriber::fmt::format::FmtSpan;
    Box::new(
        fmt::layer()
            .with_writer(std::io::stderr)
            .with_target(true)
            .with_thread_ids(false)
            .with_ansi(false)
            .with_span_events(FmtSpan::ENTER | FmtSpan::CLOSE),
    )
}

/// OP-5 (修复)：初始化日志，含 stderr 双输出 + JSON 文件滚动。
///
/// 设计：所有层先收进 `Vec<Box<dyn Layer<Registry> + Send + Sync>>`，在 `Registry`
/// 阶段一次性 `.with(layers)`，最后才挂 `EnvFilter` 作全局过滤。
///
/// 为什么不能逐层 `.with()`：`Layer<S>` 是单态化 trait，`Box<dyn Layer<Registry>>`
/// 只满足 `Layer<Registry>`；一旦先在 Registry 上 `.with(EnvFilter)` 得到
/// `Layered<EnvFilter, Registry>`，再 `.with(Box<dyn Layer<Registry>>)` 就会因
/// trait bound 不匹配而编译失败。而 tracing-subscriber 内置
/// `impl<S, L: Layer<S>> Layer<S> for Vec<L>`，可在 Registry 阶段一次性叠加。
///
/// 由此得到 4 种运行组合，均由运行时决定、无需重复 cfg 分支：
/// 1. stderr + JSON 文件（默认，生产推荐）；
/// 2. 仅 JSON 文件（`GATEWAY_LOG_STDERR=false`，容器化接文件采集）；
/// 3. 仅 stderr（文件路径不可写 → 自动降级，不丢日志）；
/// 4. 上述任一 + OTLP（`tracing-otlp` feature + 端点可达）。
pub fn init_logging(telemetry: Option<&crate::config::TelemetryConfig>) {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::EnvFilter;

    // 未启用 OTLP 时 telemetry 未被读取，显式忽略避免 unused 警告
    let _ = telemetry;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let log_dir = std::path::Path::new(crate::support::default_log_dir());

    let mut layers: Vec<
        Box<dyn tracing_subscriber::Layer<tracing_subscriber::Registry> + Send + Sync>,
    > = Vec::new();

    // 1) stderr 人类可读层：默认开启，GATEWAY_LOG_STDERR=false 关闭
    if stderr_layer_enabled() {
        #[cfg(feature = "tracing")]
        layers.push(make_stderr_layer_with_span());
        #[cfg(not(feature = "tracing"))]
        layers.push(make_stderr_layer());
    }

    // 2) JSON 文件层：失败则降级为 stderr-only，不阻断启动
    #[cfg(feature = "tracing")]
    let file_layer = make_json_file_layer_with_span(log_dir);
    #[cfg(not(feature = "tracing"))]
    let file_layer = make_json_file_layer(log_dir);

    match file_layer {
        Ok((layer, guard)) => {
            if let Ok(mut g) = LOG_GUARD.lock() {
                *g = Some(guard);
            } else {
                eprintln!("LOG_GUARD poisoned, log guard will drop on function exit");
            }
            layers.push(layer);
        }
        Err(e) => {
            eprintln!("warning: JSON file log layer init failed, fallback to stderr-only: {e}");
        }
    }

    // 3) OTLP 层：仅 tracing-otlp feature 开启时可能叠加
    #[cfg(feature = "tracing-otlp")]
    {
        let otlp_enabled = telemetry.map(|t| t.enabled).unwrap_or(false)
            || std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").is_ok();
        if otlp_enabled {
            use opentelemetry::trace::TracerProvider;
            match build_otlp_pipeline(telemetry) {
                Ok(provider) => {
                    let tracer = provider.tracer("gateway-core");
                    layers.push(Box::new(tracing_opentelemetry::layer().with_tracer(tracer)));
                    // 先留一份 Clone 句柄供 shutdown_logging 冲刷，再交给 global
                    if let Ok(mut g) = OTLP_PROVIDER.lock() {
                        *g = Some(provider.clone());
                    }
                    opentelemetry::global::set_tracer_provider(provider);
                }
                Err(e) => eprintln!("OTLP init failed, fallback to fmt-only: {e}"),
            }
        }
    }

    // 全局安装：Registry 阶段叠加所有层，EnvFilter 置于最外层对所有层统一生效。
    // 注意 `let _ =` —— 进程内只允许安装一次 subscriber，重复调用（如测试）静默失败。
    let _ = tracing_subscriber::registry()
        .with(layers)
        .with(filter)
        .try_init();
    spawn_log_rotation();
}

/// OP-5：进程退出前冲刷日志管道。**必做，且必须是停机序的最后一步。**
///
/// 为什么必需：`LOG_GUARD` 存放在 `Lazy` 静态里，而 Rust **不会为静态量运行
/// `Drop`**。`tracing_appender::non_blocking` 的 worker 线程只有在 guard 被 drop
/// 时才会排空 channel 并 flush 文件；否则进程退出时 channel 里尚未落盘的行
/// （恰恰是"最后一批"——停机原因、最终状态、崩溃前的错误）会静默丢失。
///
/// 顺序：先冲刷 OTLP（batch exporter 内还有待导出 span），再 drop `WorkerGuard`
/// （内部 `send_shutdown` + 等待 worker 排空）。两者均只生效一次，重复调用安全。
///
/// 调用后不应再写日志——channel 已关闭，新事件不会落盘（stderr 层仍会输出）。
pub fn shutdown_logging() {
    // 1) OTLP：先 force_flush 把 batch 里待导出的 span 推出去，再 shutdown 释放资源
    #[cfg(feature = "tracing-otlp")]
    {
        let provider = OTLP_PROVIDER.lock().ok().and_then(|mut g| g.take());
        if let Some(provider) = provider {
            // force_flush / shutdown 是 SdkTracerProvider 的固有方法，无需 trait 导入
            if let Err(e) = provider.force_flush() {
                eprintln!("warning: OTLP force_flush failed on shutdown: {e:?}");
            }
            if let Err(e) = provider.shutdown() {
                eprintln!("warning: OTLP shutdown failed: {e:?}");
            }
        }
    }

    // 2) JSON 文件：先记一条收尾日志，再 drop guard 触发 flush（丢弃即冲刷）
    let guard = LOG_GUARD.lock().ok().and_then(|mut g| g.take());
    if guard.is_some() {
        tracing::info!("logging pipeline flushing before exit");
    }
    drop(guard);
}

/// JSON 文件层 + span 配置（tracing feature 开启时使用）。
#[cfg(feature = "tracing")]
fn make_json_file_layer_with_span(
    log_dir: &std::path::Path,
) -> Result<
    (
        Box<dyn tracing_subscriber::Layer<tracing_subscriber::Registry> + Send + Sync>,
        tracing_appender::non_blocking::WorkerGuard,
    ),
    String,
> {
    use tracing_appender::rolling::{RollingFileAppender, Rotation};
    use tracing_subscriber::fmt;
    use tracing_subscriber::fmt::format::FmtSpan;
    if std::fs::create_dir_all(log_dir).is_err() {
        return Err(format!("create_dir_all failed for {}", log_dir.display()));
    }
    let appender = RollingFileAppender::new(Rotation::DAILY, log_dir, "gateway.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(appender);
    let layer = fmt::layer()
        .json()
        .with_writer(non_blocking)
        .with_target(true)
        .with_current_span(true)
        .with_span_events(FmtSpan::ENTER | FmtSpan::CLOSE);
    Ok((Box::new(layer) as _, guard))
}

/// 构建 OTLP trace 导出 pipeline（tracing-otlp feature 开启时）。
///
/// 端点/服务名优先级：env (OTEL_*) > config (telemetry) > default。
/// 失败时调用方降级到 fmt-only，不阻断启动。
#[cfg(feature = "tracing-otlp")]
fn build_otlp_pipeline(
    telemetry: Option<&crate::config::TelemetryConfig>,
) -> Result<opentelemetry_sdk::trace::SdkTracerProvider, Box<dyn std::error::Error + Send + Sync>> {
    use opentelemetry_otlp::{SpanExporter, WithExportConfig};

    let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .ok()
        .or_else(|| telemetry.map(|t| t.otlp_endpoint.clone()))
        .unwrap_or_else(|| "http://localhost:4317".into());

    let exporter = SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()?;

    let service_name = std::env::var("OTEL_SERVICE_NAME")
        .ok()
        .or_else(|| telemetry.map(|t| t.service_name.clone()))
        .unwrap_or_else(|| "gateway-core".into());

    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_resource(
            opentelemetry_sdk::Resource::builder()
                .with_service_name(service_name)
                .build(),
        )
        .with_batch_exporter(exporter)
        .build();
    Ok(provider)
}

#[cfg(test)]
mod tests;
