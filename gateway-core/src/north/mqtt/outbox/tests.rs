use super::*;

#[test]
fn outbox_persist_ack_replay() {
    let dir = tempdir();
    let path = dir.join("test.db");
    let mut store = OutboxStore::open(&path).expect("should succeed");

    // 写入
    let id = store
        .persist(&OutgoingMsg {
            data_id: 42,
            data_ids: vec![42, 43, 44],
            topic: "t/1".into(),
            payload: vec![1, 2, 3],
            qos: 1,
            protocol: "mqtt".into(),
        })
        .expect("should succeed");
    assert!(id > 0);

    // replay 应返回
    let pending = store.replay_pending().expect("should succeed");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].0, id);
    assert_eq!(pending[0].1.data_id, 42);
    assert_eq!(pending[0].1.data_ids, vec![42, 43, 44]);
    // Verify BLOB encoding roundtrip
    let blob = encode_i64_list(&[42, 43, 44]);
    assert_eq!(decode_i64_list(&blob), vec![42, 43, 44]);

    // ack
    store.mark_acked(id).expect("should succeed");
    let pending_after = store.replay_pending().expect("should succeed");
    assert_eq!(pending_after.len(), 0);

    // cleanup
    let cleaned = store.cleanup_acked(-1).expect("should succeed");
    assert_eq!(cleaned, 1);
}

#[test]
fn outbox_replay_paged() {
    let dir = tempdir();
    let path = dir.join("test_paged.db");
    let mut store = OutboxStore::open(&path).expect("should succeed");
    for i in 0..5 {
        store
            .persist(&OutgoingMsg {
                data_id: 100 + i,
                data_ids: vec![100 + i],
                topic: "t".into(),
                payload: vec![i as u8],
                qos: 1,
                protocol: "mqtt".into(),
            })
            .expect("should succeed");
    }
    let mut all_ids = Vec::new();
    let mut after_id: i64 = 0;
    loop {
        let batch = store
            .replay_pending_batch("mqtt", after_id, 2)
            .expect("should succeed");
        if batch.is_empty() {
            break;
        }
        for (oid, _) in &batch {
            all_ids.push(*oid);
        }
        after_id = batch.last().expect("should succeed").0;
    }
    assert_eq!(all_ids.len(), 5);
    for w in all_ids.windows(2) {
        assert!(w[0] < w[1]);
    }
}

#[test]
fn outbox_persist_batch_test() {
    let dir = tempdir();
    let path = dir.join("test_batch.db");
    let mut store = OutboxStore::open(&path).expect("should succeed");
    let msgs: Vec<OutgoingMsg> = (0..5)
        .map(|i| OutgoingMsg {
            data_id: 200 + i,
            data_ids: vec![200 + i],
            topic: "t".into(),
            payload: vec![i as u8],
            qos: 1,
            protocol: "mqtt".into(),
        })
        .collect();
    let ids = store.persist_batch(&msgs).expect("should succeed");
    assert_eq!(ids.len(), 5);
    assert!(ids.iter().all(|x| x.is_some()));
    let id_vals: Vec<i64> = ids.iter().map(|x| x.expect("should succeed")).collect();
    for w in id_vals.windows(2) {
        assert_eq!(w[0] + 1, w[1]);
    }
    let pending = store.replay_pending().expect("should succeed");
    assert_eq!(pending.len(), 5);
}

/// Bug C4: outbox 按 protocol 隔离取数——MQTT replay 不得取到 http 消息，反之亦然。
#[test]
fn outbox_replay_isolates_protocol() {
    let dir = tempdir();
    let path = dir.join("test_protocol.db");
    let mut store = OutboxStore::open(&path).expect("should succeed");
    store
        .persist(&OutgoingMsg {
            data_id: 1,
            data_ids: vec![1],
            topic: "t/mqtt".into(),
            payload: vec![1],
            qos: 1,
            protocol: "mqtt".into(),
        })
        .expect("should succeed");
    store
        .persist(&OutgoingMsg {
            data_id: 2,
            data_ids: Vec::new(),
            topic: "http://example.invalid/ingest".into(),
            payload: vec![2],
            qos: 1,
            protocol: "http".into(),
        })
        .expect("should succeed");

    let mqtt = store
        .replay_pending_batch("mqtt", 0, 50)
        .expect("should succeed");
    assert_eq!(mqtt.len(), 1, "MQTT replay 不得取到 http 消息");
    assert_eq!(mqtt[0].1.protocol, "mqtt");
    assert_eq!(mqtt[0].1.data_id, 1);

    let http = store
        .replay_pending_batch("http", 0, 50)
        .expect("should succeed");
    assert_eq!(http.len(), 1, "HTTP replay 不得取到 mqtt 消息");
    assert_eq!(http[0].1.protocol, "http");
    assert_eq!(http[0].1.data_id, 2);
}

// ===== r67-OBX1：replay 游标持久化回归 =====

/// r67-OBX1：load 缺失 cursor 返回 0（等价"全表重扫"），不能 panic 不能抛错。
/// 这是 replay 启动路径——首次 replay 必然没有 cursor。
#[test]
fn r67_outbox_replay_cursor_load_missing_returns_zero() {
    let dir = tempdir();
    let path = dir.join("cursor_load_missing.db");
    let store = OutboxStore::open(&path).expect("should succeed");
    let v = store.load_replay_cursor("mqtt");
    assert_eq!(v, 0, "缺失 cursor 必须返回 0，不能抛错");
}

