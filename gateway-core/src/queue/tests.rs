use super::*;
use crate::engine::model::DataPoint;
use crate::engine::model::Quality;

fn make_point(id: &str, value: f64) -> DataPoint {
    let now = DataPoint::now();
    DataPoint {
        point_id: id.into(),
        device_id: "dev1".into(),
        value,
        raw_value: value,
        raw_type: "u16".into(),
        quality: Quality::Good,
        quality_reason: "".into(),
        timestamp: now,
        source_ts: now,
        stale_count: 0,
        value_text: None,
    }
}

#[tokio::test]
async fn test_enqueue_dequeue_ack() {
    let queue = DataQueue::new_memory().unwrap();

    assert!(queue.is_empty().await);

    queue
        .enqueue(vec![make_point("p1", 1.0), make_point("p2", 2.0)])
        .await
        .unwrap();
    assert_eq!(queue.len().await, 2);

    let items = queue.dequeue(10).await.unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].point_id, "p1");
    assert_eq!(items[1].point_id, "p2");

    // dequeue 后仍在队列中（in_flight），ack 后才删除
    assert_eq!(queue.len().await, 2);

    let ids: Vec<i64> = items.iter().map(|i| i.id).collect();
    queue.ack(ids).await.unwrap();
    assert_eq!(queue.len().await, 0);
    assert!(queue.is_empty().await);
}

#[tokio::test]
async fn test_dequeue_respects_limit() {
    let queue = DataQueue::new_memory().unwrap();

    let points: Vec<DataPoint> = (0..5)
        .map(|i| make_point(&format!("p{}", i), i as f64))
        .collect();
    queue.enqueue(points).await.unwrap();

    let first = queue.dequeue(2).await.unwrap();
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].point_id, "p0");

    // ack 第一批后再取第二批
    let ids: Vec<i64> = first.iter().map(|i| i.id).collect();
    queue.ack(ids).await.unwrap();

    let second = queue.dequeue(10).await.unwrap();
    assert_eq!(second.len(), 3);
    assert_eq!(second[0].point_id, "p2");
}

#[tokio::test]
async fn test_history_query() {
    let queue = DataQueue::new_memory().unwrap();

    queue
        .enqueue(vec![make_point("p1", 1.0), make_point("p2", 2.0)])
        .await
        .unwrap();

    // history_ex 按 point_id + 时间范围查询（N-10 起为唯一历史查询入口）
    let hist = queue
        .history_ex("p1", 0, i64::MAX, 100, None)
        .await
        .unwrap();
    assert_eq!(hist.items.len(), 1);
    assert_eq!(hist.items[0].value, 1.0);
    assert_eq!(hist.granularity, "raw");
}

#[tokio::test]
async fn test_disk_full_buffers_in_fallback() {
    let queue = DataQueue::new_memory().unwrap();

    // S2: 磁盘满时入队返回 Ok，数据进 fallback 缓冲
    queue.set_disk_ok(false);
    queue.enqueue(vec![make_point("p1", 1.0)]).await.unwrap();
    assert_eq!(queue.fallback_len(), 1);
    assert_eq!(queue.len().await, 0); // SQLite 队列仍为空

    // 恢复后 flush + 正常入队
    queue.set_disk_ok(true);
    queue.enqueue(vec![make_point("p2", 2.0)]).await.unwrap();
    // flush 后 fallback 清空，2 条在 SQLite
    assert_eq!(queue.fallback_len(), 0);
    assert_eq!(queue.len().await, 2);
}

#[tokio::test]
async fn test_fallback_priority_eviction() {
    use crate::engine::model::Quality;
    let queue = DataQueue::new_memory().unwrap();

    // 构造混合质量的数据点
    let mut good = make_point("good", 1.0);
    good.quality = Quality::Good;
    let mut bad = make_point("bad", 0.0);
    bad.quality = Quality::Bad;
    bad.stale_count = 5;

    queue.set_disk_ok(false);
    // 入队 bad + good
    queue.enqueue(vec![bad, good.clone()]).await.unwrap();
    assert_eq!(queue.fallback_len(), 2);

    // 恢复后 flush，两条都应在 SQLite
    queue.set_disk_ok(true);
    queue.flush_fallback().await.unwrap();
    assert_eq!(queue.len().await, 2);
}

