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
    /// 队列容量淘汰告警触发次数（累计，单调递增）。
    ///
    /// 与前两者区分：`triggers_total` 是点级越限，`disk_triggers_total` 是磁盘水位，
    /// 本项是**已经发生的数据淘汰**——语义上是"丢了数据的告警"，严重度最高。
    /// 现场需要单独一条告警规则：`rate(gateway_queue_eviction_alarm_total[5m]) > 0`。
    pub queue_eviction_triggers_total: AtomicU64,
    /// 死信表当前行数（gauge，由 webhook 维护循环周期刷新）。
    ///
    /// **gauge 而非 counter**：死信可以被重投/清理而减少，counter 无法表达"现在还剩多少"。
    /// 现场告警规则建议：`gateway_dead_letter_rows > 0` 持续 10 分钟。
    pub dead_letter_rows: AtomicU64,
    /// 搬入死信表的累计行数（counter）。
    ///
    /// 与 `dead_letter_rows` 配合可算出"已处置多少"：搬入量持续增长而存量不降，
    /// 说明没人处理且对端一直拒绝。
    pub dead_letter_moved_total: AtomicU64,
    /// 因超过行数上限被裁剪（**永久删除**）的累计行数（counter）。
    ///
    /// 这是本组里最严重的指标：语义等价于 `queue_evicted_rows_total`，
    /// 都是"已发生且不可恢复的数据丢弃"。现场必须单独告警。
    pub dead_letter_trimmed_total: AtomicU64,
    /// 人工重投成功次数（counter）。
    ///
    /// 与 `dead_letter_moved_total` 成对看：搬入多、重投少，说明端点长期不可用
    /// 且没人处置；重投忽然上涨，说明现场在按「修好端点 → 重投」的流程恢复数据。
    pub dead_letter_replay_total: AtomicU64,
    /// 重投**未生效**次数（counter，对应 API 的 409）。
    ///
    /// 两种成因：死信 id 已不存在，或同一 `data_id` 仍有在途 pending 行。
    /// 旧实现这两种情况只回一个 409 文本、不留任何计数，运维在 Grafana 上
    /// 完全看不到「有人一直在点重投但一直没生效」。
    pub dead_letter_replay_conflict_total: AtomicU64,
    /// 人工清理（`DELETE /api/north/dead-letters`）累计删除条数（counter）。
    ///
    /// 这是**主动确认丢弃**的数据量，配合 `dead_letter_rows` 存量可以看出
    /// 「清得比进得快」还是相反。
    pub dead_letter_purged_total: AtomicU64,
    /// 最近一次因超上限自动裁剪的时间戳（毫秒 epoch，gauge，0 = 从未裁剪）。
    ///
    /// 裁剪是后台自动行为、没有操作人，`tracing` 日志是唯一的过程记录；
    /// 这个时间戳让界面与告警能回答「上次丢数据是什么时候」。
    pub dead_letter_last_trim_ms: AtomicU64,
    /// 最近一次自动裁剪删除的条数（gauge）。
    pub dead_letter_last_trim_rows: AtomicU64,
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
        let eviction_triggers = self
            .queue_eviction_triggers_total
            .load(std::sync::atomic::Ordering::Relaxed);
        let _ = writeln!(
            buf,
            "# HELP gateway_queue_eviction_alarm_total Queue capacity eviction alarm triggers (data actually deleted)\n\
             # TYPE gateway_queue_eviction_alarm_total counter\n\
             gateway_queue_eviction_alarm_total {eviction_triggers}"
        );
        let dl_rows = self
            .dead_letter_rows
            .load(std::sync::atomic::Ordering::Relaxed);
        let _ = writeln!(
            buf,
            "# HELP gateway_dead_letter_rows Rows currently held in the dead-letter table (undelivered data awaiting manual replay)\n\
             # TYPE gateway_dead_letter_rows gauge\n\
             gateway_dead_letter_rows {dl_rows}"
        );
        let dl_moved = self
            .dead_letter_moved_total
            .load(std::sync::atomic::Ordering::Relaxed);
        let _ = writeln!(
            buf,
            "# HELP gateway_dead_letter_moved_total Rows moved into the dead-letter table\n\
             # TYPE gateway_dead_letter_moved_total counter\n\
             gateway_dead_letter_moved_total {dl_moved}"
        );
        let dl_trimmed = self
            .dead_letter_trimmed_total
            .load(std::sync::atomic::Ordering::Relaxed);
        let _ = writeln!(
            buf,
            "# HELP gateway_dead_letter_trimmed_total Dead-letter rows permanently deleted by the row-count cap (unrecoverable)\n\
             # TYPE gateway_dead_letter_trimmed_total counter\n\
             gateway_dead_letter_trimmed_total {dl_trimmed}"
        );
        let dl_replayed = self
            .dead_letter_replay_total
            .load(std::sync::atomic::Ordering::Relaxed);
        let _ = writeln!(
            buf,
            "# HELP gateway_dead_letter_replay_total Successful manual replays of dead letters\n\
             # TYPE gateway_dead_letter_replay_total counter\n\
             gateway_dead_letter_replay_total {dl_replayed}"
        );
        let dl_replay_conflict = self
            .dead_letter_replay_conflict_total
            .load(std::sync::atomic::Ordering::Relaxed);
        let _ = writeln!(
            buf,
            "# HELP gateway_dead_letter_replay_conflict_total Replays rejected with 409 (dead letter gone, or same data_id still pending)\n\
             # TYPE gateway_dead_letter_replay_conflict_total counter\n\
             gateway_dead_letter_replay_conflict_total {dl_replay_conflict}"
        );
        let dl_purged = self
            .dead_letter_purged_total
            .load(std::sync::atomic::Ordering::Relaxed);
        let _ = writeln!(
            buf,
            "# HELP gateway_dead_letter_purged_total Dead-letter rows deleted by explicit manual purge\n\
             # TYPE gateway_dead_letter_purged_total counter\n\
             gateway_dead_letter_purged_total {dl_purged}"
        );
        let dl_last_trim_ms = self
            .dead_letter_last_trim_ms
            .load(std::sync::atomic::Ordering::Relaxed);
        let _ = writeln!(
            buf,
            "# HELP gateway_dead_letter_last_trim_ms Epoch ms of the most recent automatic trim (0 = never)\n\
             # TYPE gateway_dead_letter_last_trim_ms gauge\n\
             gateway_dead_letter_last_trim_ms {dl_last_trim_ms}"
        );
        let dl_last_trim_rows = self
            .dead_letter_last_trim_rows
            .load(std::sync::atomic::Ordering::Relaxed);
        let _ = writeln!(
            buf,
            "# HELP gateway_dead_letter_last_trim_rows Rows deleted by the most recent automatic trim\n\
             # TYPE gateway_dead_letter_last_trim_rows gauge\n\
             gateway_dead_letter_last_trim_rows {dl_last_trim_rows}"
        );
    }
}
