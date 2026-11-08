//! 告警指标（counter）
//!
//! 单字段子结构，独立于顶层 `Metrics.daily_alerts`（当日滚动）。
//! Prom 命名：`gateway_alerts_total`，与拆分前兼容。

use std::sync::atomic::AtomicU64;

#[derive(Default)]
pub struct AlarmMetrics {
    /// 告警触发总次数（累计，单调递增；recover 事件不计入）。
    pub triggers_total: AtomicU64,
    /// 磁盘水位告警触发次数（累计，单调递增；recover 事件不计入）。
    ///
    /// 与 `triggers_total` 分开计数：后者是**点级**告警（测点越限），
    /// 磁盘是**系统级**事件，二者混在一个计数器里会让现场无法区分
    /// "某个测点在越限" 与 "网关自己快写满盘了"。
    pub disk_triggers_total: AtomicU64,
}

impl AlarmMetrics {
    pub fn render(&self, buf: &mut String) {
        use std::fmt::Write as _;
        let triggers = self
            .triggers_total
            .load(std::sync::atomic::Ordering::Relaxed);
        let _ = writeln!(
            buf,
            "# HELP gateway_alerts_total Alarm triggers\n\
             # TYPE gateway_alerts_total counter\n\
             gateway_alerts_total {triggers}"
        );
        let disk_triggers = self
            .disk_triggers_total
            .load(std::sync::atomic::Ordering::Relaxed);
        let _ = writeln!(
            buf,
            "# HELP gateway_disk_alarm_total Disk watermark alarm triggers\n\
             # TYPE gateway_disk_alarm_total counter\n\
             gateway_disk_alarm_total {disk_triggers}"
        );
    }
}
