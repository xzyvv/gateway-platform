//! 配置域 SQLite 长驻连接
//!
//! 配置域的 6 个存储（配置版本快照 / 上行测试历史 / 限流规则 / API 密钥 /
//! 计划任务 / 数据质量规则）此前**每个方法都新开一次连接**：`Connection::open`
//! → `PRAGMA journal_mode=WAL` → `busy_timeout` → 用完即关。
//!
//! 其中 `ApiKeyStore::authenticate` 与 `RateRuleStore` 在**每个带 Key 的 HTTP
//! 请求**上都会被调用，属于请求热路径。单次 open 虽只有几十微秒，但包含打开
//! 文件、解析库头、建 WAL 索引、关闭时刷 page cache 等系统调用与内存分配，
//! 高频下是无谓开销，也让磁盘 IO 无法被 page cache 复用。
//!
//! 现改为每个 Store 在 `new()` 时打开**一个长驻连接**，之后全部方法经
//! `Mutex<Connection>` 串行复用。配置域写少读多、并发极低（远达不到单连接
//! 瓶颈），且同库多写者互相 SQLITE_BUSY 退避的问题也随之消失。
//!
//! `:memory:` 不再适用（连接不复用 → 每次都是空库），生产与测试均使用文件库。

use std::path::Path;

use rusqlite::Connection;

use crate::engine::error::{GatewayError, GatewayResult};

/// 单个 Store 复用的长驻 SQLite 连接
///
/// `Connection` 是 `Send` 但非 `Sync`，故用 `Mutex` 包裹以支持跨线程共享；
/// 所有访问都在同一把锁内串行，天然避免并发写同一库文件的 `SQLITE_BUSY`。
pub(super) struct SharedConn {
    conn: parking_lot::Mutex<Connection>,
}

impl SharedConn {
    /// 打开长驻连接并完成统一 PRAGMA 设置
    ///
    /// - `journal_mode=WAL`：写入不阻塞读，崩溃恢复由 WAL 保证
    /// - `busy_timeout=5000`：本库文件由多个 Store 各自持连接共享（如
    ///   `config_versions.db` 同时被版本快照 / 上行测试历史 / 磁盘历史使用），
    ///   WAL 下写者互斥，未设置超时会在 `SQLITE_BUSY` 时立即失败
    ///
    /// `ctx` 只用于错误信息，便于定位是哪个 Store 打开失败。
    pub(super) fn open(path: &Path, ctx: &str) -> GatewayResult<Self> {
        let conn = Connection::open(path)
            .map_err(|e| GatewayError::Queue(format!("{} failed: {}", ctx, e)))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")
            .map_err(|e| GatewayError::Queue(format!("set pragmas failed: {}", e)))?;
        Ok(Self {
            conn: parking_lot::Mutex::new(conn),
        })
    }

    /// 借用连接（持锁期间独占，返回的 guard drop 时自动释放）
    pub(super) fn get(&self) -> parking_lot::MutexGuard<'_, Connection> {
        self.conn.lock()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "gw-shared-conn-{}-{}-{}",
            tag,
            std::process::id(),
            nanos
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir.join("t.db")
    }

    /// 长驻连接必须设置 WAL + busy_timeout（与原每次 open 的语义保持一致）
    #[test]
    fn open_sets_wal_and_busy_timeout() {
        let db = temp_db("pragma");
        let shared = SharedConn::open(&db, "open test db").expect("open");
        let conn = shared.get();
        let journal: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .expect("read journal_mode");
        assert_eq!(journal.to_lowercase(), "wal");
        let timeout: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .expect("read busy_timeout");
        assert_eq!(timeout, 5000);
        let _ = std::fs::remove_dir_all(db.parent().unwrap());
    }

    /// 同一连接跨多次调用复用：写入在后续调用中可见（证明没有每次重开）
    #[test]
    fn connection_is_reused_across_calls() {
        let db = temp_db("reuse");
        let shared = SharedConn::open(&db, "open test db").expect("open");
        {
            let conn = shared.get();
            conn.execute("CREATE TABLE t (v INTEGER)", []).expect("ddl");
            conn.execute("INSERT INTO t (v) VALUES (42)", [])
                .expect("insert");
        }
        // 若每次重新 open，这里拿到的是空库（无表）而不是复用同一连接
        let conn = shared.get();
        let v: i64 = conn
            .query_row("SELECT v FROM t", [], |r| r.get(0))
            .expect("select from reused connection");
        assert_eq!(v, 42);
        let _ = std::fs::remove_dir_all(db.parent().unwrap());
    }

    /// 打开失败的上下文要能定位到具体 Store
    #[test]
    fn open_error_mentions_context() {
        // 目录不存在 → 打开必然失败
        let bad = std::env::temp_dir()
            .join("gw-shared-conn-no-such-dir")
            .join("nested")
            .join("t.db");
        let err = SharedConn::open(&bad, "open api key db")
            .err()
            .expect("should fail");
        assert!(
            err.to_string().contains("open api key db"),
            "错误信息应包含调用方上下文，实际: {}",
            err
        );
    }
}