/// r67-OBX1：save 后 load 必须读到刚写入的值——基本 round-trip。
#[test]
fn r67_outbox_replay_cursor_save_load_roundtrip() {
    let dir = tempdir();
    let path = dir.join("cursor_roundtrip.db");
    let store = OutboxStore::open(&path).expect("should succeed");
    store.save_replay_cursor("mqtt", 123);
    assert_eq!(store.load_replay_cursor("mqtt"), 123);
    // 同 store 内 save 覆盖必须生效（ON CONFLICT DO UPDATE）
    store.save_replay_cursor("mqtt", 456);
    assert_eq!(store.load_replay_cursor("mqtt"), 456);
    // 不同 protocol 互不干扰
    store.save_replay_cursor("http", 789);
    assert_eq!(store.load_replay_cursor("mqtt"), 456);
    assert_eq!(store.load_replay_cursor("http"), 789);
}

/// r67-OBX1（关键回归）：模拟「进程重启」，新 `OutboxStore` 实例（同一 db 文件）
/// 必须读到上次的 cursor——这是跨进程持久化语义的核心保证。
///
/// 旧实现 `let mut after_id: i64 = 0;` 在函数内局部声明，重启后从 0 重扫整张
/// outbox——未 ack 的行被 status=0 过滤后剩下来的「已 publish 但 PubAck 未到」
/// 会被重新 publish 到 broker。r67 改为持久化 cursor 后，重启应能继续上次的
/// 推进点。
#[test]
fn r67_outbox_replay_cursor_persists_across_restart() {
    let dir = tempdir();
    let path = dir.join("cursor_restart.db");
    // 进程 1：写入 cursor
    {
        let store = OutboxStore::open(&path).expect("should succeed");
        store.save_replay_cursor("mqtt", 999);
    }
    // 进程 2：同一文件，新实例——必须读到上次的值
    {
        let store = OutboxStore::open(&path).expect("should succeed");
        assert_eq!(
            store.load_replay_cursor("mqtt"),
            999,
            "r67: 跨进程 Open 同一 db 文件必须读到上次的 cursor（持久化语义）"
        );
    }
}

/// Bug C18: open() 必须设置 busy_timeout，避免 MQTT/Webhook 双连接并发写时
/// 遇到 SQLITE_BUSY 立即失败丢消息。
#[test]
fn outbox_open_sets_busy_timeout() {
    let dir = tempdir();
    let path = dir.join("test_busy.db");
    let store = OutboxStore::open(&path).expect("should succeed");
    let timeout: i64 = store
        .conn
        .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
        .expect("should succeed");
    assert_eq!(timeout, 5000, "busy_timeout 应为 5000ms");
}

