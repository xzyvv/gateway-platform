//! 队列持久化 + WAL 指标（counter / gauge）
//!
//! 取代原 `Metrics.queue_persist_*_total` + `wal_*_total` 五个独立字段。
//! Prom 命名保持向后兼容：
//! - `gateway_queue_persist_dropped_total` (Bug 183 N01)
//! - `gateway_queue_persist_failed_total` (Bug 183 N01)
//! - `gateway_queue_fallback_bounce_total` (Bug 183 N01)
//! - `gateway_wal_size_warn_total` (Bug 184 N05)
//! - `gateway_wal_checkpoint_total` (S01)
//! - `gateway_wal_size_bytes` (gauge，当前 WAL 文件大小)
//! - `gateway_sqlite_db_size_bytes` (gauge，当前主库文件大小)

use std::sync::atomic::AtomicU64;

#[derive(Default)]
pub struct QueuePersistMetrics {
    /// Bug 183 (N01): 持久化非磁盘满错误被 fallback 缓冲淘汰的数据点数。
    pub persist_dropped: AtomicU64,
    /// Bug 183 (N01): 入队持久化失败次数（无论是否被 fallback 吸收）。
    pub persist_failed: AtomicU64,
    /// Bug 183 (N01): fallback 缓冲被踢出后仍反复"写失败→回退"的反弹轮次数。
    pub fallback_bounce: AtomicU64,
    /// Bug 184 (N05): WAL 文件超过告警阈值（100MB）的次数。
    pub wal_size_warn: AtomicU64,
    /// S01: WAL checkpoint 执行总次数（无论成功失败，按 reason 区分原因）。
    pub wal_checkpoint: AtomicU64,
    /// 当前 WAL 文件大小（字节）。checkpoint(TRUNCATE) 后 WAL 被截断/删除时归零。
    pub wal_size_bytes: AtomicU64,
    /// 当前 SQLite 主库文件大小（字节）。用于观察磁盘占用增长趋势。
    pub db_size_bytes: AtomicU64,

    /// Bug-N1 (fix)：outbox sqlite 文件 open 失败、降级到内存模式的次数。
    ///
    /// 触发场景：文件被运维误删 / 目录权限异常 / 磁盘临时满 / 加密 key 失效等。
    /// 旧实现：open 失败 → bootstrap 禁用整个北向（warn + 不注册 connector），
    ///         所有数据上报停止直到运维修复文件系统。
    /// 新实现：自动降级到 `:memory:` 内存 sqlite，北向仍能上报当前进程数据；
    ///         进程崩溃时内存数据丢失（网关本身崩溃了丢失也无所谓），
    ///         下次正常启动恢复 sqlite 文件模式后重新正常工作。
    /// Prometheus 告警：`rate(gateway_outbox_degraded_total[5m]) > 0` 即报。
    pub outbox_degraded: AtomicU64,

    /// 因队列容量上限被**淘汰**（物理删除）的总行数。
    ///
    /// 与 `persist_dropped` 的区别：那是「写库失败被 fallback 吸收后丢弃」，
    /// 这是「写库成功但为腾空间主动删最旧行」。两者都是真实数据丢失，
    /// 但成因不同——前者说明存储层故障，后者说明**容量规划/上行链路长期不畅**。
    /// 单机网关无异地副本，本计数是唯一能证明「丢过多少」的凭据，
    /// 因此必须计入指标与健康口径，而不是只打一行 warn 日志。
    pub evicted_rows: AtomicU64,
    /// 上述淘汰中**未送达**（delivered=0）的行数——这部分是彻底的计费/审计数据丢失。
    pub evicted_undelivered_rows: AtomicU64,
}

impl QueuePersistMetrics {
    pub fn render(&self, buf: &mut String) {
        use std::fmt::Write as _;
        let dropped = self
            .persist_dropped
            .load(std::sync::atomic::Ordering::Relaxed);
        let failed = self
            .persist_failed
            .load(std::sync::atomic::Ordering::Relaxed);
        let bounce = self
            .fallback_bounce
            .load(std::sync::atomic::Ordering::Relaxed);
        let _ = writeln!(
            buf,
            "# HELP gateway_queue_persist_dropped_total Persisted data points dropped by fallback\n\
             # TYPE gateway_queue_persist_dropped_total counter\n\
             gateway_queue_persist_dropped_total {dropped}\n\
             # HELP gateway_queue_persist_failed_total Queue persist failures\n\
             # TYPE gateway_queue_persist_failed_total counter\n\
             gateway_queue_persist_failed_total {failed}\n\
             # HELP gateway_queue_fallback_bounce_total Fallback bounce rounds\n\
             # TYPE gateway_queue_fallback_bounce_total counter\n\
             gateway_queue_fallback_bounce_total {bounce}"
        );
        let wal_warn = self
            .wal_size_warn
            .load(std::sync::atomic::Ordering::Relaxed);
        let checkpoint = self
            .wal_checkpoint
            .load(std::sync::atomic::Ordering::Relaxed);
        let outbox_degraded = self
            .outbox_degraded
            .load(std::sync::atomic::Ordering::Relaxed);
        let _ = writeln!(
            buf,
            "# HELP gateway_wal_size_warn_total WAL size warnings\n\
             # TYPE gateway_wal_size_warn_total counter\n\
             gateway_wal_size_warn_total {wal_warn}\n\
             # HELP gateway_wal_checkpoint_total WAL checkpoints\n\
             # TYPE gateway_wal_checkpoint_total counter\n\
             gateway_wal_checkpoint_total {checkpoint}\n\
             # HELP gateway_outbox_degraded_total Outbox degraded to in-memory mode (open failed)\n\
             # TYPE gateway_outbox_degraded_total counter\n\
             gateway_outbox_degraded_total {outbox_degraded}"
        );
        let wal_size = self
            .wal_size_bytes
            .load(std::sync::atomic::Ordering::Relaxed);
        let db_size = self
            .db_size_bytes
            .load(std::sync::atomic::Ordering::Relaxed);
        let _ = writeln!(
            buf,
            "# HELP gateway_wal_size_bytes Current WAL file size in bytes\n\
             # TYPE gateway_wal_size_bytes gauge\n\
             gateway_wal_size_bytes {wal_size}\n\
             # HELP gateway_sqlite_db_size_bytes Current SQLite main database file size in bytes\n\
             # TYPE gateway_sqlite_db_size_bytes gauge\n\
             gateway_sqlite_db_size_bytes {db_size}"
        );
        let evicted = self.evicted_rows.load(std::sync::atomic::Ordering::Relaxed);
        let evicted_undelivered = self
            .evicted_undelivered_rows
            .load(std::sync::atomic::Ordering::Relaxed);
        let _ = writeln!(
            buf,
            "# HELP gateway_queue_evicted_rows_total Rows physically deleted to enforce the queue capacity limit\n\
             # TYPE gateway_queue_evicted_rows_total counter\n\
             gateway_queue_evicted_rows_total {evicted}\n\
             # HELP gateway_queue_evicted_undelivered_rows_total Evicted rows that were never delivered northbound (permanent data loss)\n\
             # TYPE gateway_queue_evicted_undelivered_rows_total counter\n\
             gateway_queue_evicted_undelivered_rows_total {evicted_undelivered}"
        );
    }
}
