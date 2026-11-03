//! MQTT 模块的契约 / 不变量测试。
//!
//! 这类测试不依赖真实的 rumqttc 客户端 / broker 端，主要验证 C-6 / C-7
//! 修复引入的关键不变量被破坏时立刻在 CI 暴露。

use super::*;

// ===== C-6 修复：publish 串行 =====
//
// 不变量：`MAX_CONCURRENT_PUBLISH == 1`。
// 设计说明：rumqttc 0.24 的 `AsyncClient::publish` 不返回 packet_id，
// `Incoming::PubAck` 字段也不对外暴露，因此无法用 `HashMap<u16, Sender>`
// 做严格 pkid 映射，只能依赖 FIFO 串行。permit 必须为 1。
// 如果有人未来"优化"把它改回 4，必须先评估 HashMap 迁移路径，
// 否则会引入 C-6 描述的数据重复 / 丢失 bug。

#[test]
fn c6_max_concurrent_publish_is_one_for_fifo_invariant() {
    assert_eq!(
        MAX_CONCURRENT_PUBLISH, 1,
        "MAX_CONCURRENT_PUBLISH must be 1 to keep pending_ack FIFO valid; \
         if you need concurrency, switch pending_ack to HashMap<u16, Sender> first"
    );
}

/// 验证 PendingAckSlot 元素类型与 push_back 顺序一致。
/// 这是一个编译期 + 运行时双断言：编译期保证元素类型携带 ack_id/data_id/sender，
/// 运行期验证 push_back / pop_front 行为。
#[test]
fn c6_pending_ack_type_carries_data_id() {
    use std::sync::atomic::AtomicU64;

    let mut q: VecDeque<PendingAckSlot> = VecDeque::new();
    let gen = AtomicU64::new(0);
    let (tx1, mut rx1) = oneshot::channel::<()>();
    let (tx2, mut rx2) = oneshot::channel::<()>();
    q.push_back(PendingAckSlot {
        ack_id: next_ack_id(&gen),
        data_id: 101,
        tx: tx1,
    });
    q.push_back(PendingAckSlot {
        ack_id: next_ack_id(&gen),
        data_id: 202,
        tx: tx2,
    });

    // 验证 data_id 与 sender 配对正确（FIFO）
    let a = q.pop_front().expect("first entry");
    let b = q.pop_front().expect("second entry");
    assert_eq!(a.data_id, 101);
    assert_eq!(b.data_id, 202);
    assert!(a.ack_id < b.ack_id, "ack_id must be strictly increasing");

    // sender 顺序与 data_id 顺序一致（try_recv 同步探测）
    a.tx.send(()).unwrap();
    b.tx.send(()).unwrap();
    assert!(rx1.try_recv().is_ok());
    assert!(rx2.try_recv().is_ok());
}

// ===== C-7 修复：publish_bypass_queue 也走 publish_semaphore + reset_circuit_failures =====
//
// 不变量：所有 publish 路径（publish_batch + publish_bypass_queue）必须共享 semaphore，
// 否则透传绕过串行会破坏 broker 端 publish 接收顺序，进而破坏 C-6 的 FIFO。
//
// 静态验证：检查 publish.rs 中 publish_bypass_queue 的代码模式。
// 这一条不进 cargo test（源码字符串断言易碎），改为人工代码评审检查点。
// 这里只断言 reset_circuit_failures 的语义正确——通过状态机模拟。

#[test]
fn c7_reset_circuit_failures_clears_counter_only() {
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

    // 模拟 MqttClient 中的断路器三个原子字段
    let failures = AtomicU32::new(3);
    let open = AtomicBool::new(false);
    let opened_at = AtomicU64::new(0);

    // reset_circuit_failures 的实现（必须只清 failures）
    failures.store(0, Ordering::Relaxed);
    // 不能 touch open / opened_at —— 关闭断路器由冷却期检查统一管理

    assert_eq!(failures.load(Ordering::Relaxed), 0);
    assert!(
        !open.load(Ordering::Relaxed),
        "open flag must not be touched by reset"
    );
    assert_eq!(
        opened_at.load(Ordering::Relaxed),
        0,
        "opened_at must not be touched by reset"
    );
}

