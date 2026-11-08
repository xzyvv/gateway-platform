//! API 限流规则 + 访问日志存储（SQLite）
//!
//! 与 config_versions / uplink_test_history 共用 `default_version_db()` 库文件，
//! 各持独立连接并发写（WAL + busy_timeout=5000，与 version.rs 同一约定）。
//!
//! - `rate_limit_rules`：按「路径前缀 + 方法」声明每分钟/每小时配额与突发余量，
//!   由 gateway-api 的 rate_limit 中间件在请求路径上真实执行。
//! - `api_access_logs`：记录每个 /api/* 请求的状态码/延迟/来源 IP/是否被限流，
//!   供「限流配置」页的访问日志与统计接口读取。保留最近 5000 行，防止膨胀。

use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::engine::error::GatewayResult;

use super::conn::SharedConn;

/// 访问日志保留行数上限：超出后裁掉最旧的一批。
/// 网关 API 流量有限，5000 行足够覆盖现场排障所需的「最近发生了什么」。
const ACCESS_LOG_KEEP: usize = 5000;
/// 达到 KEEP + BATCH 时才执行一次裁剪，避免每行插入都跑 DELETE。
const ACCESS_LOG_PRUNE_BATCH: usize = 500;

/// 限流规则
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RateRule {
    pub id: String,
    pub name: String,
    /// 匹配路径：精确路径（/api/devices）或以 `*` 结尾的前缀（/api/config/*）
    pub path: String,
    /// HTTP 方法：GET/POST/PUT/DELETE 或 `*`（全部）
    pub method: String,
    pub limit_per_minute: u32,
    pub limit_per_hour: u32,
    /// 突发余量：分钟窗口内在 limit_per_minute 之上额外允许的请求数
    pub burst: u32,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

/// API 访问日志条目
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiAccessLog {
    pub id: i64,
    /// 毫秒时间戳
    pub ts: i64,
    pub method: String,
    pub path: String,
    pub status: u16,
    pub latency_ms: u64,
    pub client_ip: String,
    /// 是否被限流拒绝（HTTP 429）
    pub limited: bool,
}

/// 限流统计（由访问日志聚合得到）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitStats {
    pub total_requests: u64,
    pub limited_requests: u64,
    pub avg_latency_ms: u64,
}

/// 规则 ID 序号：与毫秒时间戳组合，避免同毫秒并发创建撞号
static NEW_ID_SEQ: AtomicU64 = AtomicU64::new(1);

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn new_id(prefix: &str) -> String {
    let seq = NEW_ID_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{}_{:x}{:04x}", prefix, now_ms(), seq & 0xffff)
}

/// 限流规则 + 访问日志存储
pub struct RateRuleStore {
    /// 长驻连接：限流判定在请求热路径上，避免每次调用重开库
    conn: SharedConn,
}

impl RateRuleStore {
    pub fn new(db_path: &str) -> GatewayResult<Self> {
        let conn = SharedConn::open(std::path::Path::new(db_path), "open rate rule db")?;
        conn.get()
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS rate_limit_rules (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                path TEXT NOT NULL,
                method TEXT NOT NULL DEFAULT '*',
                limit_per_minute INTEGER NOT NULL,
                limit_per_hour INTEGER NOT NULL,
                burst INTEGER NOT NULL DEFAULT 0,
                enabled INTEGER NOT NULL DEFAULT 1,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS api_access_logs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts INTEGER NOT NULL,
                method TEXT NOT NULL,
                path TEXT NOT NULL,
                status INTEGER NOT NULL,
                latency_ms INTEGER NOT NULL,
                client_ip TEXT NOT NULL,
                limited INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_api_access_logs_ts ON api_access_logs(ts);",
            )
            .map_err(|e| {
                crate::engine::error::GatewayError::Queue(format!(
                    "create rate rule tables failed: {}",
                    e
                ))
            })?;
        Ok(Self { conn })
    }