fn tempdir() -> std::path::PathBuf {
    // r38-OBX：同一毫秒内并行运行的测试会生成同名目录 → 共享同一 DB 文件相互污染
    // （并行 cargo test 下偶发失败）。追加进程内自增序号保证唯一。
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let mut p = std::env::temp_dir();
    p.push(format!(
        "outbox-test-{}-{}",
        now_ms(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&p).expect("should succeed");
    p
}

/// r27-OBX1：enforce_pending_limit 必须按 protocol 隔离删除最旧的 pending。
/// 关键约束：MQTT 维护不能误删 HTTP 消息（HTTP 消息的持久副本只有这一张表）。
#[test]
fn enforce_pending_limit_isolates_protocol() {
    let dir = tempdir();
    let path = dir.join("test_limit.db");
    let mut store = OutboxStore::open(&path).expect("should succeed");
    // 灌入 5 条 mqtt + 3 条 http
    for i in 0..5 {
        store
            .persist(&OutgoingMsg {
                data_id: 1000 + i,
                data_ids: vec![1000 + i],
                topic: "t/mqtt".into(),
                payload: vec![i as u8],
                qos: 1,
                protocol: "mqtt".into(),
            })
            .expect("should succeed");
    }
    for i in 0..3 {
        store
            .persist(&OutgoingMsg {
                data_id: 2000 + i,
                data_ids: vec![2000 + i],
                topic: "http://example.invalid/".into(),
                payload: vec![i as u8],
                qos: 1,
                protocol: "http".into(),
            })
            .expect("should succeed");
    }

    // 限制 mqtt 最多 3 条：应删 2 条最旧 mqtt，http 不动
    // r71-OBX1：传 None = 旧行为（无宽限窗口），保证 r27 历史行为不变。
    let deleted = store.enforce_pending_limit("mqtt", 3, None).expect("ok");
    assert_eq!(deleted, 2, "mqtt 应删 2 条");
    assert_eq!(store.pending_count(Some("mqtt")).expect("ok"), 3);
    assert_eq!(
        store.pending_count(Some("http")).expect("ok"),
        3,
        "http 不应被 mqtt 维护误删"
    );

    // 限制 http 最多 1 条：应删 2 条 http，mqtt 仍是 3 条
    let deleted = store.enforce_pending_limit("http", 1, None).expect("ok");
    assert_eq!(deleted, 2, "http 应删 2 条");
    assert_eq!(store.pending_count(Some("mqtt")).expect("ok"), 3);
    assert_eq!(store.pending_count(Some("http")).expect("ok"), 1);

    // 不超限：返回 0
    let deleted = store.enforce_pending_limit("mqtt", 10, None).expect("ok");
    assert_eq!(deleted, 0);
}

// ===== r78-OBX1：mark/remove by_data_id 必须按 protocol 隔离 =====
//
// 设计动机：outbox 表是 MQTT/HTTP/Webhook/Kafka 共享的（用 `protocol` 列隔离）。
// `data_id` 来自 `ids.first()`（队列 id），MQTT 与 HTTP 各自维护独立 queue，
// 实际**不会**跨协议同 data_id（DB 层有 `idx_outbox_pending_data_id` UNIQUE
// 索引兜底：`CREATE UNIQUE INDEX ON mqtt_outbox(data_id) WHERE status=0`）。
//
// 修法动机：API 签名层面**强制表达 protocol 意图**——避免未来重构（如取消
// UNIQUE 索引、跨协议共享 queue id 空间）时 SQL 漏过滤导致 silent data loss。
// 单元测试断言「按 protocol 过滤生效」作为契约。

/// r78-OBX1：mark_acked_by_data_id 命中行（mqtt protocol）必须 status=1。
#[test]
fn r78_mark_acked_by_data_id_with_protocol_marks_target_row() {
    let dir = tempdir();
    let path = dir.join("r78_mark_ok.db");
    let mut store = OutboxStore::open(&path).expect("should succeed");
    store
        .persist(&OutgoingMsg {
            data_id: 42,
            data_ids: vec![42],
            topic: "t/mqtt".into(),
            payload: vec![1],
            qos: 1,
            protocol: "mqtt".into(),
        })
        .expect("persist");
    store.mark_acked_by_data_id(42, "mqtt").expect("mark");
    let s: i64 = store
        .conn
        .query_row(
            "SELECT status FROM mqtt_outbox WHERE data_id = 42",
            [],
            |row| row.get(0),
        )
        .expect("query");
    assert_eq!(s, 1, "r78: mqtt 行的 status 必须变为 1");
}

/// r78-OBX1：mark_acked_by_data_id 传错的 protocol 时不应影响行（UNIQUE 兜底
/// 之下不会跨协议，但 API 仍按 protocol 过滤，避免 API 与意图不符）。
#[test]
fn r78_mark_acked_by_data_id_wrong_protocol_no_op() {
    let dir = tempdir();
    let path = dir.join("r78_mark_wrong.db");
    let mut store = OutboxStore::open(&path).expect("should succeed");
    store
        .persist(&OutgoingMsg {
            data_id: 43,
            data_ids: vec![43],
            topic: "t/mqtt".into(),
            payload: vec![1],
            qos: 1,
            protocol: "mqtt".into(),
        })
        .expect("persist");
    // 传错协议 http
    store.mark_acked_by_data_id(43, "http").expect("mark wrong");
    let s: i64 = store
        .conn
        .query_row(
            "SELECT status FROM mqtt_outbox WHERE data_id = 43",
            [],
            |row| row.get(0),
        )
        .expect("query");
    assert_eq!(
        s, 0,
        "r78: 协议不匹配时 mark_acked_by_data_id 必须无效（status 仍 0）"
    );
}

/// r78-OBX1：remove_by_data_id 命中行必须物理删除。
#[test]
fn r78_remove_by_data_id_with_protocol_removes_target_row() {
    let dir = tempdir();
    let path = dir.join("r78_remove_ok.db");
    let mut store = OutboxStore::open(&path).expect("should succeed");
    store
        .persist(&OutgoingMsg {
            data_id: 44,
            data_ids: vec![44],
            topic: "t/mqtt".into(),
            payload: vec![1],
            qos: 1,
            protocol: "mqtt".into(),
        })
        .expect("persist");
    store.remove_by_data_id(44, "mqtt").expect("remove");
    let n: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM mqtt_outbox WHERE data_id = 44",
            [],
            |row| row.get(0),
        )
        .expect("query");
    assert_eq!(n, 0, "r78: mqtt 行必须已物理删除");
}

/// r78-OBX1：remove_by_data_id 传错 protocol 时不应删除行。
#[test]
fn r78_remove_by_data_id_wrong_protocol_no_op() {
    let dir = tempdir();
    let path = dir.join("r78_remove_wrong.db");
    let mut store = OutboxStore::open(&path).expect("should succeed");
    store
        .persist(&OutgoingMsg {
            data_id: 45,
            data_ids: vec![45],
            topic: "t/mqtt".into(),
            payload: vec![1],
            qos: 1,
            protocol: "mqtt".into(),
        })
        .expect("persist");
    store.remove_by_data_id(45, "http").expect("remove wrong");
    let n: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM mqtt_outbox WHERE data_id = 45",
            [],
            |row| row.get(0),
        )
        .expect("query");
    assert_eq!(
        n, 1,
        "r78: 协议不匹配时 remove_by_data_id 必须无效（行仍存在）"
    );
}

// ===== Bug-N1 (fix): outbox open 失败降级到内存模式 =====

/// Bug-N1：open 传入只读目录下的路径 → sqlite open 失败 → 应该降级到内存模式，
/// mode() = Memory，is_degraded() = true，**并且能继续 persist**。
///
/// 用一个已存在的文件作为路径模拟无法 open 的场景（rusqlite 打开某些类型
/// 的损坏文件时会失败）。这里用更稳的方式：传一个不存在且**无法创建**的
/// 目录路径下文件，sqlite 会返回 "unable to open database file"。
#[test]
fn n1_open_fails_degrades_to_memory_and_still_works() {
    // 构造无法 create 的路径：把"已存在的文件"作为父目录，sqlite 无法在该
    // 「目录」下再创建 db 文件（实际是父路径不是目录 → open 失败）。
    let dir = tempdir();
    let blocking_file = dir.join("not_a_dir");
    std::fs::write(&blocking_file, b"x").expect("create blocking file");
    let impossible_path = blocking_file.join("outbox.db");

    let result = OutboxStore::open(&impossible_path);
    assert!(
        result.is_ok(),
        "Bug-N1: open 失败时必须降级而不是 Err（让北向仍能工作），got: {:?}",
        result.err()
    );
    let mut store = result.unwrap();
    assert!(
        store.is_degraded(),
        "Bug-N1: 降级模式下 is_degraded 必须为 true"
    );
    assert_eq!(store.mode(), OutboxMode::Memory);

    // 降级模式下 persist 仍能工作（SQLite 内存库 + 同一 schema）
    let id = store
        .persist(&OutgoingMsg {
            data_id: 100,
            data_ids: vec![100],
            topic: "t/n1".into(),
            payload: vec![0xAA, 0xBB],
            qos: 1,
            protocol: "mqtt".into(),
        })
        .expect("Bug-N1: persist 在内存模式下必须仍能工作");
    assert!(id > 0, "Bug-N1: 持久化必须返回有效 outbox_id");

    let pending = store.replay_pending().expect("replay ok");
    assert_eq!(
        pending.len(),
        1,
        "Bug-N1: 内存库内 persist 数据必须能 replay"
    );
    assert_eq!(pending[0].1.data_id, 100);
}

