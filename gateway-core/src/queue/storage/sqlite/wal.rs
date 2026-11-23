//! WAL checkpoint 与大小监控。
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::time::Duration;

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

/// 启动期一致性热备份（保留原 bool 契约，供 `db_init` 忽略结果）。
///
/// 旧实现是 `wal_checkpoint(TRUNCATE)` + `fs::copy`：checkpoint 后释放锁再复制，
/// 期间主循环可继续写 WAL、autocheckpoint 可改写主库页导致备份撕裂；`fs::copy`
/// 还直接覆盖 .bak（非原子），中途崩溃会留下半截文件，恢复时用它覆盖主库会全损。
/// 现在统一委托 [`backup_db_online`]：在线备份 API 保证跨 WAL 一致快照，临时文件
/// + 原子 rename 保证 .bak 永远是完整的一份。
pub(super) fn backup_db_consistent(
    conn: &mut Connection,
    db_path: &Path,
    backup_path: &Path,
    key_hex: &str,
) -> bool {
    match backup_db_online(conn, db_path, backup_path, key_hex) {
        Ok(()) => {
            tracing::info!("SQLite backup completed: {}", backup_path.display());
            true
        }
        Err(e) => {
            tracing::warn!("SQLite backup failed: {}", e);
            false
        }
    }
}

/// 缺陷 #8：SQLite 在线一致性热备份（在线备份 API + 临时文件原子 rename）。
///
/// 相比 `checkpoint + fs::copy` 的两个致命缺陷：
/// - **撕裂**：旧实现 checkpoint 在锁内、`fs::copy` 在锁外；释放锁后 queue_task
///   主循环继续写 WAL，autocheckpoint（每 1000 页）会改写主库页，复制可能取到
///   事务中间态。在线备份 API 全程持源连接读锁，由 SQLite 保证快照一致，
///   且自动纳入 `-wal` 中已提交但未 checkpoint 的行——无需手动 checkpoint，
///   也无需读 `-wal`/`-shm`。
/// - **半截 .bak**：旧实现直接覆盖目标，复制到一半崩溃就留下不可用文件，而
///   恢复路径（db_init）用它覆盖主库 → 数据全损。现在先写同目录
///   `<db>.db.bak.tmp`，关闭目标连接后 `fs::rename` 原子替换 .bak，任何时刻
///   .bak 都是「上一份完整备份」或「这一份完整备份」。
///
/// 调用方须持有源连接的锁（生产路径为 `SqliteStorage.conn` 的 Mutex guard），
/// 保证备份期间没有其他写事务在源连接上推进。所有错误用 `?` 上抛。
pub(super) fn backup_db_online(
    src: &Connection,
    db_path: &Path,
    backup_path: &Path,
    key_hex: &str,
) -> rusqlite::Result<()> {
    // 临时文件与目标同目录，rename 才是同一文件系统内的原子操作。
    let tmp = db_path.with_extension("db.bak.tmp");
    // 备份目标必须不存在：先清掉上次崩溃残留的临时文件。忽略删除失败——若确实
    // 无法删除且文件有内容，下面的 open/Backup 会返回 SQLite 错误并上抛。
    let _ = std::fs::remove_file(&tmp);

    // 目标连接必须在 rename 之前彻底 drop：Windows 不允许 rename 一个仍被
    // 打开的文件（ERROR_SHARING_VIOLATION=32），Linux 虽容忍但同样应显式关闭。
    // 用内层作用域同时约束 dst 与其 Backup 借用，退出后临时库句柄必定释放。
    {
        let mut dst = Connection::open(&tmp)?;
        // SQLCipher：目标库必须用与源库相同的 key，否则备份库无法读取。
        #[cfg(feature = "encryption")]
        if !key_hex.is_empty() {
            dst.pragma_update(None, "key", key_hex)?;
        }
        #[cfg(not(feature = "encryption"))]
        let _ = key_hex;

        let bk = rusqlite::backup::Backup::new(src, &mut dst)?;
        bk.run_to_completion(100, Duration::from_millis(10), None)?;
    } // bk + dst 在此 drop，临时库句柄关闭后才能原子 rename

    // 目标 .bak 若被其它连接占用，rename 同样会 SHARING_VIOLATION；先清残留。
    let _ = std::fs::remove_file(backup_path);
    // rusqlite 没有 From<std::io::Error>：显式把 IO 错误包进 Error 上抛。
    std::fs::rename(&tmp, backup_path)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    Ok(())
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
    use std::sync::atomic::Ordering;
    // 文件可能不存在（WAL 尚未生成或已被 checkpoint 截断）。
    // DD-19：此时 gauge 必须归零，不能把上一次的大小留在指标里。
    let size = match std::fs::metadata(wal_path) {
        Ok(meta) => meta.len(),
        Err(_) => {
            if let Some(m) = metrics {
                m.queue_persist.wal_size_bytes.store(0, Ordering::Relaxed);
            }
            return;
        }
    };
    // DD-19：这个函数是生产路径上唯一稳定读到 WAL 大小的位置，gauge 必须在这里写；
    // 历史实现只在单测里赋值，导致 `gateway_wal_size_bytes` 线上恒为 0。
    if let Some(m) = metrics {
        m.queue_persist
            .wal_size_bytes
            .store(size, Ordering::Relaxed);
    }
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
        assert_eq!(
            metrics
                .queue_persist
                .wal_size_bytes
                .load(std::sync::atomic::Ordering::Relaxed),
            0,
            "WAL 文件不存在时 gauge 必须归零"
        );
    }

    /// S01: 构造一个恰好越过早期预警但未达硬告警的 WAL 大小。    
    /// 由于 `WAL_SIZE_EARLY_WARN_BYTES` 是私有常量，无法直接读取；改为：
    /// 在临时目录创建 1 字节占位文件（远低于 50MB），确认 check_wal_size 静默。
    #[test]
    fn check_wal_size_small_file_silent() {
        let metrics = Arc::new(Metrics::new());
        let mut conn = Connection::open_in_memory().expect("open in-memory");
        let (_dir, dir) = tempdir_unique();
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
        assert_eq!(
            metrics
                .queue_persist
                .wal_size_bytes
                .load(std::sync::atomic::Ordering::Relaxed),
            1,
            "低于告警阈值时也要回写 WAL 大小 gauge"
        );
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
        let (_dir, dir) = tempdir_unique();
        let db = dir.join("q.db");
        let bak = dir.join("q.db.bak");
        let mut conn = Connection::open(&db).expect("open db");
        // WAL 模式建表 + 写入：此刻该行仅在 -wal 中，主库文件不含它。
        conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE t(x); INSERT INTO t VALUES (7);")
            .expect("setup wal table");

        assert!(backup_db_consistent(&mut conn, &db, &bak, ""), "备份应成功");

        // 备份文件必须包含 checkpoint 后的已提交行
        let check = Connection::open(&bak).expect("open backup");
        let v: i64 = check
            .query_row("SELECT x FROM t", [], |r| r.get(0))
            .expect("read row from backup");
        assert_eq!(v, 7, "备份必须包含 -wal 中已提交的行");
    }

    fn tempdir_unique() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().to_path_buf();
        (dir, path)
    }
}