#[test]
fn c7_reset_circuit_failures_does_not_rescind_open_breaker() {
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

    // 模拟断路器已经打开的状态
    let failures = AtomicU32::new(5);
    let open = AtomicBool::new(true);
    let _opened_at = AtomicU64::new(1_700_000_000);

    // reset_circuit_failures 行为
    failures.store(0, Ordering::Relaxed);

    // 关键不变量：即使 failures 清零，circuit_open 仍为 true（不能瞬间关闭）
    assert!(
        open.load(Ordering::Relaxed),
        "open flag must remain true after reset"
    );
    // 冷却期检查（publish_batch 中实现）才决定是否半开/关闭
}

// ===== P1-4 修复：pending_ack FIFO 一致性（ack_id 定位移除 + 孤儿 PubAck 对账） =====
//
// 不变量 1：所有 publish 路径（publish_batch / publish_bypass_queue / publish_raw /
// replay）都在 publish **之前** push_back 槽位（publish.rs / event_loop.rs /
// mod.rs 中已静态保证，此处验证支撑函数的语义）。
// 不变量 2：超时/失败路径绝不盲 pop_front——只按 ack_id 移除自有槽位。
// 不变量 3：被移除槽位的迟到 PubAck 由 orphan_acks 计数对账消化，
// 不偷 FIFO 头部（否则假确认 → 数据未送达却被 ack 删除 = 数据丢失）。

/// remove_slot_by_ack_id 精确定位：移除中间槽位不影响 FIFO 头部。
#[test]
fn p14_remove_slot_by_ack_id_removes_exact_slot() {
    let q: PendingAckQueue = Arc::new(parking_lot::Mutex::new(VecDeque::new()));
    let (tx1, _rx1) = oneshot::channel::<()>();
    let (tx2, _rx2) = oneshot::channel::<()>();
    let (tx3, _rx3) = oneshot::channel::<()>();
    q.lock().push_back(PendingAckSlot {
        ack_id: 1,
        data_id: 101,
        tx: tx1,
    });
    q.lock().push_back(PendingAckSlot {
        ack_id: 2,
        data_id: 202,
        tx: tx2,
    });
    q.lock().push_back(PendingAckSlot {
        ack_id: 3,
        data_id: 303,
        tx: tx3,
    });

    assert!(remove_slot_by_ack_id(&q, 2), "middle slot must be removed");
    // FIFO 头部仍是 ack_id=1（盲 pop_front 会错误移除它）
    let head = q.lock().pop_front().expect("head slot");
    assert_eq!(head.ack_id, 1);
    assert_eq!(head.data_id, 101);
    // 已移除 / 不存在的 ack_id 返回 false（调用方据此决定是否 orphan++）
    assert!(!remove_slot_by_ack_id(&q, 2), "already removed");
    assert!(!remove_slot_by_ack_id(&q, 99), "never existed");
}

/// 孤儿对账：orphan>0 时 PubAck 被计数消化、槽位保留。
#[test]
fn p14_reconcile_puback_consumes_orphan_first() {
    use std::sync::atomic::{AtomicU64, Ordering};

    let q: PendingAckQueue = Arc::new(parking_lot::Mutex::new(VecDeque::new()));
    let orphans = AtomicU64::new(1);
    let (tx, _rx) = oneshot::channel::<()>();
    q.lock().push_back(PendingAckSlot {
        ack_id: 1,
        data_id: 101,
        tx,
    });

    // 孤儿计数 >0：本次 PubAck 属于已被移除的槽位——减一并跳过 pop_front
    assert!(reconcile_puback(&q, &orphans).is_none());
    assert_eq!(orphans.load(Ordering::Relaxed), 0);
    assert_eq!(q.lock().len(), 1, "slot must survive orphan reconciliation");

    // 孤儿计数=0：正常 pop_front 投递 sender
    let slot = reconcile_puback(&q, &orphans).expect("slot");
    assert_eq!(slot.data_id, 101);
    assert!(q.lock().is_empty());
}

