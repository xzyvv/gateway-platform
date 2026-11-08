//! 磁盘满 / WAL 写入失败回归
//!
//! 现场最常见的故障之一：日志把分区写满、U 盘质量差导致写失败。要求是
//! **不 panic、不损坏已有数据、空间恢复后能继续写入**，而不是把进程直接打死。
//!
//! 复现手段：`PRAGMA max_page_count` 把库尺寸钉死在当前页数，SQLite 在需要
//! 扩展页时返回 SQLITE_FULL（"database or disk is full"）——与真实 ENOSPC
//! 错误码一致，但不需要 root 挂 tmpfs，Windows / Linux 都能跑。

use std::sync::Arc;

use crate::engine::metrics::Metrics;
use crate::engine::model::DataPoint;
use crate::queue::storage::QueueStorage;

use super::SqliteStorage;

/// 构造从 `start` 开始、单调递增的 n 个数据点。
fn make_points(start: i64, n: i64) -> Vec<DataPoint> {
    (start..start + n)
        .map(|i| DataPoint::good(&format!("pt_{i}"), "dev_1", i as f64, i as f64, "u16"))
        .collect()
}

#[test]
fn enqueue_reports_disk_full_without_losing_data_and_recovers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("full.db");
    let storage = SqliteStorage::new(db.to_str().expect("utf8 path"), Arc::new(Metrics::new()))
        .expect("open queue");

    // 先落一批数据，作为「写失败后已有数据不能丢」的对照。
    storage
        .enqueue_batch(&make_points(0, 32))
        .expect("seed enqueue");
    let seeded = storage
        .history_query("pt_0", 0, i64::MAX, 10)
        .expect("query seeded rows");
    assert!(!seeded.is_empty(), "种子数据必须写入成功");

    // 把库钉死在当前页数：此后任何需要扩页的写入都会得到 SQLITE_FULL。
    let pages = storage.page_count().expect("page_count");
    storage.set_max_page_count(pages).expect("clamp pages");

    // 反复写入直到撞上上限：WAL 模式下前面几批可能仍落在已有页里。
    let mut failure = None;
    let mut start = 1000i64;
    for _ in 0..64 {
        match storage.enqueue_batch(&make_points(start, 64)) {
            Ok(_) => start += 64,
            Err(e) => {
                failure = Some(e.to_string());
                break;
            }
        }
    }
    let msg = failure.expect("页数上限内持续写入必须失败，否则磁盘满场景无错误可测");
    assert!(
        msg.contains("full"),
        "错误信息应保留 SQLite 的 disk full 语义，实际: {msg}"
    );

    // 写失败不得影响已有数据（README 承诺的「不因写失败丢已存数据」）。
    let kept = storage
        .history_query("pt_0", 0, i64::MAX, 10)
        .expect("query after disk full");
    assert_eq!(
        kept.len(),
        seeded.len(),
        "写失败后已有历史行必须仍在（期望 {} 行，实际 {} 行）",
        seeded.len(),
        kept.len()
    );

    // 腾出空间后必须能继续写入：磁盘满应是可恢复错误，不是永久损坏。
    storage
        .set_max_page_count(pages + 4096)
        .expect("raise page limit");
    storage
        .enqueue_batch(&make_points(start, 8))
        .expect("enqueue after space freed");
    assert!(!storage.is_empty(), "恢复空间后队列不应为空");
}
