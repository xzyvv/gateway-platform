//! 北向第 5 层队列（`NorthFeed`）的两条**行为**契约（不只是计数）。
//!
//! 1. 背压策略：队列满时 `send` 挂起等待空位（阻塞），既不丢最旧也不丢最新，
//!    且投递顺序保持 FIFO。本文件用「生产任务是否真的停住 + 批次内容是否原样」
//!    来断言策略，而不是只看 `backpressure_events > 0`。
//! 2. 断线收敛：北向上行中断（消费者停摆）时，积压封顶在容量、生产端被挂起；
//!    链路恢复后积压回落至 0，全部批次按序恰好投递一次（不丢、不重）。
//!
//! 与 MQTT 协议层的断线重连（`tests/chaos/scenarios.rs` 场景 26：broker 主动断开
//! → 客户端退避重连 → outbox replay → 队列排空）互补：本文件锁定**队列这一层**
//! 的收敛性，不依赖真实 broker，可在常规 CI（非 chaos job）中稳定运行。

use std::sync::Arc;
use std::time::Duration;

use gateway_core::engine::model::DataPoint;
use gateway_core::north::NorthFeed;

/// 构造一批只含 1 个点位的批次，便于用 `point_id` 断言顺序与内容。
fn batch(i: u64) -> Arc<Vec<DataPoint>> {
    Arc::new(vec![DataPoint::good(
        &format!("p{i}"),
        "d1",
        i as f64,
        i as f64,
        "u16",
    )])
}

/// 取批次内第一个点位的 id（本文件每批只有 1 个点）。
fn only_point_id(batch: &[DataPoint]) -> String {
    assert_eq!(batch.len(), 1, "本测试的批次应只含 1 个点位");
    batch[0].point_id.clone()
}

/// 场景 A（原 #6）：队列满时必须**阻塞**，而不是丢最旧 / 丢最新。
///
/// 断言方式：
/// - 容量 2 填满后，第 3 次 `send` 在 200ms 后仍未完成（生产任务没结束）；
/// - 挂起期间 `pending_batches` 停在容量值、`sent_batches` 不增长
///   （说明该批既没被"顺手挤掉别人"入队，也没有被丢弃后返回成功）；
/// - 消费端取到的前两批内容仍是 p0 / p1（丢最旧会变成 p1/p2，丢最新会变成 p0/p1
///   但生产端不会阻塞），第 3 批随后按 FIFO 到达（p2）。
#[tokio::test]
async fn feed_full_blocks_instead_of_dropping_and_keeps_fifo_order() {
    let (tx, mut rx) = NorthFeed::channel(2);

    // 填满队列：容量 2 → 两次 send 必须立即成功且不记背压
    tx.send(batch(0))
        .await
        .expect("发送端未关闭，send 不应失败");
    tx.send(batch(1))
        .await
        .expect("发送端未关闭，send 不应失败");
    let filled = tx.stats();
    assert_eq!(filled.capacity, 2, "容量应为构造时传入的值");
    assert_eq!(filled.pending_batches, 2, "两次 send 后应恰好积压 2 批");
    assert_eq!(filled.sent_batches, 2);
    assert_eq!(filled.backpressure_events, 0, "未满时 send 不应记背压事件");

    // 第 3 次 send：队列已满 → 必须挂起。用 clone 出的发送端跑生产任务，
    // 测试侧保留一个句柄读统计。
    let producer = {
        let tx = tx.clone();
        tokio::spawn(async move { tx.send(batch(2)).await.is_ok() })
    };
    // 给生产任务足够时间「如果真的会失败/丢弃，早就返回了」
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert!(
        !producer.is_finished(),
        "队列满时 send 必须挂起等待空位（丢最旧/丢最新都会立即返回）"
    );
    let blocked = tx.stats();
    assert_eq!(
        blocked.pending_batches, 2,
        "挂起期间积压不得变化（不得挤掉旧批或覆盖新批）"
    );
    assert_eq!(blocked.sent_batches, 2, "挂起中的批次不得计入已入队");
    assert_eq!(
        blocked.backpressure_events, 1,
        "满队列 send 必须记一次背压事件"
    );

    // 消费端先取到的必须仍是最早的两批（内容未被覆盖）
    let first = rx.recv().await.expect("队列应仍有数据");
    let second = rx.recv().await.expect("队列应仍有数据");
    assert_eq!(only_point_id(&first), "p0", "FIFO：最早入队的先出");
    assert_eq!(only_point_id(&second), "p1", "FIFO：第二批应原样保留");

    // 空位出现后生产端自动继续，第 3 批按 FIFO 入队
    let third = rx.recv().await.expect("第 3 批应在背压解除后到达");
    assert_eq!(only_point_id(&third), "p2", "FIFO：第 3 批应最后到达");
    assert!(producer.await.expect("生产任务不应 panic"), "send 不应失败");

    let converged = tx.stats();
    assert_eq!(converged.pending_batches, 0, "全部消费后不得残留积压");
    assert_eq!(converged.sent_batches, 3, "3 批应全部成功入队（0 丢失）");
}

