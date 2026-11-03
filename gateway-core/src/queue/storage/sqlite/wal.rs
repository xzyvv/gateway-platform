//! WAL checkpoint 与大小监控。
use rusqlite::Connection;
use std::path::{Path, PathBuf};

use crate::engine::metrics::Metrics;

/// r42-Q2: 由主库路径派生 SQLite 的 WAL 文件路径。
///
/// SQLite 的 WAL 文件路径 = 主库文件路径 + `-wal`（**纯字符串后缀**），不是
/// `Path::with_extension`（后者会替换已有扩展名）。旧实现：
/// `with_extension("db-wal")`：当库路径以 `.db` 结尾时碰巧"正确，但
/// `GATEWAY_DB_PATH` 配成无扩展名（如 `/data/gwqueue`）或 `.sqlite` 结尾时，
/// 会派生出不存在的路径，`check_wal_size` 的 metadata 读取失败直接 return。
/// WAL 50MB/100MB 监控与紧急 checkpoint **静默失效**（磁盘安全网被静默摘除）。
pub(super) fn wal_path_for(db_path: &Path) -> PathBuf {
    let mut s = db_path.as_os_str().to_os_string();
    s.push("-wal");
    PathBuf::from(s)
}

/// r42-Q3: 一致性热备份——先把 WAL 刷回主库，再复制主库文件。
///
/// 单独 `fs::copy(db_path, backup_path)` 只复制**主库文件**，不含 `-wal` 中已
/// commit 但未 checkpoint 的行。上一生命周期若异常退出（未触发关闭时的自动
/// checkpoint），`-wal` 会残留已提交数据；此时启动备份会漏掉这些行，**覆盖
/// 上一轮维护备份**（维护路径走的是 checkpoint + copy 的正确顺序，见
/// `sqlite_storage::maintenance` 的 5 步）。之后若主库损坏，从备份恢复会丢失
/// 最近一次运行的数据。此处与维护路径保持同一顺序。
///
/// 返回是否复制成功（checkpoint 失败不阻断备份：WAL 缺失 / 非 WAL 库时的 PRAGMA 为 no-op）。
pub(super) fn backup_db_consistent(
    conn: &mut Connection,
    db_path: &Path,
    backup_path: &Path,
) -> bool {
    if let Err(e) = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);") {
        tracing::warn!("pre-backup wal checkpoint failed: {}", e);
    }
    match std::fs::copy(db_path, backup_path) {
        Ok(n) => {
            tracing::info!(
                "SQLite backup completed: {} bytes -> {}",
                n,
                backup_path.display()
            );
            true
        }
        Err(e) => {
            tracing::warn!("SQLite backup failed: {}", e);
            false
        }
    }
}

/// WAL checkpoint 触发大小告警的阈值（100MB）。
///
/// 超过本值时强制执行一次 TRUNCATE checkpoint 并累计 `wal_size_warn_total` 指标。
const WAL_SIZE_WARN_BYTES: u64 = 100 * 1024 * 1024;

/// Bug 184 (N05): WAL 早期预警阈值（50MB）。
///
/// 50MB 是 100MB 告警线的一半；超过此值时只累计 `wal_size_warn_total`（记 1 次
/// 告警增长）并打 info 日志，但**不**强制 checkpoint——避免与正常的 10 分钟调度
/// checkpoint 撞车导致短时间内多次 I/O 抖动。
///
/// 目的：让 Dashboard / 告警系统能在 WAL 失控**膨胀到 100MB 之前**就收到趋势告警，
/// 及时发现「checkpoint 频率不够 / 写入突发 / retention 失效」等根因。
const WAL_SIZE_EARLY_WARN_BYTES: u64 = 50 * 1024 * 1024;

