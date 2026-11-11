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
    // 唯一被淘汰的就是 bad——新点最终留下来了，retained 必须是 1。
    // （曾经用 n_in - dropped 计算，会把 1-1=0 报成「本批全丢」，日志误导运维。）
    let mut new_good = make_point("new_good", 9.0);
    new_good.quality = Quality::Good;
    let (retained, dropped) = push_into_fallback(&mut buf, 2, vec![new_good], &mut retry);
    assert_eq!(retained, 1, "被淘汰的是旧点，本批新点仍在缓冲里");
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

// ===== 保留期（固定 30 天）驱动的 FIFO 截断 =====

/// 构造指定时间戳的数据点（毫秒）
fn make_point_at(id: &str, value: f64, timestamp_ms: i64) -> DataPoint {
    DataPoint {
        timestamp: timestamp_ms,
        source_ts: timestamp_ms,
        ..make_point(id, value)
    }
}

#[test]
fn test_storage_dedups_same_point_and_timestamp() {
    use crate::queue::storage::QueueStorage;
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("queue.db");
    let storage = SqliteStorage::new(db_path.to_str().unwrap(), Arc::new(Metrics::new())).unwrap();

    let now = DataPoint::now();
    // 同一批内重复：(p1, now) 出现两次
    let inserted = storage
        .enqueue_batch(&[
            make_point_at("p1", 1.0, now),
            make_point_at("p1", 9.0, now),
            make_point_at("p2", 2.0, now),
        ])
        .unwrap();
    assert_eq!(inserted, 2, "P2-1：重复 (point_id, timestamp) 只能落一行");
    assert_eq!(storage.len(), 2, "depth 必须按实际插入行数累加");

    // 跨批重复：再入队同一 (p1, now)，不得新增行、不得报错
    let inserted2 = storage
        .enqueue_batch(&[make_point_at("p1", 3.0, now)])
        .unwrap();
    assert_eq!(inserted2, 0, "重复入队应被唯一约束静默跳过");
    assert_eq!(storage.len(), 2, "重复入队不得增加深度");

    // 同一点位不同时间戳是合法的新数据
    let inserted3 = storage
        .enqueue_batch(&[make_point_at("p1", 4.0, now + 1)])
        .unwrap();
    assert_eq!(inserted3, 1);
    assert_eq!(storage.len(), 3);
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

/// v2.0 配额用量口径：`oldest_data_timestamp` 返回最旧一条持久化数据的时间戳。
///
/// 空库 → None（用量按 0 计）；有数据 → 最旧那条的时间戳（与保留天数配额同源）。
#[test]
fn test_oldest_data_timestamp_reflects_oldest_row() {
    use crate::queue::storage::QueueStorage;
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("queue.db");
    let storage = SqliteStorage::new(db_path.to_str().unwrap(), Arc::new(Metrics::new())).unwrap();

    // 空库 → None
    assert_eq!(storage.oldest_data_timestamp(), None);

    let now = DataPoint::now();
    let old_ts = now - 5 * 24 * 3600 * 1000;
    storage
        .enqueue_batch(&[
            make_point_at("p-old", 1.0, old_ts),
            make_point_at("p-new", 2.0, now - 3600 * 1000),
        ])
        .unwrap();

    assert_eq!(
        storage.oldest_data_timestamp(),
        Some(old_ts),
        "应返回最旧一条数据的时间戳"
    );
}

/// v2.0：内存后端无持久化历史 → 恒为 None（用量按 0 计，不报错）。
#[tokio::test]
async fn test_oldest_data_timestamp_memory_backend_is_none() {
    let queue = DataQueue::new_memory().unwrap();
    queue.enqueue(vec![make_point("p1", 1.0)]).await.unwrap();
    assert_eq!(queue.oldest_data_timestamp().await, None);
}

/// `DataQueue` 的包装层必须如实透传存储后端的时间戳。
///
/// 单测 `SqliteStorage` 只能证明后端本身正确；包装层若写成恒返回 `None`，
/// 配额用量会被永久判成 0，商业侧的保留天数约束就形同失效。
#[tokio::test]
async fn test_dataqueue_oldest_data_timestamp_forwards_storage_value() {
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("queue_forward.db");
    let storage = SqliteStorage::new(db_path.to_str().unwrap(), Arc::new(Metrics::new())).unwrap();
    let queue = DataQueue::with_storage(Arc::new(storage)).unwrap();

    let ts = DataPoint::now() - 86_400_000;
    queue
        .enqueue(vec![make_point_at("p-old", 1.0, ts)])
        .await
        .unwrap();

    assert_eq!(
        queue.oldest_data_timestamp().await,
        Some(ts),
        "包装层应透传存储后端的最旧时间戳，不能恒为 None"
    );
}

/// 磁盘可用标志的读-写往返：两个方向都要能翻转。
///
/// 只断言默认值 `true` 无法区分「真实读取」与「恒返回 true」，
/// 降级路径（磁盘压力时停止写库）就可能在实现被改坏后静默失效。
#[tokio::test]
async fn test_disk_ok_flag_round_trip() {
    let queue = DataQueue::new_memory().unwrap();
    assert!(queue.is_disk_ok(), "默认应允许写库");

    queue.set_disk_ok(false);
    assert!(
        !queue.is_disk_ok(),
        "置为 false 后必须能读到 false，否则降级路径永不生效"
    );

    queue.set_disk_ok(true);
    assert!(queue.is_disk_ok(), "恢复后必须能读到 true");
}

/// 行数上限的读-写往返，且小于 1 的输入必须被夹到 1。
///
/// 上限为 0 会让存储层每次写入都触发全表淘汰；这里把夹取行为钉死。
#[tokio::test]
async fn test_max_rows_set_get_and_clamp() {
    let queue = DataQueue::new_memory().unwrap();

    queue.set_max_rows(4321);
    assert_eq!(queue.max_rows(), 4321, "设置后应读回同一值");

    queue.set_max_rows(0);
    assert_eq!(queue.max_rows(), 1, "0 必须被夹到 1，避免每次写入都全表淘汰");

    queue.set_max_rows(-9);
    assert_eq!(queue.max_rows(), 1, "负数同样必须夹到 1");
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
    assert_eq!(
        queue_health(12, DEFAULT_MAX_QUEUE_ROWS, false),
        QueueHealth::Ok
    );
    assert_eq!(
        queue_health(BACKLOG_WARN_ROWS, DEFAULT_MAX_QUEUE_ROWS, false),
        QueueHealth::Backlog
    );
    assert_eq!(
        queue_health(BACKPRESSURE_ROWS * 2, DEFAULT_MAX_QUEUE_ROWS, false),
        QueueHealth::Backlog,
        "无北向消费者时 20 万行不应判为背压"
    );
    // 有消费者：超过背压阈值即故障
    assert_eq!(
        queue_health(BACKPRESSURE_ROWS, DEFAULT_MAX_QUEUE_ROWS, true),
        QueueHealth::Backpressure
    );
    // 逼近硬上限：与消费者无关，均为容量压力
    assert_eq!(
        queue_health(
            capacity_pressure_rows(DEFAULT_MAX_QUEUE_ROWS),
            DEFAULT_MAX_QUEUE_ROWS,
            false
        ),
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

/// 配置化回归（P1）：把行数上限改成配置项后，四级水位顺序必须在**任意合法
/// 上限**下都成立；越界/失序的上限必须被配置校验拒绝，而不是运行期静默错判。
#[test]
fn configurable_max_rows_keeps_threshold_order() {
    for max_rows in [
        MIN_QUEUE_ROWS,
        500_000,
        DEFAULT_MAX_QUEUE_ROWS,
        MAX_QUEUE_ROWS,
    ] {
        assert!(
            max_rows_errors(max_rows).is_empty(),
            "合法上限 {max_rows} 不应被拒: {:?}",
            max_rows_errors(max_rows)
        );
        let pressure = capacity_pressure_rows(max_rows);
        assert!(
            pressure > BACKPRESSURE_ROWS,
            "上限 {max_rows} 的容量压力线 {pressure} 必须高于背压线 {BACKPRESSURE_ROWS}"
        );
        // 压力线上一点：必须是容量压力（与消费者无关）
        assert_eq!(
            queue_health(pressure, max_rows, true),
            QueueHealth::CapacityPressure
        );
        // 压力线下一行：有消费者时回落为背压，说明水位不会越级
        assert_eq!(
            queue_health(pressure - 1, max_rows, true),
            QueueHealth::Backpressure
        );
    }
    // 区间外：低于下界（会把压力线压到背压线以下）/ 高于上界，都必须拒绝
    assert!(!max_rows_errors(MIN_QUEUE_ROWS - 1).is_empty());
    assert!(!max_rows_errors(0).is_empty());
    assert!(!max_rows_errors(MAX_QUEUE_ROWS + 1).is_empty());
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

// ===== P2-9：磁盘满 → 恢复的端到端契约 =====

/// 磁盘满期间的点全部进内存缓冲（一行不写库），恢复后全部落盘：
/// 不丢、不重、顺序保持。
///
/// 反证闭环：把 `flush_fallback` 里的 `drain` 改成 `clone`（不清空缓冲）→
/// 缓冲长度断言失败，且出队会出现重复 id。
#[tokio::test]
async fn p2_disk_full_recover_flushes_all_points_without_duplication() {
    let queue = DataQueue::new_memory().unwrap();
    queue.set_disk_ok(false);

    // 3 批 × 2 点：磁盘满期间只能进 fallback
    for batch in 0..3 {
        queue
            .enqueue(vec![
                make_point(&format!("p{batch}a"), batch as f64),
                make_point(&format!("p{batch}b"), batch as f64),
            ])
            .await
            .unwrap();
    }
    assert_eq!(queue.fallback_len(), 6, "磁盘满期间的点必须全部进内存缓冲");
    assert_eq!(queue.len().await, 0, "磁盘满期间不得写入存储");

    // 恢复后的第一次 enqueue 会先 flush_fallback，再写本批
    queue.set_disk_ok(true);
    queue.enqueue(vec![make_point("p3", 3.0)]).await.unwrap();
    assert_eq!(queue.fallback_len(), 0, "恢复后缓冲必须清空");
    assert_eq!(queue.len().await, 7, "缓冲 6 个 + 新 1 个，一个都不能少");

    let items = queue.dequeue(20).await.unwrap();
    assert_eq!(items.len(), 7);
    let order: Vec<&str> = items.iter().map(|i| i.point_id.as_str()).collect();
    assert_eq!(
        order,
        vec!["p0a", "p0b", "p1a", "p1b", "p2a", "p2b", "p3"],
        "恢复后必须按缓冲顺序先落盘，再做新数据"
    );

    let ids: Vec<i64> = items.iter().map(|i| i.id).collect();
    let unique: std::collections::HashSet<i64> = ids.iter().copied().collect();
    assert_eq!(unique.len(), 7, "不得重复出队（flush 不能把同一批发两遍）");

    queue.ack(ids).await.unwrap();
    assert_eq!(queue.len().await, 0, "ack 后不得留下幽灵行");
}

/// fallback 超容量时按优先级淘汰：低优先级（Bad / stale_count 高）先被踢，
/// 高优先级保留。这里用小容量直接驱动 `push_into_fallback`，
/// 避免为测 5000 上限而灌 5001 个点。
///
/// 反证闭环：把 `find_lowest_priority_idx` 换成 `pop_front` → 保留集合里会出现 Bad 点。
#[test]
fn p2_fallback_pressure_evicts_lowest_priority_first() {
    use super::push_into_fallback;
    use std::collections::VecDeque;

    let mut buf: VecDeque<DataPoint> = VecDeque::new();
    let mut retry = 0u32;

    let bad: Vec<DataPoint> = (0..5)
        .map(|i| {
            let mut p = make_point(&format!("bad{i}"), 0.0);
            p.quality = Quality::Bad;
            p
        })
        .collect();
    let good: Vec<DataPoint> = (0..5)
        .map(|i| make_point(&format!("good{i}"), 1.0))
        .collect();

    let (retained1, dropped1) = push_into_fallback(&mut buf, 10, bad, &mut retry);
    assert_eq!((retained1, dropped1), (5, 0), "容量 10 装 5 个不该淘汰");

    // 容量收紧到 5：新增 5 个 Good 后总长 10，必须淘汰 5 个
    let (_retained2, dropped2) = push_into_fallback(&mut buf, 5, good, &mut retry);
    assert_eq!(dropped2, 5, "超容量必须淘汰 5 个");
    assert_eq!(buf.len(), 5, "淘汰后长度必须回到容量上限");
    assert!(
        buf.iter().all(|p| p.quality == Quality::Good),
        "留下的必须全是高优先级，实际: {:?}",
        buf.iter()
            .map(|p| (&p.point_id, p.quality))
            .collect::<Vec<_>>()
    );
}

/// enqueue 走 fallback 且超容量时，被踢出的点数必须计入
/// `gateway_queue_persist_dropped_total`——否则真丢数据在监控上完全不可见。
#[tokio::test]
async fn p2_disk_full_overflow_counts_dropped_metric() {
    use super::FALLBACK_CAPACITY;
    use crate::engine::metrics::Metrics;
    use std::sync::Arc;

    let queue = DataQueue::new_memory().unwrap();
    let metrics = Arc::new(Metrics::new());
    queue.set_metrics(metrics.clone());
    queue.set_disk_ok(false);

    // 1 个 Bad + 恰好满容量的 Good → 溢出的 1 个必须是最低优先级的 Bad
    let mut points = vec![{
        let mut p = make_point("must-drop", 0.0);
        p.quality = Quality::Bad;
        p
    }];
    points.extend((0..FALLBACK_CAPACITY).map(|i| make_point(&format!("g{i}"), 1.0)));

    queue.enqueue(points).await.unwrap();
    assert_eq!(
        queue.fallback_len(),
        FALLBACK_CAPACITY,
        "缓冲不得超过容量上限"
    );
    assert_eq!(
        metrics
            .queue_persist
            .persist_dropped
            .load(std::sync::atomic::Ordering::Relaxed),
        1,
        "被容量淘汰的 1 个点必须计入 dropped_total"
    );
}

// ===== P2-9：fallback 反弹上限（FALLBACK_MAX_RETRIES）的端到端时序 =====
//
// 既有 `test_fallback_bounce_cap_drops_batch` 只覆盖纯函数的计数语义，
// 没有覆盖真实 `queue_task` 的写失败分支。本组用例用「可切换失败的存储替身」
// 真实驱动该分支，钉住三个现场可观测的行为：
//   1. 预算耗尽前不丢任何数据；
//   2. 耗尽那一次的写尝试把**积压整批**丢弃，并计入 dropped / bounce 指标；
//   3. 成功写入会复位预算，瞬时故障不会长期污染后续的重试机会。

/// 可切换写失败的存储替身。
///
/// 只拦截 `enqueue_batch`，其余方法全部委托给 `MemoryStorage`，
/// 保证 len / dequeue 等观测路径与生产后端行为一致。
struct FlakyEnqueueStorage {
    inner: MemoryStorage,
    fail: std::sync::atomic::AtomicBool,
    enqueue_calls: std::sync::atomic::AtomicUsize,
    maintenance_calls: std::sync::atomic::AtomicUsize,
}

impl FlakyEnqueueStorage {
    fn new() -> Self {
        Self {
            inner: MemoryStorage::new(),
            fail: std::sync::atomic::AtomicBool::new(true),
            enqueue_calls: std::sync::atomic::AtomicUsize::new(0),
            maintenance_calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn recover(&self) {
        self.fail.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    fn break_again(&self) {
        self.fail.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// 等到 `queue_task` 的首次维护 tick 真正执行完。
    ///
    /// `tokio::time::interval` 的首个 tick 立即就绪，而该分支也会 drain fallback
    /// 去写库。若让它在第一轮 enqueue 之后才抢到运行权，就会替我们多消耗一次
    /// 反弹预算，使次数断言不可复现。`needs_maintenance()` 返回 true 后维护调用
    /// 可被计数，因此无需 sleep 猜时序。
    async fn wait_first_maintenance_tick(&self) {
        for _ in 0..400 {
            if self
                .maintenance_calls
                .load(std::sync::atomic::Ordering::SeqCst)
                >= 1
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("queue_task 的首次维护 tick 未在 2s 内执行");
    }
}

impl QueueStorage for FlakyEnqueueStorage {
    fn enqueue_batch(&self, points: &[DataPoint]) -> crate::engine::error::GatewayResult<usize> {
        self.enqueue_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
            // 文案刻意避开 disk full / I/O error：走 fallback 反弹计数路径，
            // 而不是「磁盘满 30s 退避」路径。
            return Err(crate::engine::error::GatewayError::Queue(
                "test: simulated persist failure".into(),
            ));
        }
        self.inner.enqueue_batch(points)
    }

    fn dequeue_batch(&self, limit: usize) -> crate::engine::error::GatewayResult<Vec<QueuedItem>> {
        self.inner.dequeue_batch(limit)
    }

    fn ack_batch(&self, ids: &[i64]) -> crate::engine::error::GatewayResult<()> {
        self.inner.ack_batch(ids)
    }

    fn release_in_flight(&self, ids: &[i64]) -> crate::engine::error::GatewayResult<()> {
        self.inner.release_in_flight(ids)
    }

    fn cleanup_stale_in_flight(
        &self,
        cutoff_ts: i64,
    ) -> crate::engine::error::GatewayResult<usize> {
        self.inner.cleanup_stale_in_flight(cutoff_ts)
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn in_flight_count(&self) -> i64 {
        self.inner.in_flight_count()
    }

    fn history_query(
        &self,
        point_id: &str,
        start: i64,
        end: i64,
        limit: usize,
    ) -> crate::engine::error::GatewayResult<Vec<QueuedItem>> {
        self.inner.history_query(point_id, start, end, limit)
    }

    fn maintenance(&self) -> crate::engine::error::GatewayResult<()> {
        self.maintenance_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.maintenance()
    }

    fn needs_maintenance(&self) -> bool {
        // 置为 true 才能让维护 tick 有可观测的副作用（供测试握手）
        true
    }

    fn name(&self) -> &str {
        "flaky-enqueue"
    }
}

/// P2-9：预算耗尽时序。前 3 轮（共 5 次写尝试）一条不丢；第 4 轮 flush
/// 把积压整批重写时预算已耗尽 → 整批丢弃并计数，随后新点重新获得预算。
#[tokio::test]
async fn p2_fallback_retry_budget_exhausts_and_drops_accumulated_batch() {
    use crate::engine::metrics::Metrics;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    let storage = Arc::new(FlakyEnqueueStorage::new());
    let queue = DataQueue::with_storage(storage.clone()).unwrap();
    let metrics = Arc::new(Metrics::new());
    queue.set_metrics(metrics.clone());
    // 磁盘标志默认为 true：数据全部经 queue_task 写库，命中写失败分支
    assert!(queue.is_disk_ok(), "默认应走正常写库路径");
    // queue_task 的 30s 维护定时器用 `tokio::time::interval`，首次 tick 立即就绪，
    // 且该分支也会 drain fallback 去写库——必须先确认它已执行完，否则它会替我们
    // 额外消耗一次反弹预算，使下面的次数断言不可复现。
    storage.wait_first_maintenance_tick().await;
    assert_eq!(
        storage.enqueue_calls.load(Ordering::SeqCst),
        0,
        "首次维护 tick 时缓冲为空，不应产生写尝试"
    );

    // 第 1~3 轮共 5 次写尝试（1 + 2 + 2），恰好耗尽 FALLBACK_MAX_RETRIES；
    // 此期间一条都不能丢。
    for (round, id) in ["a", "b", "c"].iter().enumerate() {
        queue
            .enqueue(vec![make_point(id, round as f64)])
            .await
            .unwrap();
        // Len 与 Enqueue 走同一条 mpsc 且 FIFO：拿到 reply 即代表此前命令
        // （含 spawn_blocking 写库）全部处理完毕，无需 sleep 等时序。
        let _ = queue.len().await;
        assert_eq!(
            metrics
                .queue_persist
                .persist_dropped
                .load(Ordering::Relaxed),
            0,
            "第 {} 轮：预算未耗尽前不得丢弃任何数据",
            round + 1
        );
        assert_eq!(
            queue.fallback_len(),
            round + 1,
            "第 {} 轮：新点应进入 fallback 缓冲",
            round + 1
        );
    }
    assert_eq!(
        storage.enqueue_calls.load(Ordering::SeqCst),
        5,
        "前 3 轮应产生 5 次写尝试（每轮都先 flush 积压）"
    );

    // 第 4 轮：预算已耗尽 → 本轮的 flush（积压 3 条）被整批丢弃；
    // 随后本批新点再失败，从 0 重新计数，因此仍能进缓冲。
    queue.enqueue(vec![make_point("d", 3.0)]).await.unwrap();
    let _ = queue.len().await;

    assert_eq!(
        metrics
            .queue_persist
            .persist_dropped
            .load(Ordering::Relaxed),
        3,
        "积压的 3 条必须在预算耗尽时整批计入 dropped_total"
    );
    assert_eq!(
        metrics
            .queue_persist
            .fallback_bounce
            .load(Ordering::Relaxed),
        1,
        "反弹事件每次耗尽只记一次"
    );
    assert_eq!(
        metrics.queue_persist.persist_failed.load(Ordering::Relaxed),
        7,
        "写失败计数必须包含被丢弃的那一批（1+2+2+2）"
    );
    assert_eq!(queue.fallback_len(), 1, "只剩本轮新点，积压已被丢弃");

    // 恢复存储：缓冲里的新点落盘，且能查回 → 证明丢的是积压批，不是全部
    storage.recover();
    queue.flush_fallback().await.unwrap();
    assert_eq!(queue.fallback_len(), 0, "恢复后缓冲应清空");
    let items = queue.dequeue(10).await.unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].point_id, "d", "被丢弃的是积压批，新点必须保留");
}

/// P2-9：成功写入必须复位反弹计数。否则一次瞬时故障烧掉的预算会让之后
/// （相隔很久的）单次失败立刻丢数据，把「病态反弹保护」变成长期丢数开关。
#[tokio::test]
async fn p2_fallback_retry_budget_resets_after_successful_write() {
    use crate::engine::metrics::Metrics;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    let storage = Arc::new(FlakyEnqueueStorage::new());
    let queue = DataQueue::with_storage(storage.clone()).unwrap();
    let metrics = Arc::new(Metrics::new());
    queue.set_metrics(metrics.clone());
    // 同上前置：先确认 30s 维护定时器的首次立即 tick 已执行完，避免它替我们
    // 消耗反弹预算
    storage.wait_first_maintenance_tick().await;
    assert_eq!(storage.enqueue_calls.load(Ordering::SeqCst), 0);

    // 前 3 轮把预算烧到上限（retry == FALLBACK_MAX_RETRIES）
    for (round, id) in ["a", "b", "c"].iter().enumerate() {
        queue
            .enqueue(vec![make_point(id, round as f64)])
            .await
            .unwrap();
        let _ = queue.len().await;
    }
    assert_eq!(queue.fallback_len(), 3);

    // 恢复后成功写一次：积压 3 条 + 新点 1 条全部落盘，计数复位
    storage.recover();
    queue.enqueue(vec![make_point("d", 3.0)]).await.unwrap();
    assert_eq!(queue.len().await, 4, "积压与新点都应成功落盘");
    assert_eq!(queue.fallback_len(), 0);

    // 再次故障：若计数未复位，这次失败会立即被丢弃；复位后必须照常进缓冲
    storage.break_again();
    queue.enqueue(vec![make_point("e", 4.0)]).await.unwrap();
    let _ = queue.len().await;
    assert_eq!(
        metrics
            .queue_persist
            .persist_dropped
            .load(Ordering::Relaxed),
        0,
        "瞬时故障恢复后，新的失败必须重新获得完整重试预算"
    );
    assert_eq!(queue.fallback_len(), 1, "新失败的点应进入缓冲而非被丢弃");
}

/// 回归：批量 INSERT 的占位符串按行数预拼接（`insert_sql_for_rows`），
/// 必须覆盖跨 chunk 边界的各种行数组合——满块 50 行、余块、以及 50/50/23
/// 这种多块混合。若某行数的预拼接串与实际绑定参数个数不一致，
/// `tx.execute` 会失败并静默回退逐行插入（吞吐塌陷）或直接丢行。
#[test]
fn test_bulk_insert_covers_chunk_boundary_row_counts() {
    use crate::engine::metrics::Metrics;
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("queue.db");
    let storage = SqliteStorage::new(db_path.to_str().unwrap(), Arc::new(Metrics::new())).unwrap();

    // 行数覆盖：1 / 满块 50 / 满块+1 / 跨三块 123
    for rows in [1usize, 50, 51, 123] {
        let points: Vec<DataPoint> = (0..rows)
            .map(|i| make_point(&format!("p-{}-{}", rows, i), i as f64))
            .collect();

        let inserted = storage.enqueue_batch(&points).unwrap();
        assert_eq!(
            inserted, rows,
            "rows={} 时全部行都应插入成功（预拼接占位符数必须与绑定参数数一致）",
            rows
        );
    }

    // 总行数校验：1 + 50 + 51 + 123
    assert_eq!(storage.len(), 225, "四批合计应落库 225 行");

    // 值回读校验（抽查首尾，确认列序未错位）
    let items = storage.history_query("p-123-0", 0, i64::MAX, 10).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].value, 0.0);
    let items = storage.history_query("p-123-122", 0, i64::MAX, 10).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].value, 122.0);
}

// ===== snapshot 上报：折叠命令 + 周期 cutoff（actor 级） =====

/// `CollapseSnapshot` 命令必须走通 actor 并回传折叠行数 / 周期上界；
/// 同一 cutoff 下重复出队不得再取到任何行，排空期间新到的样本留到下个周期。
#[tokio::test]
async fn snapshot_collapse_command_bounds_one_cycle() {
    use crate::queue::storage::QueueStorage;
    use std::sync::Arc;

    let (_dir, storage) = temp_sqlite();
    let storage: Arc<dyn QueueStorage> = Arc::new(storage);
    let queue = DataQueue::with_storage(storage.clone()).unwrap();

    // 必须用「当下」附近的时间戳：actor 会定期跑 retention（30 天），
    // 1970 年量级的固定时间戳会被当成过期行直接回收。
    let base = DataPoint::now() - 60_000;
    // 2 个点位 × 3 拍（时间戳错开，避开同毫秒去重索引）
    for shot in 0..3i64 {
        let points: Vec<DataPoint> = (0..2)
            .map(|i| make_point_at(&format!("p{}", i), (shot * 10 + i) as f64, base + shot))
            .collect();
        storage.enqueue_batch(&points).expect("enqueue");
    }

    let collapse = queue.collapse_snapshot().await.expect("collapse command");
    assert_eq!(collapse.superseded, 4, "6 行按点位只留最新 2 行");

    let items = queue
        .dequeue_snapshot(10, collapse.cutoff)
        .await
        .expect("dequeue snapshot");
    let mut values: Vec<f64> = items.iter().map(|i| i.value).collect();
    values.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    assert_eq!(values, vec![20.0, 21.0], "只应发送最新一拍");

    // 同一周期再排一批：cutoff 内已无未送达行（在途行也会被排除）
    assert!(
        queue
            .dequeue_snapshot(10, collapse.cutoff)
            .await
            .expect("second drain")
            .is_empty(),
        "同一周期不得重复上行同一点位"
    );

    // 排空期间新到的样本（id > cutoff）本周期不发
    storage
        .enqueue_batch(&[make_point_at("p0", 99.0, base + 10)])
        .expect("enqueue late sample");
    assert!(
        queue
            .dequeue_snapshot(10, collapse.cutoff)
            .await
            .expect("drain after late sample")
            .is_empty(),
        "cutoff 之后的样本必须留到下个周期"
    );

    // 下个周期重新折叠 + 新 cutoff 才能取到
    let next = queue.collapse_snapshot().await.expect("next collapse");
    let items = queue
        .dequeue_snapshot(10, next.cutoff)
        .await
        .expect("next cycle drain");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].value, 99.0);
}
