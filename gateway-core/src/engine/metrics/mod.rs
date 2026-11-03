//! 指标采集与导出（可观测性）
//!
//! 基于 std::sync::atomic 实现，无外部依赖。
//! 通过 `/api/metrics` 端点以 Prometheus 文本格式暴露，供 Grafana 等监控系统抓取。
//!
//! 采用依赖注入：`Metrics` 实例在 bootstrap 阶段创建一次，通过构造函数参数
//! 或 `Gateway.metrics` 字段传递到各模块，避免全局可变状态。
//!
//! ## 拆分说明（Metrics 拆分重构）
//!
//! 历史上 `Metrics` 是单结构体承载 42+ 个字段，跨 3 文件维护。新结构按"领域子结构"组织：
//!
//! - **顶层字段**（跨子系统的状态 / 当日滚动 / 杂项 Gauge）：`queue_depth`、
//!   `points_online/offline`、`outbox_pending`、`process_rss_bytes`、
//!   `daily_*`、`metric_day`、`ota_progress`。
//! - **8 个子结构**：`MqttMetrics` / `WebhookMetrics` / `QueuePersistMetrics` /
//!   `CollectMetrics` / `TransactionMetrics` / `AlarmMetrics` /
//!   `LifecyleMetrics` / `ProtocolExceptionMetrics`。
//! - **公共直方图**：`Histogram`（在 `histogram.rs`），被 `CollectMetrics` 与
//!   `TransactionMetrics` 复用，提供 `record(value_us, &bounds)` 与 `render(...)`。
//! - **sink fan-out**：`sink_send` / `sink_dropped` / `sink_channel_depth` 三个
//!   `RwLock<HashMap>` 保留在顶层（与子结构语义无关，generic fan-out 机制）。
//!
//! Prom 命名 100% 向后兼容——所有写死的 `# HELP`/`# TYPE` 字符串与拆分前一致。
//! 已有 Grafana dashboard / alert rule 无需迁移。
//!
//! 写入 API：保留原 `Metrics::inc_*` / `set_*` / `record_*` 方法作为唯一 facade，
//! 内部委托到子结构字段；调用方代码几乎不受影响。

pub mod alarm;
pub mod bus;
pub mod collect;
pub mod counter;
pub mod gauge;
pub mod histogram;
pub mod lifecyle;
pub mod mqtt;
pub mod pipeline;
pub mod prometheus;
pub mod protocol_exception;
pub mod queue_persist;
pub mod scoped;
pub mod sim_consumer;
pub mod sink;
pub mod transaction;
pub mod webhook;

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicU64};

use parking_lot::RwLock;

pub use alarm::AlarmMetrics;
pub use bus::BusMetrics;
pub use collect::CollectMetrics;
pub use histogram::{Histogram, BUCKETS_LEN};
pub use lifecyle::LifecyleMetrics;
pub use mqtt::MqttMetrics;
pub use pipeline::PipelineMetrics;
pub use protocol_exception::ProtocolExceptionMetrics;
pub use queue_persist::QueuePersistMetrics;
pub use scoped::{
    CollectErrorCode, CollectSample, ReasonCounters, ScopeStats, ScopedCollectMetrics,
    REASON_LABELS,
};
pub use sim_consumer::SimConsumerMetrics;
pub use transaction::TransactionMetrics;
pub use webhook::WebhookMetrics;

/// 采集耗时直方图桶边界（微秒）。
///
/// 覆盖范围 100us..1s（含 +Inf 兜底），用于 `CollectMetrics.duration`。
pub(super) const COLLECT_DURATION_BUCKETS: [u64; BUCKETS_LEN] = [
    100,
    500,
    1_000,
    5_000,
    10_000,
    50_000,
    100_000,
    500_000,
    1_000_000,
    u64::MAX,
];

/// 单事务往返延迟直方图桶边界（微秒）。
///
/// 覆盖范围较采集耗时更宽（串口 9600bps 一帧往返可达数百毫秒~秒级），
/// 上限放到 5s 以适应慢速 RS485 与超时边界观测。
pub(super) const TRANSACTION_DURATION_BUCKETS: [u64; BUCKETS_LEN] = [
    200,
    1_000,
    5_000,
    10_000,
    50_000,
    100_000,
    500_000,
    1_000_000,
    5_000_000,
    u64::MAX,
];