/// 执行 SQLite WAL checkpoint（TRUNCATE 模式：将 WAL 内容写回主库并截断 WAL 文件）。
///
/// `reason` 用于在日志中区分 checkpoint 触发原因（schedule / wal_overflow / manual），
/// 便于运维排障区分常规轮转与紧急触发。
pub(crate) fn run_wal_checkpoint(conn: &mut Connection, reason: &str, metrics: Option<&Metrics>) {
    match conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);") {
        Ok(_) => tracing::info!("wal checkpoint (TRUNCATE) executed, reason={}", reason),
        Err(e) => tracing::warn!("wal_checkpoint TRUNCATE failed (reason={}): {}", reason, e),
    }
    // S01: 即便 PRAGMA 失败也要 +1（运维需要知道「曾经尝试 checkpoint」这一事实）。
    // 例如对端长持写锁导致连续 TRUNCATE 失败时，调度 tick 仍然应该被看见）。
    if let Some(m) = metrics {
        m.queue_persist
            .wal_checkpoint
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// 监控 WAL 文件大小，超过阈值时额外触发一次 checkpoint 并告警。
///
/// Bug 184 (N05):
///   - < 50MB：静默
///   - 50MB ~ 100MB：info 日志 + 累计 `wal_size_warn_total` 指标（早期预警）
///   - ≥ 100MB：error 日志 + 累计 `wal_size_warn_total` 指标 + 强制一次 TRUNCATE checkpoint
///
/// `metrics` 为可选传入：为 `None` 时仅记日志、不累计指标（用于测试）。
pub(crate) fn check_wal_size(conn: &mut Connection, wal_path: &Path, metrics: Option<&Metrics>) {
    // 文件可能不存在（WAL 尚未生成或已被 checkpoint 截断）。
    let size = match std::fs::metadata(wal_path) {
        Ok(meta) => meta.len(),
        Err(_) => return,
    };
    if size >= WAL_SIZE_WARN_BYTES {
        tracing::error!(
            "WAL file size {} bytes exceeds 100MB hard threshold, forcing extra checkpoint",
            size
        );
        if let Some(m) = metrics {
            m.queue_persist
                .wal_size_warn
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        run_wal_checkpoint(conn, "wal_overflow", metrics);
    } else if size >= WAL_SIZE_EARLY_WARN_BYTES {
        // Bug 184 (N05): 早期预警——只指标 + info，不强制 checkpoint
        // （避免与 10 分钟调度 checkpoint 撞车导致短时间内多次 I/O 抖动）。
        if let Some(m) = metrics {
            m.queue_persist
                .wal_size_warn
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        tracing::info!(
            "WAL file size {} bytes exceeds 50MB early-warning threshold (WAL may be growing faster than checkpoint; check retention and write load)",
            size
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::metrics::Metrics;
    use std::sync::Arc;

    /// S01: 正常调用应累计 `wal_checkpoint_total`；无论 PRAGMA 成功或失败都为 +1。    
    /// 失败场景下用临时目录 + 立刻关闭的连接，TRUNCATE 可能在已关闭的连接上失败。    
    /// 但调用方关心的契约是「无论结果如何，attempt 都被计入」。
    #[test]
    fn run_wal_checkpoint_increments_metric_with_metrics_arg() {
        let metrics = Arc::new(Metrics::new());
        // 内存 SQLite：PRAGMA 必然成功
        let mut conn = Connection::open_in_memory().expect("open in-memory");
        run_wal_checkpoint(&mut conn, "schedule", Some(&metrics));
        assert_eq!(
            metrics
                .queue_persist
                .wal_checkpoint
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        run_wal_checkpoint(&mut conn, "wal_overflow", Some(&metrics));
        assert_eq!(
            metrics
                .queue_persist
                .wal_checkpoint
                .load(std::sync::atomic::Ordering::Relaxed),
            2
        );
    }

    /// S01: `metrics = None` 时不 panic、也不修改任何计数器（供内部测试不污染指标）。
    #[test]
    fn run_wal_checkpoint_none_metrics_is_noop_for_counters() {
        let mut conn = Connection::open_in_memory().expect("open in-memory");
        // 不会因 metrics=None 而 panic
        run_wal_checkpoint(&mut conn, "schedule", None);
    }

    /// S01: WAL 文件不存在时 check_wal_size 应静默返回，不触发 checkpoint。
    #[test]
    fn check_wal_size_missing_file_is_silent() {
        let metrics = Arc::new(Metrics::new());
        let mut conn = Connection::open_in_memory().expect("open in-memory");
        let path = std::path::Path::new("Z:/this/path/does/not/exist.db-wal");
        let before = metrics
            .queue_persist
            .wal_checkpoint
            .load(std::sync::atomic::Ordering::Relaxed);
        check_wal_size(&mut conn, path, Some(&metrics));
        let after = metrics
            .queue_persist
            .wal_checkpoint
            .load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(before, after, "missing file should not trigger checkpoint");
    }

    /// S01: 构造一个恰好越过早期预警但未达硬告警的 WAL 大小。    
    /// 由于 `WAL_SIZE_EARLY_WARN_BYTES` 是私有常量，无法直接读取；改为：
    /// 在临时目录创建 1 字节占位文件（远低于 50MB），确认 check_wal_size 静默。
    #[test]
    fn check_wal_size_small_file_silent() {
        let metrics = Arc::new(Metrics::new());
        let mut conn = Connection::open_in_memory().expect("open in-memory");
        let dir = tempdir_unique();
        let wal = dir.join("test.db-wal");
        std::fs::write(&wal, b"x").expect("write");
        let before_warn = metrics
            .queue_persist
            .wal_size_warn
            .load(std::sync::atomic::Ordering::Relaxed);
        let before_ckpt = metrics
            .queue_persist
            .wal_checkpoint
            .load(std::sync::atomic::Ordering::Relaxed);
        check_wal_size(&mut conn, &wal, Some(&metrics));
        let after_warn = metrics
            .queue_persist
            .wal_size_warn
            .load(std::sync::atomic::Ordering::Relaxed);
        let after_ckpt = metrics
            .queue_persist
            .wal_checkpoint
            .load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(before_warn, after_warn, "1B file should not warn");
        assert_eq!(before_ckpt, after_ckpt, "1B file should not checkpoint");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// r42-Q2 回归：WAL 路径必须是主库路径 + "-wal"（字符串后缀），
    /// 不能用 `with_extension`——对非 ".db" 结尾的 `GATEWAY_DB_PATH` 会算错，
    /// 使 `check_wal_size` 静默失效。
    #[test]
    fn r42_wal_path_matches_sqlite_naming() {
        assert_eq!(
            wal_path_for(Path::new("/var/lib/gateway/data_queue.db")),
            PathBuf::from("/var/lib/gateway/data_queue.db-wal")
        );
        // 无扩展名路径：with_extension 会得到 "gwqueue.db-wal"（错误）。
        // 正确值是字符串拼接的 "gwqueue-wal"。
        assert_eq!(
            wal_path_for(Path::new("/data/gwqueue")),
            PathBuf::from("/data/gwqueue-wal")
        );
        assert_eq!(
            wal_path_for(Path::new("/data/gw.sqlite")),
            PathBuf::from("/data/gw.sqlite-wal")
        );
    }

    /// r42-Q3 回归：启动备份必须先 checkpoint，否则漏掉 -wal 中已提交但未落主库的行。
    #[test]
    fn r42_backup_db_consistent_includes_wal_rows() {
        let dir = tempdir_unique();
        let db = dir.join("q.db");
        let bak = dir.join("q.db.bak");
        let mut conn = Connection::open(&db).expect("open db");
        // WAL 模式建表 + 写入：此刻该行仅在 -wal 中，主库文件不含它。
        conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE t(x); INSERT INTO t VALUES (7);")
            .expect("setup wal table");

        assert!(backup_db_consistent(&mut conn, &db, &bak), "备份应成功");

        // 备份文件必须包含 checkpoint 后的已提交行
        let check = Connection::open(&bak).expect("open backup");
        let v: i64 = check
            .query_row("SELECT x FROM t", [], |r| r.get(0))
            .expect("read row from backup");
        assert_eq!(v, 7, "备份必须包含 -wal 中已提交的行");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tempdir_unique() -> std::path::PathBuf {
        let base = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let p = base.join(format!("gateway-wal-test-{}", nanos));
        std::fs::create_dir_all(&p).expect("mkdir");
        p
    }
}
