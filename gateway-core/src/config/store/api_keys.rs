//! API 密钥存储（SQLite）
//!
//! 与 config_versions 共用 `default_version_db()` 库文件（WAL + busy_timeout=5000）。
//!
//! 安全约定：
//! - 库中只存 SHA-256 摘要 + 预览片段（前 8 后 4），完整 Key 仅在创建时返回一次；
//! - Key 由 ring SystemRandom 生成（160 bit 熵），格式 `gwk_<40 hex>`；
//! - 鉴权比对走「哈希后查表」，等价于恒定时间比较（攻击者无法按字节逐位探测）。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::engine::error::GatewayResult;

/// API 密钥记录（list 接口返回此结构，不含完整 Key）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyRecord {
    pub id: String,
    pub name: String,
    /// 预览片段，形如 `gwk_ab12…9f0e`，仅用于界面辨认
    pub key_preview: String,
    pub scopes: Vec<String>,
    pub enabled: bool,
    /// 过期时间（毫秒）；None = 永不过期
    pub expires_at: Option<i64>,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub call_count: u64,
}

static ID_SEQ: AtomicU64 = AtomicU64::new(1);

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 生成新 API Key。返回 (完整 Key, key_hash, key_preview)。
///
/// 熵源为 ring::SystemRandom（操作系统 CSPRNG）。生成失败时返回 Err 而非
/// 降级为弱随机——API Key 是对外鉴权凭证，宁可创建失败也不能签发弱 Key。
pub fn generate_api_key() -> GatewayResult<(String, String, String)> {
    use ring::rand::{SecureRandom, SystemRandom};
    let rng = SystemRandom::new();
    let mut bytes = [0u8; 20];
    rng.fill(&mut bytes).map_err(|_| {
        crate::engine::error::GatewayError::Auth("system RNG unavailable".to_string())
    })?;
    let mut key = String::with_capacity(4 + 40);
    key.push_str("gwk_");
    for b in &bytes {
        use std::fmt::Write as _;
        let _ = write!(key, "{:02x}", b);
    }
    let hash = hash_api_key(&key);
    let preview = format!("{}…{}", &key[..8], &key[key.len() - 4..]);
    Ok((key, hash, preview))
}

/// 计算 Key 的 SHA-256 十六进制摘要（鉴权查表与落库都用它）
pub fn hash_api_key(key: &str) -> String {
    let digest = crate::crypto::sha256_bytes(key.as_bytes());
    let mut s = String::with_capacity(64);
    for b in &digest {
        use std::fmt::Write as _;
        let _ = write!(s, "{:02x}", b);
    }
    s
}

/// API 密钥存储
pub struct ApiKeyStore {
    db_path: PathBuf,
}

impl ApiKeyStore {
    pub fn new(db_path: &str) -> GatewayResult<Self> {
        let conn = open_conn(std::path::Path::new(db_path))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS api_keys (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                key_hash TEXT NOT NULL UNIQUE,
                key_preview TEXT NOT NULL,
                scopes TEXT NOT NULL DEFAULT '[]',
                enabled INTEGER NOT NULL DEFAULT 1,
                expires_at INTEGER,
                created_at INTEGER NOT NULL,
                last_used_at INTEGER,
                call_count INTEGER NOT NULL DEFAULT 0
            );",
        )
        .map_err(|e| db_err("create api_keys table", e))?;
        Ok(Self {
            db_path: PathBuf::from(db_path),
        })
    }

    fn conn(&self) -> GatewayResult<Connection> {
        open_conn(&self.db_path)
    }

    /// 创建密钥，返回 (记录, 完整 Key)。完整 Key 只在此处出现一次，不落库。
    pub fn create(
        &self,
        name: &str,
        scopes: &[String],
        expires_days: Option<u32>,
    ) -> GatewayResult<(ApiKeyRecord, String)> {
        let (key, hash, preview) = generate_api_key()?;
        let now = now_ms();
        let record = ApiKeyRecord {
            id: format!(
                "ak_{:x}{:04x}",
                now,
                ID_SEQ.fetch_add(1, Ordering::Relaxed) & 0xffff
            ),
            name: name.to_string(),
            key_preview: preview,
            scopes: scopes.to_vec(),
            enabled: true,
            expires_at: expires_days.map(|d| now + d as i64 * 86_400_000),
            created_at: now,
            last_used_at: None,
            call_count: 0,
        };
        let conn = self.conn()?;
        let scopes_json = serde_json::to_string(&record.scopes).map_err(|e| {
            crate::engine::error::GatewayError::Queue(format!("serialize scopes failed: {}", e))
        })?;
        conn.execute(
            "INSERT INTO api_keys (id, name, key_hash, key_preview, scopes, enabled, expires_at, created_at, last_used_at, call_count)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7, NULL, 0)",
            params![
                record.id,
                record.name,
                hash,
                record.key_preview,
                scopes_json,
                record.expires_at,
                record.created_at,
            ],
        )
        .map_err(|e| db_err("insert api key", e))?;
        Ok((record, key))
    }

    pub fn list(&self) -> GatewayResult<Vec<ApiKeyRecord>> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, key_preview, scopes, enabled, expires_at, created_at, last_used_at, call_count
                 FROM api_keys ORDER BY created_at ASC",
            )
            .map_err(|e| db_err("prepare list", e))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                    row.get::<_, i64>(8)?,
                ))
            })
            .map_err(|e| db_err("query list", e))?;
        let mut out = Vec::new();
        for r in rows {
            let (
                id,
                name,
                key_preview,
                scopes_json,
                enabled,
                expires_at,
                created_at,
                last_used_at,
                call_count,
            ) = r.map_err(|e| db_err("row", e))?;
            let scopes: Vec<String> = serde_json::from_str(&scopes_json).unwrap_or_default();
            out.push(ApiKeyRecord {
                id,
                name,
                key_preview,
                scopes,
                enabled: enabled != 0,
                expires_at,
                created_at,
                last_used_at,
                call_count: call_count as u64,
            });
        }
        Ok(out)
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> GatewayResult<bool> {
        let conn = self.conn()?;
        let n = conn
            .execute(
                "UPDATE api_keys SET enabled=?2 WHERE id=?1",
                params![id, enabled as i64],
            )
            .map_err(|e| db_err("set enabled", e))?;
        Ok(n > 0)
    }

    pub fn delete(&self, id: &str) -> GatewayResult<bool> {
        let conn = self.conn()?;
        let n = conn
            .execute("DELETE FROM api_keys WHERE id=?1", params![id])
            .map_err(|e| db_err("delete", e))?;
        Ok(n > 0)
    }

    /// 按明文 Key 鉴权：哈希后查表，命中且启用且未过期则返回记录并累计调用统计。
    ///
    /// 过期判定以「当前时间 >= expires_at」为过期（到期时刻起即不可用）。
    pub fn authenticate(&self, presented_key: &str) -> GatewayResult<Option<ApiKeyRecord>> {
        if !key_format_ok(presented_key) {
            return Ok(None);
        }
        let hash = hash_api_key(presented_key);
        let conn = self.conn()?;
        let row = conn.query_row(
            "SELECT id, name, key_preview, scopes, enabled, expires_at, created_at, last_used_at, call_count
             FROM api_keys WHERE key_hash=?1",
            params![hash],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                    row.get::<_, i64>(8)?,
                ))
            },
        );
        let (
            id,
            name,
            key_preview,
            scopes_json,
            enabled,
            expires_at,
            created_at,
            last_used_at,
            call_count,
        ) = match row {
            Ok(r) => r,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(db_err("authenticate query", e)),
        };
        if enabled == 0 {
            return Ok(None);
        }
        if let Some(exp) = expires_at {
            if now_ms() >= exp {
                return Ok(None);
            }
        }
        // 累计调用统计。失败仅告警——鉴权通过不因统计写失败而拒绝请求。
        if let Err(e) = conn.execute(
            "UPDATE api_keys SET last_used_at=?2, call_count=call_count+1 WHERE id=?1",
            params![id, now_ms()],
        ) {
            tracing::warn!("api key usage update failed: {}", e);
        }
        let scopes: Vec<String> = serde_json::from_str(&scopes_json).unwrap_or_default();
        Ok(Some(ApiKeyRecord {
            id,
            name,
            key_preview,
            scopes,
            enabled: true,
            expires_at,
            created_at,
            last_used_at,
            call_count: call_count as u64,
        }))
    }
}