    fn conn(&self) -> parking_lot::MutexGuard<'_, Connection> {
        self.conn.get()
    }

    /// 列出全部规则（创建时间升序，先建的在前）
    pub fn list_rules(&self) -> GatewayResult<Vec<RateRule>> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, name, path, method, limit_per_minute, limit_per_hour, burst, enabled, created_at, updated_at
                 FROM rate_limit_rules ORDER BY created_at ASC",
            )
            .map_err(|e| db_err("prepare list rules", e))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(RateRule {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    path: row.get(2)?,
                    method: row.get(3)?,
                    limit_per_minute: row.get::<_, i64>(4)? as u32,
                    limit_per_hour: row.get::<_, i64>(5)? as u32,
                    burst: row.get::<_, i64>(6)? as u32,
                    enabled: row.get::<_, i64>(7)? != 0,
                    created_at: row.get(8)?,
                    updated_at: row.get(9)?,
                })
            })
            .map_err(|e| db_err("query rules", e))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| db_err("row", e))?);
        }
        Ok(out)
    }

    /// 仅列出启用中的规则（中间件热路径用）
    pub fn list_enabled_rules(&self) -> GatewayResult<Vec<RateRule>> {
        Ok(self
            .list_rules()?
            .into_iter()
            .filter(|r| r.enabled)
            .collect())
    }

    pub fn create_rule(&self, mut rule: RateRule) -> GatewayResult<RateRule> {
        let now = now_ms();
        rule.id = new_id("rr");
        rule.created_at = now;
        rule.updated_at = now;
        let conn = self.conn();
        conn.execute(
            "INSERT INTO rate_limit_rules (id, name, path, method, limit_per_minute, limit_per_hour, burst, enabled, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                rule.id,
                rule.name,
                rule.path,
                rule.method,
                rule.limit_per_minute as i64,
                rule.limit_per_hour as i64,
                rule.burst as i64,
                rule.enabled as i64,
                rule.created_at,
                rule.updated_at,
            ],
        )
        .map_err(|e| db_err("insert rule", e))?;
        Ok(rule)
    }

    /// 更新规则；返回 None 表示规则不存在
    pub fn update_rule(&self, id: &str, rule: &RateRule) -> GatewayResult<Option<RateRule>> {
        // 连接锁不可重入：delegation 回查前必须先释放，否则自死锁
        let n = {
            let conn = self.conn();
            conn.execute(
                "UPDATE rate_limit_rules SET name=?2, path=?3, method=?4, limit_per_minute=?5,
                 limit_per_hour=?6, burst=?7, enabled=?8, updated_at=?9 WHERE id=?1",
                params![
                    id,
                    rule.name,
                    rule.path,
                    rule.method,
                    rule.limit_per_minute as i64,
                    rule.limit_per_hour as i64,
                    rule.burst as i64,
                    rule.enabled as i64,
                    now_ms(),
                ],
            )
            .map_err(|e| db_err("update rule", e))?
        };
        if n == 0 {
            return Ok(None);
        }
        self.get_rule(id)
    }

    pub fn get_rule(&self, id: &str) -> GatewayResult<Option<RateRule>> {
        Ok(self.list_rules()?.into_iter().find(|r| r.id == id))
    }

    /// 删除规则；返回是否删除了行
    pub fn delete_rule(&self, id: &str) -> GatewayResult<bool> {
        let conn = self.conn();
        let n = conn
            .execute("DELETE FROM rate_limit_rules WHERE id=?1", params![id])
            .map_err(|e| db_err("delete rule", e))?;
        Ok(n > 0)
    }

    /// 追加一条访问日志。写入频繁，故：
    /// - 每行只一条 INSERT（WAL 下毫秒级）；
    /// - 每 ACCESS_LOG_PRUNE_BATCH 行才做一次保留裁剪。
    pub fn append_access_log(&self, entry: &ApiAccessLog) -> GatewayResult<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO api_access_logs (ts, method, path, status, latency_ms, client_ip, limited)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                entry.ts,
                entry.method,
                entry.path,
                entry.status as i64,
                entry.latency_ms as i64,
                entry.client_ip,
                entry.limited as i64,
            ],
        )
        .map_err(|e| db_err("insert access log", e))?;
        let id = conn.last_insert_rowid();
        if id % (ACCESS_LOG_PRUNE_BATCH as i64) == 0 {
            let keep_from = id - ACCESS_LOG_KEEP as i64;
            if let Err(e) = conn.execute(
                "DELETE FROM api_access_logs WHERE id < ?1",
                params![keep_from],
            ) {
                tracing::warn!("api_access_logs retention cleanup failed: {}", e);
            }
        }
        Ok(())
    }

    /// 最近访问日志（最新在前）
    pub fn recent_access_logs(&self, limit: usize) -> GatewayResult<Vec<ApiAccessLog>> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, ts, method, path, status, latency_ms, client_ip, limited
                 FROM api_access_logs ORDER BY id DESC LIMIT ?1",
            )
            .map_err(|e| db_err("prepare access logs", e))?;
        let rows = stmt
            .query_map(params![limit as i64], |row| {
                Ok(ApiAccessLog {
                    id: row.get(0)?,
                    ts: row.get(1)?,
                    method: row.get(2)?,
                    path: row.get(3)?,
                    status: row.get::<_, i64>(4)? as u16,
                    latency_ms: row.get::<_, i64>(5)? as u64,
                    client_ip: row.get(6)?,
                    limited: row.get::<_, i64>(7)? != 0,
                })
            })
            .map_err(|e| db_err("query access logs", e))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| db_err("row", e))?);
        }
        Ok(out)
    }

    /// 聚合统计（基于保留窗口内的访问日志）
    pub fn stats(&self) -> GatewayResult<RateLimitStats> {
        let conn = self.conn();
        // avg 只统计未被限流的请求：429 不过 handler、延迟≈0，混入会把均值拉低——
        // 限流越狠面板显得「越快」，与运维直觉相反。total/limited 口径不变（limited 是 total 子集）。
        let (total, limited, avg): (i64, i64, f64) = conn
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(limited), 0),
                        COALESCE(AVG(CASE WHEN limited = 0 THEN latency_ms END), 0)
                 FROM api_access_logs",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|e| db_err("stats", e))?;
        Ok(RateLimitStats {
            total_requests: total as u64,
            limited_requests: limited as u64,
            avg_latency_ms: avg.round() as u64,
        })
    }
}

