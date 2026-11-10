use super::*;

/// 预拼接的 INSERT 语句必须严格含 `rows * INSERT_COLS_PER_ROW` 个匿名占位符：
/// 少一个 → `tx.execute` 失败并**静默**退化为逐行插入（吞吐塌陷，难以察觉）；
/// 多一个 → 参数个数不匹配直接报错丢批。
#[test]
fn insert_sql_placeholder_count_matches_row_count() {
    for rows in 1..=INSERT_CHUNK_SIZE {
        let sql = insert_sql_for_rows(rows);
        assert_eq!(
            sql.matches('?').count(),
            rows * INSERT_COLS_PER_ROW,
            "rows={} 的预拼接 SQL 占位符数必须与绑定参数数一致",
            rows
        );
        assert!(
            sql.starts_with("INSERT INTO data_queue"),
            "rows={} 的预拼接 SQL 必须指向 data_queue 表",
            rows
        );
    }
}

/// 建一张与生产同构的 `data_queue` 表（列定义对齐 `db_init::init_queue_db`）。
fn setup_conn() -> Connection {
    let conn = Connection::open_in_memory().expect("open in-memory sqlite");
    conn.execute_batch(
        "CREATE TABLE data_queue (
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
        );",
    )
    .expect("create data_queue");
    conn
}

/// 插入 `n` 行，时间戳从 1000 递增（淘汰顺序可预期）；`delivered=1` 表示已送达。
fn insert_rows(conn: &Connection, n: i64, delivered: i64) {
    insert_rows_from(conn, n, delivered, 1000);
}

/// 同上，但指定起始时间戳，便于构造「已送达批次在前、未送达批次在后」的场景。
fn insert_rows_from(conn: &Connection, n: i64, delivered: i64, ts0: i64) {
    for i in 0..n {
        conn.execute(
            "INSERT INTO data_queue (point_id, value, raw_value, raw_type, quality, timestamp, in_flight, delivered)
             VALUES (?1, 0.0, 0.0, 'f64', 'good', ?2, 0, ?3)",
            params![format!("p-{}", i), ts0 + i, delivered],
        )
        .expect("insert row");
    }
}

/// 插入 `n` 行「在途」记录（in_flight=1）——任何淘汰/清理路径都不得删除它们。
fn insert_rows_in_flight(conn: &Connection, n: i64) {
    for i in 0..n {
        conn.execute(
            "INSERT INTO data_queue (point_id, value, raw_value, raw_type, quality, timestamp, in_flight, delivered)
             VALUES (?1, 0.0, 0.0, 'f64', 'good', ?2, 1, 0)",
            params![format!("p-{}", i), 1000 + i],
        )
        .expect("insert in-flight row");
    }
}

fn row_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM data_queue", [], |r| r.get(0))
        .expect("count rows")
}

/// 测试辅助：按表内真实行数调用 `enforce_queue_limit_with`。
///
/// 生产路径由 O(1) 内存计数器传入 total/unsent（P1-2），测试里没有计数器，
/// 直接查一次表即可——被测的仍是淘汰逻辑本身。
fn enforce(conn: &mut Connection, max_rows: i64) -> super::OverflowReport {
    let (total, unsent): (i64, i64) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(in_flight = 0 AND delivered = 0), 0) FROM data_queue",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("count rows");
    SqliteStorage::enforce_queue_limit_with(conn, max_rows, total, unsent)
}

/// 修复(审查#6) 回归：已送达行堆积到超过上限时必须被回收。
#[test]
fn delivered_rows_are_pruned_by_total_row_count() {
    let mut conn = setup_conn();
    insert_rows(&conn, 10, 1);
    let report = enforce(&mut conn, 4);

    assert_eq!(row_count(&conn), 4, "应被削到上限行数");
    assert_eq!(report.deleted, 6);
    assert_eq!(report.undelivered_deleted, 0);
    assert_eq!(report.total, 4, "report.total 应为删行后的真实总行数");
    assert_eq!(report.unsent, 0);
    assert!(!report.unsent_overflow);
    let min_id: i64 = conn
        .query_row("SELECT MIN(id) FROM data_queue", [], |r| r.get(0))
        .expect("min id");
    assert_eq!(min_id, 7, "必须按最旧优先淘汰");
}

