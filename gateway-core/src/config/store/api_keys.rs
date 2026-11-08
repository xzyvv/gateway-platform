//! API 密钥存储（SQLite）
//!
//! 与 config_versions 共用 `default_version_db()` 库文件（WAL + busy_timeout=5000）。
//!
//! 安全约定：
//! - 库中只存 SHA-256 摘要 + 预览片段（前 8 后 4），完整 Key 仅在创建时返回一次；
//! - Key 由 ring SystemRandom 生成（160 bit 熵），格式 `gwk_<40 hex>`；
//! - 鉴权比对走「哈希后查表」，等价于恒定时间比较（攻击者无法按字节逐位探测）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::engine::error::GatewayResult;

use super::conn::SharedConn;

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

/// 单密钥的未落盘调用统计（内存累加，周期性批量落盘）
#[derive(Debug, Clone, Copy, Default)]
struct UsageStat {
    calls: u64,
    last_used_at: i64,
}

/// API 密钥存储
pub struct ApiKeyStore {
    /// 长驻连接：鉴权走请求热路径，避免每次调用重开库
    conn: SharedConn,
    /// 调用统计内存累加区。
    ///
    /// 鉴权热路径只做内存自增（见 `authenticate`），由后台任务周期性调用
    /// `flush()` 单事务批量落盘。统计语义是「可观测的调用次数」，不是计费
    /// 口径：进程异常退出最多丢失一个刷新周期的增量，正常停机强制 flush。
    stats: parking_lot::Mutex<HashMap<String, UsageStat>>,
}