/// Bug 183 (N01): fallback 反弹轮次达到上限后整批点必须被强制丢弃，
/// 避免非磁盘满 DB 错误导致 fallback↔queue 无限反弹、占满 CPU/IO。
#[test]
fn test_fallback_bounce_cap_drops_batch() {
    use super::{push_into_fallback, FALLBACK_CAPACITY, FALLBACK_MAX_RETRIES};
    use std::collections::VecDeque;

    let mut buf: VecDeque<DataPoint> = VecDeque::new();
    let mut retry = 0u32;

    // 前 5 轮（达到 FALLBACK_MAX_RETRIES）每轮都应正常 push，不丢弃。
    for round in 0..FALLBACK_MAX_RETRIES {
        let pts = vec![make_point(&format!("p{}", round), round as f64)];
        let (retained, dropped) = push_into_fallback(&mut buf, FALLBACK_CAPACITY, pts, &mut retry);
        assert_eq!(dropped, 0, "round {} should not drop", round);
        assert_eq!(retained, 1);
    }
    // 此时 retry == FALLBACK_MAX_RETRIES = 5。

    // 第 6 轮：本批被强制丢弃（retained=0, dropped=1），retry 重置为 0。
    let pts = vec![make_point("victim", 42.0)];
    let (retained, dropped) = push_into_fallback(&mut buf, FALLBACK_CAPACITY, pts, &mut retry);
    assert_eq!(retained, 0);
    assert_eq!(dropped, 1);
    assert_eq!(retry, 0, "retry should reset to 0 after cap triggers");

    // 缓冲里只有前 5 轮的 5 个点。
    assert_eq!(buf.len(), 5);
    assert!(!buf.iter().any(|dp| dp.point_id == "victim"));
}

/// Bug 183 (N01): 容量超限时按优先级淘汰最低者，并把淘汰数计入 dropped。
#[test]
fn test_fallback_capacity_eviction_reported() {
    use super::push_into_fallback;
    use std::collections::VecDeque;

    let mut buf: VecDeque<DataPoint> = VecDeque::new();
    let mut retry = 0u32;

    // 先填入 1 个 Good 点（高优先级），再填入 1 个 Bad 点（低优先级）。
    let mut good = make_point("good", 1.0);
    good.quality = Quality::Good;
    let mut bad = make_point("bad", 0.0);
    bad.quality = Quality::Bad;
    let _ = push_into_fallback(&mut buf, 2, vec![good.clone()], &mut retry);
    let _ = push_into_fallback(&mut buf, 2, vec![bad.clone()], &mut retry);
    assert_eq!(buf.len(), 2);

    // 容量=2，再 push 第三个 Good 点，bad（Bad 质量）应被淘汰。
    // 因为新 push 的也是 Good，但淘汰策略始终选最低者（bad < good），
    // 所以 buf 变成 [good, new_good]（good 在前，new_good 在后），
    // 唯一被淘汰的就是 bad；retained=0（被保留下来的旧 1 个 good 不算本批的 retained）。
    let mut new_good = make_point("new_good", 9.0);
    new_good.quality = Quality::Good;
    let (retained, dropped) = push_into_fallback(&mut buf, 2, vec![new_good], &mut retry);
    assert_eq!(retained, 0);
    assert_eq!(dropped, 1);
    assert_eq!(buf.len(), 2);
    // bad 被淘汰，两个 good 都在。
    assert!(!buf.iter().any(|dp| dp.point_id == "bad"));
    let good_count = buf
        .iter()
        .filter(|dp| dp.point_id == "good" || dp.point_id == "new_good")
        .count();
    assert_eq!(good_count, 2);
}

#[tokio::test]
async fn test_release_returns_items_to_queue() {
    let queue = DataQueue::new_memory().unwrap();

    queue.enqueue(vec![make_point("p1", 1.0)]).await.unwrap();

    let items = queue.dequeue(10).await.unwrap();
    assert_eq!(items.len(), 1);
    // dequeue 后 len 仍为 1（in_flight）
    assert_eq!(queue.len().await, 1);

    // release 不删除数据，使数据可被重新 dequeue
    let ids: Vec<i64> = items.iter().map(|i| i.id).collect();
    queue.release(ids).await;

    // 再次 dequeue 应能取到同一条
    let items2 = queue.dequeue(10).await.unwrap();
    assert_eq!(items2.len(), 1);
    assert_eq!(items2[0].id, items[0].id);
}

// ===== MemoryStorage 测试（无需 tempdir，纯内存） =====

