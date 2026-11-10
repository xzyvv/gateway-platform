//! 队列库的 schema 版本与迁移链。
//!
//! 从 `db_init.rs` 抽出：建表/索引/自愈流程与「版本推进」是两件事，
//! 分开后各自的改动面更小，迁移链也更容易单独审阅。

use rusqlite::Connection;

use crate::engine::error::{GatewayError, GatewayResult};

/// 当前队列库 schema 版本，存于 `PRAGMA user_version`。
///
/// 版本历史（每次表结构变更都必须 +1 并补一条迁移分支）：
/// - v1：最初 7 列（point_id / value / raw_value / raw_type / quality / timestamp / id），
///   以及所有「引入版本号之前」的老库（它们的 `user_version` 为 0）。
/// - v2：补齐 7 个业务列（device_id / quality_reason / source_ts / stale_count /
///   in_flight / dequeued_at / delivered）。
/// - v3：value_text 列 + 降采样聚合表 `data_queue_aggregated` 及其索引。
/// - v4：聚合表补 `window_start` 单列索引（支撑行数上界的按序淘汰）（当前）。
pub(super) const SCHEMA_VERSION: i32 = 4;

fn read_schema_version(conn: &Connection) -> GatewayResult<i32> {
    conn.pragma_query_value(None, "user_version", |row| row.get::<_, i32>(0))
        .map_err(|e| GatewayError::Queue(format!("read schema user_version failed: {}", e)))
}

fn write_schema_version(conn: &Connection, version: i32) -> GatewayResult<()> {
    conn.pragma_update(None, "user_version", version)
        .map_err(|e| GatewayError::Queue(format!("write schema user_version failed: {}", e)))
}

/// 把库迁移到 [`SCHEMA_VERSION`]。
///
/// - `user_version` 为 0 的库来自「引入版本号之前」的老版本，按 v1 处理；
/// - 每一步成功后立刻落盘版本号，中途崩溃时下次启动从已完成的那步继续，
///   不会把已做过的 ALTER 重新判定为异常状态；
/// - `user_version` 高于本程序支持的版本时**拒绝启动**：那是被更新版本写过的库，
///   用旧程序打开会因结构不识别而静默丢数据（回滚部署的典型事故）。
pub(super) fn migrate_schema(conn: &Connection) -> GatewayResult<()> {
    let from = read_schema_version(conn)?;
    if from > SCHEMA_VERSION {
        return Err(GatewayError::Queue(format!(
            "数据库 schema 版本 {} 高于本程序支持的 {}：该库由更新版本写入，\
             用旧程序打开会因结构不识别而丢数据，已拒绝启动。请升级程序后再运行",
            from, SCHEMA_VERSION
        )));
    }
    if from == SCHEMA_VERSION {
        return Ok(());
    }
    tracing::info!("迁移队列库 schema: v{} -> v{}", from, SCHEMA_VERSION);

    if from < 2 {
        migrate_to_v2(conn)?;
        write_schema_version(conn, 2)?;
    }
    if from < 3 {
        migrate_to_v3(conn)?;
        write_schema_version(conn, 3)?;
    }
    if from < 4 {
        migrate_to_v4(conn)?;
        write_schema_version(conn, 4)?;
    }
    Ok(())
}