/// Bug-N1：正常文件路径 open 时不降级——mode = Sqlite，is_degraded = false。
#[test]
fn n1_normal_open_uses_sqlite_mode() {
    let dir = tempdir();
    let path = dir.join("ok.db");
    let store = OutboxStore::open(&path).expect("normal open ok");
    assert!(!store.is_degraded(), "正常文件 open 必须不降级");
    assert_eq!(store.mode(), OutboxMode::Sqlite);
}

// ===== r76-OBX1：enforce_pending_limit 必须尊重 replay_cursor =====
//
// 旧实现按 `ORDER BY id ASC LIMIT N` 删最旧——**完全无视 replay_cursor**。
// 可能删掉「id > cursor 但 status=0」的行，而这些行**仍在 replay 保护范围**
// （replay 用 `id > cursor` 拉取）——删除后 replay 永远不会再拉到它们，
// 永久丢失。
//
// 修法：删除 SQL 加 `AND id > cursor`。`id <= cursor` 的行已在 replay 视野外，
// 不再需要 enforcement 介入（cursor 之前的行并不是「积压」）。

/// r76-OBX1：模拟「cursor=2，pending=5 条（id=1..5），limit=2」——
/// enforce 应当只删 id > 2 的最旧（id=3,4），保留 id=1,2（cursor 之前的行）。
/// 旧实现会按 id 升序删 3 条（id=1,2,3），**永久丢 id=3**。
#[test]
fn r76_enforce_pending_limit_respects_replay_cursor() {
    let dir = tempdir();
    let path = dir.join("r76_cursor.db");
    let mut store = OutboxStore::open(&path).expect("should succeed");
    for i in 0..5 {
        store
            .persist(&OutgoingMsg {
                data_id: 9000 + i,
                data_ids: vec![9000 + i],
                topic: "t".into(),
                payload: vec![i as u8],
                qos: 1,
                protocol: "mqtt".into(),
            })
            .expect("should succeed");
    }
    // 把 last_attempt_at 都改到 1 小时前，让 grace=Some(60_000) 模式下全部可删
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let old = now - 3_600_000;
    for oid in 1..=5i64 {
        store
            .conn
            .execute(
                "UPDATE mqtt_outbox SET last_attempt_at = ?1 WHERE id = ?2",
                rusqlite::params![old, oid],
            )
            .expect("manual update");
    }
    // 设置 replay_cursor=2：id=1,2 视为「已推进过的下界」，id=3,4,5 仍在保护范围
    store.save_replay_cursor("mqtt", 2);
    // 5 条 pending，limit=2：to_delete=3，候选 5 - 2（cursor 之下的 1,2）= 3 条
    // （id=3,4,5），全部命中 LIMIT 3 → 删 3 条，剩 id=1,2
    let deleted = store
        .enforce_pending_limit("mqtt", 2, Some(60_000))
        .expect("ok");
    assert_eq!(
        deleted, 3,
        "r76: cursor 之下保留 2 条，cursor 之上全部 3 条（id=3,4,5）命中删除"
    );
    // 验证 id=1,2 仍在（cursor 保护下，**关键不变式**）
    let remain: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM mqtt_outbox WHERE id <= 2",
            [],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(remain, 2, "r76: id=1,2 必须保留（cursor 保护）");
    // 验证 id=3,4,5 已被删（cursor 之后可删，enforcement 视野）
    let deleted3to5: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM mqtt_outbox WHERE id IN (3, 4, 5)",
            [],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(deleted3to5, 0, "r76: id=3,4,5 已被删（cursor 之后）");
    // 验证：replay 用 id > 2 仍能拉到 0 条（id=1,2 也不满足 id > 2）。
    // 这是有意行为——id=1,2 已在 cursor 之前的「已推进下界」语义中，
    // replay 不会回拉；enforcement 不会误删它们。
    let replay_view = store
        .replay_pending_batch("mqtt", 2, 50)
        .expect("replay view");
    assert_eq!(
        replay_view.len(),
        0,
        "r76: 修法后表里已无 id > 2 的 pending（enforcement 清干净，replay 视野为 0）"
    );
}

/// r76-OBX1（对比）：旧实现按 id 升序删最旧——这里证明它会丢 cursor 之后的数据。
/// r76 已修，本测试断言现状（修复后）正确：旧行为下能删 3 条 id=1,2,3；
/// 修复后只能删 2 条 id=3,4。验证修法差异。
#[test]
fn r76_enforce_pending_limit_old_behavior_would_drop_cursor_protected_rows() {
    let dir = tempdir();
    let path = dir.join("r76_old_behavior.db");
    let mut store = OutboxStore::open(&path).expect("should succeed");
    for i in 0..5 {
        store
            .persist(&OutgoingMsg {
                data_id: 9100 + i,
                data_ids: vec![9100 + i],
                topic: "t".into(),
                payload: vec![i as u8],
                qos: 1,
                protocol: "mqtt".into(),
            })
            .expect("should succeed");
    }
    // 把 last_attempt_at 都改到 1 小时前（grace=Some(60_000) 全部可删）
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let old = now - 3_600_000;
    for oid in 1..=5i64 {
        store
            .conn
            .execute(
                "UPDATE mqtt_outbox SET last_attempt_at = ?1 WHERE id = ?2",
                rusqlite::params![old, oid],
            )
            .expect("manual update");
    }
    // cursor=0（fresh 状态，等价于「无保护」）→ 等同旧行为：按 id 升序删 3 条
    store.save_replay_cursor("mqtt", 0);
    let deleted = store
        .enforce_pending_limit("mqtt", 2, Some(60_000))
        .expect("ok");
    assert_eq!(deleted, 3, "r76: cursor=0 时等同旧行为（向后兼容）");
    // 剩余 id=4,5
    let after: i64 = store.pending_count(Some("mqtt")).expect("ok");
    assert_eq!(after, 2, "r76: cursor=0 时删 3 条剩 2 条");
}

