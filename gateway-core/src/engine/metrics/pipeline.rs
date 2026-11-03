//! 处理管道（pipeline）指标
//!
//! Prom 命名：
//! - `gateway_pipeline_submitted_total` —— 成功 submit 的批次数（含成功路径对照组）
//! - `gateway_pipeline_dropped_total` —— submit 时 mpsc 通道已关闭导致整批丢弃的次数
//!
//! 历史：[`ProcessingPipeline::submit`][crate::pipeline::ProcessingPipeline::submit]
//! 失败只 `tracing::error!`，运维窗口期只在 Prometheus 看不到数据丢失频率。
//! Grafana 只能看 logs —— 同一窗口下高频告警聚合困难。

use std::sync::atomic::AtomicU64;

#[derive(Default)]
pub struct PipelineMetrics {
    /// 处理管道 submit 成功的批次数（含对照组，便于算 success rate）。
    pub submitted: AtomicU64,
    /// submit 时 mpsc 通道已关闭导致整批丢弃的次数（X1）。
    pub dropped: AtomicU64,
}

impl PipelineMetrics {
    pub fn render(&self, buf: &mut String) {
        use std::fmt::Write as _;
        let submitted = self.submitted.load(std::sync::atomic::Ordering::Relaxed);
        let dropped = self.dropped.load(std::sync::atomic::Ordering::Relaxed);
        let _ = writeln!(
            buf,
            "# HELP gateway_pipeline_submitted_total Pipeline.submit successful batches\n\
             # TYPE gateway_pipeline_submitted_total counter\n\
             gateway_pipeline_submitted_total {submitted}\n\
             # HELP gateway_pipeline_dropped_total Pipeline.submit batches dropped (mpsc channel closed)\n\
             # TYPE gateway_pipeline_dropped_total counter\n\
             gateway_pipeline_dropped_total {dropped}"
        );
    }
}