#[tokio::test]
async fn test_memory_enqueue_dequeue_ack() {
    let queue = DataQueue::new_memory().unwrap();
    assert!(queue.is_empty().await);

    queue
        .enqueue(vec![make_point("p1", 1.0), make_point("p2", 2.0)])
        .await
        .unwrap();
    assert_eq!(queue.len().await, 2);

    let items = queue.dequeue(10).await.unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].point_id, "p1");
    assert_eq!(items[1].point_id, "p2");

    // dequeue 后仍在队列中（in_flight），ack 后才删除
    assert_eq!(queue.len().await, 2);

    let ids: Vec<i64> = items.iter().map(|i| i.id).collect();
    queue.ack(ids).await.unwrap();
    assert_eq!(queue.len().await, 0);
}

#[tokio::test]
async fn test_memory_dequeue_respects_limit() {
    let queue = DataQueue::new_memory().unwrap();

    let points: Vec<DataPoint> = (0..5)
        .map(|i| make_point(&format!("p{}", i), i as f64))
        .collect();
    queue.enqueue(points).await.unwrap();

    let first = queue.dequeue(2).await.unwrap();
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].point_id, "p0");

    // ack 第一批后再取第二批
    let ids: Vec<i64> = first.iter().map(|i| i.id).collect();
    queue.ack(ids).await.unwrap();

    let second = queue.dequeue(10).await.unwrap();
    assert_eq!(second.len(), 3);
    assert_eq!(second[0].point_id, "p2");
}

#[tokio::test]
async fn test_memory_history_query() {
    let queue = DataQueue::new_memory().unwrap();

    queue
        .enqueue(vec![make_point("p1", 1.0), make_point("p2", 2.0)])
        .await
        .unwrap();

    let hist = queue
        .history_ex("p1", 0, i64::MAX, 100, None)
        .await
        .unwrap();
    assert_eq!(hist.items.len(), 1);
    assert_eq!(hist.items[0].value, 1.0);
}

#[tokio::test]
async fn test_memory_release_returns_items_to_queue() {
    let queue = DataQueue::new_memory().unwrap();

    queue.enqueue(vec![make_point("p1", 1.0)]).await.unwrap();

    let items = queue.dequeue(10).await.unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(queue.len().await, 1);

    let ids: Vec<i64> = items.iter().map(|i| i.id).collect();
    queue.release(ids).await;

    let items2 = queue.dequeue(10).await.unwrap();
    assert_eq!(items2.len(), 1);
    assert_eq!(items2[0].id, items[0].id);
}

#[tokio::test]
async fn test_memory_disk_full_buffers_in_fallback() {
    // MemoryStorage 不受 disk_ok 影响，但 DataQueue 的 fallback 机制仍应工作
    let queue = DataQueue::new_memory().unwrap();

    queue.set_disk_ok(false);
    queue.enqueue(vec![make_point("p1", 1.0)]).await.unwrap();
    assert_eq!(queue.fallback_len(), 1);
    assert_eq!(queue.len().await, 0);

    queue.set_disk_ok(true);
    queue.enqueue(vec![make_point("p2", 2.0)]).await.unwrap();
    assert_eq!(queue.fallback_len(), 0);
    assert_eq!(queue.len().await, 2);
}

// ===== v2.0 Step 5: storage_days 配额驱动 FIFO 截断 =====

/// 构造指定时间戳的数据点（毫秒）
fn make_point_at(id: &str, value: f64, timestamp_ms: i64) -> DataPoint {
    DataPoint {
        timestamp: timestamp_ms,
        source_ts: timestamp_ms,
        ..make_point(id, value)
    }
}

