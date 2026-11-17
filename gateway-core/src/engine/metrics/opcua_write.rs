//! OPC UA Server 写回持锁等待指标（histogram + counter）
//!
//! OPC UA 的写回调在持有地址空间**写锁**期间被同步调用：等待多久，该 Server 的
//! 读 / 订阅通知就阻塞多久（`gateway-core/src/north/server/opcua_server.rs` 的
//! `WRITE_TIMEOUT_MIN/MAX` 注释）。此前这段等待完全不可观测，现场遇到
//! 「写频繁导致读变慢」只能靠猜。
//!
//! 本子结构记录一次写回调从发起真实写到拿到结果（或超时）的**持锁等待时长**，
//! 以及超时 / 失败次数，经 `/api/metrics` 以 Prometheus 文本暴露。写入点只有
//! `opcua_server::execute_write` 一处；指标本身可在未启用 OPC UA 时恒为 0，
//! 不引入运行时成本。

use std::sync::atomic::{AtomicU64, Ordering};

use super::histogram::{Histogram, BUCKETS_LEN};

/// 写回持锁等待时长桶边界（微秒），覆盖写回调的 3s 下限 ~ 5s 上限预算。
/// 1e5=100ms / 5e5=500ms / 1s / 2s / 3s / 4s / 5s / 10s / 30s / +Inf。
pub(super) const OPCUA_WRITE_WAIT_BUCKETS: [u64; BUCKETS_LEN] = [
    100_000,
    500_000,
    1_000_000,
    2_000_000,
    3_000_000,
    4_000_000,
    5_000_000,
    10_000_000,
    30_000_000,
    u64::MAX,
];

#[derive(Default)]
pub struct OpcUaWriteMetrics {
    /// 持锁等待时长直方图（微秒）。
    pub wait_duration: Histogram,
    /// 写回超时次数（等待预算内未拿到结果）。
    pub timeouts: AtomicU64,
    /// 写回失败次数（拿到结果但写失败）。
    pub failures: AtomicU64,
}

impl OpcUaWriteMetrics {
    fn buckets() -> &'static [u64; BUCKETS_LEN] {
        &OPCUA_WRITE_WAIT_BUCKETS
    }

    /// 记录一次写回调的持锁等待。
    ///
    /// `wait_us`：从发起写到结果返回的微秒数；`timed_out` / `failed` 分别标记
    /// 超时与写失败（两者互斥：超时时拿不到真实结果，`failed=false`）。
    pub fn record(&self, wait_us: u64, timed_out: bool, failed: bool) {
        self.wait_duration.record(wait_us, Self::buckets());
        if timed_out {
            self.timeouts.fetch_add(1, Ordering::Relaxed);
        } else if failed {
            self.failures.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 输出到 Prometheus 文本缓冲区。
    pub fn render(&self, buf: &mut String) {
        use std::fmt::Write as _;
        let _ = writeln!(
            buf,
            "# HELP gateway_opcua_write_wait_us OPC UA server write callback lock-held wait time histogram in microseconds\n\
             # TYPE gateway_opcua_write_wait_us histogram"
        );
        self.wait_duration
            .render("opcua_write_wait_us", Self::buckets(), buf);
        let _ = writeln!(
            buf,
            "# HELP gateway_opcua_write_timeouts_total Total OPC UA write callbacks that timed out waiting for the real write result\n\
             # TYPE gateway_opcua_write_timeouts_total counter"
        );
        let _ = writeln!(
            buf,
            "gateway_opcua_write_timeouts_total {}",
            self.timeouts.load(Ordering::Relaxed)
        );
        let _ = writeln!(
            buf,
            "# HELP gateway_opcua_write_failures_total Total OPC UA write callbacks with a failed real write result\n\
             # TYPE gateway_opcua_write_failures_total counter"
        );
        let _ = writeln!(
            buf,
            "gateway_opcua_write_failures_total {}",
            self.failures.load(Ordering::Relaxed)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_counts_wait_and_outcomes() {
        let m = OpcUaWriteMetrics::default();
        m.record(2_500_000, false, false); // 2.5s 成功
        m.record(5_100_000, true, false); // 超时
        m.record(800_000, false, true); // 0.8s 写失败
        assert_eq!(m.wait_duration.count(), 3);
        assert_eq!(m.timeouts.load(Ordering::Relaxed), 1);
        assert_eq!(m.failures.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn timeout_and_failure_are_mutually_exclusive() {
        let m = OpcUaWriteMetrics::default();
        // 即便调用方同时传 true，也只计入超时，避免 counter 双算。
        m.record(5_000_000, true, true);
        assert_eq!(m.timeouts.load(Ordering::Relaxed), 1);
        assert_eq!(m.failures.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn render_emits_histogram_and_counters() {
        let m = OpcUaWriteMetrics::default();
        m.record(3_200_000, false, false);
        m.record(6_000_000, true, false);
        let mut buf = String::new();
        m.render(&mut buf);
        assert!(buf.contains("# TYPE gateway_opcua_write_wait_us histogram"));
        assert!(buf.contains("gateway_opcua_write_wait_us_bucket{le=\"3000000\"}"));
        assert!(buf.contains("gateway_opcua_write_wait_us_count 2"));
        assert!(buf.contains("gateway_opcua_write_timeouts_total 1"));
        assert!(buf.contains("gateway_opcua_write_failures_total 0"));
    }
}
