//! 队列库 schema 迁移回归测试（从 `db_init.rs` 拆出）。
//!
//! 拆出原因：`db_init.rs` 同时承载建库/迁移逻辑与全部迁移回归用例，
//! 早已触及行数预算上限；新增加密构建的用例只会让它继续增长。
//! 这里只放**测试**，不改动任何生产代码路径。
//!
//! 用例覆盖：列增补（v2/v3）、索引与 `auto_vacuum`（v4）、
//! `user_version` 版本号与跨版本迁移、幂等重复升级、拒绝高版本库。
use super::*;
use crate::queue::storage::QueueStorage;

/// 测试内直接查库：加密构建下必须先用 keyfile 里的密钥 `PRAGMA key`，
/// 否则 SQLCipher 会把密文库当成损坏文件（与生产 `open_with_key` 同序）。
fn inspect_conn(db_path: &Path) -> Connection {
    let conn = Connection::open(db_path).expect("open db");
    #[cfg(feature = "encryption")]
    {
        let key_hex = crate::queue::core::ensure_key(db_path).expect("load db key");
        conn.pragma_update(None, "key", key_hex.as_str())
            .expect("set db key");
    }
    conn
}

/// 批次 C：迁移前（旧 schema）的 `data_queue` 列定义。
/// 刻意不含 `value_text`——用于模拟升级前已落库的历史库。
///
/// 明文老库迁移只在未启用加密的构建里成立：`encryption` 打开后
/// 生产入口会先 `PRAGMA key` 再读库，明文库被视为损坏，故相关用例整体跳过。
#[cfg(not(feature = "encryption"))]
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
#[cfg(not(feature = "encryption"))]
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

    let conn = inspect_conn(&db_path);
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
#[cfg(not(feature = "encryption"))]
const ANCIENT_SCHEMA: &str = "CREATE TABLE data_queue (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    point_id TEXT NOT NULL,
    value REAL NOT NULL,
    raw_value REAL NOT NULL,
    raw_type TEXT NOT NULL,
    quality TEXT NOT NULL,
    timestamp INTEGER NOT NULL
)";

#[cfg(not(feature = "encryption"))]
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

#[cfg(not(feature = "encryption"))]
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

#[cfg(not(feature = "encryption"))]
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
#[cfg(not(feature = "encryption"))]
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
#[cfg(not(feature = "encryption"))]
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
    let conn = inspect_conn(db_path);
    conn.pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
        .expect("read user_version")
}

#[cfg(not(feature = "encryption"))]
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
#[cfg(not(feature = "encryption"))]
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
#[cfg(not(feature = "encryption"))]
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
#[cfg(not(feature = "encryption"))]
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

/// 全新库必须开启 `auto_vacuum=INCREMENTAL`：retention 删除后由 maintenance
/// 周期 `incremental_vacuum` 把空闲页还给文件系统，否则主库只增不减。
#[test]
fn fresh_db_enables_incremental_auto_vacuum() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db_path = dir.path().join("queue.db");

    let _storage = crate::queue::storage::SqliteStorage::new(
        db_path.to_str().expect("db path utf8"),
        Arc::new(Metrics::new()),
    )
    .expect("open fresh db");

    let conn = inspect_conn(&db_path);
    let mode: i64 = conn
        .pragma_query_value(None, "auto_vacuum", |r| r.get(0))
        .expect("read auto_vacuum");
    assert_eq!(mode, 2, "auto_vacuum 必须为 INCREMENTAL(2)，实际 {}", mode);
}

/// 已存在的 v3 库（auto_vacuum 默认 NONE）打开后必须一次性切换为
/// INCREMENTAL，并补出 v4 的行数上界索引。
#[cfg(not(feature = "encryption"))]
#[test]
fn legacy_v3_db_upgraded_to_v4_index_and_incremental_vacuum() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db_path = dir.path().join("queue.db");
    {
        let conn = Connection::open(&db_path).expect("open v3 db");
        conn.execute_batch(OLD_SCHEMA)
            .expect("create v3 data_queue");
        conn.execute_batch("ALTER TABLE data_queue ADD COLUMN value_text TEXT;")
            .expect("add value_text");
        conn.execute_batch(
            "CREATE TABLE data_queue_aggregated (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    point_id TEXT NOT NULL,
                    window_start INTEGER NOT NULL,
                    avg_value REAL NOT NULL,
                    min_value REAL NOT NULL,
                    max_value REAL NOT NULL,
                    sample_count INTEGER NOT NULL,
                    UNIQUE(point_id, window_start)
                );
                CREATE INDEX idx_agg_point_ts
                    ON data_queue_aggregated(point_id, window_start);",
        )
        .expect("create v3 aggregated table");
        let before: i64 = conn
            .pragma_query_value(None, "auto_vacuum", |r| r.get(0))
            .expect("read auto_vacuum");
        assert_eq!(before, 0, "前置条件：v3 库默认 auto_vacuum=NONE");
        conn.pragma_update(None, "user_version", 3)
            .expect("stamp v3");
    }

    let _storage = crate::queue::storage::SqliteStorage::new(
        db_path.to_str().expect("db path utf8"),
        Arc::new(Metrics::new()),
    )
    .expect("v3 库必须能升到 v4");

    assert_eq!(user_version_of(&db_path), SCHEMA_VERSION);
    let conn = Connection::open(&db_path).expect("open migrated db");
    let mode: i64 = conn
        .pragma_query_value(None, "auto_vacuum", |r| r.get(0))
        .expect("read auto_vacuum");
    assert_eq!(mode, 2, "已有库必须一次性切换为 INCREMENTAL");
    let idx: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type='index' AND name='idx_agg_window'",
            [],
            |r| r.get(0),
        )
        .expect("check idx_agg_window");
    assert_eq!(idx, 1, "v4 迁移必须建出聚合表 window_start 索引");
}

/// 回滚保护：库版本高于程序支持版本时必须拒绝启动。
///
/// 用旧程序打开被新版本升级过的库，旧代码不认识新列/新表结构，
/// 最坏情况是静默写入错误数据或把结构改回去。宁可启动失败并给出
/// 明确提示，也不能让现场带着一个「看起来能跑」的库继续计费采集。
#[cfg(not(feature = "encryption"))]
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