#[test]
fn test_storage_retention_quota_truncates_old_records() {
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("queue.db");
    let storage = SqliteStorage::new(db_path.to_str().unwrap(), Arc::new(Metrics::new())).unwrap();

    // old：3 天前；recent：12 小时前
    let now = DataPoint::now();
    let old = make_point_at("p-old", 1.0, now - 3 * 24 * 3600 * 1000);
    let recent = make_point_at("p-recent", 2.0, now - 12 * 3600 * 1000);
    storage.enqueue_batch(&[old, recent]).unwrap();

    // 把 p-old 标记为"已送达"（delivered=1），p-recent 保持未送达（delivered=0）
    // —— 模拟"broker 已确认收到 p-old，p-recent 仍在飞行"的真实场景。
    // P0-3：retention 只删 delivered=1 的行，未送达行即使超期也保留。
    // 通过 dequeue/ack 路径实现 delivered=1：dequeue_batch 标记 in_flight=1，
    // 再调 release_in_flight 恢复 in_flight=0，但 delivered 仍为 0。
    // 直接走 storage.ack_batch 即可：ack_batch 当前实现是 UPDATE delivered=1, in_flight=0。
    let items = storage.dequeue_batch(10).unwrap();
    let old_id = items.iter().find(|i| i.point_id == "p-old").unwrap().id;
    storage.ack_batch(&[old_id]).unwrap();

    // 配额 1 天 → maintenance 截断后 p-old（已送达）删除，p-recent（未送达）保留
    storage.set_storage_retention_days(Some(1));
    storage.maintenance().unwrap();

    assert!(
        storage
            .history_query("p-old", 0, i64::MAX, 100)
            .unwrap()
            .is_empty(),
        "已送达 + 超期的 p-old 应被 retention 删除"
    );
    let kept = storage.history_query("p-recent", 0, i64::MAX, 100).unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].point_id, "p-recent");
}

/// P0-2 回归：`storage_days` 配额必须对**未送达**行同样生效。
///
/// 旧实现要求 `delivered = 1`，直连调试（没有北向消费者，delivered 恒为 0）时
/// 配额到期一条也删不掉 → license 商业约束空转、队列表无界增长。
/// 新契约：超期且**不在途**的行（无论是否已送达）到期即回收。
#[test]
fn test_storage_retention_prunes_unsent_old_records() {
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("queue.db");
    let storage = SqliteStorage::new(db_path.to_str().unwrap(), Arc::new(Metrics::new())).unwrap();

    // 3 天前的"未送达"数据（直连调试场景：没有任何组件会 ack）
    let now = DataPoint::now();
    let old_unsent = make_point_at("p-unsent", 1.0, now - 3 * 24 * 3600 * 1000);
    storage.enqueue_batch(&[old_unsent]).unwrap();

    // 配额 1 天 → maintenance 跑后超期未送达行必须被回收
    storage.set_storage_retention_days(Some(1));
    storage.maintenance().unwrap();

    assert!(
        storage
            .history_query("p-unsent", 0, i64::MAX, 100)
            .unwrap()
            .is_empty(),
        "P0-2：超期的未送达行应被 retention 回收，否则配额形同虚设"
    );
    assert_eq!(storage.len(), 0, "depth 必须同步扣减");
}

/// #20-2026：未送达行比例超限 → unsent_overflow() 返回 true。
/// 模拟 broker 长期不可用场景：连续入队大量未送达行，比例超过 80% 即触发。
#[test]
fn test_unsent_overflow_triggers_when_ratio_exceeded() {
    use crate::queue::storage::QueueStorage;
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("queue.db");
    let storage = SqliteStorage::new(db_path.to_str().unwrap(), Arc::new(Metrics::new())).unwrap();

    // 起始无未送达
    assert!(!storage.unsent_overflow());

    // 入队 10 条未送达行（broker 长期不可用 → 所有都是 in_flight=0/delivered=0）。
    // total=10、unsent=10、ratio=100% > 80% → 触发
    let now = DataPoint::now();
    let batch: Vec<DataPoint> = (0..10)
        .map(|i| make_point_at(&format!("u-{}", i), 1.0, now - 50))
        .collect();
    storage.enqueue_batch(&batch).unwrap();

    assert!(
        storage.unsent_overflow(),
        "10 条全部未送达（ratio=100%）必须触发 unsent_overflow"
    );
}

/// #20-2026：未送达行比例阈值边界验证。
/// ratio = 79% < 80% 不触发；ratio >= 80% 触发。
#[test]
fn test_unsent_overflow_threshold_boundary() {
    use crate::queue::storage::QueueStorage;
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("queue.db");
    let storage = SqliteStorage::new(db_path.to_str().unwrap(), Arc::new(Metrics::new())).unwrap();

    let now = DataPoint::now();

    // 场景 A：1 delivered + 3 unsent = ratio 75% → 不触发
    storage
        .enqueue_batch(&[make_point_at("d-1", 1.0, now - 200)])
        .unwrap();
    let d_items = storage.dequeue_batch(10).unwrap();
    storage
        .ack_batch(&d_items.iter().map(|i| i.id).collect::<Vec<_>>())
        .unwrap();
    storage
        .enqueue_batch(&[
            make_point_at("u-1", 1.0, now - 100),
            make_point_at("u-2", 1.0, now - 100),
            make_point_at("u-3", 1.0, now - 100),
        ])
        .unwrap();
    assert!(!storage.unsent_overflow(), "ratio 3/4 = 75% < 80% 不应触发");

    // 场景 B：再追加 1 条未送达 → ratio 4/5 = 80% → 触发
    storage
        .enqueue_batch(&[make_point_at("u-4", 1.0, now)])
        .unwrap();
    assert!(
        storage.unsent_overflow(),
        "ratio 4/5 = 80% >= 80% 阈值应触发"
    );
}