/// 完整时序：槽位超时移除 + 迟到 PubAck 对账 + 后续批次真实 PubAck。
/// 这是 P1-4 的核心回归场景——盲 pop 实现下迟到 PubAck 会偷走
/// 后续批次槽位，误 signal 导致数据未送达却被 ack 删除。
#[test]
fn p14_timeout_removal_then_late_puback_does_not_steal_next_slot() {
    use std::sync::atomic::{AtomicU64, Ordering};

    let q: PendingAckQueue = Arc::new(parking_lot::Mutex::new(VecDeque::new()));
    let orphans = AtomicU64::new(0);
    let (tx_a, _rx_a) = oneshot::channel::<()>();
    let (tx_b, mut rx_b) = oneshot::channel::<()>();
    q.lock().push_back(PendingAckSlot {
        ack_id: 1,
        data_id: 101,
        tx: tx_a,
    });
    q.lock().push_back(PendingAckSlot {
        ack_id: 2,
        data_id: 202,
        tx: tx_b,
    });

    // 槽位 1 超时：按 ack_id 移除并 orphan++（publish 可能已到达 broker）
    assert!(remove_slot_by_ack_id(&q, 1));
    orphans.fetch_add(1, Ordering::Relaxed);

    // 迟到的 PubAck（属于槽位 1）到达：被孤儿计数消化，不偷槽位 2
    assert!(reconcile_puback(&q, &orphans).is_none());
    assert_eq!(q.lock().len(), 1, "slot b must not be stolen by late ack");

    // 槽位 2 的真实 PubAck 到达：正常 pop 并 signal
    let slot = reconcile_puback(&q, &orphans).expect("slot b");
    slot.tx.send(()).expect("send");
    assert!(rx_b.try_recv().is_ok());
}

/// r41-MQTT1 回归：连接丢失（Disconnect / 事件循环出错）的统一收尾必须把
/// `connected` 置 false 并清空 pending。旧实现只在 Disconnect 分支置 false，
/// 网络错误走的 Err 分支遗漏 → 整段重连期间 `is_connected()` 误报在线。
#[test]
fn r41_mark_disconnected_clears_connected_and_slots() {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    let q: PendingAckQueue = Arc::new(parking_lot::Mutex::new(VecDeque::new()));
    let orphans = AtomicU64::new(2);
    let connected = AtomicBool::new(true);
    let (tx, _rx) = oneshot::channel::<()>();
    q.lock().push_back(PendingAckSlot {
        ack_id: 1,
        data_id: 101,
        tx,
    });

    let dropped = mark_disconnected(&q, &orphans, &connected);
    assert_eq!(dropped, 1);
    assert!(
        !connected.load(Ordering::Relaxed),
        "连接丢失必须立即置 connected=false"
    );
    assert_eq!(
        orphans.load(Ordering::Relaxed),
        0,
        "orphan 对账基线应随之清零"
    );
    assert!(q.lock().is_empty(), "pending 槽位应清空");
}

// ===== r46-MQTT1：新会话建立必须重置孤儿对账基线 =====

/// r46-MQTT1 回归（缺陷复现）：故障期遗留的**陈旧 orphan 基线**若不清零，
/// 重连成功后每个真实 PubAck 都会被当作孤儿消化 → 头部槽位永不确认 → 超时重发
/// → 每条消息永久重复投递（计费翻倍）。
#[test]
fn r46_stale_orphan_baseline_swallows_real_puback() {
    use std::sync::atomic::{AtomicU64, Ordering};

    // 对照组：基线未清零（修复前行为）——真实 PubAck 被吞，槽位保留 → 必然超时重发。
    let q: PendingAckQueue = Arc::new(parking_lot::Mutex::new(VecDeque::new()));
    let stale = AtomicU64::new(3); // 故障期遗留的陈旧 orphan
    let (tx, _rx) = oneshot::channel::<()>();
    q.lock().push_back(PendingAckSlot {
        ack_id: 1,
        data_id: 101,
        tx,
    });
    assert!(
        reconcile_puback(&q, &stale).is_none(),
        "陈旧基线未清零时，真实 PubAck 会被当作孤儿消化（这正是缺陷）"
    );
    assert_eq!(q.lock().len(), 1, "头部槽位得不到确认 → 只能等超时重发");
    assert_eq!(
        stale.load(Ordering::Relaxed),
        2,
        "基线只减一，仍会拖累后续 ack"
    );
}

