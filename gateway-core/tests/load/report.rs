//! 负载采样与报告生成。
use std::sync::atomic::Ordering;
use std::time::Instant;

use gateway_core::engine::metrics::Metrics;

use super::north_stub::CountingSink;

/// 某一时刻的计数器快照。
#[derive(Clone, Copy)]
pub struct Snapshot {
    pub at: Instant,
    pub collect_total: u64,
    pub collect_failures: u64,
    pub decode_errors: u64,
    pub serial_errors: u64,
    pub queue_depth: i64,
    pub sink_batches: u64,
    pub sink_points: u64,
}

/// 采集一次快照。
pub fn sample(metrics: &Metrics, sink: &CountingSink) -> Snapshot {
    Snapshot {
        at: Instant::now(),
        collect_total: metrics.collect.collect_total.load(Ordering::Relaxed),
        collect_failures: metrics.collect.collect_failures.load(Ordering::Relaxed),
        decode_errors: metrics
            .collect
            .protocol_decode_errors
            .load(Ordering::Relaxed),
        serial_errors: metrics.collect.serial_errors.load(Ordering::Relaxed),
        queue_depth: metrics.queue_depth.load(Ordering::Relaxed),
        sink_batches: sink.batches(),
        sink_points: sink.points(),
    }
}

/// 两次快照之间的增量与速率。
pub struct Report {
    pub secs: f64,
    pub collect_delta: u64,
    pub failures_delta: u64,
    pub decode_delta: u64,
    pub serial_delta: u64,
    pub queue_depth_peak: i64,
    pub sink_batches_delta: u64,
    pub sink_points_delta: u64,
}

impl Report {
    pub fn between(t0: Snapshot, t1: Snapshot, queue_depth_peak: i64) -> Self {
        let secs = (t1.at - t0.at).as_secs_f64().max(1e-6);
        Self {
            secs,
            collect_delta: t1.collect_total.saturating_sub(t0.collect_total),
            failures_delta: t1.collect_failures.saturating_sub(t0.collect_failures),
            decode_delta: t1.decode_errors.saturating_sub(t0.decode_errors),
            serial_delta: t1.serial_errors.saturating_sub(t0.serial_errors),
            queue_depth_peak,
            sink_batches_delta: t1.sink_batches.saturating_sub(t0.sink_batches),
            sink_points_delta: t1.sink_points.saturating_sub(t0.sink_points),
        }
    }

    /// 采集事务速率（事务/秒）。
    pub fn collect_rate(&self) -> f64 {
        self.collect_delta as f64 / self.secs
    }

    /// 北向点数速率（点/秒）。
    pub fn sink_point_rate(&self) -> f64 {
        self.sink_points_delta as f64 / self.secs
    }

    /// 渲染为多行报告文本。
    pub fn render(&self) -> String {
        format!(
            "负载报告 (duration={:.1}s)\n  \
             采集事务: {}  ({:.1}/s)\n  \
             采集失败: {} | 解码错误 {} | 串口错误 {}\n  \
             队列深度峰值 {}\n  \
             北向扇出: {} 批 / {} 点 ({:.1} 点/s)",
            self.secs,
            self.collect_delta,
            self.collect_rate(),
            self.failures_delta,
            self.decode_delta,
            self.serial_delta,
            self.queue_depth_peak,
            self.sink_batches_delta,
            self.sink_points_delta,
            self.sink_point_rate(),
        )
    }
}