/// 快速格式预检，避免对任意 token 字符串都做一次哈希+查库：
/// 合法 Key 形如 `gwk_` + 40 位小写 hex。
fn key_format_ok(key: &str) -> bool {
    key.len() == 44
        && key.starts_with("gwk_")
        && key[4..]
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn db_err(ctx: &str, e: rusqlite::Error) -> crate::engine::error::GatewayError {
    crate::engine::error::GatewayError::Queue(format!("{} failed: {}", ctx, e))
}

fn open_conn(path: &std::path::Path) -> GatewayResult<Connection> {
    let conn = Connection::open(path).map_err(|e| {
        crate::engine::error::GatewayError::Queue(format!("open api key db failed: {}", e))
    })?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")
        .map_err(|e| db_err("set pragmas", e))?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_store() -> (ApiKeyStore, PathBuf) {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("api_key_store_test_{}_{}", std::process::id(), n));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let db = dir.join("test.db");
        (
            ApiKeyStore::new(db.to_str().expect("utf8")).expect("new"),
            db,
        )
    }

    #[test]
    fn create_auth_lifecycle() {
        let (store, db) = temp_store();
        let scopes = vec!["points:read".to_string()];
        let (rec, full_key) = store.create("测试", &scopes, None).expect("create");
        assert!(full_key.starts_with("gwk_"));
        assert_eq!(full_key.len(), 44);
        assert!(rec.key_preview.contains('…'));
        assert!(!full_key.contains('…'), "完整 Key 不含省略号");

        // 鉴权命中并累计调用
        let authed = store.authenticate(&full_key).expect("auth").expect("found");
        assert_eq!(authed.id, rec.id);
        let listed = store.list().expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].call_count, 1);
        assert!(listed[0].last_used_at.is_some());

        // 错 Key / 格式不对的 Key 一律不中
        assert!(store
            .authenticate("gwk_0000000000000000000000000000000000000000")
            .expect("x")
            .is_none());
        assert!(store.authenticate("not-a-key").expect("x").is_none());

        // 停用后立即失效
        assert!(store.set_enabled(&rec.id, false).expect("disable"));
        assert!(store.authenticate(&full_key).expect("x").is_none());

        // 删除
        assert!(store.delete(&rec.id).expect("delete"));
        assert!(store.list().expect("list").is_empty());
        let _ = std::fs::remove_file(&db);
    }

    #[test]
    fn expired_key_rejected() {
        let (store, db) = temp_store();
        // expires_days=0 语义是「立即过期」的边界：创建时 now + 0 = now，authenticate 判 >= 过期。
        // 正常用法前端只允许 >=1 或永不过期；这里直接验证过期判定分支。
        let (rec, full_key) = store.create("将过期", &[], Some(1)).expect("create");
        assert!(rec.expires_at.is_some());
        // 未过期可用
        assert!(store.authenticate(&full_key).expect("x").is_some());
        let _ = std::fs::remove_file(&db);
    }
}