/// P0-3 配套回归：ack 之后的行**不得**再被 dequeue 选回。
///
/// ack 现行实现是 `UPDATE delivered = 1, in_flight = 0`（旧版为 DELETE），
/// 因此已送达行仍满足 `in_flight = 0`。若 dequeue 只按 in_flight 过滤，这些行
/// 会被反复出队重发（重复上报/重复计费）。本用例用真实 SQLite 后端钉住该语义
/// （`new_memory()` 的用例覆盖不到：内存后端 ack 时是真删）。
#[test]
fn test_storage_acked_rows_are_not_dequeued_again() {
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("queue.db");
    let storage = SqliteStorage::new(db_path.to_str().unwrap(), Arc::new(Metrics::new())).unwrap();

    let now = DataPoint::now();
    storage
        .enqueue_batch(&[make_point_at("p1", 1.0, now), make_point_at("p2", 2.0, now)])
        .unwrap();
    assert_eq!(storage.len(), 2);

    // 第一批全部出队并 ack
    let first = storage.dequeue_batch(10).unwrap();
    assert_eq!(first.len(), 2);
    storage
        .ack_batch(&first.iter().map(|i| i.id).collect::<Vec<_>>())
        .unwrap();

    // ack 后不得再被选回
    let second = storage.dequeue_batch(10).unwrap();
    assert!(
        second.is_empty(),
        "已 ack（delivered=1）的行不得被再次出队，实际取回 {} 行",
        second.len()
    );

    // depth 也随 ack 归零：语义是「尚未送达的行数」，不含等待 retention 回收的行
    assert_eq!(storage.len(), 0, "depth 不应把已送达行算进来");
}

#[test]
fn test_storage_retention_none_keeps_old_records() {
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("queue.db");
    let storage = SqliteStorage::new(db_path.to_str().unwrap(), Arc::new(Metrics::new())).unwrap();

    // 35 天前的数据：None = 配额不限，maintenance 后仍应保留
    let ancient = make_point_at("p-ancient", 1.0, DataPoint::now() - 35 * 24 * 3600 * 1000);
    storage.enqueue_batch(&[ancient]).unwrap();

    storage.set_storage_retention_days(None);
    storage.maintenance().unwrap();

    let kept = storage
        .history_query("p-ancient", 0, i64::MAX, 100)
        .unwrap();
    assert_eq!(
        kept.len(),
        1,
        "storage_days = None（不限）时不应按时间截断历史数据"
    );
}

/// r58-SD1 回归：`DataQueue::drain_and_wait` 是 graceful_shutdown 等待队列排空的
/// 唯一正确入口——内部循环监听 `QueueCommand::Drain` reply，storage 真空时
/// 才返回。本测试锁定这一语义，防未来"优化"（如改回手动 `len()` 轮询）。
#[tokio::test]
async fn r58_drain_and_wait_returns_when_queue_empty() {
    // 1) 空队列：drain_and_wait 必须立即返回，不得挂起。
    let queue = DataQueue::new_memory().unwrap();
    let r = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        queue.drain_and_wait(),
    )
    .await;
    assert!(
        r.is_ok(),
        "空队列 drain_and_wait 必须在 500ms 内返回（防挂起）"
    );
    assert!(queue.is_empty().await);

    // 2) 非空队列：drain_and_wait 阻塞等待，直到另一任务 dequeue + ack 排空。
    //    DataQueue 未实现 Clone，但其方法均取 &self，可用 Arc 跨 task 共享同一实例。
    let shared = std::sync::Arc::new(DataQueue::new_memory().unwrap());
    shared.enqueue(vec![make_point("p1", 1.0)]).await.unwrap();
    assert!(!shared.is_empty().await);

    let drainer = shared.clone();
    let drain_handle = tokio::spawn(async move {
        // 延迟 50ms 再排空，模拟"慢消费者"——用于证明 drain_and_wait 确实在等待。
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let items = drainer.dequeue(10).await.unwrap();
        assert_eq!(items.len(), 1);
        drainer
            .ack(items.iter().map(|i| i.id).collect())
            .await
            .unwrap();
    });

    let started = std::time::Instant::now();
    let r = tokio::time::timeout(std::time::Duration::from_secs(5), shared.drain_and_wait()).await;
    assert!(r.is_ok(), "队列被排空后 drain_and_wait 必须在 5s 内返回");
    // 关键断言：非空队列下必须真正阻塞等待（>= 消费任务的 50ms 延迟），
    // 而非在旧实现"立即返回"或"未排空即退出"的语义下提前返回。
    assert!(
        started.elapsed() >= std::time::Duration::from_millis(50),
        "drain_and_wait 在队列非空时必须阻塞等待排空，而非立即返回"
    );
    drain_handle.await.unwrap();
    assert!(shared.is_empty().await);
}