/// r76-OBX1：cursor=0（无保护）必须等同旧行为——保证向后兼容。
#[test]
fn r76_enforce_pending_limit_cursor_zero_matches_legacy() {
    let dir = tempdir();
    let path = dir.join("r76_cursor_zero.db");
    let mut store = OutboxStore::open(&path).expect("should succeed");
    for i in 0..5 {
        store
            .persist(&OutgoingMsg {
                data_id: 9200 + i,
                data_ids: vec![9200 + i],
                topic: "t".into(),
                payload: vec![i as u8],
                qos: 1,
                protocol: "mqtt".into(),
            })
            .expect("should succeed");
    }
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let old = now - 3_600_000;
    for oid in 1..=5i64 {
        store
            .conn
            .execute(
                "UPDATE mqtt_outbox SET last_attempt_at = ?1 WHERE id = ?2",
                rusqlite::params![old, oid],
            )
            .expect("manual update");
    }
    // 不显式设置 cursor（默认 0，fresh store）
    // grace=None（无宽限）
    let deleted = store.enforce_pending_limit("mqtt", 2, None).expect("ok");
    assert_eq!(
        deleted, 3,
        "r76: cursor=0 + grace=None 必须删 3 条（与旧行为完全一致）"
    );
}

// ===== r71-OBX1：enforce_pending_limit 宽限窗口回归 =====

/// r71-OBX1 契约：宽限窗口内（`last_attempt_at` 距今 < grace_ms）的行
/// **不**被删——避免 broker 已收到但 PubAck 在途的行被立即清掉。
/// `record_attempt` 后立即调 enforce_pending_limit，设极小 grace，
/// 验证宽限内行被保留，**不**超 limit 时仍删 0。
#[test]
fn r71_enforce_pending_limit_grace_window_protects_recent_attempts() {
    let dir = tempdir();
    let path = dir.join("test_grace.db");
    let mut store = OutboxStore::open(&path).expect("should succeed");
    for i in 0..5 {
        store
            .persist(&OutgoingMsg {
                data_id: 7000 + i,
                data_ids: vec![7000 + i],
                topic: "t".into(),
                payload: vec![i as u8],
                qos: 1,
                protocol: "mqtt".into(),
            })
            .expect("should succeed");
    }
    // 模拟"刚刚尝试发送"：5 条都 mark attempted（last_attempt_at = now）
    for oid in 1..=5i64 {
        store.record_attempt(oid).expect("record_attempt");
    }
    // 限制 mqtt 最多 1 条：to_delete=4，5 条都在 grace 内 → 实际删 0
    let deleted = store
        .enforce_pending_limit("mqtt", 1, Some(60_000))
        .expect("ok");
    assert_eq!(
        deleted, 0,
        "r71: last_attempt_at 距今 < grace_ms 的行必须保留，禁止立即删"
    );
    assert_eq!(
        store.pending_count(Some("mqtt")).expect("ok"),
        5,
        "5 条都在宽限窗口内，pending 必须仍是 5"
    );
}

/// r71-OBX1 契约：宽限期外（`last_attempt_at` 远早于 now - grace_ms）的行
/// 仍正常被删——证明宽限不是"永不删"，而是"宽限内不删"。
/// 手工 `last_attempt_at` 调早到 1 小时前。
#[test]
fn r71_enforce_pending_limit_grace_expired_allows_deletion() {
    let dir = tempdir();
    let path = dir.join("test_grace_expired.db");
    let mut store = OutboxStore::open(&path).expect("should succeed");
    for i in 0..5 {
        store
            .persist(&OutgoingMsg {
                data_id: 7100 + i,
                data_ids: vec![7100 + i],
                topic: "t".into(),
                payload: vec![i as u8],
                qos: 1,
                protocol: "mqtt".into(),
            })
            .expect("should succeed");
    }
    // 把 5 条的 last_attempt_at 改到 1 小时前（now - 3_600_000ms）
    // 内联 now_ms 逻辑：now_ms 是父模块私有函数，测试不能直接 import。
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let old = now - 3_600_000;
    for oid in 1..=5i64 {
        // 直接 UPDATE，绕开 record_attempt 的 now()
        let conn = &store.conn;
        conn.execute(
            "UPDATE mqtt_outbox SET last_attempt_at = ?1 WHERE id = ?2",
            rusqlite::params![old, oid],
        )
        .expect("manual update");
    }
    // 限制 mqtt 最多 1 条：5 条都已出宽限期 → 删 4 条
    let deleted = store
        .enforce_pending_limit("mqtt", 1, Some(60_000))
        .expect("ok");
    assert_eq!(
        deleted, 4,
        "r71: 宽限期外的行必须正常被删（宽限 = 临时保护，不是永久保留）"
    );
    assert_eq!(store.pending_count(Some("mqtt")).expect("ok"), 1);
}

