//! 采集指标（counter + 耗时直方图）
//!
//! 取代旧 `Metrics` 顶层字段：`serial_errors_total` / `collect_total` /
//! `collect_failures` / `protocol_decode_errors_total` / `collect_duration_*`。
//! 历史直方图名 `gateway_collect_duration_us_*` 保持不变。

use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use super::histogram::{Histogram, BUCKETS_LEN};
use super::scoped::{CollectErrorCode, ReasonCounters};

#[derive(Default)]
pub struct CollectMetrics {
    /// 采集事务总次数。
    pub collect_total: AtomicU64,
    /// 采集失败次数（总量口径；直读方：OTel / soak / load / stats API）。
    pub collect_failures: AtomicU64,
    /// 采集失败次数（按原因拆分，Prometheus `by_reason` 标签的数据源）。
    ///
    /// 与 `collect_failures` 总量口径一致（每次失败两边各 +1），
    /// 只是多一个维度；总量保持无标签，避免影响既有直读逻辑。
    pub failures_by_reason: ReasonCounters,
    /// 串口 IO 错误总次数。
    pub serial_errors: AtomicU64,
    /// 协议解码错误总次数（CRC / 校验和失败、帧格式非法等）。
    pub protocol_decode_errors: AtomicU64,
    /// 采集耗时直方图（桶边界见 `super::COLLECT_DURATION_BUCKETS`）。
    pub duration: Histogram,
}

impl CollectMetrics {
    /// 直方图桶边界（供渲染使用）。
    fn buckets() -> &'static [u64; BUCKETS_LEN] {
        &super::COLLECT_DURATION_BUCKETS
    }

    /// 记录一次采集结果与耗时（微秒）。
    ///
    /// `error` 为失败时的稳定错误码（成功传 `None`）：失败时同时累加
    /// `collect_failures` 总量与 `failures_by_reason` 分桶。缺省错误码归入
    /// `other` 桶，保证分桶总和恒等于总量。
    pub fn record(&self, success: bool, duration_us: u64, error: Option<CollectErrorCode>) {
        self.collect_total.fetch_add(1, Ordering::Relaxed);
        if !success {
            self.collect_failures.fetch_add(1, Ordering::Relaxed);
            self.failures_by_reason.inc(match error {
                Some(code) => code,
                None => CollectErrorCode::Other,
            });
        }
        self.duration.record(duration_us, Self::buckets());
    }

    /// 记录一次协议解码错误。
    pub fn inc_decode_error(&self) {
        self.protocol_decode_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// 输出到 Prometheus 文本缓冲区（与拆分前同文案）。
    pub fn render(&self, buf: &mut String) {
        use std::fmt::Write as _;
        let _ = writeln!(
            buf,
            "# HELP gateway_serial_errors_total Serial IO errors\n\
             # TYPE gateway_serial_errors_total counter\n\
             gateway_serial_errors_total {}",
            self.serial_errors.load(Ordering::Relaxed),
        );
        let _ = writeln!(
            buf,
            "# HELP gateway_collect_total Collection attempts\n\
             # TYPE gateway_collect_total counter\n\
             gateway_collect_total {}",
            self.collect_total.load(Ordering::Relaxed),
        );
        // 采集失败按原因拆分：`gateway_collect_failures` 由「1 条无标签序列」
        // 变为「N 条带 by_reason 标签序列」，各标签值之和恒等于失败总量
        // （既有告警规则需改写为 `sum(gateway_collect_failures)`）。
        let _ = writeln!(
            buf,
            "# HELP gateway_collect_failures Collection failures by reason\n\
             # TYPE gateway_collect_failures counter"
        );
        self.failures_by_reason
            .render("gateway_collect_failures", buf);
        let _ = writeln!(
            buf,
            "# HELP gateway_protocol_decode_errors_total Protocol decode errors (CRC/checksum failures, invalid frames)\n\
             # TYPE gateway_protocol_decode_errors_total counter\n\
             gateway_protocol_decode_errors_total {}",
            self.protocol_decode_errors.load(Ordering::Relaxed),
        );
        // ===== 采集耗时直方图（微秒）=====
        // 供 histogram_quantile 计算 p50/p95/p99；桶边界与 record() 分桶一致。
        let _ = writeln!(
            buf,
            "# HELP gateway_collect_duration_us Collect duration histogram in microseconds\n\
             # TYPE gateway_collect_duration_us histogram"
        );
        self.duration
            .render("collect_duration_us", Self::buckets(), buf);
    }
}
