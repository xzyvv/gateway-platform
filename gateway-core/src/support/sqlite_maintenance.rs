//! SQLite 维护的公共实现：队列主库与配置域共享库共用同一套语义。
//!
//! 两个库的维护动作完全一致，此前是两份各自演进的实现
//! （`queue/storage/sqlite/db_init.rs` 与 `engine/monitor/db_maintenance.rs`），
//! 改一处容易漏另一处，所以在这里收口：
//!
//! 1. 启动时把 `auto_vacuum` 从 `NONE` 切到 `INCREMENTAL`——已有库必须跑一次
//!    全量 `VACUUM` 才会写进库头（SQLite 的语义，只设 pragma 不生效）；
//! 2. 之后周期调用 `PRAGMA incremental_vacuum(N)`，分小批把空闲页还给文件系统，
//!    避免全量 `VACUUM` 长时间锁表；
//! 3. 采样主库文件大小与空闲页数，用于判断「回收是否真的生效」。
//!
//! 所有失败只告警、不阻断：维护是兜底，不能影响采集与上报。`ctx` 是日志前缀
//! （`队列主库` / `配置库`），保留现场可区分的定位信息。

use std::path::Path;

use rusqlite::Connection;

/// `PRAGMA auto_vacuum` 的 INCREMENTAL 取值（SQLite 定义为 2）。
const AUTO_VACUUM_INCREMENTAL: i64 = 2;

/// 打开短生命周期的维护连接：用完即关，不与长驻连接争用。
///
/// `busy_timeout` 是必须的：维护任务与业务写入并发时，立刻返回 SQLITE_BUSY
/// 会让回收静默失败，看起来像「vacuum 没生效」。
pub fn open_short_lived(path: &Path, ctx: &str) -> Option<Connection> {
    let conn = match Connection::open(path) {
        Ok(conn) => conn,
        Err(e) => {
            tracing::warn!("{}: 打开 {} 失败: {}", ctx, path.display(), e);
            return None;
        }
    };
    if let Err(e) = conn.execute_batch("PRAGMA busy_timeout=5000;") {
        tracing::warn!("{}: 设置 busy_timeout 失败: {}", ctx, e);
    }
    Some(conn)
}

/// 开启增量 vacuum（幂等）。
///
/// - 新建库：设置后立即写入库头，`VACUUM` 代价可忽略；
/// - 已有库：必须跑一次全量 `VACUUM` 才生效（仅设置 pragma 不够）。
///
/// 失败不阻断启动：磁盘不足/库被占用时降级为「本次运行不回收空闲页」，
/// 下次启动会重试。
pub fn ensure_incremental_auto_vacuum(conn: &Connection, ctx: &str) {
    let mode: i64 = conn
        .pragma_query_value(None, "auto_vacuum", |row| row.get(0))
        .unwrap_or(0);
    if mode == AUTO_VACUUM_INCREMENTAL {
        return;
    }
    if let Err(e) = conn.execute_batch("PRAGMA auto_vacuum=INCREMENTAL;") {
        tracing::warn!("{}开启 incremental auto_vacuum 失败: {}", ctx, e);
        return;
    }
    let started = std::time::Instant::now();
    match conn.execute_batch("VACUUM;") {
        Ok(()) => tracing::info!(
            "{} auto_vacuum 已切换为 INCREMENTAL（VACUUM 耗时 {:?}）",
            ctx,
            started.elapsed()
        ),
        Err(e) => tracing::warn!(
            "为{}启用 incremental auto_vacuum 执行 VACUUM 失败（本次运行不回收空闲页）: {}",
            ctx,
            e
        ),
    }
}

/// 回收空闲页（需库已启用 `auto_vacuum=INCREMENTAL`）。
///
/// 每次只回收 `pages` 页（4KB/页），由 maintenance 周期调用。
/// `auto_vacuum=NONE` 时该语句是空操作，不会报错。
pub fn run_incremental_vacuum(conn: &Connection, pages: i64, ctx: &str) {
    if let Err(e) = conn.execute_batch(&format!("PRAGMA incremental_vacuum({});", pages)) {
        tracing::warn!("{} incremental_vacuum 失败: {}", ctx, e);
    }
}

/// 读取主库文件大小（字节）与空闲页数。
///
/// 文件大小取主库文件（不含 `-wal`）：WAL 有 autocheckpoint 兜底，长期稳定在
/// 数 MB，跟踪主库文件更能反映「回收是否生效」。
pub fn file_size_and_freelist(path: &Path, ctx: &str) -> Option<(i64, i64)> {
    let size = std::fs::metadata(path).ok()?.len() as i64;
    let conn = open_short_lived(path, ctx)?;
    let freelist: i64 = conn
        .query_row("PRAGMA freelist_count", [], |row| row.get(0))
        .ok()?;
    Some((size, freelist))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(tag: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join(format!("{}.db", tag));
        (dir, path)
    }

    #[test]
    fn ensure_auto_vacuum_is_idempotent() {
        let (_dir, path) = temp_db("shared");
        let conn = open_short_lived(&path, "测试库").expect("open");
        ensure_incremental_auto_vacuum(&conn, "测试库");
        ensure_incremental_auto_vacuum(&conn, "测试库");

        let mode: i64 = conn
            .pragma_query_value(None, "auto_vacuum", |row| row.get(0))
            .expect("read auto_vacuum");
        assert_eq!(mode, AUTO_VACUUM_INCREMENTAL);
    }

    #[test]
    fn convert_keeps_existing_rows_and_reports_stats() {
        let (_dir, path) = temp_db("shared-data");
        {
            let conn = Connection::open(&path).expect("open");
            conn.execute_batch("CREATE TABLE t (v INTEGER); INSERT INTO t VALUES (7);")
                .expect("seed");
            ensure_incremental_auto_vacuum(&conn, "测试库");
        }

        let conn = Connection::open(&path).expect("open");
        let v: i64 = conn
            .query_row("SELECT v FROM t", [], |row| row.get(0))
            .expect("read row");
        assert_eq!(v, 7, "VACUUM 不能丢数据");

        let (size, freelist) = file_size_and_freelist(&path, "测试库").expect("stats");
        assert!(size > 0);
        assert!(freelist >= 0);
    }

    #[test]
    fn incremental_vacuum_on_plain_db_is_noop() {
        let (_dir, path) = temp_db("shared-plain");
        let conn = open_short_lived(&path, "测试库").expect("open");
        // 未开启 auto_vacuum 时该语句为空操作，不能报错也不能中断调用方。
        run_incremental_vacuum(&conn, 16, "测试库");
    }
}