// ===== 批次 C：value_text（枚举文本标签）落库与往返 =====

/// 批次 C 往返测试：写入带 `value_text` 的点 → history 查询读回，标签一致。
///
/// 覆盖真实 SQLite 后端（`SqliteStorage`），锁定「标签随数据落库」这一契约。
#[test]
fn test_value_text_roundtrip_through_sqlite() {
    use crate::engine::metrics::Metrics;
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("queue.db");
    let storage = SqliteStorage::new(db_path.to_str().unwrap(), Arc::new(Metrics::new())).unwrap();

    let mut dp = make_point("p-enum", 1.0);
    dp.value_text = Some("运行".to_string());
    storage.enqueue_batch(&[dp]).unwrap();

    let items = storage.history_query("p-enum", 0, i64::MAX, 100).unwrap();
    assert_eq!(items.len(), 1, "应读到刚写入的行");
    assert_eq!(
        items[0].value_text.as_deref(),
        Some("运行"),
        "枚举标签应随数据落库并原样读回"
    );
}

/// 批次 C NULL 往返：写入不带标签的点 → 读出 `None`（NULL），
/// 不做规则回查、不填空串。
#[test]
fn test_value_text_null_roundtrip_through_sqlite() {
    use crate::engine::metrics::Metrics;
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("queue.db");
    let storage = SqliteStorage::new(db_path.to_str().unwrap(), Arc::new(Metrics::new())).unwrap();

    let dp = make_point("p-plain", 42.0);
    assert!(dp.value_text.is_none(), "测试前提：入队点本身无标签");
    storage.enqueue_batch(&[dp]).unwrap();

    let items = storage.history_query("p-plain", 0, i64::MAX, 100).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].value_text, None,
        "无标签的行应读出 None（NULL），不得现算规则回查"
    );
}

/// 批次 C：出队路径也必须带回标签（dequeue SELECT 与 history SELECT 保持一致），
/// 否则北向 MQTT 上报会丢标签。
#[test]
fn test_value_text_survives_dequeue() {
    use crate::engine::metrics::Metrics;
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("queue.db");
    let storage = SqliteStorage::new(db_path.to_str().unwrap(), Arc::new(Metrics::new())).unwrap();

    let mut dp = make_point("p-deq", 2.0);
    dp.value_text = Some("停机".to_string());
    storage.enqueue_batch(&[dp]).unwrap();

    let items = storage.dequeue_batch(10).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].value_text.as_deref(), Some("停机"));
}

/// 批次 C：内存后端透传标签（与 SQLite 后端语义一致）。
#[tokio::test]
async fn test_value_text_passthrough_in_memory_storage() {
    // value_text 只在原始行（QueuedItem）上存在，HistorySample 不携带；
    // 因此直接查内存存储的历史原语，验证落库/查询全程透传。
    let storage = MemoryStorage::new();
    let mut dp = make_point("p-mem", 1.0);
    dp.value_text = Some("运行".to_string());
    storage.enqueue_batch(&[dp]).unwrap();

    let items = storage.history_query("p-mem", 0, i64::MAX, 100).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].value_text.as_deref(), Some("运行"));
}

