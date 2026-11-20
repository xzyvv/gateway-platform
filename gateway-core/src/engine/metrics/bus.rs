//! 串口总线自检（C-2）指标（counter）
//!
//! 背景：设备连续静默时无法直接判断是"该从站坏了"还是"RS485 口/接线坏了"，
//! 因此每连续 `PROBE_AFTER_SILENT` 次设备级失败会向最近一次成功的从站发一帧
//! 自检帧（FC03 addr=0 count=1）：
//!
//! - 收到任意合法帧（含异常响应）→ 端口收发链路正常，静默归因**设备级**；
//! - 自检帧也收不到任何合法帧 → 端口可疑，丢句柄重开（带冷却）。
//!
//! 这套判定此前只写 tracing 日志。现场网关通常没有日志订阅者/日志易被冲掉，
//! 运维看到"设备离线"时无法区分是表计故障还是接线故障——即本机制的结论
//! **在产线上不可见**。本子结构把判定结果转为可抓取计数：
//!
//! - `gateway_bus_self_check_ok_total`：端口健康（故障在设备侧）
//! - `gateway_bus_self_check_fail_total`：自检失败（端口可疑）
//! - `gateway_bus_self_check_skipped_total`：无历史成功从站，无法区分，保守不动端口
//! - `gateway_bus_port_reopen_by_self_check_total`：因自检失败**实际**重开了端口
//!   （`fail` 中扣除冷却期内跳过的次数，用于判断"端口在反复坏"）

use super::histogram::Histogram;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

#[derive(Default)]
pub struct BusMetrics {
    /// 自检帧成功：端口收发链路正常，静默归因设备级。
    pub self_check_ok: AtomicU64,
    /// 自检帧失败：收不到任何合法帧，端口可疑。
    pub self_check_fail: AtomicU64,
    /// 自检跳过：尚无"最近成功从站"，无法区分设备/端口。
    pub self_check_skipped: AtomicU64,
    /// 因自检失败实际丢句柄重开的次数（不含冷却期内被抑制的）。
    pub port_reopen_by_self_check: AtomicU64,
    /// Modbus ASCII 接收缓冲触及硬上限、整段丢弃的次数（异常设备持续发无
    /// CRLF 数据时的内存保护信号；持续增长说明有设备/线路在灌噪声）。
    pub ascii_frame_dropped: AtomicU64,
    /// 事务开始前丢弃的陈旧字节总数（上一事务超时/被中断后残留在接收缓冲里的字节）。
    ///
    /// 非 0 说明上一帧没有收完整，是「CRC 错 / 丢帧」最直接的证据：
    /// 现场看到 CRC 错频发时，这个计数能区分「设备回帧不完整」
    /// 与「总线电气噪声导致 CRC 算错」——两者在错误码上完全一样。
    pub stale_bytes_dropped: AtomicU64,
    /// 当前排队等待上总线的在途事务数（全部总线合计，进程级 gauge）。
    ///
    /// 与 [`Self::queue_wait`] 配合区分「设备慢」与「排队拥堵」：深度长期 > 0
    /// 且排队等待上升，说明总线带宽不足，或某台设备在反复超时重试堵住队头。
    pub queue_depth: AtomicI64,
    /// 事务在队列中的等待时长（入队成功 → 总线 task 取出执行，微秒）。
    ///
    /// 与 `gateway_bus_transaction_duration_us` 分开：后者含排队，本项只测排队，
    /// 用于判断延迟到底来自「排队」还是「设备/链路」。
    pub queue_wait: Histogram,
}