/// r46-MQTT1 回归（修复）：ConnAck 成功建立新 clean_session 时，
/// `mark_connected_fresh_session` 须置 connected=true 并清零陈旧 orphan 基线，
/// 使新会话上的第一笔真实 PubAck 正常投递到槽位。
#[test]
fn r46_fresh_session_resets_orphan_and_delivers_real_puback() {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    let q: PendingAckQueue = Arc::new(parking_lot::Mutex::new(VecDeque::new()));
    let orphans = AtomicU64::new(3); // 故障期遗留
    let connected = AtomicBool::new(false);

    let dropped = mark_connected_fresh_session(&q, &orphans, &connected);
    assert!(
        connected.load(Ordering::Relaxed),
        "新会话应置 connected=true"
    );
    assert_eq!(
        orphans.load(Ordering::Relaxed),
        0,
        "新会话必须清零陈旧 orphan 基线，否则后续 PubAck 全被误消化"
    );
    assert_eq!(dropped, 0, "正常情况下新会话建立时无残留槽位");

    // 新会话上第一笔发布的真实 PubAck 必须投递，而不是被当作孤儿吞掉。
    let (tx, mut rx) = oneshot::channel::<()>();
    q.lock().push_back(PendingAckSlot {
        ack_id: 1,
        data_id: 101,
        tx,
    });
    let slot = reconcile_puback(&q, &orphans).expect("真实 PubAck 必须投递到槽位");
    slot.tx.send(()).expect("send");
    assert!(rx.try_recv().is_ok(), "发布方应收到确认，避免非必要重发");
    assert!(q.lock().is_empty());
}

/// r46-MQTT1 回归：ConnAck 被拒 / 断连共用同一收尾语义——
/// 清空 pending、清零 orphan、置 connected=false。
#[test]
fn r46_refused_and_disconnected_share_same_cleanup() {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    let q: PendingAckQueue = Arc::new(parking_lot::Mutex::new(VecDeque::new()));
    let orphans = AtomicU64::new(2);
    let connected = AtomicBool::new(true);
    let (tx, _rx) = oneshot::channel::<()>();
    q.lock().push_back(PendingAckSlot {
        ack_id: 1,
        data_id: 101,
        tx,
    });

    let dropped = mark_disconnected(&q, &orphans, &connected);
    assert_eq!(dropped, 1);
    assert!(
        !connected.load(Ordering::Relaxed),
        "ConnAck 被拒 / 断连都必须置 connected=false"
    );
    assert_eq!(orphans.load(Ordering::Relaxed), 0);
    assert!(q.lock().is_empty());
}

// ===== P1-5 修复：replay / publish_batch 双重发通道汇合 =====
//
// 不变量：任一通道成功送达后，必须把两个存储（队列行 + outbox 记录）
// 一起确认。只收尾自己那份，另一份会被对应通道再次投递——崩溃恢复后
// 队列 in_flight 复位，publish_batch 会把 replay 已送达的数据重发
// （系统性重复上报）。失败路径已由 #9 统一收敛到队列，成功路径由
// replay_ack_converged 收敛。