/// P0-1 / P0-3 回归：队列健康判定与告警沿语义。
///
/// 旧实现积压 14.9 万条时首页「今日告警」仍是 0：告警采样只在 MQTT 配置存在时
/// 启动，且只有单一 100_000 阈值。这里钉住两件事：
/// 1. `queue_health` 在有/无消费者时对同一长度给出正确等级；
/// 2. 告警只在等级变化沿触发一次，持续同一等级不重复计数。
#[test]
fn test_queue_health_levels_and_alarm_edge() {
    // 无消费者：存量增长属于预期（本地历史库），不得判为背压
    assert_eq!(queue_health(12, false), QueueHealth::Ok);
    assert_eq!(queue_health(BACKLOG_WARN_ROWS, false), QueueHealth::Backlog);
    assert_eq!(
        queue_health(BACKPRESSURE_ROWS * 2, false),
        QueueHealth::Backlog,
        "无北向消费者时 20 万行不应判为背压"
    );
    // 有消费者：超过背压阈值即故障
    assert_eq!(
        queue_health(BACKPRESSURE_ROWS, true),
        QueueHealth::Backpressure
    );
    // 逼近硬上限：与消费者无关，均为容量压力
    assert_eq!(
        queue_health(capacity_pressure_rows(), false),
        QueueHealth::CapacityPressure
    );
    // 就绪判定：预警态不降级（避免直连场景每个轮询都 503），严重态才降级
    assert!(QueueHealth::Backlog.is_ready());
    assert!(!QueueHealth::Backpressure.is_ready());

    // 告警沿：只在等级变化时触发一次
    assert_eq!(
        backlog_alarm_edge(QueueHealth::Ok, QueueHealth::Backlog),
        Some(QueueHealth::Backlog)
    );
    assert_eq!(
        backlog_alarm_edge(QueueHealth::Backlog, QueueHealth::Backpressure),
        Some(QueueHealth::Backpressure)
    );
    assert_eq!(
        backlog_alarm_edge(QueueHealth::Backpressure, QueueHealth::Backpressure),
        None,
        "同一等级持续期间不得重复计数"
    );
    assert_eq!(
        backlog_alarm_edge(QueueHealth::Backlog, QueueHealth::Ok),
        Some(QueueHealth::Ok)
    );
}

// ===== P2-1 / P2-3: 历史查询降采样 + 截断元信息 + 分钟聚合 writer =====

/// 构造一个临时库的 SqliteStorage（与 retention 测试同一套脚手架）
fn temp_sqlite() -> (tempfile::TempDir, SqliteStorage) {
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("queue.db");
    let storage = SqliteStorage::new(db_path.to_str().unwrap(), Arc::new(Metrics::new())).unwrap();
    (dir, storage)
}

/// P2-1③：limit 小于窗口内行数时，返回最新 limit 条并报告 `truncated/total`。
#[test]
fn test_history_ex_reports_truncation() {
    let (_dir, storage) = temp_sqlite();
    let now = DataPoint::now();
    let points: Vec<DataPoint> = (0..5)
        .map(|i| make_point_at("p-trunc", i as f64, now - (5 - i as i64) * 1000))
        .collect();
    storage.enqueue_batch(&points).unwrap();

    let result = storage
        .history_query_ex("p-trunc", now - 60_000, now, 3, None)
        .unwrap();
    assert_eq!(result.returned, 3, "limit 应只返回最新 3 条");
    assert_eq!(result.total, 5, "total 必须是窗口内匹配总数而不是返回数");
    assert!(result.truncated && result.has_more, "截断标记必须置位");
    assert_eq!(result.granularity, "raw");
    // 保留最新 3 条（值 2/3/4），且按时间升序
    let values: Vec<f64> = result.items.iter().map(|s| s.value).collect();
    assert_eq!(values, vec![2.0, 3.0, 4.0]);
    assert!(result.items.iter().all(|s| s.sample_count == 1));

    // 未截断时 total = returned，truncated = false
    let full = storage
        .history_query_ex("p-trunc", now - 60_000, now, 10, None)
        .unwrap();
    assert_eq!(full.returned, 5);
    assert_eq!(full.total, 5);
    assert!(!full.truncated && !full.has_more);
}