/// v1 → v2：补齐 7 个业务列。
///
/// 列已存在视为已迁移（幂等），其余错误必须中止启动 —— 磁盘满 / 锁库导致的
/// ALTER 失败若被吞掉，后续查询会报 "no such column"，错误现场离根因很远（E-1）。
fn migrate_to_v2(conn: &Connection) -> GatewayResult<()> {
    ensure_column(
        conn,
        "ALTER TABLE data_queue ADD COLUMN device_id TEXT NOT NULL DEFAULT ''",
        "device_id",
    )?;
    ensure_column(
        conn,
        "ALTER TABLE data_queue ADD COLUMN quality_reason TEXT NOT NULL DEFAULT ''",
        "quality_reason",
    )?;
    ensure_column(
        conn,
        "ALTER TABLE data_queue ADD COLUMN source_ts INTEGER NOT NULL DEFAULT 0",
        "source_ts",
    )?;
    ensure_column(
        conn,
        "ALTER TABLE data_queue ADD COLUMN stale_count INTEGER NOT NULL DEFAULT 0",
        "stale_count",
    )?;
    ensure_column(
        conn,
        "ALTER TABLE data_queue ADD COLUMN in_flight INTEGER NOT NULL DEFAULT 0",
        "in_flight",
    )?;
    // Bug 163: 新增 dequeued_at 列——记录每行"出队时刻"，使 stale in_flight 超时
    // 以出队时间为基准判定，而非采集时间戳（详见 sqlite_storage 的清理 SQL）。
    ensure_column(
        conn,
        "ALTER TABLE data_queue ADD COLUMN dequeued_at INTEGER NOT NULL DEFAULT 0",
        "dequeued_at",
    )?;
    // P0-3: 新增 delivered 列（0=未送达，1=已 ack 等待 retention 删除）。
    // 旧表无此列，回填为 0（语义"未送达"），与恢复时统一清 in_flight 的处理一致：
    // 重启后未 ack 的行一律按"未送达"对待，由 outbox 持久层或 replay 路径重新投递。
    ensure_column(
        conn,
        "ALTER TABLE data_queue ADD COLUMN delivered INTEGER NOT NULL DEFAULT 0",
        "delivered",
    )?;
    Ok(())
}

/// v2 → v3：value_text 列 + 降采样聚合表及其索引。
fn migrate_to_v3(conn: &Connection) -> GatewayResult<()> {
    // 批次 C：新增 value_text 列（枚举转换的文本标签，如 "运行"/"停机"）。
    // 可空、无 DEFAULT——旧行迁移后为 NULL，语义为「该行写入时尚未支持标签」；
    // 查询期不做规则回查，如实返回 NULL。
    ensure_column(
        conn,
        "ALTER TABLE data_queue ADD COLUMN value_text TEXT",
        "value_text",
    )?;

    // F5: 降采样聚合表（按分钟存储 avg/min/max，用于长期历史查询）
    conn.execute(
        "CREATE TABLE IF NOT EXISTS data_queue_aggregated (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                point_id TEXT NOT NULL,
                window_start INTEGER NOT NULL,
                avg_value REAL NOT NULL,
                min_value REAL NOT NULL,
                max_value REAL NOT NULL,
                sample_count INTEGER NOT NULL,
                UNIQUE(point_id, window_start)
            )",
        [],
    )
    .map_err(|e| GatewayError::Queue(format!("create aggregated table failed: {}", e)))?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_agg_point_ts ON data_queue_aggregated(point_id, window_start)",
        [],
    )
    .map_err(|e| GatewayError::Queue(format!("create aggregated index failed: {}", e)))?;
    Ok(())
}

/// v3 → v4：聚合表补 `window_start` 单列索引。
///
/// 行数上界淘汰的删除条件是「按 `window_start` 从旧到新取 N 行」，
/// 只有 (point_id, window_start) 复合索引无法支撑全局排序，会退化为全表排序。
fn migrate_to_v4(conn: &Connection) -> GatewayResult<()> {
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_agg_window ON data_queue_aggregated(window_start)",
        [],
    )
    .map_err(|e| GatewayError::Queue(format!("create aggregated window index failed: {}", e)))?;
    Ok(())
}

/// 幂等执行 `ALTER TABLE ... ADD COLUMN`。
///
/// E-1：SQLite 没有 `ADD COLUMN IF NOT EXISTS`，旧实现用 `let _ =` 吞掉所有错误。
/// 只有「列已存在」（duplicate column name，正常升级/重复启动）可以继续；
/// 磁盘满、锁库、语法错误等一律返回 Err 中止启动，避免表结构与代码版本不一致后
/// 才在业务查询处报 `no such column`。
fn ensure_column(conn: &Connection, ddl: &str, column: &str) -> GatewayResult<()> {
    match conn.execute(ddl, []) {
        Ok(_) => {
            tracing::info!("migrated data_queue: added column `{}`", column);
            Ok(())
        }
        Err(e) if e.to_string().contains("duplicate column name") => {
            tracing::debug!("data_queue column `{}` already exists (idempotent)", column);
            Ok(())
        }
        Err(e) => Err(GatewayError::Queue(format!(
            "migrate data_queue column `{}` failed: {}",
            column, e
        ))),
    }
}
