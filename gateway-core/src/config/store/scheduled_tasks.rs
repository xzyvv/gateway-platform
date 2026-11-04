//! 计划任务存储（SQLite）
//!
//! 与 config_versions 共用 `default_version_db()` 库文件（WAL + busy_timeout=5000）。
//!
//! 调度语义：
//! - `cron` 为 5 段标准 cron 表达式（分 时 日 月 周），由 `support::cron` 解析
//!   计算下次执行时间；
//! - 执行状态（last_run_at / run_count / last_status / last_error）由
//!   gateway-api 的调度执行器在每次执行后回写；
//! - 任务的增删改立即重算 next_run_at，执行器只需轮询「next_run_at <= now」。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::engine::error::GatewayResult;

/// 任务类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskType {
    /// 读取单个点位
    #[serde(rename = "read_point")]
    ReadPoint,
    /// 读取设备全部点位
    #[serde(rename = "read_device")]
    ReadDevice,
    /// 备份当前配置（写一条配置版本快照）
    #[serde(rename = "backup_config")]
    BackupConfig,
    /// 立即同步 NTP 时间
    #[serde(rename = "sync_ntp")]
    SyncNtp,
}

impl TaskType {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskType::ReadPoint => "read_point",
            TaskType::ReadDevice => "read_device",
            TaskType::BackupConfig => "backup_config",
            TaskType::SyncNtp => "sync_ntp",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "read_point" => Some(TaskType::ReadPoint),
            "read_device" => Some(TaskType::ReadDevice),
            "backup_config" => Some(TaskType::BackupConfig),
            "sync_ntp" => Some(TaskType::SyncNtp),
            _ => None,
        }
    }
}

/// 执行结果
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskRunStatus {
    #[serde(rename = "success")]
    Success,
    #[serde(rename = "error")]
    Error,
}

impl TaskRunStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskRunStatus::Success => "success",
            TaskRunStatus::Error => "error",
        }
    }
}

/// 计划任务
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledTask {
    pub id: String,
    pub name: String,
    /// JSON 字段名为 `type`（与前端 ScheduleTask.type 对齐）
    #[serde(rename = "type")]
    pub task_type: TaskType,
    /// 目标 ID（read_point=点位 ID / read_device=设备 ID），其余类型为 None
    pub target: Option<String>,
    /// 5 段 cron 表达式
    pub cron: String,
    pub enabled: bool,
    /// 上次执行时间（毫秒）
    pub last_run_at: Option<i64>,
    /// 下次执行时间（毫秒）；enabled=false 时为 None
    pub next_run_at: Option<i64>,
    pub run_count: u64,
    pub last_status: Option<TaskRunStatus>,
    pub last_error: Option<String>,
    pub created_at: i64,
}

static ID_SEQ: AtomicU64 = AtomicU64::new(1);

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 计划任务存储
pub struct ScheduledTaskStore {
    db_path: PathBuf,
}

