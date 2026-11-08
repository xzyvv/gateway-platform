//! QoS1 重复投递去重（outbox 部分唯一索引的行为断言）。
//!
//! QoS1 是 at-least-once：同一条数据可能在「已 publish 未收到 PUBACK → 断线重连
//! replay」或「publish 失败 → queue.release 后重新出队」路径上被投递多次。防止
//! 重复在**持久化层**放大的唯一防线是 outbox 的部分唯一索引：
//!
//! ```sql
//! CREATE UNIQUE INDEX idx_outbox_pending_data_id
//!     ON mqtt_outbox(data_id) WHERE status = 0;
//! ```
//!
//! 语义：同一 `data_id` 在**未确认期间**只允许存在 1 条 pending 记录；一旦 ack
//! （status=1）后同一 id 可以再次入队。缺少该约束时，重复 persist 会留下两条
//! pending，重连后的 replay 就会把同一批数据再发一遍——上报量翻倍。
//!
//! 注意边界：本文件锁的是「outbox/replay 不因重复投递而放大」。broker 侧仍可能
//! 收到重复报文（at-least-once 的固有语义），接收端需按 payload 内 id 幂等。

use std::path::PathBuf;

use gateway_core::north::mqtt::{OutboxStore, OutgoingMsg};

/// 构造一条最小 MQTT 出站消息（`data_id` 为该批的幂等键）。
fn msg(data_id: i64, data_ids: Vec<i64>) -> OutgoingMsg {
    OutgoingMsg {
        data_id,
        data_ids,
        topic: "gw/dedup".into(),
        payload: vec![1, 2, 3],
        qos: 1,
        protocol: "mqtt".into(),
    }
}

/// 临时目录 + 库文件路径；TempDir 必须由调用方持有到测试结束。
fn temp_db() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().join("outbox.db");
    (dir, path)
}

/// 同一批数据重复 persist：只保留 1 条 pending，replay 只取到 1 条。
///
/// 第二次 persist 必须返回 0（被 INSERT OR IGNORE 跳过），而不是新的 rowid——
/// 上层据此区分「新入队」与「重复投递」，避免把重复行当作新消息登记待确认槽位。
#[test]
fn duplicate_persist_keeps_single_pending_and_replays_once() {
    let (_dir, path) = temp_db();
    let mut store = OutboxStore::open(&path).expect("open outbox");

    // 同一批（data_id=7, 含点位 7/8）连续投递两次
    let first = store
        .persist(&msg(7, vec![7, 8]))
        .expect("首次 persist 应成功");
    assert!(first > 0, "首次 persist 必须真实插入");
    let second = store
        .persist(&msg(7, vec![7, 8]))
        .expect("重复 persist 不应报错");
    assert_eq!(
        second, 0,
        "重复投递必须被唯一约束忽略并返回 0（否则会产生两条待发记录）"
    );

    assert_eq!(
        store.pending_count(Some("mqtt")).expect("count"),
        1,
        "重复投递后 pending 必须仍为 1 条"
    );
    let replay = store
        .replay_pending_batch("mqtt", 0, 100)
        .expect("replay 查询失败");
    assert_eq!(
        replay.len(),
        1,
        "重连 replay 只能取到 1 条（>1 说明去重失效，会双发）"
    );
    assert_eq!(replay[0].1.data_id, 7, "replay 取到的应是首次入队那条");

    // ack 之后同 data_id 允许再次入队（清理 / 回滚后重发的场景）
    store.mark_acked(first).expect("mark_acked 失败");
    assert_eq!(
        store.pending_count(Some("mqtt")).expect("count"),
        0,
        "ack 后 pending 应清零"
    );
    let again = store
        .persist(&msg(7, vec![7, 8]))
        .expect("ack 后重新 persist 应成功");
    assert!(
        again > 0,
        "已确认的行不占用幂等槽位，同一 data_id 必须能再次入队"
    );
    assert_eq!(
        store
            .replay_pending_batch("mqtt", 0, 100)
            .expect("replay 查询失败")
            .len(),
        1,
        "再次入队后 replay 仍应只有 1 条"
    );
}

/// 同一批内出现重复 data_id：批量写入逐条 IGNORE，结果只落 1 条且不串位。
///
/// `persist_batch` 返回与输入等长的 `Vec<Option<i64>>`：`None` 表示该条被幂等跳过
/// （或单条失败），`Some(rowid)` 表示真实插入——上层的 ack 逻辑依赖这个对齐关系。
#[test]
fn duplicate_rows_inside_one_batch_collapse_to_one_pending() {
    let (_dir, path) = temp_db();
    let mut store = OutboxStore::open(&path).expect("open outbox");

    // [1, 1, 2]：第 2 条与第 1 条撞同一 data_id，第 3 条是新数据
    let ids = store
        .persist_batch(&[msg(1, vec![1]), msg(1, vec![1]), msg(2, vec![2])])
        .expect("persist_batch 失败");
    assert_eq!(ids.len(), 3, "返回值必须与输入等长，供上层逐条对齐");
    assert!(ids[0].is_some(), "第 1 条应真实插入");
    assert_eq!(ids[1], None, "批内重复 data_id 必须被忽略（返回 None）");
    assert!(
        ids[2].is_some(),
        "新 data_id 应真实插入，不能被前一条的 IGNORE 串位"
    );

    assert_eq!(
        store.pending_count(Some("mqtt")).expect("count"),
        2,
        "3 条输入（含 1 条重复）应只落 2 条 pending"
    );
    let replay = store
        .replay_pending_batch("mqtt", 0, 100)
        .expect("replay 查询失败");
    let data_ids: Vec<i64> = replay.iter().map(|(_, m)| m.data_id).collect();
    assert_eq!(data_ids, vec![1, 2], "replay 顺序应保持入队顺序且无重复");
}
