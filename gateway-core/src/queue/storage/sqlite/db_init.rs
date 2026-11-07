//! 队列数据库初始化（建表、索引、备份、完整性检查）

use crate::engine::metrics::Metrics;
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use rusqlite::Connection;

use crate::engine::error::{GatewayError, GatewayResult};

use super::wal::backup_db_consistent;

/// 初始化队列数据库：打开/恢复连接、设置 WAL、建表/索引、校准计数器
///
/// 包含：
/// - SQLite 连接打开 + integrity_check + 备份恢复
/// - WAL 模式设置
/// - data_queue 表 + schema 版本校验与迁移链（`PRAGMA user_version`）
/// - in_flight 恢复 + 队列深度计数器校准
/// - 索引创建 + 降采样聚合表
pub(super) fn init_queue_db(
    db_path: &Path,
    backup_path: &Path,
    depth: &AtomicI64,
    key_hex: &str,
    metrics: &Arc<Metrics>,
) -> GatewayResult<Connection> {
    let mut conn = match open_with_key(db_path, key_hex) {
        Ok(conn) => {
            let integrity_ok: bool = conn
                .query_row("PRAGMA integrity_check", [], |row| {
                    let result: String = row.get(0)?;
                    Ok(result == "ok")
                })
                .unwrap_or(false);
            if integrity_ok {
                conn
            } else {
                tracing::error!("SQLite integrity check failed, attempting restore from backup...");
                drop(conn);
                if backup_path.exists() {
                    std::fs::copy(backup_path, db_path).map_err(|e| {
                        GatewayError::Queue(format!("restore from backup failed: {}", e))
                    })?;
                    tracing::warn!("restored data_queue.db from backup");
                    open_with_key(db_path, key_hex).map_err(|e| {
                        GatewayError::Queue(format!("reopen after restore failed: {}", e))
                    })?
                } else {
                    tracing::error!("no backup available, recreating empty database");
                    let _ = std::fs::remove_file(db_path);
                    open_with_key(db_path, key_hex)
                        .map_err(|e| GatewayError::Queue(format!("recreate db failed: {}", e)))?
                }
            }
        }
        Err(e) => {
            tracing::error!(
                "open sqlite failed ({}), attempting restore from backup...",
                e
            );
            if backup_path.exists() {
                std::fs::copy(backup_path, db_path).map_err(|e2| {
                    GatewayError::Queue(format!("restore from backup failed: {}", e2))
                })?;
                tracing::warn!("restored data_queue.db from backup after open failure");
                open_with_key(db_path, key_hex).map_err(|e2| {
                    GatewayError::Queue(format!("reopen after restore failed: {}", e2))
                })?
            } else {
                return Err(GatewayError::Queue(format!(
                    "open sqlite failed and no backup: {}",
                    e
                )));
            }
        }
    };

    // r42-Q3: 启动备份必须先 checkpoint（TRUNCATE）把 -wal 中已 commit 但未落主库
    // 的行刷回主库，再复制。旧实现直接 fs::copy 只复制主库文件：上一生命周期若异常
    // 退出、-wal 残留已提交数据，备份会漏掉这些行并覆盖掉上一轮维护备份 → 之后从
    // 备份恢复将丢失最近一次运行的数据。与维护路径（maintenance 第 5 步）保持同序。
    let _ = backup_db_consistent(&mut conn, db_path, backup_path);

    // WAL 调优：综合设置 journal_mode + synchronous + busy_timeout + cache_size + wal_autocheckpoint + mmap_size
    // - journal_mode=WAL: 写入不阻塞读，崩溃恢复由 WAL 保证
    // - synchronous=FULL: 计费场景必须保证已 commit 数据不丢——NORMAL 下 WAL 不 fsync，
    //   嵌入式设备掉电会丢最近未 checkpoint 的计费行；FULL 在每事务 commit 时 fsync 写盘，
    //   性能开销对秒级采集网关可忽略，符合产品「稳定性第一」原则。
    // - busy_timeout=5000ms: 多连接争用时等待 5s 而非立即返回 SQLITE_BUSY（避免读池与主连接冲突）
    // - cache_size=-65536: 64MB 页缓存（负值=KB 单位），减少磁盘 I/O
    // - wal_autocheckpoint=1000: 每 1000 页（约 4MB）自动 checkpoint，防止 WAL 增长过大
    // - mmap_size=268435456: 256MB 内存映射，提升读取性能（仅在 64 位系统生效）
    conn.execute_batch(
        "PRAGMA journal_mode=WAL; \
         PRAGMA synchronous=FULL; \
         PRAGMA busy_timeout=5000; \
         PRAGMA cache_size=-65536; \
         PRAGMA wal_autocheckpoint=1000; \
         PRAGMA mmap_size=268435456;",
    )
    .map_err(|e| GatewayError::Queue(format!("set WAL mode failed: {}", e)))?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS data_queue (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                point_id TEXT NOT NULL,
                device_id TEXT NOT NULL DEFAULT '',
                value REAL NOT NULL,
                raw_value REAL NOT NULL,
                raw_type TEXT NOT NULL,
                quality TEXT NOT NULL,
                quality_reason TEXT NOT NULL DEFAULT '',
                timestamp INTEGER NOT NULL,
                source_ts INTEGER NOT NULL DEFAULT 0,
                stale_count INTEGER NOT NULL DEFAULT 0,
                in_flight INTEGER NOT NULL DEFAULT 0,
                dequeued_at INTEGER NOT NULL DEFAULT 0,
                delivered INTEGER NOT NULL DEFAULT 0,
                value_text TEXT
            )",
        [],
    )
    .map_err(|e| GatewayError::Queue(format!("create table failed: {}", e)))?;

    // schema 版本校验 + 迁移链。
    //
    // 旧实现只是在这里平铺一串 `ensure_column`：每次升级都要靠「逐列探测」判断
    // 走到哪一步，既无法表达「改列 / 迁数据」这类迁移，也无法证明升级链完整，
    // 更挡不住用旧程序打开新库（回滚部署）这种会静默丢数据的操作。
    //
    // 现在统一由 `PRAGMA user_version` 记录版本，迁移链逐步推进并在每一步成功后
    // 落盘版本号（中途崩溃时下次启动从已完成的那步继续）。
    migrate_schema(&conn)?;

    // Bug r08-DB2 (fix): 加 WHERE delivered = 0 限定只重置"未送达"的行。
    // 旧实现更新全表（包括 delivered=1 等 ack 等待 retention 删除的行），
    // 功能无影响（delivered=1 行不出队）但语义混乱，干扰后续读代码者。
    conn.execute(
        "UPDATE data_queue SET in_flight = 0 WHERE delivered = 0",
        [],
    )
    .map_err(|e| GatewayError::Queue(format!("recover in_flight failed: {}", e)))?;

    // P3: 启动时校准内存队列深度计数器（一次性 COUNT，之后纯内存维护）。
    //
    // 必须只统计 `delivered = 0` 的行：运行时 depth 的增减就是
    // 「enqueue_batch 加 inserted / ack_batch 减 ids_len」，语义恒等于
    // 「尚未送达（含 in_flight=1）的行数」。P0-3 之后 ack 不再删行，而是置
    // delivered=1 并保留到 retention 回收；若此处按 COUNT(*) 记账，重启后 depth
    // 会把已 ack 的行重新算进来 → 长期虚高且不可自愈（retention 删行不扣 depth）：
    // 指标失真、drain_and_wait 可能永不返回、enforce_queue_limit 误删仍在
    // 保留期内的历史行。
    let initial_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM data_queue WHERE delivered = 0",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    depth.store(initial_count, Ordering::Relaxed);
    metrics.set_queue_depth(initial_count);

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_queue_ts ON data_queue(timestamp)",
        [],
    )
    .map_err(|e| GatewayError::Queue(format!("create index failed: {}", e)))?;

    // 复合索引：覆盖 history 查询 (point_id 等值 + timestamp 范围 + DESC 排序)
    // 原 idx_queue_ts(timestamp) 仅支持时间范围扫描，多点位场景退化为近全表扫描
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_queue_point_ts ON data_queue(point_id, timestamp DESC)",
        [],
    )
    .map_err(|e| GatewayError::Queue(format!("create point_ts index failed: {}", e)))?;

    // 降采样聚合表（F5）与它的索引由 v2 → v3 迁移步骤创建，不在这里重复建。

    Ok(conn)
}