/// 场景 B（原 #4）：北向上行中断 → 积压冲高并封顶 → 恢复后回落至 0。
///
/// 「断线」用消费者停摆建模（等价于上行链路不可用期间 ConversionManager 无法
/// 把批次交出去）；「重连」即消费者恢复取数。断言：
/// - 中断期间积压停在容量上限（不会无限增长，也不会越过容量）；
/// - 生产端被挂起（背压真实生效，而不是静默丢弃后继续跑）；
/// - 恢复后积压归零，20 批**按序恰好各到一次**（不丢不重）；
/// - 恢复后新的 send 立即返回（队列未死锁）。
#[tokio::test]
async fn north_feed_pending_peaks_at_capacity_then_converges_after_recovery() {
    const CAPACITY: usize = 4;
    const TOTAL: u64 = 20;

    let (tx, mut rx) = NorthFeed::channel(CAPACITY);
    let probe = tx.clone();

    let producer = tokio::spawn(async move {
        for i in 0..TOTAL {
            tx.send(batch(i))
                .await
                .expect("发送端未关闭，send 不应失败");
        }
        tx
    });

    // 断链窗口：无人消费 → 队列填满后生产端被背压挂起
    tokio::time::sleep(Duration::from_millis(300)).await;
    let stalled = probe.stats();
    assert_eq!(stalled.capacity, CAPACITY);
    assert_eq!(
        stalled.pending_batches, CAPACITY,
        "断链期间积压必须封顶在容量，不能无限增长"
    );
    assert_eq!(
        stalled.sent_batches, CAPACITY as u64,
        "断链期间只有容量内的批次能入队"
    );
    assert!(
        stalled.backpressure_events > 0,
        "断链期间必须触发背压（否则说明数据被静默丢弃）"
    );
    assert!(!producer.is_finished(), "队列满时生产端必须被挂起");

    // 链路恢复：消费者开始取数
    let mut received = Vec::new();
    while (received.len() as u64) < TOTAL {
        let b = rx.recv().await.expect("恢复后应能取完所有批次");
        received.push(only_point_id(&b));
    }

    let expected: Vec<String> = (0..TOTAL).map(|i| format!("p{i}")).collect();
    assert_eq!(
        received, expected,
        "恢复后必须按入队顺序恰好各投递一次（不丢批、不重复、不乱序）"
    );

    let tx = producer.await.expect("生产任务不应 panic");
    let converged = tx.stats();
    assert_eq!(converged.pending_batches, 0, "收敛：积压必须回落到 0");
    assert_eq!(converged.sent_batches, TOTAL, "收敛：全部批次都应已入队");

    // 队列未死锁：恢复后新批次可立即入队并取出
    tokio::time::timeout(Duration::from_millis(500), tx.send(batch(999)))
        .await
        .expect("恢复后 send 必须立即完成，而不是继续被背压挂起")
        .expect("发送端仍存活，send 不应失败");
    let tail = rx.recv().await.expect("恢复后新批次应能取出");
    assert_eq!(only_point_id(&tail), "p999");
}
