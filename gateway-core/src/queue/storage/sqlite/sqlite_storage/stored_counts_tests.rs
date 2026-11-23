//! DD-52：`stored_counts` 必须返回窗口内真实落库的样本数。
//!
//! 覆盖两点：
//! 1. 原始表按行计数，聚合表按 `sample_count` 还原原始样本数，两者相加；
//! 2. 窗口边界外（早于 start / 晚于 end）的行不计入。

use super::SqliteStorage;
use crate::engine::metrics::Metrics;
use crate::queue::storage::QueueStorage;
use std::sync::Arc;

fn make_storage() -> (tempfile::TempDir, SqliteStorage) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("queue.db");
    let storage = SqliteStorage::new(db_path.to_str().unwrap(), Arc::new(Metrics::new())).unwrap();
    (dir, storage)
}

fn insert_raw(storage: &SqliteStorage, point_id: &str, ts: i64) {
    let guard = storage.conn.lock();
    guard
        .execute(
            "INSERT INTO data_queue (point_id, value, raw_value, raw_type, quality, timestamp) \
             VALUES (?1, 1.0, 1.0, 'f64', 'good', ?2)",
            rusqlite::params![point_id, ts],
        )
        .unwrap();
}

fn insert_agg(storage: &SqliteStorage, point_id: &str, window_start: i64, sample_count: i64) {
    let guard = storage.conn.lock();
    guard
        .execute(
            "INSERT INTO data_queue_aggregated \
                 (point_id, window_start, avg_value, min_value, max_value, sample_count) \
             VALUES (?1, ?2, 1.0, 1.0, 1.0, ?3)",
            rusqlite::params![point_id, window_start, sample_count],
        )
        .unwrap();
}

#[test]
fn counts_raw_and_aggregated_samples() {
    let (_dir, storage) = make_storage();
    let start = 1_700_000_000_000i64;
    let end = start + 60_000;

    insert_raw(&storage, "p1", start + 1_000);
    insert_raw(&storage, "p1", start + 2_000);
    insert_raw(&storage, "p2", start + 3_000);
    // 聚合表：一个分钟桶代表 10 条原始样本
    insert_agg(&storage, "p1", start + 30_000, 10);

    let counts = storage.stored_counts(start, end).unwrap();
    assert_eq!(counts.get("p1"), Some(&12), "原始 2 行 + 聚合 10 样本");
    assert_eq!(counts.get("p2"), Some(&1));
    assert_eq!(counts.get("missing"), None);
}

#[test]
fn excludes_rows_outside_window() {
    let (_dir, storage) = make_storage();
    let start = 1_700_000_000_000i64;
    let end = start + 60_000;

    insert_raw(&storage, "p1", start - 1);
    insert_raw(&storage, "p1", end + 1);
    insert_raw(&storage, "p1", start);
    insert_raw(&storage, "p1", end);
    insert_agg(&storage, "p1", start - 60_000, 99);

    let counts = storage.stored_counts(start, end).unwrap();
    assert_eq!(
        counts.get("p1"),
        Some(&2),
        "只统计闭区间 [start, end] 内的行"
    );
}
