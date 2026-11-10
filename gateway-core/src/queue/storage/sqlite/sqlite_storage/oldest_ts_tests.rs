use super::SqliteStorage;
use crate::engine::metrics::Metrics;
use crate::queue::storage::QueueStorage;
use std::sync::Arc;

#[test]
fn includes_aggregated_table() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("queue.db");
    let storage = SqliteStorage::new(db_path.to_str().unwrap(), Arc::new(Metrics::new())).unwrap();

    let old_ts = 1_700_000_000_000i64;
    // 只往聚合表写一行（模拟原始行已被 retention 清理、只剩分钟桶的历史）
    {
        let guard = storage.conn.lock();
        guard
            .execute(
                "INSERT INTO data_queue_aggregated \
                     (point_id, window_start, avg_value, min_value, max_value, sample_count) \
                 VALUES (?1, ?2, 1.0, 1.0, 1.0, 1)",
                rusqlite::params!["p-agg", old_ts],
            )
            .unwrap();
    }

    assert_eq!(
        storage.oldest_data_timestamp(),
        Some(old_ts),
        "聚合表里的历史也必须计入保留跨度"
    );
}