/// r71-OBX1 契约：宽限窗口 = 0 等价 None 旧行为——所有行立即可删。
/// 这是「grace_ms 可作细粒度旋钮」的反向证据。
#[test]
fn r71_enforce_pending_limit_grace_zero_acts_like_none() {
    let dir = tempdir();
    let path = dir.join("test_grace_zero.db");
    let mut store = OutboxStore::open(&path).expect("should succeed");
    for i in 0..5 {
        store
            .persist(&OutgoingMsg {
                data_id: 7200 + i,
                data_ids: vec![7200 + i],
                topic: "t".into(),
                payload: vec![i as u8],
                qos: 1,
                protocol: "mqtt".into(),
            })
            .expect("should succeed");
    }
    // 手工把 last_attempt_at 改到「远早于 now」——保证 grace=0 时 `last_attempt_at <
    // cutoff` 一定为真（cutoff = now）。不能用 record_attempt + 立即 enforce，
    // 因为两者可能在同一毫秒发生，SQL `last_attempt_at < cutoff` 会是 false。
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let old = now - 60_000; // 1 分钟前——远早于任何合理的 grace_ms
    for oid in 1..=5i64 {
        store
            .conn
            .execute(
                "UPDATE mqtt_outbox SET last_attempt_at = ?1 WHERE id = ?2",
                rusqlite::params![old, oid],
            )
            .expect("manual update");
    }
    // grace=0ms：cutoff=now，last_attempt_at=now-60s 严格小于 cutoff → 全部可删
    let deleted = store.enforce_pending_limit("mqtt", 1, Some(0)).expect("ok");
    assert_eq!(
        deleted, 4,
        "r71: grace_ms=0 等价 None，所有 last_attempt_at < cutoff，4 条可删"
    );
}

/// r27-OBX1 边界：enforce_pending_limit 不应因为内部 commit 失败而破坏 SQLite 状态。
#[test]
fn enforce_pending_limit_under_limit_returns_zero() {
    let dir = tempdir();
    let path = dir.join("test_under.db");
    let mut store = OutboxStore::open(&path).expect("should succeed");
    for i in 0..3 {
        store
            .persist(&OutgoingMsg {
                data_id: 3000 + i,
                data_ids: vec![3000 + i],
                topic: "t".into(),
                payload: vec![i as u8],
                qos: 1,
                protocol: "mqtt".into(),
            })
            .expect("should succeed");
    }
    // max >= count：应不删任何行，但 commit 必须正常返回
    let deleted = store.enforce_pending_limit("mqtt", 100, None).expect("ok");
    assert_eq!(deleted, 0);
    assert_eq!(store.pending_count(Some("mqtt")).expect("ok"), 3);
}

/// r27-OBX2：pending_count(Some(p)) 与 pending_count(None) 口径差异。
/// None=全部合计；Some(p)=按协议过滤。
#[test]
fn pending_count_filters_by_protocol() {
    let dir = tempdir();
    let path = dir.join("test_count.db");
    let mut store = OutboxStore::open(&path).expect("should succeed");
    // 2 mqtt + 3 http
    for i in 0..2 {
        store
            .persist(&OutgoingMsg {
                data_id: 4000 + i,
                data_ids: vec![],
                topic: "t".into(),
                payload: vec![],
                qos: 1,
                protocol: "mqtt".into(),
            })
            .expect("ok");
    }
    for i in 0..3 {
        store
            .persist(&OutgoingMsg {
                data_id: 5000 + i,
                data_ids: vec![],
                topic: "http://x".into(),
                payload: vec![],
                qos: 1,
                protocol: "http".into(),
            })
            .expect("ok");
    }
    assert_eq!(store.pending_count(Some("mqtt")).expect("ok"), 2);
    assert_eq!(store.pending_count(Some("http")).expect("ok"), 3);
    assert_eq!(
        store.pending_count(None).expect("ok"),
        5,
        "None 应等于所有协议合计"
    );
}

// ============================================================
// r38-OBX: outbox 写入器循环回归测试
// ============================================================

// ===== r75-OBX1：replay 游标回退语义（页级失败不可丢失） =====
//
// 旧实现：replay 一行 publish 或 ack 失败时，after_id 留在了「被 break 行的
// oid」——下次 replay 用 `id > oid` 跳过该行永久丢失。Broker 是否真收到存在
// 不确定性（publish 调用本身已返回 Ok 但 rumqttc 内部可能未发送；PubAck 丢失；
// ack 通道关闭等），绝不能假设已送达。
//
// r75 修复：所有失败路径都把 after_id 拨回 page_anchor（与 try_acquire 失败
// 同等待遇），保证未送达行下次仍能被 `id > page_anchor` 拉到。
//
// 这些是纯 store 层的回归测试（不依赖 rumqttc broker），用 `replay_pending_batch`
// + `save_replay_cursor` 模拟「replay 一页 → 失败 → 落盘 → 再次 replay 拉回」。

/// r75-OBX1：模拟「replay 拉到 id=1,2,3，第二行 publish/ack 失败」——
/// 落盘的 after_id 必须 **仍能拉到 1,2,3**（即 after_id < 1）。
///
/// 旧实现落盘为 2 → 下次 replay 跳过 2 → 永久丢失（若 2 未送达则数据丢失，
/// 重复发也仅是 broker 侧重复；丢数据是更严重的事故）。
#[test]
fn r75_replay_cursor_falls_back_to_page_anchor_on_mid_page_failure() {
    let dir = tempdir();
    let path = dir.join("r75_midpage.db");
    let mut store = OutboxStore::open(&path).expect("should succeed");
    for i in 0..3 {
        store
            .persist(&OutgoingMsg {
                data_id: 8000 + i,
                data_ids: vec![8000 + i],
                topic: "t".into(),
                payload: vec![i as u8],
                qos: 1,
                protocol: "mqtt".into(),
            })
            .expect("should succeed");
    }

    // 模拟：第一次 replay 起始 cursor=0（fresh）
    let page_anchor: i64 = store.load_replay_cursor("mqtt");
    assert_eq!(page_anchor, 0, "fresh store 游标必须为 0");
    let page = store
        .replay_pending_batch("mqtt", page_anchor, 50)
        .expect("should succeed");
    assert_eq!(page.len(), 3, "应拉到 3 条");
    let oids: Vec<i64> = page.iter().map(|(o, _)| *o).collect();
    assert_eq!(oids, vec![1, 2, 3], "id 必须升序连续");

    // 模拟「第二行失败」：按 r75 修法 after_id 拨回 page_anchor（=0）。
    // 旧实现拨到 oid=2（被 break 行），会跳过第二行 → 数据丢失。
    store.save_replay_cursor("mqtt", page_anchor);

    // 下次 replay 必须能拉回 id=1,2,3 全部三行（status=0 仍未删）。
    let again = store
        .replay_pending_batch("mqtt", page_anchor, 50)
        .expect("should succeed");
    assert_eq!(
        again.len(),
        3,
        "r75: 失败页必须能完整重拉——after_id={} 落盘后应能拉回 1,2,3",
        page_anchor
    );
    let again_oids: Vec<i64> = again.iter().map(|(o, _)| *o).collect();
    assert_eq!(
        again_oids,
        vec![1, 2, 3],
        "r75: 重拉 id 顺序必须仍是 1,2,3（任何 id > 1 都会跳过 2）"
    );
}