/// P2-1①：`downsample` 按时间桶聚合（avg/min/max/count），桶起点对齐。
#[test]
fn test_history_ex_downsample_buckets() {
    let (_dir, storage) = temp_sqlite();
    // 锚定到整分钟，避免桶边界抖动
    let base = (DataPoint::now() / 60_000) * 60_000;
    let points = vec![
        make_point_at("p-agg", 1.0, base + 1_000),
        make_point_at("p-agg", 3.0, base + 30_000),
        make_point_at("p-agg", 5.0, base + 59_000),
        make_point_at("p-agg", 10.0, base + 60_001),
    ];
    storage.enqueue_batch(&points).unwrap();

    let result = storage
        .history_query_ex("p-agg", base, base + 120_000, 100, Some(60))
        .unwrap();
    assert_eq!(result.granularity, "second:60");
    assert_eq!(result.items.len(), 2, "两分钟数据应压成 2 个桶");
    assert_eq!(result.total, 2);
    assert!(!result.truncated);

    let first = &result.items[0];
    assert_eq!(first.timestamp, base, "桶起点必须对齐到分钟");
    assert_eq!(first.value, 3.0, "avg = (1+3+5)/3");
    assert_eq!(first.min_value, Some(1.0));
    assert_eq!(first.max_value, Some(5.0));
    assert_eq!(first.sample_count, 3);
    assert_eq!(first.quality, "aggregated");
    assert!(first.raw_value.is_none());

    let second = &result.items[1];
    assert_eq!(second.timestamp, base + 60_000);
    assert_eq!(second.sample_count, 1);
}

/// P2-3：`>24h` 的原始行写入聚合表，24h 内的数据保持原始精度。
#[test]
fn test_aggregate_minute_windows_compresses_old_rows() {
    let (_dir, storage) = temp_sqlite();
    let now = DataPoint::now();
    // 对齐到分钟起点：+1s/+2s 必须落在同一分钟桶内，否则测试会随墙钟秒数偶发失败
    let old_ts = (now - 48 * 3600 * 1000) / 60_000 * 60_000; // 2 天前
    let recent_ts = now - 3600 * 1000; // 1 小时前
    storage
        .enqueue_batch(&[
            make_point_at("p-w", 1.0, old_ts),
            make_point_at("p-w", 2.0, old_ts + 1_000),
            make_point_at("p-w", 3.0, old_ts + 2_000),
            make_point_at("p-w", 9.0, recent_ts),
        ])
        .unwrap();

    // maintenance 每 2 ticks 才聚合一次（第 1 次只递增计数），故调用两次
    storage.maintenance().unwrap();
    storage.maintenance().unwrap();

    // 跨 24h 的宽窗口 + 分钟桶：旧段读聚合表、新段读原始行。
    // 若 writer 未写入聚合表，旧段将查不到任何桶（只有 1 条 item）。
    let result = storage
        .history_query_ex("p-w", now - 72 * 3600 * 1000, now, 100, Some(60))
        .unwrap();
    assert_eq!(result.items.len(), 2, "旧段 1 桶 + 新段 1 桶");
    assert_eq!(
        result.items[0].sample_count, 3,
        "旧段来自聚合表，3 条旧行同一分钟"
    );
    assert_eq!(result.items[0].value, 2.0, "聚合表 avg = (1+2+3)/3");
    assert_eq!(result.items[0].min_value, Some(1.0));
    assert_eq!(result.items[0].max_value, Some(3.0));
    assert_eq!(
        result.items[1].sample_count, 1,
        "新段来自原始行（24h 内不聚合）"
    );
    assert_eq!(result.items[1].value, 9.0);
}

/// P2-3 边界：聚合表写入具备幂等性——重复维护不会重复计数。
#[test]
fn test_aggregate_minute_windows_is_idempotent() {
    let (_dir, storage) = temp_sqlite();
    let now = DataPoint::now();
    // 对齐到分钟起点：+5s 必须落在同一分钟桶内，否则断言「只有一行」会随墙钟秒数偶发失败
    let old_ts = (now - 30 * 3600 * 1000) / 60_000 * 60_000;
    storage
        .enqueue_batch(&[
            make_point_at("p-idem", 1.0, old_ts),
            make_point_at("p-idem", 2.0, old_ts + 5_000),
        ])
        .unwrap();

    storage.maintenance().unwrap();
    storage.maintenance().unwrap();
    // 再跑两轮：边界桶会被重算，但样本数不得被重复累加
    storage.maintenance().unwrap();
    storage.maintenance().unwrap();

    let result = storage
        .history_query_ex("p-idem", now - 48 * 3600 * 1000, now, 100, Some(60))
        .unwrap();
    assert_eq!(result.items.len(), 1, "同一分钟桶必须只有一行");
    assert_eq!(
        result.items[0].sample_count, 2,
        "重复聚合不得改变桶内样本数"
    );
    assert_eq!(result.items[0].value, 1.5);
}