impl ScheduledTaskStore {
    pub fn new(db_path: &str) -> GatewayResult<Self> {
        let conn = open_conn(std::path::Path::new(db_path))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS scheduled_tasks (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                task_type TEXT NOT NULL,
                target TEXT,
                cron TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                last_run_at INTEGER,
                next_run_at INTEGER,
                run_count INTEGER NOT NULL DEFAULT 0,
                last_status TEXT,
                last_error TEXT,
                created_at INTEGER NOT NULL
            );",
        )
        .map_err(|e| db_err("create scheduled_tasks table", e))?;
        Ok(Self {
            db_path: PathBuf::from(db_path),
        })
    }

    fn conn(&self) -> GatewayResult<Connection> {
        open_conn(&self.db_path)
    }

    pub fn list(&self) -> GatewayResult<Vec<ScheduledTask>> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, task_type, target, cron, enabled, last_run_at, next_run_at,
                        run_count, last_status, last_error, created_at
                 FROM scheduled_tasks ORDER BY created_at ASC",
            )
            .map_err(|e| db_err("prepare list", e))?;
        let rows = stmt
            .query_map([], row_to_task)
            .map_err(|e| db_err("query list", e))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| db_err("row", e))?);
        }
        Ok(out)
    }

    pub fn get(&self, id: &str) -> GatewayResult<Option<ScheduledTask>> {
        Ok(self.list()?.into_iter().find(|t| t.id == id))
    }

    /// 创建任务。next_run_at 由调用方按 cron 先行计算（enabled=true 时必填）。
    pub fn create(&self, mut task: ScheduledTask) -> GatewayResult<ScheduledTask> {
        let now = now_ms();
        task.id = format!(
            "st_{:x}{:04x}",
            now,
            ID_SEQ.fetch_add(1, Ordering::Relaxed) & 0xffff
        );
        task.created_at = now;
        task.last_run_at = None;
        task.run_count = 0;
        task.last_status = None;
        task.last_error = None;
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO scheduled_tasks (id, name, task_type, target, cron, enabled, last_run_at, next_run_at, run_count, last_status, last_error, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, 0, NULL, NULL, ?8)",
            params![
                task.id,
                task.name,
                task.task_type.as_str(),
                task.target,
                task.cron,
                task.enabled as i64,
                task.next_run_at,
                task.created_at,
            ],
        )
        .map_err(|e| db_err("insert task", e))?;
        Ok(task)
    }

    /// 更新可变字段（名称/类型/目标/cron/启用态/下次执行时间）；返回 None = 不存在。
    /// 调用方负责先 get 合并字段并算好 next_run_at（enabled=false 时必须传 None）。
    /// 执行统计字段不归本函数管——由 `record_run` 回写。
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &self,
        id: &str,
        name: &str,
        task_type: TaskType,
        target: Option<&str>,
        cron: &str,
        enabled: bool,
        next_run_at: Option<i64>,
    ) -> GatewayResult<Option<ScheduledTask>> {
        if self.get(id)?.is_none() {
            return Ok(None);
        }
        let conn = self.conn()?;
        conn.execute(
            "UPDATE scheduled_tasks SET name=?2, task_type=?3, target=?4, cron=?5, enabled=?6, next_run_at=?7
             WHERE id=?1",
            params![id, name, task_type.as_str(), target, cron, enabled as i64, next_run_at],
        )
        .map_err(|e| db_err("update task", e))?;
        self.get(id)
    }

    pub fn delete(&self, id: &str) -> GatewayResult<bool> {
        let conn = self.conn()?;
        let n = conn
            .execute("DELETE FROM scheduled_tasks WHERE id=?1", params![id])
            .map_err(|e| db_err("delete task", e))?;
        Ok(n > 0)
    }

    /// 回写一次执行的结果，并按 cron 重算下次执行时间。
    pub fn record_run(
        &self,
        id: &str,
        status: TaskRunStatus,
        error: Option<&str>,
        next_run_at: Option<i64>,
    ) -> GatewayResult<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE scheduled_tasks SET last_run_at=?2, run_count=run_count+1, last_status=?3, last_error=?4, next_run_at=?5
             WHERE id=?1",
            params![id, now_ms(), status.as_str(), error, next_run_at],
        )
        .map_err(|e| db_err("record run", e))?;
        Ok(())
    }

    /// 仅重排下次执行时间（不改执行统计）。
    ///
    /// 供调度器在时钟被大幅校正/恢复可信后重新锚定：漏掉的窗口直接跳过，
    /// 避免一批任务在错误时刻集中补跑。
    pub fn set_next_run_at(&self, id: &str, next_run_at: Option<i64>) -> GatewayResult<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE scheduled_tasks SET next_run_at=?2 WHERE id=?1",
            params![id, next_run_at],
        )
        .map_err(|e| db_err("set next_run_at", e))?;
        Ok(())
    }

    /// 列出到期应执行的启用任务（next_run_at <= now_ms）
    pub fn due_tasks(&self, now_ms: i64) -> GatewayResult<Vec<ScheduledTask>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|t| t.enabled && t.next_run_at.map(|n| n <= now_ms).unwrap_or(false))
            .collect())
    }
}

fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<ScheduledTask> {
    let task_type_str: String = row.get(2)?;
    let last_status_str: Option<String> = row.get(9)?;
    Ok(ScheduledTask {
        id: row.get(0)?,
        name: row.get(1)?,
        task_type: TaskType::parse(&task_type_str).unwrap_or(TaskType::ReadPoint),
        target: row.get(3)?,
        cron: row.get(4)?,
        enabled: row.get::<_, i64>(5)? != 0,
        last_run_at: row.get(6)?,
        next_run_at: row.get(7)?,
        run_count: row.get::<_, i64>(8)? as u64,
        last_status: last_status_str.as_deref().and_then(|s| match s {
            "success" => Some(TaskRunStatus::Success),
            "error" => Some(TaskRunStatus::Error),
            _ => None,
        }),
        last_error: row.get(10)?,
        created_at: row.get(11)?,
    })
}

fn db_err(ctx: &str, e: rusqlite::Error) -> crate::engine::error::GatewayError {
    crate::engine::error::GatewayError::Queue(format!("{} failed: {}", ctx, e))
}

fn open_conn(path: &std::path::Path) -> GatewayResult<Connection> {
    let conn = Connection::open(path).map_err(|e| {
        crate::engine::error::GatewayError::Queue(format!("open scheduled task db failed: {}", e))
    })?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")
        .map_err(|e| db_err("set pragmas", e))?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_store() -> (ScheduledTaskStore, PathBuf) {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "sched_task_store_test_{}_{}",
            std::process::id(),
            n
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let db = dir.join("test.db");
        (
            ScheduledTaskStore::new(db.to_str().expect("utf8")).expect("new"),
            db,
        )
    }

    fn sample_task() -> ScheduledTask {
        ScheduledTask {
            id: String::new(),
            name: "每5分钟读温度".into(),
            task_type: TaskType::ReadPoint,
            target: Some("point_1".into()),
            cron: "*/5 * * * *".into(),
            enabled: true,
            last_run_at: None,
            next_run_at: Some(now_ms() + 60_000),
            run_count: 0,
            last_status: None,
            last_error: None,
            created_at: 0,
        }
    }

    #[test]
    fn task_crud_and_run_record() {
        let (store, db) = temp_store();
        let created = store.create(sample_task()).expect("create");
        assert!(created.id.starts_with("st_"));
        assert_eq!(created.run_count, 0);

        // due 判定
        assert!(store.due_tasks(now_ms()).expect("due").is_empty());
        assert_eq!(store.due_tasks(now_ms() + 120_000).expect("due").len(), 1);

        // 记录一次执行
        store
            .record_run(
                &created.id,
                TaskRunStatus::Success,
                None,
                Some(now_ms() + 300_000),
            )
            .expect("record");
        let t = store.get(&created.id).expect("get").expect("exists");
        assert_eq!(t.run_count, 1);
        assert_eq!(t.last_status, Some(TaskRunStatus::Success));
        assert!(t.last_run_at.is_some());

        // 更新：改名 + 停用（停用时 next_run_at 清空）
        let updated = store
            .update(
                &created.id,
                "新名字",
                TaskType::ReadPoint,
                Some("point_1"),
                "*/5 * * * *",
                false,
                None,
            )
            .expect("update")
            .expect("exists");
        assert_eq!(updated.name, "新名字");
        assert!(!updated.enabled);
        assert!(updated.next_run_at.is_none());

        // 停用后不再 due
        assert!(store
            .due_tasks(now_ms() + 999_999_999)
            .expect("due")
            .is_empty());

        assert!(store.delete(&created.id).expect("delete"));
        assert!(store.list().expect("list").is_empty());
        let _ = std::fs::remove_file(&db);
    }
}