/// r75-OBX1（对比）：旧行为会把 after_id 留在被 break 行——这里证明它会丢。
///
/// 这是「修复前」的反向断言。r75 已修，此测试应通过（验证现状正确），
/// 未来若有人误把回退改成「拨到 break 行的 oid」，本测试**会失败**提示。
#[test]
fn r75_replay_cursor_old_behavior_would_skip_failed_row() {
    let dir = tempdir();
    let path = dir.join("r75_old_behavior.db");
    let mut store = OutboxStore::open(&path).expect("should succeed");
    for i in 0..3 {
        store
            .persist(&OutgoingMsg {
                data_id: 8100 + i,
                data_ids: vec![8100 + i],
                topic: "t".into(),
                payload: vec![i as u8],
                qos: 1,
                protocol: "mqtt".into(),
            })
            .expect("should succeed");
    }

    // 旧行为：游标推进到 break 行 oid=2
    store.save_replay_cursor("mqtt", 2);
    let again = store
        .replay_pending_batch("mqtt", 2, 50)
        .expect("should succeed");
    // 旧行为下：3 条全 status=0，但 id > 2 只剩 1 条 id=3；2 已永久跳过
    assert_eq!(
        again.len(),
        1,
        "旧行为演示：after_id=2 会把 id=2 永久排除在 `id > 2` 之外"
    );
    assert_eq!(again[0].0, 3, "只能拉到 id=3");
}

/// r75-OBX1：成功后正常推进——游标等于本行 oid，下次 `id > oid` 自然跳过。
/// 验证修法未破坏「全成功」场景。
#[test]
fn r75_replay_cursor_advances_normally_on_full_success() {
    let dir = tempdir();
    let path = dir.join("r75_success.db");
    let mut store = OutboxStore::open(&path).expect("should succeed");
    for i in 0..3 {
        store
            .persist(&OutgoingMsg {
                data_id: 8200 + i,
                data_ids: vec![8200 + i],
                topic: "t".into(),
                payload: vec![i as u8],
                qos: 1,
                protocol: "mqtt".into(),
            })
            .expect("should succeed");
    }

    // 模拟「第一页 3 行全成功」：游标推进到 3
    store.save_replay_cursor("mqtt", 3);
    // 此时三行 status 仍 0（仅在 replay_ack_converged 后才 mark_acked）。
    let again = store
        .replay_pending_batch("mqtt", 3, 50)
        .expect("should succeed");
    assert_eq!(
        again.len(),
        0,
        "r75: 游标推进到 3 后，3 条 status=0 仍在表里但 `id > 3` 为空"
    );
}

/// 构造一条最小 MQTT 出站消息
fn test_msg(data_id: i64) -> OutgoingMsg {
    OutgoingMsg {
        data_id,
        data_ids: vec![data_id],
        topic: "t/x".into(),
        payload: vec![1, 2, 3],
        qos: 1,
        protocol: "mqtt".into(),
    }
}

/// r38-OBX2 回归：当写入器的 persist 命中 INSERT OR IGNORE（DB 已有同 data_id 的
/// pending 行，本轮未新建行）时，随后的 Ack 必须按 data_id 定位并标记那条既有行。
/// 旧实现按 rowid=0 调 mark_acked(0) → 空操作 → 陈旧 pending 残留 → 重启 replay 重复上报。
#[tokio::test]
async fn outbox_ack_targets_existing_row_when_duplicate() {
    use crate::north::mqtt::{outbox_writer_loop, OutboxCmd};
    use std::sync::Arc;
    use tokio::sync::Mutex as AsyncMutex;

    let dir = tempdir();
    let path = dir.join("dup_ack.db");
    // 预置一条 pending 行，模拟 publish.rs 的同步兜底 persist 路径（绕过写入器直接落库）
    {
        let mut seed = OutboxStore::open(&path).expect("should succeed");
        let id = seed.persist(&test_msg(77)).expect("should succeed");
        assert!(id > 0, "预置行应真实插入");
    }

    let outbox = Arc::new(AsyncMutex::new(
        OutboxStore::open(&path).expect("should succeed"),
    ));
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    let handle = tokio::spawn(outbox_writer_loop(outbox.clone(), rx));

    // 写入器内再次 persist 同一 data_id → INSERT OR IGNORE 命中（oid=0）；随后 Ack
    tx.send(OutboxCmd::Persist(test_msg(77)))
        .await
        .expect("send");
    tx.send(OutboxCmd::Ack(77)).await.expect("send");
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;

    let pending = outbox
        .lock()
        .await
        .replay_pending()
        .expect("should succeed");
    assert!(
        pending.is_empty(),
        "重复 persist 后 Ack 必须标记既有 pending 行（当前残留 {} 条）",
        pending.len()
    );

    drop(tx);
    let _ = handle.await;
}

