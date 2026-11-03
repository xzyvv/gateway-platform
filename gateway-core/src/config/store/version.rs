//! 配置版本管理
//!
//! 每次配置变更自动创建版本快照，支持回滚到任意历史版本。
//! 版本存储在 SQLite 中（复用 data_queue 所在数据库，或独立文件），
//! 每条记录包含：版本号、时间戳、配置 JSON、备注。

use std::path::PathBuf;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::config::GatewayConfig;
use crate::engine::error::GatewayResult;

/// 配置版本记录
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ConfigVersion {
    /// 版本号（自增主键）
    #[ts(type = "number")]
    pub id: i64,
    /// 创建时间戳（毫秒）
    #[ts(type = "number")]
    pub timestamp: i64,
    /// 配置 JSON 字符串
    pub config_json: String,
    /// 备注（如 "user edit"、"rollback from v3"）
    pub note: String,
}

/// 配置版本管理器
pub struct ConfigVersionStore {
    db_path: PathBuf,
}

impl ConfigVersionStore {
    /// 创建版本管理器并初始化表
    pub fn new(db_path: &str) -> GatewayResult<Self> {
        let conn = Connection::open(db_path).map_err(|e| {
            crate::engine::error::GatewayError::Queue(format!("open version db failed: {}", e))
        })?;
        // r39-VER1：本库文件（default_version_db）由 config_versions / uplink_test_history /
        // disk_history 三处共享，各自持有独立连接并发写。WAL 下写者互斥，未设置 busy_timeout
        // 时遇 SQLITE_BUSY 会立即失败（默认 0ms）——表现为版本快照偶发丢失。
        // 与 outbox（C18）/ 队列主库统一为 5s 退避排队。
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")
            .map_err(|e| {
                crate::engine::error::GatewayError::Queue(format!("set WAL failed: {}", e))
            })?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS config_versions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                config_json TEXT NOT NULL,
                note TEXT NOT NULL DEFAULT ''
            )",
            [],
        )
        .map_err(|e| {
            crate::engine::error::GatewayError::Queue(format!(
                "create versions table failed: {}",
                e
            ))
        })?;
        Ok(Self {
            db_path: PathBuf::from(db_path),
        })
    }

    /// 保存一个配置版本快照
    ///
    /// Bug C5: 快照落在 SQLite 中，而内存 config 为解密后明文，直接序列化会把凭据明文
    /// 写入 config_versions.db。与 `ConfigLoader::save` 一致，先在副本上重新加密再落盘。
    pub fn save_version(&self, config: &GatewayConfig, note: &str) -> GatewayResult<i64> {
        let mut to_write = config.clone();
        crate::crypto::encrypt_config_secrets(&mut to_write).map_err(|e| {
            crate::engine::error::GatewayError::Queue(format!(
                "encrypt config secrets before version snapshot failed: {}",
                e
            ))
        })?;
        let config_json = serde_json::to_string(&to_write).map_err(|e| {
            crate::engine::error::GatewayError::Queue(format!("serialize config failed: {}", e))
        })?;
        let timestamp = chrono::Utc::now().timestamp_millis();
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO config_versions (timestamp, config_json, note) VALUES (?1, ?2, ?3)",
            params![timestamp, config_json, note],
        )
        .map_err(|e| {
            crate::engine::error::GatewayError::Queue(format!("insert version failed: {}", e))
        })?;
        let id = conn.last_insert_rowid();
        // 保留最近 50 个版本，删除更早的（防止无限增长）
        // P2-d: 原实现用 `let _ =` 静默吞掉裁剪失败，版本表可能持续膨胀而不留痕。
        if let Err(e) = conn.execute(
            "DELETE FROM config_versions WHERE id NOT IN (
                SELECT id FROM config_versions ORDER BY id DESC LIMIT 50
            )",
            [],
        ) {
            tracing::warn!("config_versions retention cleanup failed: {}", e);
        }
        Ok(id)
    }

    /// 列出所有版本（按 id 降序，最新在前）
    pub fn list_versions(&self) -> GatewayResult<Vec<ConfigVersion>> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, timestamp, config_json, note FROM config_versions ORDER BY id DESC",
            )
            .map_err(|e| {
                crate::engine::error::GatewayError::Queue(format!("prepare failed: {}", e))
            })?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ConfigVersion {
                    id: row.get(0)?,
                    timestamp: row.get(1)?,
                    config_json: row.get(2)?,
                    note: row.get(3)?,
                })
            })
            .map_err(|e| {
                crate::engine::error::GatewayError::Queue(format!("query failed: {}", e))
            })?;
        let mut versions = Vec::new();
        for row in rows {
            versions.push(row.map_err(|e| {
                crate::engine::error::GatewayError::Queue(format!("row failed: {}", e))
            })?);
        }
        Ok(versions)
    }

    /// 获取指定版本的配置
    pub fn get_version(&self, id: i64) -> GatewayResult<Option<ConfigVersion>> {
        let conn = self.conn()?;
        let result = conn.query_row(
            "SELECT id, timestamp, config_json, note FROM config_versions WHERE id = ?1",
            params![id],
            |row| {
                Ok(ConfigVersion {
                    id: row.get(0)?,
                    timestamp: row.get(1)?,
                    config_json: row.get(2)?,
                    note: row.get(3)?,
                })
            },
        );
        match result {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(crate::engine::error::GatewayError::Queue(format!(
                "query version failed: {}",
                e
            ))),
        }
    }

    /// 回滚到指定版本：返回该版本的配置
    pub fn rollback(&self, id: i64) -> GatewayResult<Option<GatewayConfig>> {
        match self.get_version(id)? {
            Some(v) => {
                let mut config: GatewayConfig =
                    serde_json::from_str(&v.config_json).map_err(|e| {
                        crate::engine::error::GatewayError::Queue(format!(
                            "deserialize config failed: {}",
                            e
                        ))
                    })?;
                // Bug C5: 快照落盘时已加密，回滚须解密回明文，否则运行时会拿到 `enc:...` 字面串。
                crate::crypto::decrypt_config_secrets(&mut config).map_err(|e| {
                    crate::engine::error::GatewayError::Queue(format!(
                        "decrypt config secrets of version {} failed: {}",
                        id, e
                    ))
                })?;
                Ok(Some(config))
            }
            None => Ok(None),
        }
    }

    fn conn(&self) -> GatewayResult<Connection> {
        let conn = Connection::open(&self.db_path).map_err(|e| {
            crate::engine::error::GatewayError::Queue(format!("open db failed: {}", e))
        })?;
        // r39-VER1：busy_timeout 是 per-connection 设置，new() 里设过的不会继承到
        // 后续按需新开的连接，故每次 open 都必须重设，否则并发写仍会立即 SQLITE_BUSY。
        conn.busy_timeout(std::time::Duration::from_millis(5000))
            .map_err(|e| {
                crate::engine::error::GatewayError::Queue(format!("set busy_timeout failed: {}", e))
            })?;
        Ok(conn)
    }
}

#[cfg(test)]
mod tests {
    use super::ConfigVersionStore;

    /// r39-VER1 回归：连接须设置 busy_timeout —— 该库文件被 config 版本 / 上行测试历史 /
    /// 磁盘历史三处共享并发写，未设超时会在 SQLITE_BUSY 时立即失败，导致快照偶发丢失。
    #[test]
    fn version_store_conn_sets_busy_timeout() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "version-store-test-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("should succeed");
        let db = dir.join("versions.db");

        let store = ConfigVersionStore::new(db.to_str().expect("utf8")).expect("should succeed");
        let conn = store.conn().expect("should succeed");
        let timeout: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .expect("should succeed");
        assert_eq!(timeout, 5000, "busy_timeout 应为 5000ms");
    }
}
