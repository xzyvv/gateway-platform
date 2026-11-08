//! 只读 SQLite 连接池
//!
//! WAL 模式下多个只读连接可并发查询，不被写操作阻塞。

use std::collections::VecDeque;

use rusqlite::Connection;

/// 轻量级 SQLite 只读连接池
/// WAL 模式下多个只读连接可并发查询，不被写操作阻塞。
pub(super) struct ReadPool {
    conns: std::sync::Mutex<VecDeque<Connection>>,
}

impl ReadPool {
    pub(super) fn new(size: usize, db_path: &std::path::Path, key_hex: &str) -> Self {
        let mut conns = VecDeque::with_capacity(size);
        for _ in 0..size {
            if let Ok(rc) = Connection::open_with_flags(
                db_path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
            ) {
                // SQLCipher: 必须在 journal_mode 之前设置 key
                if !key_hex.is_empty() {
                    // P3: PRAGMA 失败此前被静默忽略，连接照旧入池，后续查询才
                    // 报 "file is not a database" 之类的错，掩盖真实原因。
                    if let Err(e) = rc.pragma_update(None, "key", key_hex) {
                        tracing::warn!("read pool: set SQLCipher key failed: {}", e);
                    }
                }
                // r42-Q1：只读池连接也必须设置 busy_timeout——busy_timeout 是
                // **per-connection** 设置（见 r39 同类修复）。维护每 10 分钟执行
                // `wal_checkpoint(TRUNCATE)`、以及 WAL 崩溃恢复窗口内，读连接可能
                // 瞬时拿到 SQLITE_BUSY；默认 timeout=0 会立即失败 → /api/history
                // 把瞬时锁冲突当成查询错误抛给前端。单独设置（不并入下面的
                // execute_batch）：只读连接无法切换已有非 WAL 库的 journal_mode，
                // 该 batch 可能整体失败，届时超时保护也必须已经生效。
                if let Err(e) = rc.busy_timeout(std::time::Duration::from_millis(5000)) {
                    tracing::warn!("read pool: set busy_timeout failed: {}", e);
                }
                if let Err(e) = rc.execute_batch("PRAGMA journal_mode=WAL; PRAGMA query_only=ON;") {
                    tracing::warn!("read pool: set journal_mode/query_only failed: {}", e);
                }
                conns.push_back(rc);
            }
        }
        Self {
            conns: std::sync::Mutex::new(conns),
        }
    }
    pub(super) fn get(&self) -> Option<Connection> {
        self.conns.lock().ok().and_then(|mut q| q.pop_front())
    }
    pub(super) fn release(&self, conn: Connection) {
        if let Ok(mut q) = self.conns.lock() {
            q.push_back(conn);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// r42-Q1 回归：只读池连接必须设置 busy_timeout，避免 checkpoint / WAL 恢复
    /// 窗口内的瞬时 SQLITE_BUSY 把 /api/history 查询直接打成错误。
    #[test]
    fn r42_read_pool_conn_sets_busy_timeout() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db = dir.path().join("q.db");
        // 只读打开不存在的文件会失败（池为空），先建库
        Connection::open(&db).expect("create db file");
        let pool = ReadPool::new(1, &db, "");
        let conn = pool.get().expect("pool should hold one connection");
        let timeout: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .expect("read busy_timeout");
        assert_eq!(timeout, 5000, "只读池连接应设置 busy_timeout=5000");
        pool.release(conn);
    }
}