// ══════════════════════════════════════════════════════════
// schema 版本与迁移链
// ══════════════════════════════════════════════════════════

/// 当前队列库 schema 版本，存于 `PRAGMA user_version`。
///
/// 版本历史（每次表结构变更都必须 +1 并补一条迁移分支）：
/// - v1：最初 7 列（point_id / value / raw_value / raw_type / quality / timestamp / id），
///   以及所有「引入版本号之前」的老库（它们的 `user_version` 为 0）。
/// - v2：补齐 7 个业务列（device_id / quality_reason / source_ts / stale_count /
///   in_flight / dequeued_at / delivered）。
/// - v3：value_text 列 + 降采样聚合表 `data_queue_aggregated` 及其索引（当前）。
pub(super) const SCHEMA_VERSION: i32 = 3;

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
fn migrate_schema(conn: &Connection) -> GatewayResult<()> {
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

/// 打开 SQLite 连接并设置 SQLCipher 加密密钥
///
/// 必须在任何其他 PRAGMA 或查询之前执行 `PRAGMA key`。
fn open_with_key(db_path: &Path, key_hex: &str) -> Result<Connection, rusqlite::Error> {
    let conn = Connection::open(db_path)?;
    if !key_hex.is_empty() {
        conn.pragma_update(None, "key", key_hex)?;
    }
    Ok(conn)
}

#[cfg(test)]
mod migration_tests {
    use super::*;
    use crate::queue::storage::QueueStorage;

    /// 批次 C：迁移前（旧 schema）的 `data_queue` 列定义。
    /// 刻意不含 `value_text`——用于模拟升级前已落库的历史库。
    const OLD_SCHEMA: &str = "CREATE TABLE data_queue (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        point_id TEXT NOT NULL,
        device_id TEXT NOT NULL DEFAULT '',
        value REAL NOT NULL,
        raw_value REAL NOT NULL,
        raw_type TEXT NOT NULL,
        quality TEXT NOT NULL,
        quality_reason TEXT NOT NULL DEFAULT '',
        timestamp INTEGER NOT NULL,
        source_ts INTEGER NOT NULL DEFAULT 0,
        stale_count INTEGER NOT NULL DEFAULT 0,
        in_flight INTEGER NOT NULL DEFAULT 0,
        dequeued_at INTEGER NOT NULL DEFAULT 0,
        delivered INTEGER NOT NULL DEFAULT 0
    )";

    /// 批次 C 迁移测试：旧 schema 库被 `SqliteStorage::new` 打开后
    /// 1) `value_text` 列存在（ALTER TABLE ADD COLUMN 生效）；
    /// 2) 旧行读出 `value_text = None`（NULL 表示「写入时尚未支持标签」）。
    ///
    /// 用 `tempfile::tempdir` 把库放在系统临时目录，避免污染仓库。
    #[test]
    fn value_text_column_added_to_legacy_schema() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("queue.db");

        // 1) 手工建「旧 schema」库并写入一行历史数据
        {
            let conn = Connection::open(&db_path).expect("open legacy db");
            conn.execute_batch(OLD_SCHEMA).expect("create legacy table");
            conn.execute(
                "INSERT INTO data_queue (point_id, device_id, value, raw_value, raw_type, quality, timestamp)
                 VALUES ('p-old', 'dev1', 1.0, 1.0, 'u16', 'good', 1700000000000)",
                [],
            )
            .expect("insert legacy row");
        }

        // 2) 用生产入口打开（触发 init_queue_db → 幂等 ALTER 迁移）
        let storage = crate::queue::storage::SqliteStorage::new(
            db_path.to_str().expect("db path utf8"),
            Arc::new(Metrics::new()),
        )
        .expect("open storage on legacy db");

        // 3) 断言列存在
        {
            let conn = Connection::open(&db_path).expect("open migrated db");
            let mut stmt = conn
                .prepare("PRAGMA table_info(data_queue)")
                .expect("prepare table_info");
            let mut rows = stmt.query([]).expect("query table_info");
            let mut columns: Vec<String> = Vec::new();
            while let Some(row) = rows.next().expect("next row") {
                columns.push(row.get::<_, String>(1).expect("column name"));
            }
            columns.sort();
            assert!(
                columns.iter().any(|c| c == "value_text"),
                "迁移后 data_queue 必须存在 value_text 列，实际列: {:?}",
                columns
            );
        }

        // 4) 旧行读出 value_text = None（不因迁移被回填成空串或默认值）
        let items = storage
            .history_query("p-old", 0, i64::MAX, 10)
            .expect("history query legacy row");
        assert_eq!(items.len(), 1, "旧行应仍在库中");
        assert_eq!(
            items[0].value_text, None,
            "迁移前的行没有标签，应读出 None（NULL）"
        );
    }

    /// 批次 C 迁移幂等性：对同一库连续两次打开（第二次 ALTER 必然报「列已存在」），
    /// 不应报错，且列定义保持唯一。
    #[test]
    fn value_text_migration_is_idempotent() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("queue.db");

        for _ in 0..2 {
            let storage = crate::queue::storage::SqliteStorage::new(
                db_path.to_str().expect("db path utf8"),
                Arc::new(Metrics::new()),
            )
            .expect("open storage twice");
            let items = storage
                .history_query("p-none", 0, i64::MAX, 10)
                .expect("history query");
            assert!(items.is_empty());
        }

        let conn = Connection::open(&db_path).expect("open db");
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('data_queue') WHERE name = 'value_text'",
                [],
                |r| r.get(0),
            )
            .expect("count value_text columns");
        assert_eq!(count, 1, "value_text 列应唯一");
    }

    /// 现场最老版本的 schema：只有最初 7 列。
    ///
    /// 既有 `OLD_SCHEMA` 只缺 `value_text`，等于只验证了「单列迁移」；真实升级
    /// 路径上 PDF 里记录的 device_id / quality_reason / source_ts / stale_count /
    /// in_flight / dequeued_at / delivered 会同时缺失，需要 7 次 ALTER 全部成功。
    const ANCIENT_SCHEMA: &str = "CREATE TABLE data_queue (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        point_id TEXT NOT NULL,
        value REAL NOT NULL,
        raw_value REAL NOT NULL,
        raw_type TEXT NOT NULL,
        quality TEXT NOT NULL,
        timestamp INTEGER NOT NULL
    )";

    const REQUIRED_COLUMNS: [&str; 8] = [
        "device_id",
        "quality_reason",
        "source_ts",
        "stale_count",
        "in_flight",
        "dequeued_at",
        "delivered",
        "value_text",
    ];

    fn columns_of(db_path: &Path) -> Vec<String> {
        let conn = Connection::open(db_path).expect("open db");
        let mut stmt = conn
            .prepare("PRAGMA table_info(data_queue)")
            .expect("prepare table_info");
        let mut rows = stmt.query([]).expect("query table_info");
        let mut columns = Vec::new();
        while let Some(row) = rows.next().expect("next row") {
            columns.push(row.get::<_, String>(1).expect("column name"));
        }
        columns
    }

    fn seed_ancient_db(db_path: &Path, rows: usize) {
        let conn = Connection::open(db_path).expect("open ancient db");
        conn.execute_batch(ANCIENT_SCHEMA)
            .expect("create ancient table");
        for i in 0..rows {
            conn.execute(
                "INSERT INTO data_queue (point_id, value, raw_value, raw_type, quality, timestamp)
                 VALUES (?1, ?2, ?2, 'u16', 'good', ?3)",
                rusqlite::params!["p-old", i as f64, 1_700_000_000_000_i64 + i as i64],
            )
            .expect("insert ancient row");
        }
    }

    /// 老库升级：8 个新增列必须全部补齐，历史行不丢，深度按 delivered=0 重算。
    ///
    /// 这是「升级演练」的代码级替身：若某次 ALTER 失败被吞掉，后续查询会在离
    /// 根因很远的地方报 `no such column`；此处一次性钉住表结构结果。
    #[test]
    fn ancient_schema_upgrades_all_columns_and_preserves_rows() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("queue.db");
        seed_ancient_db(&db_path, 3);

        let storage = crate::queue::storage::SqliteStorage::new(
            db_path.to_str().expect("db path utf8"),
            Arc::new(Metrics::new()),
        )
        .expect("open storage on ancient db");

        let columns = columns_of(&db_path);
        for required in REQUIRED_COLUMNS {
            assert!(
                columns.iter().any(|c| c == required),
                "迁移后缺少列 `{}`，实际列: {:?}",
                required,
                columns
            );
        }

        assert_eq!(storage.len(), 3, "历史行必须全部保留且计入深度");
        let items = storage
            .history_query("p-old", 0, i64::MAX, 10)
            .expect("history query ancient rows");
        assert_eq!(items.len(), 3, "历史行应可按 point_id 查回");
    }

    /// 升级幂等：同一老库连续打开两次，表格列不重复、数据不丢、深度不漂移。
    ///
    /// 第二次打开时 7 次 ALTER 必然全部返回「duplicate column name」，正是
    /// `ensure_column` 的幂等分支；若该分支被写成吞掉所有错误，也测不出来，
    /// 但至少能保证「重复升级不会破坏数据」。
    #[test]
    fn repeated_upgrade_is_idempotent_and_keeps_depth() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("queue.db");
        seed_ancient_db(&db_path, 2);

        for round in 1..=2 {
            let storage = crate::queue::storage::SqliteStorage::new(
                db_path.to_str().expect("db path utf8"),
                Arc::new(Metrics::new()),
            )
            .unwrap_or_else(|e| panic!("第 {} 次打开老库失败: {}", round, e));
            assert_eq!(storage.len(), 2, "第 {} 次打开后历史行应保留", round);
            drop(storage);
        }

        let columns = columns_of(&db_path);
        for required in REQUIRED_COLUMNS {
            let n = columns.iter().filter(|c| c.as_str() == required).count();
            assert_eq!(n, 1, "列 `{}` 应唯一，实际出现 {} 次", required, n);
        }
    }

    // ── P2-2：user_version 版本号 + 跨版本迁移回归 ──

    fn user_version_of(db_path: &Path) -> i32 {
        let conn = Connection::open(db_path).expect("open db");
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
            .expect("read user_version")
    }

    fn stamp_user_version(db_path: &Path, version: i32) {
        let conn = Connection::open(db_path).expect("open db to stamp version");
        conn.pragma_update(None, "user_version", version)
            .expect("stamp user_version");
    }

    /// 全新库：首次打开后必须被盖章为当前版本，而不是停在 0。
    ///
    /// 版本号停在 0 意味着「这个库从来没被版本化过」，下次升级会把它当成
    /// v1 老库再跑一遍完整迁移链 —— 迁移链若含不可重入步骤（改列/迁数据），
    /// 就会在全新库上重复执行而出错。
    #[test]
    fn fresh_db_is_stamped_with_current_schema_version() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("queue.db");

        let _storage = crate::queue::storage::SqliteStorage::new(
            db_path.to_str().expect("db path utf8"),
            Arc::new(Metrics::new()),
        )
        .expect("open fresh db");

        assert_eq!(
            user_version_of(&db_path),
            SCHEMA_VERSION,
            "全新库必须盖章为当前 schema 版本"
        );
    }

    /// 跨版本回归：v1（最初 7 列）老库快照升到最新。
    ///
    /// 这是「现场跑着两年前版本，升级程序后重启」的代码级替身：
    /// 历史行一条不能少、8 个新列必须补齐、版本号必须推到最新，
    /// 且聚合表（v3 才引入）必须建出来。
    #[test]
    fn legacy_v1_snapshot_migrates_to_current_version() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("queue.db");
        seed_ancient_db(&db_path, 3);
        stamp_user_version(&db_path, 1);

        let storage = crate::queue::storage::SqliteStorage::new(
            db_path.to_str().expect("db path utf8"),
            Arc::new(Metrics::new()),
        )
        .expect("v1 老库必须能升到最新");

        let columns = columns_of(&db_path);
        for required in REQUIRED_COLUMNS {
            assert!(
                columns.iter().any(|c| c == required),
                "v1 → v{} 后缺少列 `{}`，实际列: {:?}",
                SCHEMA_VERSION,
                required,
                columns
            );
        }
        assert_eq!(storage.len(), 3, "历史行必须全部保留");
        assert_eq!(
            user_version_of(&db_path),
            SCHEMA_VERSION,
            "v1 老库升级后版本号必须推进到最新"
        );

        let conn = Connection::open(&db_path).expect("open migrated db");
        let agg_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='data_queue_aggregated'",
                [],
                |r| r.get(0),
            )
            .expect("check aggregated table");
        assert_eq!(agg_exists, 1, "v3 迁移必须建出聚合表");
    }

    /// 跨版本回归：v2（缺 value_text、无聚合表）升到最新。
    ///
    /// 覆盖「上一版刚升完 v2、程序又发了一版」的场景。v1 → v2 那一步不应被
    /// 重复执行（版本号已到 2），只需补 v3。
    #[test]
    fn legacy_v2_snapshot_migrates_to_current_version() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("queue.db");

        {
            let conn = Connection::open(&db_path).expect("open legacy db");
            conn.execute_batch(OLD_SCHEMA).expect("create v2 table");
            conn.execute(
                "INSERT INTO data_queue (point_id, device_id, value, raw_value, raw_type, quality, timestamp)
                 VALUES ('p-v2', 'dev1', 2.0, 2.0, 'u16', 'good', 1700000000000)",
                [],
            )
            .expect("insert v2 row");
        }
        stamp_user_version(&db_path, 2);

        let storage = crate::queue::storage::SqliteStorage::new(
            db_path.to_str().expect("db path utf8"),
            Arc::new(Metrics::new()),
        )
        .expect("v2 老库必须能升到最新");

        let columns = columns_of(&db_path);
        assert!(
            columns.iter().any(|c| c == "value_text"),
            "v2 → v3 必须补出 value_text 列，实际列: {:?}",
            columns
        );
        assert_eq!(
            user_version_of(&db_path),
            SCHEMA_VERSION,
            "v2 老库升级后版本号必须推进到最新"
        );

        let items = storage
            .history_query("p-v2", 0, i64::MAX, 10)
            .expect("history query v2 row");
        assert_eq!(items.len(), 1, "v2 历史行必须保留");
        assert_eq!(
            items[0].value_text, None,
            "v2 行没有标签，迁移后应读出 None 而不是被回填"
        );
    }

    /// 未版本化的老库（`user_version = 0`）按 v1 处理，同样能升到最新。
    ///
    /// 这是所有「引入版本号之前就已在现场运行」的库的实际形态 —— 升级时
    /// 最容易漏掉的正是这一档。
    #[test]
    fn unversioned_legacy_db_is_treated_as_v1() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("queue.db");
        seed_ancient_db(&db_path, 1);
        assert_eq!(user_version_of(&db_path), 0, "前置条件：库里没有版本号");

        let storage = crate::queue::storage::SqliteStorage::new(
            db_path.to_str().expect("db path utf8"),
            Arc::new(Metrics::new()),
        )
        .expect("未版本化老库必须能升到最新");

        assert_eq!(storage.len(), 1, "历史行必须保留");
        assert_eq!(
            user_version_of(&db_path),
            SCHEMA_VERSION,
            "未版本化老库升级后必须被盖章为最新版本"
        );
    }

    /// 回滚保护：库版本高于程序支持版本时必须拒绝启动。
    ///
    /// 用旧程序打开被新版本升级过的库，旧代码不认识新列/新表结构，
    /// 最坏情况是静默写入错误数据或把结构改回去。宁可启动失败并给出
    /// 明确提示，也不能让现场带着一个「看起来能跑」的库继续计费采集。
    #[test]
    fn database_from_newer_program_is_rejected() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("queue.db");
        seed_ancient_db(&db_path, 1);
        stamp_user_version(&db_path, SCHEMA_VERSION + 1);

        let err = crate::queue::storage::SqliteStorage::new(
            db_path.to_str().expect("db path utf8"),
            Arc::new(Metrics::new()),
        )
        .err()
        .expect("更高版本的库必须拒绝启动");

        let msg = err.to_string();
        assert!(
            msg.contains("高于本程序支持"),
            "错误信息必须点明版本过高，实际: {}",
            msg
        );
    }

    /// 重复打开最新库不应再触发任何迁移写入。
    ///
    /// 版本号已在最新时 `migrate_schema` 直接返回；若这里仍去跑 ALTER，
    /// 每次启动都会在日志里刷一遍「列已存在」，也说明版本判断失效。
    #[test]
    fn opening_current_db_is_a_noop() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("queue.db");

        for _ in 0..2 {
            let _storage = crate::queue::storage::SqliteStorage::new(
                db_path.to_str().expect("db path utf8"),
                Arc::new(Metrics::new()),
            )
            .expect("open current db twice");
            assert_eq!(user_version_of(&db_path), SCHEMA_VERSION);
        }
    }
}
