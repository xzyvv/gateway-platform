//! 数据质量规则存储（SQLite）
//!
//! 与 config_versions / rate_limit_rules 共用 `default_version_db()` 库文件，
//! 每次操作独立开连接（WAL + busy_timeout=5000，与 version.rs 同一约定）。
//!
//! 语义说明：
//! - `range` / `rate` / `nan` 规则由处理管道在数据点出口前真实执行
//!   （见 `gateway_core::pipeline::quality_rules`），命中时把质量码降级为
//!   `result_quality` 并写入 `quality_reason`。
//! - `stale` 规则**不在管道执行**：超时未更新由调度器的连续失败/超时机制
//!   （`Scheduler` 的 stale 计数与退避）负责，避免两套口径互相覆盖。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::engine::error::GatewayResult;

/// 质量规则
///
/// 字段与前端 `gateway-web/src/api/dataQuality.ts` 的 `QualityRule` 一一对应
/// （camelCase 序列化）：`condition` 取 `range|rate|stale|nan`，
/// `result_quality` 取 `good|bad|uncertain|old`（`old` 映射核心质量码 `Stale`）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QualityRule {
    pub id: String,
    pub name: String,
    /// 检查条件：`range` 取值范围 / `rate` 变化率 / `stale` 超时 / `nan` 非有限值
    pub condition: String,
    /// range：合法下限
    #[serde(default)]
    pub min: Option<f64>,
    /// range：合法上限
    #[serde(default)]
    pub max: Option<f64>,
    /// rate：相邻两次采样允许的最大绝对变化量
    #[serde(default)]
    pub max_rate: Option<f64>,
    /// stale：超时秒数（仅调度器使用，管道不执行）
    #[serde(default)]
    pub stale_seconds: Option<u32>,
    /// 命中后的质量码：`good|bad|uncertain|old`
    pub result_quality: String,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 规则 ID 序号：与毫秒时间戳组合，避免同毫秒并发创建撞号
static NEW_ID_SEQ: AtomicU64 = AtomicU64::new(1);

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn new_id() -> String {
    let seq = NEW_ID_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("qr_{:x}{:04x}", now_ms(), seq & 0xffff)
}

/// 数据质量规则存储
pub struct QualityRuleStore {
    db_path: PathBuf,
}

impl QualityRuleStore {
    pub fn new(db_path: &str) -> GatewayResult<Self> {
        let conn = open_conn(std::path::Path::new(db_path))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS quality_rules (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                condition TEXT NOT NULL,
                min REAL,
                max REAL,
                max_rate REAL,
                stale_seconds INTEGER,
                result_quality TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );",
        )
        .map_err(|e| {
            crate::engine::error::GatewayError::Queue(format!(
                "create quality rule table failed: {}",
                e
            ))
        })?;
        Ok(Self {
            db_path: PathBuf::from(db_path),
        })
    }

    fn conn(&self) -> GatewayResult<Connection> {
        open_conn(&self.db_path)
    }

    /// 列出全部规则（创建时间升序，先建的在前）
    pub fn list_rules(&self) -> GatewayResult<Vec<QualityRule>> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, condition, min, max, max_rate, stale_seconds,
                        result_quality, enabled, created_at, updated_at
                 FROM quality_rules ORDER BY created_at ASC",
            )
            .map_err(|e| db_err("prepare list quality rules", e))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(QualityRule {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    condition: row.get(2)?,
                    min: row.get(3)?,
                    max: row.get(4)?,
                    max_rate: row.get(5)?,
                    stale_seconds: row.get::<_, Option<i64>>(6)?.map(|v| v as u32),
                    result_quality: row.get(7)?,
                    enabled: row.get::<_, i64>(8)? != 0,
                    created_at: row.get(9)?,
                    updated_at: row.get(10)?,
                })
            })
            .map_err(|e| db_err("query quality rules", e))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| db_err("row", e))?);
        }
        Ok(out)
    }

    /// 仅列出启用中的规则（处理管道热路径快照用）
    pub fn list_enabled_rules(&self) -> GatewayResult<Vec<QualityRule>> {
        Ok(self
            .list_rules()?
            .into_iter()
            .filter(|r| r.enabled)
            .collect())
    }

    pub fn get_rule(&self, id: &str) -> GatewayResult<Option<QualityRule>> {
        Ok(self.list_rules()?.into_iter().find(|r| r.id == id))
    }

    pub fn create_rule(&self, mut rule: QualityRule) -> GatewayResult<QualityRule> {
        let now = now_ms();
        rule.id = new_id();
        rule.created_at = now;
        rule.updated_at = now;
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO quality_rules
                (id, name, condition, min, max, max_rate, stale_seconds,
                 result_quality, enabled, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                rule.id,
                rule.name,
                rule.condition,
                rule.min,
                rule.max,
                rule.max_rate,
                rule.stale_seconds.map(|v| v as i64),
                rule.result_quality,
                rule.enabled as i64,
                rule.created_at,
                rule.updated_at,
            ],
        )
        .map_err(|e| db_err("insert quality rule", e))?;
        Ok(rule)
    }

    /// 更新规则；返回 None 表示规则不存在
    pub fn update_rule(&self, id: &str, rule: &QualityRule) -> GatewayResult<Option<QualityRule>> {
        let conn = self.conn()?;
        let n = conn
            .execute(
                "UPDATE quality_rules SET name=?2, condition=?3, min=?4, max=?5,
                 max_rate=?6, stale_seconds=?7, result_quality=?8, enabled=?9,
                 updated_at=?10 WHERE id=?1",
                params![
                    id,
                    rule.name,
                    rule.condition,
                    rule.min,
                    rule.max,
                    rule.max_rate,
                    rule.stale_seconds.map(|v| v as i64),
                    rule.result_quality,
                    rule.enabled as i64,
                    now_ms(),
                ],
            )
            .map_err(|e| db_err("update quality rule", e))?;
        if n == 0 {
            return Ok(None);
        }
        self.get_rule(id)
    }

    /// 删除规则；返回是否删除了行
    pub fn delete_rule(&self, id: &str) -> GatewayResult<bool> {
        let conn = self.conn()?;
        let n = conn
            .execute("DELETE FROM quality_rules WHERE id=?1", params![id])
            .map_err(|e| db_err("delete quality rule", e))?;
        Ok(n > 0)
    }
}