/// 网关指标采集器
///
/// 顶层字段仅承载"跨子系统状态量 / 当日滚动 / OTA"等不属于任何子结构语义
/// 的指标；其余写入请求由 9 个子结构 + 公共 [Histogram] 负责。
///
/// 所有原子操作均使用 `Ordering::Relaxed`——指标采集不要求与其它内存事件建立
/// 同步关系，仅需保证单计数器自身的原子性。
///
/// 详见模块级文档。
#[derive(Default)]
pub struct Metrics {
    // ===== 顶层 Gauge / 当日滚动 =====
    /// 当前队列深度。
    pub queue_depth: AtomicI64,
    /// 在线点位数。
    pub points_online: AtomicI64,
    /// 离线点位数。
    pub points_offline: AtomicI64,
    /// MQTT in-flight 记录数（dequeued 未 ack，S-5）。
    pub mqtt_in_flight: AtomicI64,
    /// 北向 outbox 待发数。
    pub outbox_pending: AtomicI64,
    /// r52-MEM1：进程 RSS 字节数（自监控）。0 表示采集失败。
    pub process_rss_bytes: AtomicI64,
    /// P9：磁盘压力标志（1 = 剩余空间低于阈值，入队被阻断；0 = 正常）。
    /// Prom 命名：`gateway_disk_pressure`。
    pub disk_pressure: AtomicI64,
    /// OTA 升级进度（0-100）。注：写入仅来自 `ota` 模块，但语义属于"对外状态"
    /// 而非横切 lifecyle，故留在顶层。
    pub ota_progress: AtomicI64,

    /// 点位写入总次数（Web/API 下行写入：单点写、批量写、远程控制、GPIO DO 输出）。
    /// Prom 命名：`gateway_point_write_total`。
    pub point_write_total: AtomicU64,
    /// 点位写入失败次数（与 `point_write_total` 同口径）。
    /// Prom 命名：`gateway_point_write_error_total`。
    pub point_write_failures: AtomicU64,

    /// 当日采集事务次数（UTC 跨日惰性滚动）。
    pub daily_collect: AtomicU64,
    /// 当日告警触发次数（UTC 跨日惰性滚动）。
    pub daily_alerts: AtomicU64,
    /// 当日计数器所属的 UTC epoch day（1970-01-01 起的天数）。
    /// 0 为初始哨兵值，首次写入即刷新。
    pub(crate) metric_day: AtomicI64,

    // ===== 9 个子结构 =====
    /// MQTT 北向指标。Prom 命名：`gateway_mqtt_*`。
    pub mqtt: MqttMetrics,
    /// Webhook 北向指标。Prom 命名：`gateway_webhook_*`。
    pub webhook: WebhookMetrics,
    /// 队列持久化 + WAL 指标。Prom 命名：`gateway_queue_persist_*` / `gateway_wal_*`。
    pub queue_persist: QueuePersistMetrics,
    /// 串口总线自检（C-2 设备级/端口级归因）指标。Prom 命名：`gateway_bus_*`。
    pub bus: BusMetrics,
    /// 采集事务指标（counter + histogram）。Prom 命名：`gateway_collect_*` /
    /// `gateway_serial_errors_total` / `gateway_protocol_decode_errors_total` /
    /// `gateway_collect_duration_microseconds_*`。
    pub collect: CollectMetrics,
    /// 单事务往返延迟指标（counter + histogram）。Prom 命名：
    /// `gateway_transaction_*`。
    pub transaction: TransactionMetrics,
    /// 告警指标。Prom 命名：`gateway_alerts_total`。
    pub alarm: AlarmMetrics,
    /// 横切系统级指标（任务 panic / config reload / sink / 停机）。Prom 命名见
    /// 各 `# HELP` 行。
    pub lifecyle: LifecyleMetrics,
    /// 协议异常分类指标（当前仅 Modbus）。Prom 命名：
    /// `gateway_modbus_exception_*`。
    pub protocol_exception: ProtocolExceptionMetrics,
    /// 处理管道（ProcessingPipeline）指标。Prom 命名：
    /// `gateway_pipeline_*`。
    pub pipeline: PipelineMetrics,
    /// 按总线 / 点位的真实采集统计（日桶，供运维报表与健康评分读取，
    /// 同时导出 Prometheus 的 `gateway_collect_failures_scoped` /
    /// `gateway_collect_scoped_points_failing`）。
    pub scoped: ScopedCollectMetrics,
    /// 内置模拟消费者（SimConsumer）指标。Prom 命名：`gateway_sim_consumer_*`。
    ///
    /// 未注入 metrics 的部署不写这些计数（恒 0），不影响其它子系统。
    pub sim_consumer: SimConsumerMetrics,

    // ===== sink fan-out（通用机制，独立于子结构语义）=====
    /// sink 投递成功（send 进 mpsc，不丢）。Prom 命名：
    /// `gateway_sink_send_total{name="..."}`。
    pub sink_send: RwLock<HashMap<String, AtomicU64>>,
    /// sink 投递被丢弃（mpsc 满）。Prom 命名：
    /// `gateway_sink_dropped_total{name="..."}`。
    pub sink_dropped: RwLock<HashMap<String, AtomicU64>>,
    /// sink 内部 mpsc 通道当前深度。Prom 命名：
    /// `gateway_sink_channel_depth{name="..."}`。
    pub sink_channel_depth: RwLock<HashMap<String, AtomicI64>>,
}

impl Metrics {
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests;