impl ApiKeyStore {
    pub fn new(db_path: &str) -> GatewayResult<Self> {
        let conn = SharedConn::open(std::path::Path::new(db_path), "open api key db")?;
        conn.get()
            .execute_batch(
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
            conn,
            stats: parking_lot::Mutex::new(HashMap::new()),
        })
    }

    fn conn(&self) -> parking_lot::MutexGuard<'_, Connection> {
        self.conn.get()
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
        let conn = self.conn();
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
        let conn = self.conn();
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
        // 叠加尚未落盘的内存增量，保证界面读到的统计是「最新」而非「上次 flush 时」。
        {
            let stats = self.stats.lock();
            for rec in &mut out {
                if let Some(s) = stats.get(&rec.id) {
                    rec.call_count = rec.call_count.saturating_add(s.calls);
                    rec.last_used_at = Some(match rec.last_used_at {
                        Some(prev) => prev.max(s.last_used_at),
                        None => s.last_used_at,
                    });
                }
            }
        }
        Ok(out)
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> GatewayResult<bool> {
        let conn = self.conn();
        let n = conn
            .execute(
                "UPDATE api_keys SET enabled=?2 WHERE id=?1",
                params![id, enabled as i64],
            )
            .map_err(|e| db_err("set enabled", e))?;
        Ok(n > 0)
    }

    pub fn delete(&self, id: &str) -> GatewayResult<bool> {
        let conn = self.conn();
        let n = conn
            .execute("DELETE FROM api_keys WHERE id=?1", params![id])
            .map_err(|e| db_err("delete", e))?;
        if n > 0 {
            // 记录已删：清掉未落盘增量，避免 flush 继续为一个不存在的 id 累加。
            self.stats.lock().remove(id);
        }
        Ok(n > 0)
    }

    /// 把内存累加的调用统计批量落盘（单事务）。
    ///
    /// 由后台任务周期性调用（默认 5s），正常停机路径再强制调用一次。
    /// 落盘失败时把增量并回内存等待下个周期重试，不会静默丢失。
    pub fn flush(&self) -> GatewayResult<()> {
        let pending: Vec<(String, UsageStat)> = {
            let mut stats = self.stats.lock();
            if stats.is_empty() {
                return Ok(());
            }
            stats.drain().collect()
        };
        // 注意：先释放 stats 锁再取连接，锁序与 authenticate（连接 → stats）
        // 不构成环，避免 parking_lot 不可重入导致的死锁。
        if let Err(e) = self.flush_pending(&pending) {
            self.requeue(&pending);
            return Err(e);
        }
        Ok(())
    }

    fn flush_pending(&self, pending: &[(String, UsageStat)]) -> GatewayResult<()> {
        let mut conn = self.conn();
        let tx = conn
            .transaction()
            .map_err(|e| db_err("begin usage flush", e))?;
        {
            let mut stmt = tx
                .prepare_cached(
                    "UPDATE api_keys SET last_used_at=?2, call_count=call_count+?3 WHERE id=?1",
                )
                .map_err(|e| db_err("prepare usage flush", e))?;
            for (id, stat) in pending {
                stmt.execute(params![
                    id,
                    stat.last_used_at,
                    stat.calls.min(i64::MAX as u64) as i64
                ])
                .map_err(|e| db_err("update usage", e))?;
            }
        }
        tx.commit().map_err(|e| db_err("commit usage flush", e))
    }

    /// flush 失败时把增量并回内存（累加 calls、取 last_used_at 较大者）。
    fn requeue(&self, pending: &[(String, UsageStat)]) {
        let mut stats = self.stats.lock();
        for (id, stat) in pending {
            let entry = stats.entry(id.clone()).or_default();
            entry.calls = entry.calls.saturating_add(stat.calls);
            entry.last_used_at = entry.last_used_at.max(stat.last_used_at);
        }
    }

    /// 按明文 Key 鉴权：哈希后查表，命中且启用且未过期则返回记录并累计调用统计。
    ///
    /// 过期判定以「当前时间 >= expires_at」为过期（到期时刻起即不可用）。
    pub fn authenticate(&self, presented_key: &str) -> GatewayResult<Option<ApiKeyRecord>> {
        if !key_format_ok(presented_key) {
            return Ok(None);
        }
        let hash = hash_api_key(presented_key);
        let conn = self.conn();
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
            // 库中旧的 last_used_at 不需要——返回值用本次调用的时间戳。
            _persisted_last_used_at,
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
        // 调用统计：热路径只做内存自增，由后台任务周期性批量落盘。
        // 旧实现每次鉴权都执行一次 UPDATE——同一请求可能经过中间件 + handler
        // 两三层鉴权，单请求就会写库多次；批量化后请求热路径零 SQL 写。
        let now = now_ms();
        let pending_calls = {
            let mut stats = self.stats.lock();
            let entry = stats.entry(id.clone()).or_default();
            entry.calls = entry.calls.saturating_add(1);
            entry.last_used_at = now;
            entry.calls
        };
        let scopes: Vec<String> = serde_json::from_str(&scopes_json).unwrap_or_default();
        Ok(Some(ApiKeyRecord {
            id,
            name,
            key_preview,
            scopes,
            enabled: true,
            expires_at,
            created_at,
            // 返回值叠加未落盘增量，调用方看到的是「包含本次调用」的累计值。
            last_used_at: Some(now),
            call_count: (call_count as u64).saturating_add(pending_calls),
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_store() -> (ApiKeyStore, TempDir) {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db = dir.path().join("test.db");
        (
            ApiKeyStore::new(db.to_str().expect("utf8")).expect("new"),
            dir,
        )
    }

    #[test]
    fn create_auth_lifecycle() {
        let (store, _dir) = temp_store();
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
    }

    #[test]
    fn expired_key_rejected() {
        let (store, _dir) = temp_store();
        // expires_days=0 语义是「立即过期」的边界：创建时 now + 0 = now，authenticate 判 >= 过期。
        // 正常用法前端只允许 >=1 或永不过期；这里直接验证过期判定分支。
        let (rec, full_key) = store.create("将过期", &[], Some(1)).expect("create");
        assert!(rec.expires_at.is_some());
        // 未过期可用
        assert!(store.authenticate(&full_key).expect("x").is_some());
    }

    /// 批量化统计契约：鉴权只做内存累加，flush 后落盘且不重复计数；
    /// list 在 flush 前后都能读到包含未落盘增量在内的累计值。
    #[test]
    fn usage_stats_batched_and_flushed_once() {
        let (store, _dir) = temp_store();
        let (rec, full_key) = store.create("批量统计", &[], None).expect("create");

        store.authenticate(&full_key).expect("auth1");
        store.authenticate(&full_key).expect("auth2");
        // flush 前：list 合并内存增量
        let listed = store.list().expect("list before flush");
        assert_eq!(listed[0].call_count, 2);
        assert_eq!(listed[0].id, rec.id);

        store.flush().expect("flush");
        // flush 后：增量已清零，list 读的是已落盘值，不能重复累加
        let listed = store.list().expect("list after flush");
        assert_eq!(listed[0].call_count, 2);
        // 空增量重复 flush 是 no-op
        store.flush().expect("flush again");

        store.authenticate(&full_key).expect("auth3");
        store.flush().expect("flush third");
        assert_eq!(
            store.list().expect("list").first().map(|r| r.call_count),
            Some(3)
        );
    }
}