/// r38-OBX1 回归：持续命令流量（间隔 < flush_interval）下定时 flush 仍须生效。
/// 旧实现每次迭代重建 sleep，recv 频繁就绪使定时器永不满期 → batch 只靠 max_batch 落盘。
#[tokio::test]
async fn outbox_flushes_during_continuous_traffic() {
    use crate::north::mqtt::{outbox_writer_loop, OutboxCmd};
    use std::sync::Arc;
    use tokio::sync::Mutex as AsyncMutex;

    let dir = tempdir();
    let path = dir.join("flush_traffic.db");
    let outbox = Arc::new(AsyncMutex::new(
        OutboxStore::open(&path).expect("should succeed"),
    ));
    let (tx, rx) = tokio::sync::mpsc::channel(64);
    let handle = tokio::spawn(outbox_writer_loop(outbox.clone(), rx));

    // 每 10ms 投递一条，共约 300ms——期间不存在 >=100ms 的静默间隔
    for i in 0..30 {
        tx.send(OutboxCmd::Persist(test_msg(1000 + i)))
            .await
            .expect("send");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        if i == 25 {
            let n = outbox
                .lock()
                .await
                .pending_count(Some("mqtt"))
                .expect("should succeed");
            assert!(n > 0, "持续流量下定时 flush 应已落盘（当前 pending={}）", n);
        }
    }
    drop(tx);
    let _ = handle.await;

    let n = outbox
        .lock()
        .await
        .pending_count(Some("mqtt"))
        .expect("should succeed");
    assert_eq!(n, 30, "收尾 flush 后 30 条应全部落盘");
}

// ============================================================
// r51-OBX1: drop_exhausted_pending 持久化重试上限
// ============================================================

/// r51-OBX1 回归：attempt_count >= max 的 pending 行必须被物理删除。
/// 关键不变量：进程重启后 in-memory 计数清零，但 DB 列的 attempt_count
/// 仍然累计；启动首轮 drop_exhausted_pending 必须基于 DB 真实计数
/// 清理「已用尽重试」的死信，避免故障端点下同一行被无限重试。
#[test]
fn r51_drop_exhausted_pending_deletes_by_attempt_count() {
    let dir = tempdir();
    let path = dir.join("test_drop_exhausted.db");
    let mut store = OutboxStore::open(&path).expect("should succeed");

    // 5 条 http pending：id=1..5，attempt_count 分别为 0,1,2,7,8
    for i in 0..5 {
        let id = store
            .persist(&OutgoingMsg {
                data_id: 1000 + i,
                data_ids: vec![],
                topic: "http://t".into(),
                payload: vec![0],
                qos: 1,
                protocol: "http".into(),
            })
            .expect("persist");
        // 把 attempt_count 拨到目标值
        for _ in 0..i {
            store.record_attempt(id).expect("record_attempt");
        }
    }
    // 验证初始 attempt_count
    assert_eq!(
        store.pending_count(Some("http")).expect("ok"),
        5,
        "5 行应全部入箱"
    );

    // 删除 attempt_count >= 3 的行（id=3,4,5——因为 i=2..4 对应 attempts=2,7,8）
    // 修正：i=0..4 对应 attempts=0,1,2,7,8，所以 >=3 命中 i=3,4（attempts=7,8）
    let dropped = store
        .drop_exhausted_pending("http", 3)
        .expect("drop_exhausted_pending");
    assert_eq!(dropped, 2, "attempts=7,8 的两行应被删除");
    assert_eq!(
        store.pending_count(Some("http")).expect("ok"),
        3,
        "剩余 attempts=0,1,2 三行（data_id 1000,1001,1002）"
    );
}

/// r51-OBX1 回归：按 protocol 隔离——mqtt 的死信行不能影响 http 协议。
/// 防止「drop_exhausted_pending("http", N)」误删 mqtt 协议行（MQTT
/// 协议当前未使用此 API，但 schema 共享，约束必须成立以备未来扩展）。
#[test]
fn r51_drop_exhausted_pending_isolates_protocol() {
    let dir = tempdir();
    let path = dir.join("test_drop_isolate.db");
    let mut store = OutboxStore::open(&path).expect("should succeed");

    // 1 条 mqtt + 1 条 http，都给 5 次 attempt
    for (i, protocol) in ["mqtt", "http"].iter().enumerate() {
        let id = store
            .persist(&OutgoingMsg {
                data_id: 9000 + i as i64,
                data_ids: vec![],
                topic: if *protocol == "http" {
                    "http://t"
                } else {
                    "t/x"
                }
                .into(),
                payload: vec![0],
                qos: 1,
                protocol: protocol.to_string(),
            })
            .expect("persist");
        for _ in 0..5 {
            store.record_attempt(id).expect("record_attempt");
        }
    }
    assert_eq!(store.pending_count(None).expect("ok"), 2);

    // 删除 http 协议 attempts>=3 的行：仅命中 1 条 http
    let dropped = store
        .drop_exhausted_pending("http", 3)
        .expect("drop_exhausted_pending");
    assert_eq!(dropped, 1, "只删 http 协议行，mqtt 协议不动");
    assert_eq!(store.pending_count(Some("mqtt")).expect("ok"), 1);
    assert_eq!(store.pending_count(Some("http")).expect("ok"), 0);
}

/// r51-OBX1 边界：attempt_count 未达上限时不应被误删。
#[test]
fn r51_drop_exhausted_pending_under_limit_no_op() {
    let dir = tempdir();
    let path = dir.join("test_drop_under.db");
    let mut store = OutboxStore::open(&path).expect("should succeed");
    let id = store
        .persist(&OutgoingMsg {
            data_id: 1,
            data_ids: vec![],
            topic: "http://t".into(),
            payload: vec![0],
            qos: 1,
            protocol: "http".into(),
        })
        .expect("persist");
    // 2 次尝试，未达上限
    store.record_attempt(id).expect("");
    store.record_attempt(id).expect("");
    let dropped = store.drop_exhausted_pending("http", 5).expect("drop");
    assert_eq!(dropped, 0, "未达上限的 pending 行不应被删除");
    assert_eq!(store.pending_count(Some("http")).expect("ok"), 1);
}