impl BusMetrics {
    /// 输出到 Prometheus 文本缓冲区。
    pub fn render(&self, buf: &mut String) {
        use std::fmt::Write as _;
        let _ = writeln!(
            buf,
            "# HELP gateway_bus_self_check_ok_total Serial bus self-check succeeded (port healthy, fault is device-level)\n\
             # TYPE gateway_bus_self_check_ok_total counter\n\
             gateway_bus_self_check_ok_total {}\n\
             # HELP gateway_bus_self_check_fail_total Serial bus self-check failed (port/RS485 link suspect)\n\
             # TYPE gateway_bus_self_check_fail_total counter\n\
             gateway_bus_self_check_fail_total {}\n\
             # HELP gateway_bus_self_check_skipped_total Serial bus self-check skipped (no known-good slave yet)\n\
             # TYPE gateway_bus_self_check_skipped_total counter\n\
             gateway_bus_self_check_skipped_total {}\n\
             # HELP gateway_bus_port_reopen_by_self_check_total Serial port reopened because self-check failed (excludes cooldown-suppressed)\n\
             # TYPE gateway_bus_port_reopen_by_self_check_total counter\n\
             gateway_bus_port_reopen_by_self_check_total {}\n\
             # HELP gateway_bus_ascii_frame_dropped_total Modbus ASCII receive buffer overflow: partial frame dropped\n\
             # TYPE gateway_bus_ascii_frame_dropped_total counter\n\
             gateway_bus_ascii_frame_dropped_total {}\n\
             # HELP gateway_bus_stale_bytes_dropped_total Stale bytes discarded before a transaction (previous frame was incomplete)\n\
             # TYPE gateway_bus_stale_bytes_dropped_total counter\n\
             gateway_bus_stale_bytes_dropped_total {}\n\
             # HELP gateway_bus_queue_depth Bus transactions queued but not yet executed (all buses)\n\
             # TYPE gateway_bus_queue_depth gauge\n\
             gateway_bus_queue_depth {}",
            self.self_check_ok.load(Ordering::Relaxed),
            self.self_check_fail.load(Ordering::Relaxed),
            self.self_check_skipped.load(Ordering::Relaxed),
            self.port_reopen_by_self_check.load(Ordering::Relaxed),
            self.ascii_frame_dropped.load(Ordering::Relaxed),
            self.stale_bytes_dropped.load(Ordering::Relaxed),
            self.queue_depth.load(Ordering::Relaxed),
        );
        let _ = writeln!(
            buf,
            "# HELP gateway_bus_queue_wait_us Time a transaction waited in the bus queue before execution (microseconds)\n\
             # TYPE gateway_bus_queue_wait_us histogram"
        );
        self.queue_wait.render(
            "bus_queue_wait_us",
            &super::TRANSACTION_DURATION_BUCKETS,
            buf,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_exports_all_self_check_counters() {
        let m = BusMetrics::default();
        m.self_check_ok.fetch_add(3, Ordering::Relaxed);
        m.self_check_fail.fetch_add(2, Ordering::Relaxed);
        m.self_check_skipped.fetch_add(1, Ordering::Relaxed);
        m.port_reopen_by_self_check.fetch_add(1, Ordering::Relaxed);
        m.ascii_frame_dropped.fetch_add(4, Ordering::Relaxed);
        m.stale_bytes_dropped.fetch_add(7, Ordering::Relaxed);
        m.queue_depth.fetch_add(2, Ordering::Relaxed);
        m.queue_wait
            .record(1_500, &super::super::TRANSACTION_DURATION_BUCKETS);

        let mut out = String::new();
        m.render(&mut out);

        assert!(out.contains("gateway_bus_queue_depth 2"), "{out}");
        assert!(out.contains("gateway_bus_queue_wait_us_count 1"), "{out}");
        assert!(
            out.contains("# TYPE gateway_bus_queue_wait_us histogram"),
            "{out}"
        );

        assert!(out.contains("gateway_bus_self_check_ok_total 3"), "{out}");
        assert!(out.contains("gateway_bus_self_check_fail_total 2"), "{out}");
        assert!(
            out.contains("gateway_bus_self_check_skipped_total 1"),
            "{out}"
        );
        assert!(
            out.contains("gateway_bus_port_reopen_by_self_check_total 1"),
            "{out}"
        );
        assert!(
            out.contains("gateway_bus_ascii_frame_dropped_total 4"),
            "{out}"
        );
        assert!(
            out.contains("gateway_bus_stale_bytes_dropped_total 7"),
            "{out}"
        );
        // 每个指标都必须带 HELP / TYPE，否则 Prometheus 抓取端告警
        for name in [
            "gateway_bus_self_check_ok_total",
            "gateway_bus_self_check_fail_total",
            "gateway_bus_self_check_skipped_total",
            "gateway_bus_port_reopen_by_self_check_total",
            "gateway_bus_ascii_frame_dropped_total",
            "gateway_bus_stale_bytes_dropped_total",
        ] {
            assert!(out.contains(&format!("# HELP {name}")), "缺少 HELP: {name}");
            assert!(
                out.contains(&format!("# TYPE {name} counter")),
                "缺少 TYPE: {name}"
            );
        }
    }
}