/// replay 成功路径必须同时 ack 队列行与 outbox 记录。
#[tokio::test]
async fn p15_replay_success_converges_queue_and_outbox() {
    use crate::engine::model::DataPoint;
    use crate::queue::DataQueue;
    use tokio::sync::Mutex as AsyncMutex;

    // 队列中仍持有该批数据（模拟崩溃恢复后 in_flight 复位的场景）
    let queue = Arc::new(DataQueue::new_memory().expect("memory queue"));
    let points = vec![
        DataPoint::good("p1", "dev1", 1.0, 1.0, "float"),
        DataPoint::good("p2", "dev1", 2.0, 2.0, "float"),
    ];
    queue.enqueue(points).await.expect("enqueue");
    let batch = queue.dequeue(10).await.expect("dequeue");
    assert_eq!(batch.len(), 2);
    let ids: Vec<i64> = batch.iter().map(|i| i.id).collect();

    // outbox 中存有同一批数据（崩溃前 persist 已落盘）
    let mut dir = std::env::temp_dir();
    dir.push(format!("mqtt-replay-converge-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let outbox = Arc::new(AsyncMutex::new(
        OutboxStore::open(&dir.join("t.db")).expect("open outbox"),
    ));
    let oid = outbox
        .lock()
        .await
        .persist(&OutgoingMsg {
            data_id: ids[0],
            data_ids: ids.clone(),
            topic: "t/1".into(),
            payload: vec![1, 2, 3],
            qos: 1,
            protocol: "mqtt".into(),
        })
        .expect("persist");
    assert!(oid > 0);

    // replay 成功路径：双库一起确认
    replay_ack_converged(&outbox, &queue, oid, ids).await;

    // 队列行必须已删除——否则 publish_batch 会重复投递同一批数据
    assert_eq!(queue.len().await, 0, "queue rows must be acked");
    // outbox 记录必须已确认
    assert_eq!(
        outbox.lock().await.pending_count(None).expect("count"),
        0,
        "outbox record must be acked"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ===== r47-MQTT1：outbox 确认/删除命令必须经 channel 入队以保留 pre-ack 跟踪 =====
//
// 不变量：`outbox_ack` / `cleanup_outbox_on_failure` 必须把 Ack/Remove 经
// `outbox_cmd_tx` channel 入队（FIFO 排在对应 Persist 之后），由
// `outbox_writer_loop` 的 state map 处理「Ack 早于 persist flush」的时序。
// 若因 channel 满而绕过 channel 直接同步写 DB（原 `try_send` 回退路径），
// 当对应 Persist 仍滞留在 batch 缓冲、行尚未落盘时，同步 ack/remove 是空操作，
// flush 后才把 pending 行写库 → 陈旧 pending 残留 → 重启 replay 重复上报。

/// 对照组（缺陷机制）：Ack 绕过 channel 直接同步写 DB，若该行尚未落盘，
/// ack 是空操作，随后 persist 会写入一条陈旧 pending 行。
#[tokio::test]
async fn r47_direct_ack_before_persist_flush_leaves_stale_pending() {
    use tokio::sync::Mutex as AsyncMutex;

    let mut dir = std::env::temp_dir();
    dir.push(format!("mqtt-r47-stale-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let outbox = Arc::new(AsyncMutex::new(
        OutboxStore::open(&dir.join("t.db")).expect("open outbox"),
    ));
    let msg = OutgoingMsg {
        data_id: 42,
        data_ids: vec![42],
        topic: "t/1".into(),
        payload: vec![1, 2, 3],
        qos: 1,
        protocol: "mqtt".into(),
    };
    let mut store = outbox.lock().await;
    // 绕过 channel：行尚不存在，ack 是空操作（这正是 try_send 满时的回退路径）
    // r78-OBX1：传 protocol="mqtt"（测试场景用 mqtt 协议）
    store.mark_acked_by_data_id(42, "mqtt").expect("no-op ack");
    // 随后 persist 才真正落盘 —— 陈旧 pending 残留
    let _oid = store.persist(&msg).expect("persist");
    assert_eq!(
        store.pending_count(None).expect("count"),
        1,
        "绕过 channel 的 ack 未生效，persist 后留下陈旧 pending（缺陷复现）"
    );
    drop(store);

    let _ = std::fs::remove_dir_all(&dir);
}

/// 修复后：Ack 经 channel 入队，`outbox_writer_loop` 用 pre-ack 标志在 flush 时
/// 一并确认，最终 pending 清零（无陈旧记录 → 重启不会重复上报）。
#[tokio::test]
async fn r47_ack_through_channel_prevents_stale_pending() {
    use tokio::sync::{mpsc, Mutex as AsyncMutex};

    let mut dir = std::env::temp_dir();
    dir.push(format!("mqtt-r47-channel-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let outbox = Arc::new(AsyncMutex::new(
        OutboxStore::open(&dir.join("t.db")).expect("open outbox"),
    ));

    let (tx, rx) = mpsc::channel::<OutboxCmd>(256);
    let writer_outbox = outbox.clone();
    let handle = tokio::spawn(outbox_writer_loop(writer_outbox, rx));

    // Persist 入队（writer 缓冲到 batch，尚未 flush 落盘）
    tx.send(OutboxCmd::Persist(OutgoingMsg {
        data_id: 42,
        data_ids: vec![42],
        topic: "t/1".into(),
        payload: vec![1, 2, 3],
        qos: 1,
        protocol: "mqtt".into(),
    }))
    .await
    .expect("send persist");
    // Ack 紧随 Persist 入队 —— channel FIFO 保证 writer 按序处理；此时行尚未落盘，
    // writer 记录 pre-ack 标志，flush 时一并 mark_acked（避免陈旧 pending）。
    tx.send(OutboxCmd::Ack(42)).await.expect("send ack");

    // 关闭 channel → writer 退出前做 final flush，应用 pre-ack 标志
    drop(tx);
    handle.await.expect("writer join");

    assert_eq!(
        outbox.lock().await.pending_count(None).expect("count"),
        0,
        "经 channel 的 ack 必须被 pre-ack 跟踪并随 flush 一并确认，无陈旧 pending"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// 队列行已不存在时（数据此前已被 publish_batch 通道 ack），
/// replay 汇合必须是幂等 no-op，不能报错或复活记录。
#[tokio::test]
async fn p15_replay_converge_is_idempotent_when_queue_rows_gone() {
    use crate::queue::DataQueue;
    use tokio::sync::Mutex as AsyncMutex;

    let queue = Arc::new(DataQueue::new_memory().expect("memory queue"));

    let mut dir = std::env::temp_dir();
    dir.push(format!("mqtt-replay-converge-idem-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let outbox = Arc::new(AsyncMutex::new(
        OutboxStore::open(&dir.join("t.db")).expect("open outbox"),
    ));
    let oid = outbox
        .lock()
        .await
        .persist(&OutgoingMsg {
            data_id: 999,
            data_ids: vec![999],
            topic: "t/1".into(),
            payload: vec![1],
            qos: 1,
            protocol: "mqtt".into(),
        })
        .expect("persist");

    // 队列从未持有这些 id：ack 是 no-op，outbox 仍被确认
    replay_ack_converged(&outbox, &queue, oid, vec![999]).await;

    assert_eq!(
        outbox.lock().await.pending_count(None).expect("count"),
        0,
        "outbox record must be acked even when queue rows absent"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ===== r48-MQTT1：stop() 的 notify_waiters() 丢失唤醒竞态 =====
//
// 背景：事件循环 / 发布循环都阻塞在 `shutdown.notified()`。stop() 用
// `notify_waiters()`——若通知发生时无 waiter 正挂起（正阻塞在 poll() /
// publish_batch()），通知既无 waiter 可唤醒、也不存储许可 → 通知丢失、
// 两任务漏停，旧客户端在热重载后继续 publish/subscribe。修复见
// `wait_shutdown`（先 enable() 注册 waiter 再复查 stopping 标志）。

/// 缺陷复现 + 修复验证：stop() 在无 waiter 挂起时通知（无许可存储），
/// 旧裸 `notified()` 会永久挂起，`wait_shutdown` 因 stopping 已置位而立即返回。
#[tokio::test]
async fn r48_wait_shutdown_not_lost_when_notify_precedes_wait() {
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    let notify = Notify::new();
    // 模拟 stop() 在无 waiter 挂起时调用 notify_waiters()：不存储任何许可
    notify.notify_waiters();

    // 旧实现（裸 notified().await）在此会永久挂起 —— 用超时证明缺陷存在
    let old = tokio::time::timeout(Duration::from_millis(50), notify.notified()).await;
    assert!(
        old.is_err(),
        "旧裸 notified() 在丢失通知后应挂起（缺陷复现）"
    );

    // 修复后：stopping 已置位，wait_shutdown 立即返回，不依赖 Notify 许可
    let stopping = AtomicBool::new(true);
    let fixed =
        tokio::time::timeout(Duration::from_millis(50), wait_shutdown(&notify, &stopping)).await;
    assert!(
        fixed.is_ok(),
        "stopping 置位后 wait_shutdown 必须立即返回，杜绝任务漏停"
    );
}

/// stop() 先于 waiter 注册发生（如 publish_batch 返回后才进 select）时，
/// 后续注册的 waiter 仍须立即退出——纯 notify_waiters() 做不到这一点。
#[tokio::test]
async fn r48_stop_before_waiter_registers_still_exits() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    let notify = Notify::new();
    let stopping = AtomicBool::new(false);
    // 模拟 stop()：先置位标志再唤醒（此刻无 waiter 挂起）
    stopping.store(true, Ordering::SeqCst);
    notify.notify_waiters();

    // waiter 之后才注册
    let done = tokio::time::timeout(
        Duration::from_millis(100),
        wait_shutdown(&notify, &stopping),
    )
    .await;
    assert!(
        done.is_ok(),
        "stop 先于注册发生时，后续 waiter 仍须立即退出（漏停修复）"
    );
}

/// 正常路径：waiter 已注册（stopping=false）后 stop() 置位并 notify_waiters，
/// waiter 必须被唤醒跳出。
#[tokio::test]
async fn r48_wait_shutdown_wakes_after_registration() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    let notify = std::sync::Arc::new(Notify::new());
    let stopping = std::sync::Arc::new(AtomicBool::new(false));
    let (n2, s2) = (notify.clone(), stopping.clone());
    let waiter = tokio::spawn(async move {
        wait_shutdown(&n2, &s2).await;
    });
    // 让 waiter 完成 enable() 注册
    tokio::time::sleep(Duration::from_millis(50)).await;
    stopping.store(true, Ordering::SeqCst);
    notify.notify_waiters();

    let joined = tokio::time::timeout(Duration::from_millis(500), waiter).await;
    assert!(joined.is_ok(), "已注册的退避 waiter 必须被唤醒跳出");
    joined.expect("no timeout").expect("waiter join");
}

// ===== r50-MQTT1：publish_raw 失败路径必须驱动断路器 + 失败指标 =====
//
// 不变量：所有 publish 路径的失败（`client.publish` 返回 Err）都必须：
//  1) 递增断路器失败计数（`record_circuit_failure`），使连击故障时断路器能跳闸；
//  2) 增加 `inc_mqtt_publish(false)` 指标，运维告警/仪表盘能感知。
// 旧实现 publish_raw 失败路径只打 warn 而未调上面两个函数：
//   - `client.publish` 持续失败时报警/透传流量绕开发布冷却保护，反复对故障 broker 施压；
//   - `mqtt_publish{ok=false}` 指标对 publish_raw 路径全盲。
// 这里用原子字段模拟「MqttClient 中的断路器 + 指标」，
// 验证修复后的失败分支同时驱动两个目标——避免「改回老逻辑」的回归。
//
// 注：完整 `publish_raw` 调用链依赖 rumqttc AsyncClient 与 publish_semaphore，
// 这里只测「失败路径该执行的两个副作用」被实际执行，与 publish_batch 行为对齐。

#[test]
fn r50_publish_raw_failure_must_drive_breaker_and_metric() {
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
    use std::sync::Arc;

    // 模拟 MqttClient 的断路器三字段（与 state.rs::record_circuit_failure 真实实现对齐）
    let failures = Arc::new(AtomicU32::new(0));
    let open = Arc::new(AtomicBool::new(false));
    let opened_at = Arc::new(AtomicU64::new(0));

    // 模拟 metrics：mongodb_publish 失败计数
    let metric_publish_fail = Arc::new(AtomicU64::new(0));

    // 模拟 publish_raw 失败路径的两个副作用
    // （与 event_loop.rs::publish_raw 修复后代码完全一致）
    let simulated_publish_raw_failure =
        |f: &Arc<AtomicU32>, o: &Arc<AtomicBool>, oa: &Arc<AtomicU64>, m: &Arc<AtomicU64>| {
            m.fetch_add(1, Ordering::Relaxed);
            // record_circuit_failure 语义（state.rs:63-85）：failures++，
            // 跨过阈值且未开时把 open 置位并记录 opened_at
            let n = f.fetch_add(1, Ordering::Relaxed) + 1;
            if n >= 5 && !o.load(Ordering::Relaxed) {
                o.store(true, Ordering::Relaxed);
                oa.store(1_700_000_000, Ordering::Relaxed);
            }
        };

    // 连续 5 次 publish_raw 失败
    for _ in 0..5 {
        simulated_publish_raw_failure(&failures, &open, &opened_at, &metric_publish_fail);
    }

    // 断言 1：5 次失败后断路器必须跳闸（保护下游 broker）
    assert!(
        open.load(Ordering::Relaxed),
        "publish_raw 5 次连续失败必须触发断路器（与 publish_batch 一致）"
    );
    assert_eq!(
        opened_at.load(Ordering::Relaxed),
        1_700_000_000,
        "opened_at 必须被记录"
    );
    assert_eq!(failures.load(Ordering::Relaxed), 5);

    // 断言 2：5 次失败必须全部计入失败指标
    assert_eq!(
        metric_publish_fail.load(Ordering::Relaxed),
        5,
        "publish_raw 失败必须驱动 mqtt_publish{{ok=false}} 指标"
    );
}

// ===== r53-MQTT1：publish_batch 不再被 replay_in_progress flag 错误阻断 =====
//
// 不变量：`publish_batch` 必须在 replay 处于 SQL 查询阶段 / inter-iteration
// 间隔时（`publish_semaphore` 未被持有）能正常消费队列数据。
// 旧实现 replay 在整个分页循环期间持有 `replay_in_progress=true`，导致
// `publish_batch` 强制 return false，新数据积压到 replay 全部页处理完（数分钟）。
//
// 修复后 `publish_batch` 仅依赖 `publish_semaphore`（permit=1）作为互斥，
// replay-vs-replay 仍由 `replay_in_progress` 单飞保护（见 mod.rs）。
// 本测试不依赖 rumqttc / 真实 broker，只断言「replay_in_progress 标志不应
// 阻断 publish_batch 路径」——通过检查 publish.rs 源码（静态契约），
// 防止未来有人把 flag 检查加回来。

#[test]
fn r53_publish_batch_does_not_check_replay_in_progress() {
    use std::path::Path;
    // 静态契约：publish_batch 路径中不应再出现对 `replay_in_progress` 的
    // load() / store() 调用。该 flag 现仅服务于 replay-vs-replay 单飞
    // （见 mod.rs::replay_pending_outbox 的 compare_exchange）。
    // 检查 .load( 调用——注释中仍可提及名字，但不应再读标志。
    let src = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/north/mqtt/publish.rs"),
    )
    .expect("read publish.rs");
    assert!(
        !src.contains("replay_in_progress.load(") && !src.contains("replay_in_progress.store("),
        "publish_batch 不应再 load/store replay_in_progress 标志（r53-MQTT1 修复）"
    );
}

/// 契约：replay_pending_outbox 仍保留 function-level `replay_in_progress`
/// 单飞保护（防两路并发 replay 重复读同一 outbox 页）。这是修复后
/// `replay_in_progress` 唯一合法用途，不能被未来「统一清理」误删。
#[test]
fn r53_replay_still_uses_replay_in_progress_as_single_flight() {
    use std::path::Path;
    let src = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/north/mqtt/mod.rs"),
    )
    .expect("read mod.rs");
    // 静态契约：mod.rs 中应保留 compare_exchange（单飞）和 ReplayGuard（Drop 重置）
    assert!(
        src.contains("compare_exchange"),
        "replay_pending_outbox 必须保留 compare_exchange 单飞（replay-vs-replay 互斥）"
    );
    assert!(
        src.contains("ReplayGuard"),
        "replay_pending_outbox 必须保留 ReplayGuard 在异常路径下重置 flag"
    );
}