/// P0-2 回归：未送达行（delivered=0，直连场景的全部行）在超限时同样要被淘汰，
/// 否则行数上限到顶也一条删不掉 → 队列无界增长。
/// 同时 report 必须精确回报被删的未送达行数，供调用方扣减 depth。
#[test]
fn undelivered_rows_pruned_when_over_capacity() {
    let mut conn = setup_conn();
    insert_rows(&conn, 9, 0);
    let report = enforce(&mut conn, 4);

    assert_eq!(report.deleted, 5, "超出上限的 5 行未送达必须被淘汰");
    assert_eq!(report.undelivered_deleted, 5, "被删的 5 行全部是未送达行");
    assert_eq!(row_count(&conn), 4, "必须削到上限");
    assert_eq!(report.total, 4);
    assert_eq!(report.unsent, 4);
    assert!(
        report.unsent_overflow,
        "ratio=100% 必须触发 unsent_overflow（enqueue 走 fallback）"
    );
}

/// 混合场景：按「最旧优先」淘汰，可见未送达与已送达行都会被削掉。
#[test]
fn mixed_rows_pruned_oldest_first() {
    let mut conn = setup_conn();
    insert_rows_from(&conn, 6, 1, 1000); // 最旧 6 行，已送达
    insert_rows_from(&conn, 3, 0, 2000); // 较新 3 行，未送达
    let report = enforce(&mut conn, 2);

    assert_eq!(report.deleted, 7, "10 行削到 2 行，应删 7 行");
    assert_eq!(
        report.undelivered_deleted, 1,
        "最旧的 6 行已送达，另删 1 行未送达"
    );
    assert_eq!(row_count(&conn), 2);
    assert_eq!(report.total, 2);
    assert_eq!(report.unsent, 2, "剩余 2 行都是未送达");
    assert!(report.unsent_overflow);
}

/// 在途行（in_flight=1）永不删除：删了会造成重复上报/丢包。
#[test]
fn in_flight_rows_are_never_pruned() {
    let mut conn = setup_conn();
    insert_rows_in_flight(&conn, 5);
    let report = enforce(&mut conn, 1);

    assert_eq!(report.deleted, 0, "在途行一条都不能删");
    assert_eq!(row_count(&conn), 5);
    assert!(report.total > 1, "删不到上限时 total 仍高于 max_rows");
}

/// 未超限时不删任何行，ratio 正常计算。
#[test]
fn below_limit_deletes_nothing() {
    let mut conn = setup_conn();
    insert_rows(&conn, 1, 1);
    insert_rows(&conn, 3, 0);
    let report = enforce(&mut conn, 1000);

    assert_eq!(report.deleted, 0);
    assert_eq!(row_count(&conn), 4);
    assert_eq!(report.unsent, 3);
    assert!((report.ratio - 0.75).abs() < 1e-9);
    assert!(!report.unsent_overflow, "3/4=75% < 80% 不应触发");
}

