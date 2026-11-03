//! 单事务往返延迟指标（counter + 直方图）
//!
//! 决策表 #25：SLA 埋点。`duration_us` 为提交请求到收到响应（或超时返回）的
//! 完整耗时，覆盖排队 + 收发 + 协议解析前的链路时间。
//!
//! 取代旧 `Metrics` 顶层字段 `transaction_duration_*` / `transaction_failures_total`。
//! 历史直方图名 `gateway_bus_transaction_duration_us_*` 保持不变。

use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use super::histogram::{Histogram, BUCKETS_LEN};
use super::scoped::{CollectErrorCode, ReasonCounters};

#[derive(Default)]
pub struct TransactionMetrics {
    /// 事务失败总数（超时 / 无应答 / 链路错误）。
    pub failures: AtomicU64,
    /// 事务失败数（按原因拆分，Prometheus `by_reason` 标签的数据源）。
    ///
    /// 与 `failures` 总量口径一致（每次失败两边各 +1），只是多一个维度。
    pub failures_by_reason: ReasonCounters,
    /// 单事务往返延迟直方图（桶边界见 `super::TRANSACTION_DURATION_BUCKETS`）。
    pub duration: Histogram,
}

impl TransactionMetrics {
    /// 直方图桶边界（供渲染使用）。
    fn buckets() -> &'static [u64; BUCKETS_LEN] {
        &super::TRANSACTION_DURATION_BUCKETS
    }

    /// 记录一次事务往返延迟（微秒）并区分成功/失败。
    ///
    /// `error` 为失败时的稳定错误码（成功传 `None`）：失败时同时累加
    /// `failures` 总量与 `failures_by_reason` 分桶；缺省错误码归入 `other` 桶。
    pub fn record(&self, success: bool, duration_us: u64, error: Option<CollectErrorCode>) {
        self.duration.record(duration_us, Self::buckets());
        if !success {
            self.failures.fetch_add(1, Ordering::Relaxed);
            self.failures_by_reason.inc(match error {
                Some(code) => code,
                None => CollectErrorCode::Other,
            });
        }
    }

    /// 输出到 Prometheus 文本缓冲区（与拆分前同文案）。
    pub fn render(&self, buf: &mut String) {
        use std::fmt::Write as _;
        // 与 `gateway_collect_duration_us` 口径区分：后者是采集器整体耗时（含多点批量），
        // 本项是单次总线事务的链路延迟。
        let _ = writeln!(
            buf,
            "# HELP gateway_bus_transaction_duration_us Single bus transaction round-trip latency histogram in microseconds\n\
             # TYPE gateway_bus_transaction_duration_us histogram"
        );
        self.duration
            .render("bus_transaction_duration_us", Self::buckets(), buf);
        // 事务失败按原因拆分：同名指标由「1 条无标签序列」变为「N 条带
        // by_reason 标签序列」，各标签值之和恒等于失败总量。
        let _ = writeln!(
            buf,
            "# HELP gateway_bus_transaction_failures_total Total failed bus transactions by reason (timeout / no reply / link error)\n\
             # TYPE gateway_bus_transaction_failures_total counter"
        );
        self.failures_by_reason
            .render("gateway_bus_transaction_failures_total", buf);
    }
}