fn db_err(ctx: &str, e: rusqlite::Error) -> crate::engine::error::GatewayError {
    crate::engine::error::GatewayError::Queue(format!("{} failed: {}", ctx, e))
}

/// 打开连接并设置 busy_timeout（per-connection，每次 open 都必须重设）
fn open_conn(path: &std::path::Path) -> GatewayResult<Connection> {
    let conn = Connection::open(path).map_err(|e| {
        crate::engine::error::GatewayError::Queue(format!("open quality rule db failed: {}", e))
    })?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")
        .map_err(|e| db_err("set pragmas", e))?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_store() -> (QualityRuleStore, PathBuf) {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "quality_rule_store_test_{}_{}",
            std::process::id(),
            n
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let db = dir.join("test.db");
        (
            QualityRuleStore::new(db.to_str().expect("utf8")).expect("new"),
            db,
        )
    }

    fn sample_rule() -> QualityRule {
        QualityRule {
            id: String::new(),
            name: "超量程标记 Bad".into(),
            condition: "range".into(),
            min: Some(-100.0),
            max: Some(9999.0),
            max_rate: None,
            stale_seconds: None,
            result_quality: "bad".into(),
            enabled: true,
            created_at: 0,
            updated_at: 0,
        }
    }

    /// CRUD 往返：创建 → 列出 → 更新 → 删除，字段（含可选数值）完整保留。
    #[test]
    fn rule_crud_roundtrip() {
        let (store, db) = temp_store();
        let created = store.create_rule(sample_rule()).expect("create");
        assert!(created.id.starts_with("qr_"), "id 应为 qr_ 前缀");
        assert!(created.created_at > 0);

        let listed = store.list_rules().expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].min, Some(-100.0));
        assert_eq!(listed[0].max, Some(9999.0));
        assert_eq!(
            store.get_rule(&created.id).expect("get").unwrap().name,
            "超量程标记 Bad"
        );

        let mut patch = created.clone();
        patch.name = "改名为变化率".into();
        patch.condition = "rate".into();
        patch.min = None;
        patch.max = None;
        patch.max_rate = Some(100.0);
        patch.result_quality = "uncertain".into();
        patch.enabled = false;
        let updated = store
            .update_rule(&created.id, &patch)
            .expect("update")
            .expect("exists");
        assert_eq!(updated.condition, "rate");
        assert_eq!(updated.max_rate, Some(100.0));
        assert!(updated.min.is_none());
        assert!(!updated.enabled);
        assert!(updated.updated_at >= updated.created_at);

        assert!(store.delete_rule(&created.id).expect("delete"));
        assert!(store.list_rules().expect("list").is_empty());
        assert!(!store.delete_rule("qr_nonexistent").expect("delete none"));

        let _ = std::fs::remove_file(&db);
    }

    /// enabled 过滤：仅启用规则的快照入口不返回停用规则。
    #[test]
    fn enabled_filter() {
        let (store, db) = temp_store();
        let a = store.create_rule(sample_rule()).expect("create a");
        let mut b = sample_rule();
        b.name = "停用规则".into();
        b.enabled = false;
        let b = store.create_rule(b).expect("create b");

        let enabled = store.list_enabled_rules().expect("enabled");
        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0].id, a.id);
        assert!(enabled.iter().all(|r| r.id != b.id));

        let _ = std::fs::remove_file(&db);
    }
}