/// r30-QL1 (回归)：enforce_queue_limit 现在把 COUNT + DELETE 包入单个事务。
/// 旧实现两条 query_row 与 delete 分开执行，期间另一线程可能 INSERT/DELETE
/// 导致 total/unsent 与实际删行结果不一致。本测试覆盖：事务 commit 路径正常
/// 工作 + 删行后表内状态 = OverflowReport 报告值（两者必须一致）。
#[test]
fn tx_commit_keeps_report_consistent_with_table_state() {
    let mut conn = setup_conn();
    // 7 行已送达 + 3 行未送达 = 10 行，max_rows=5 → 删最旧 5 行（均为已送达）。
    insert_rows_from(&conn, 7, 1, 1000);
    insert_rows_from(&conn, 3, 0, 2000);

    let report = enforce(&mut conn, 5);

    assert_eq!(report.deleted, 5);
    assert_eq!(report.undelivered_deleted, 0);
    assert_eq!(report.total, 5);
    assert_eq!(report.unsent, 3);
    // 表内实际行数 = report.total（事务 commit 成功，二者必须一致）
    assert_eq!(
        row_count(&conn),
        report.total,
        "事务 commit 后表内行数应等于报告值"
    );
    // 剩 2 行已送达 + 3 行未送达
    let remaining_delivered: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM data_queue WHERE delivered = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(remaining_delivered, 2);
}

/// r30-QL1 补充：满表场景下事务能完整 commit（无 SQLite BUSY 错误）。
#[test]
fn tx_commit_under_full_table() {
    let mut conn = setup_conn();
    // 100 行已送达，max_rows=10 → 删 90 行
    insert_rows(&conn, 100, 1);
    let report = enforce(&mut conn, 10);
    assert_eq!(report.deleted, 90);
    assert_eq!(row_count(&conn), 10);
    assert_eq!(report.unsent, 0);
}

/// P0-2 回归：保留期到点必须真正生效——超期**未送达**行要被删除，
/// 且 depth 计数同步扣减（旧实现因 `delivered=1` 守卫一条都删不掉）。
#[test]
fn retention_prunes_undelivered_old_rows_and_decrements_depth() {
    let mut conn = setup_conn();
    insert_rows(&conn, 3, 0); // timestamp 1000..1002，远早于当前时间
    let depth = AtomicI64::new(3);
    let rows_total = AtomicI64::new(3);

    let deleted = SqliteStorage::cleanup_old_records(&mut conn, Some(1), &depth, &rows_total);

    assert_eq!(deleted, 3, "超期未送达行必须被回收");
    assert_eq!(row_count(&conn), 0);
    assert_eq!(
        rows_total.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "retention 删行必须同步扣减 rows_total，否则容量判定恒超限"
    );
    assert_eq!(
        depth.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "depth 必须同步扣减，否则溢出判据虚高"
    );
}

/// retention 不得删除在途行（in_flight=1）。
#[test]
fn retention_protects_in_flight_rows() {
    let mut conn = setup_conn();
    insert_rows_in_flight(&conn, 2);
    let depth = AtomicI64::new(0);
    let rows_total = AtomicI64::new(2);

    let deleted = SqliteStorage::cleanup_old_records(&mut conn, Some(1), &depth, &rows_total);

    assert_eq!(deleted, 0, "在途行不能删");
    assert_eq!(row_count(&conn), 2);
    assert_eq!(
        rows_total.load(std::sync::atomic::Ordering::Relaxed),
        2,
        "没删行时 rows_total 不得变化"
    );
}

/// r30c-QL6 (回归)：enqueue_batch 收到空切片应早返 Ok(0)，不开事务。
/// 旧实现会无意义开 BEGIN→空循环→COMMIT，浪费 2 次 fsync（synchronous=FULL）。
/// 验证：直接调 enqueue_batch(&mut conn, &[]) 应返回 Ok(0)，
/// 且数据库 WAL 文件不会被改写（间接通过 savepoint 验证：嵌套事务可正常 BEGIN）。
#[test]
fn enqueue_batch_empty_early_returns_without_opening_tx() {
    let mut conn = setup_conn();
    let n = SqliteStorage::enqueue_batch(&mut conn, &[]).expect("empty batch");
    assert_eq!(n, 0);
    assert_eq!(row_count(&conn), 0, "空 batch 不应写任何行");

    // 后续事务仍能正常开启——证明空 batch 没遗留 BEGIN 状态
    insert_rows(&conn, 3, 0);
    assert_eq!(row_count(&conn), 3);
}
