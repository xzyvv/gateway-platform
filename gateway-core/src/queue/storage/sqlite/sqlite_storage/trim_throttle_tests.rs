use super::SqliteStorage;
use crate::engine::metrics::Metrics;
use crate::engine::model::DataPoint;
use crate::queue::storage::QueueStorage;
use std::sync::atomic::Ordering;
use std::sync::Arc;

fn make_points(prefix: &str, n: usize) -> Vec<DataPoint> {
    (0..n)
        .map(|i| DataPoint::good(&format!("{prefix}-{i}"), "dev", 1.0, 1.0, "f64"))
        .collect()
}

fn rows(storage: &SqliteStorage) -> i64 {
    storage
        .conn
        .lock()
        .query_row("SELECT COUNT(*) FROM data_queue", [], |r| r.get(0))
        .expect("count rows")
}

#[test]
fn enqueue_trims_to_ninety_percent_then_throttles() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("queue.db");
    let storage =
        SqliteStorage::new_with_limit(db_path.to_str().unwrap(), Arc::new(Metrics::new()), 10)
            .unwrap();

    // 1) 未超限：不 trim，last_trim_ms 保持 0（热路径零 DB 写）。
    storage.enqueue_batch(&make_points("a", 10)).unwrap();
    assert_eq!(rows(&storage), 10);
    assert_eq!(
        storage.last_trim_ms.load(Ordering::Relaxed),
        0,
        "未超限时不得 trim"
    );

    // 2) 超限：一次削到 90%（10 → 9），而不是刚好削到 max_rows。
    storage.enqueue_batch(&make_points("b", 1)).unwrap();
    assert_eq!(rows(&storage), 9, "超限后一次削到上限的 90%");
    assert_eq!(
        storage.rows_total.load(Ordering::Relaxed),
        9,
        "rows_total 必须与表内真实行数一致"
    );
    assert_eq!(
        storage.depth.load(Ordering::Relaxed),
        9,
        "被淘汰的未送达行必须同步扣减 depth"
    );
    let first_trim = storage.last_trim_ms.load(Ordering::Relaxed);
    assert!(first_trim > 0, "trim 后必须记录时刻");

    // 3) 冷却期内再次超限：不得再 trim（行数只增不减即证明没跑 DELETE）。
    // 这里显式把「上次 trim 时刻」钉在当前时刻，避免依赖步骤 2 与 3 之间的
    // 真实耗时（CI 偶发卡顿超过 1s 会让用例误判）。
    let just_trimmed = DataPoint::now();
    storage.last_trim_ms.store(just_trimmed, Ordering::Relaxed);
    storage.enqueue_batch(&make_points("c", 2)).unwrap();
    assert_eq!(rows(&storage), 11, "冷却期内超限也不得删行");
    assert_eq!(
        storage.last_trim_ms.load(Ordering::Relaxed),
        just_trimmed,
        "冷却期内不得刷新 trim 时刻"
    );

    // 4) 冷却期过后：恢复 trim，仍削到 90%。
    storage
        .last_trim_ms
        .store(DataPoint::now() - 5_000, Ordering::Relaxed);
    storage.enqueue_batch(&make_points("d", 1)).unwrap();
    assert_eq!(rows(&storage), 9, "冷却期过后应恢复 trim 到 90%");
    assert_eq!(storage.rows_total.load(Ordering::Relaxed), 9);
    assert_eq!(storage.depth.load(Ordering::Relaxed), 9);
}

/// 单轮删除量必须被预算限住：远超上限时不能一次删几十万行（会长时间占住写锁），
/// 而是分多轮逼近 90% 水位。
#[test]
fn enqueue_caps_rows_deleted_per_trim() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("queue.db");
    let storage =
        SqliteStorage::new_with_limit(db_path.to_str().unwrap(), Arc::new(Metrics::new()), 1_000)
            .unwrap();

    // 预置 3000 行：把「上次 trim 时刻」钉到未来，抑制 seeding 阶段的淘汰。
    storage
        .last_trim_ms
        .store(DataPoint::now() + 60_000, Ordering::Relaxed);
    for i in 0..30 {
        storage
            .enqueue_batch(&make_points(&format!("seed{i}"), 100))
            .unwrap();
    }
    assert_eq!(rows(&storage), 3000);

    // 放开 trim：3010 行，单轮预算 = max(2000, 2×10) = 2000 → 只删到 1010 行。
    storage.last_trim_ms.store(0, Ordering::Relaxed);
    storage.enqueue_batch(&make_points("cap", 10)).unwrap();
    assert_eq!(
        rows(&storage),
        1010,
        "单轮删除量必须被预算限住（3010 - 2000）"
    );

    // 下一轮：剩余缺口（110 行）小于预算，一次削到 90% 水位。
    storage.last_trim_ms.store(0, Ordering::Relaxed);
    storage.enqueue_batch(&make_points("finish", 10)).unwrap();
    assert_eq!(rows(&storage), 900, "分轮逼近后必须收敛到上限的 90%");
    assert_eq!(storage.rows_total.load(Ordering::Relaxed), 900);
    assert_eq!(storage.depth.load(Ordering::Relaxed), 900);
}