fn db_err(ctx: &str, e: rusqlite::Error) -> crate::engine::error::GatewayError {
    crate::engine::error::GatewayError::Queue(format!("{} failed: {}", ctx, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_store() -> (RateRuleStore, PathBuf) {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("rate_rule_store_test_{}_{}", std::process::id(), n));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let db = dir.join("test.db");
        (
            RateRuleStore::new(db.to_str().expect("utf8")).expect("new"),
            db,
        )
    }

    fn sample_rule() -> RateRule {
        RateRule {
            id: String::new(),
            name: "设备列表".into(),
            path: "/api/devices".into(),
            method: "GET".into(),
            limit_per_minute: 60,
            limit_per_hour: 1000,
            burst: 20,
            enabled: true,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn rule_crud_roundtrip() {
        let (store, db) = temp_store();
        let created = store.create_rule(sample_rule()).expect("create");
        assert!(created.id.starts_with("rr_"));
        assert!(created.created_at > 0);

        let listed = store.list_rules().expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "设备列表");

        let mut patch = created.clone();
        patch.enabled = false;
        patch.limit_per_minute = 30;
        let updated = store
            .update_rule(&created.id, &patch)
            .expect("update")
            .expect("exists");
        assert!(!updated.enabled);
        assert_eq!(updated.limit_per_minute, 30);
        assert!(updated.updated_at >= updated.created_at);

        assert!(store.list_enabled_rules().expect("enabled").is_empty());

        assert!(store.delete_rule(&created.id).expect("delete"));
        assert!(store.list_rules().expect("list").is_empty());
        assert!(!store.delete_rule("rr_nonexistent").expect("delete none"));

        let _ = std::fs::remove_file(&db);
    }

    #[test]
    fn access_log_append_query_stats() {
        let (store, db) = temp_store();
        for i in 0..10 {
            store
                .append_access_log(&ApiAccessLog {
                    id: 0,
                    ts: now_ms() + i,
                    method: "GET".into(),
                    path: "/api/devices".into(),
                    status: if i == 9 { 429 } else { 200 },
                    latency_ms: 10 + i as u64,
                    client_ip: "127.0.0.1".into(),
                    limited: i == 9,
                })
                .expect("append");
        }
        let logs = store.recent_access_logs(5).expect("recent");
        assert_eq!(logs.len(), 5, "limit 必须生效");
        assert!(logs[0].ts >= logs[4].ts, "最新在前");
        assert!(logs[0].limited, "最新一条是 429");

        let stats = store.stats().expect("stats");
        assert_eq!(stats.total_requests, 10);
        assert_eq!(stats.limited_requests, 1);
        // 均值必须排除 429 样本：非限流请求延迟 10..=18，均值 14；
        // 若混入 429 样本（延迟 19）则为 14.5→15。
        assert_eq!(stats.avg_latency_ms, 14, "avg 不得混入被限流请求");

        let _ = std::fs::remove_file(&db);
    }

    /// 全部请求都被限流时 avg 应为 0（CASE 全 NULL → COALESCE 兜底），
    /// 而不是除零错误或沿用旧值。
    #[test]
    fn stats_avg_zero_when_all_limited() {
        let (store, db) = temp_store();
        store
            .append_access_log(&ApiAccessLog {
                id: 0,
                ts: now_ms(),
                method: "GET".into(),
                path: "/api/devices".into(),
                status: 429,
                latency_ms: 0,
                client_ip: "127.0.0.1".into(),
                limited: true,
            })
            .expect("append");
        let stats = store.stats().expect("stats");
        assert_eq!(stats.total_requests, 1);
        assert_eq!(stats.limited_requests, 1);
        assert_eq!(stats.avg_latency_ms, 0);

        let _ = std::fs::remove_file(&db);
    }
}
