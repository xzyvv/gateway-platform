//! 队列容量策略（内核侧）
//!
//! 定义队列深度与 in_flight 的理论上限、**队列健康判定的唯一口径**
//! （[`queue_health`]），以及队列指标更新辅助。
//! 具体存储后端的批量读写 / 淘汰 SQL 见 `crate::queue::storage::sqlite::limit`。
//!
//! P0-1 修复：此前 `/health/ready`（status.rs）、`/api/health/report`（status.rs）
//! 与 `/api/watchdog/services`（watchdog.rs）各自硬编码阈值（100000 / 10000），
//! 且只有其中一处感知「有没有北向消费者」，同一队列长度会同时给出
//! 「ok / critical / Restarting」三种互斥结论。现统一收敛到本模块的
//! [`queue_health`]：阈值只在这里定义，三处调用点全部改为调用该函数。

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use crate::engine::metrics::Metrics;

/// 更新队列深度指标
pub(crate) fn update_queue_depth_metric(depth: &AtomicI64, metrics: &Arc<Metrics>) {
    let len = depth.load(Ordering::Relaxed).max(0);
    metrics.set_queue_depth(len);
}

/// S-5: 更新 in-flight 指标（dequeued but not yet acked）
pub(crate) fn update_in_flight_metric(in_flight: &AtomicI64, metrics: &Arc<Metrics>) {
    let n = in_flight.load(Ordering::Relaxed).max(0);
    metrics.set_in_flight(n);
}

/// 队列行数上限：超过时由存储后端的淘汰逻辑删除最旧的**不在途**记录
pub const MAX_QUEUE_ROWS: i64 = 1_000_000;

/// 降采样聚合表（`data_queue_aggregated`）行数上限。
///
/// 时间维（license `storage_days`）之外的第二重保护：点位很多时分钟桶仍是
/// 库里最大的表，且「保留 30 天」在 10k 点位量级下可以是数亿行。超过此上限后
/// 由 retention 按 `window_start` 从旧到新分批淘汰，避免磁盘被历史桶写满。
pub const MAX_AGGREGATED_ROWS: i64 = 5_000_000;

/// 积压预警阈值：达到即产生 warning 级告警（无论有没有北向消费者）
pub const BACKLOG_WARN_ROWS: i64 = 10_000;

/// 背压阈值：达到即视为上行通道故障（**仅在有北向消费者时**才作为故障，
/// 无消费者时队列只增不减属于预期形态，见 [`queue_health`]）
pub const BACKPRESSURE_ROWS: i64 = 100_000;

/// 容量压力阈值（行数）：硬上限的 90% = 900,000 行。
/// 达到即开始淘汰最旧数据，属于必须介入的严重态（与有没有北向消费者无关——
/// 磁盘要被写满了）
pub const fn capacity_pressure_rows() -> i64 {
    MAX_QUEUE_ROWS / 10 * 9
}

/// 队列健康等级（由宽松到严格）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum QueueHealth {
    /// 正常
    Ok,
    /// 存量偏多（预警）：有消费者 = 上行欠佳；无消费者 = 本地历史库的正常存量
    Backlog,
    /// 背压：有消费者且积压超过 [`BACKPRESSURE_ROWS`]，上行通道不可达/处理不足
    Backpressure,
    /// 容量压力：总行数逼近硬上限 [`MAX_QUEUE_ROWS`]，存储层开始淘汰最旧数据
    CapacityPressure,
}

impl QueueHealth {
    /// 健康组件标签（`/health/ready` 的 components 值）
    pub fn label(self) -> &'static str {
        match self {
            QueueHealth::Ok => "ok",
            QueueHealth::Backlog => "ok",
            QueueHealth::Backpressure => "backpressure",
            QueueHealth::CapacityPressure => "capacity_pressure",
        }
    }

    /// 是否属于必须介入的严重态（告警计数 / Critical 事件）
    pub fn is_critical(self) -> bool {
        matches!(
            self,
            QueueHealth::Backpressure | QueueHealth::CapacityPressure
        )
    }

    /// 就绪判定：只有严重态才让 `/health/ready` 降级。
    /// 预警态（Backlog）不影响就绪——直连调试场景下队列兼作本地历史库，
    /// 存量增长是预期行为，不应让每个页面轮询都刷 503。
    pub fn is_ready(self) -> bool {
        !self.is_critical()
    }
}

/// **队列健康判定的唯一口径**（P0-1）
///
/// - `queue_len`：`DataQueue::len()` 的返回值，即**未 ack** 的行数
///   （`delivered=0`，含在途行）。它随 ack 递减，是最贴近"待上传积压"的口径；
///   存储层实际总行数可能更大（已送达但尚未被 retention 回收的行不计入），
///   因此用 depth 对 [`MAX_QUEUE_ROWS`] 判定容量压力只会偏保守。
/// - `has_north_consumer`：是否存在会 ack 队列的北向消费者。生产上有两个：
///   MQTT publish 循环与内置模拟消费者（SimConsumer），二者都 dequeue/ack；
///   webhook / kafka 走独立 outbox 表，不消费本队列。
///   该值必须由调用方**运行期求值**（消费者可随时启停），不要用启动期快照。
///
/// 判定顺序：
/// 1. 逼近硬上限 → [`QueueHealth::CapacityPressure`]（与消费者无关，磁盘风险）
/// 2. 有消费者且 ≥ [`BACKPRESSURE_ROWS`] → [`QueueHealth::Backpressure`]
/// 3. ≥ [`BACKLOG_WARN_ROWS`] → [`QueueHealth::Backlog`]
/// 4. 其余 → [`QueueHealth::Ok`]
pub fn queue_health(queue_len: i64, has_north_consumer: bool) -> QueueHealth {
    if queue_len >= capacity_pressure_rows() {
        QueueHealth::CapacityPressure
    } else if has_north_consumer && queue_len >= BACKPRESSURE_ROWS {
        QueueHealth::Backpressure
    } else if queue_len >= BACKLOG_WARN_ROWS {
        QueueHealth::Backlog
    } else {
        QueueHealth::Ok
    }
}

/// A2: in_flight 硬上限。broker 长时间不 ACK 时 in_flight 会持续累积；
/// enforce_queue_limit 只删 in_flight=0 的旧记录，无法回收 in_flight=1 的行，
/// 导致 queue depth 持续超过 MAX_QUEUE_ROWS 但 DELETE 无效，limit 失效。
/// 加硬上限后，Dequeue 在 in_flight 超过此值时直接拒绝取出，反压上游采集。
pub(super) const MAX_IN_FLIGHT: i64 = 100_000;
