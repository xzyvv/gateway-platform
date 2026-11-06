//! SQLite 队列后端「内存计数器 ↔ 库内真值」一致性回归测试。
//!
//! `SqliteStorage` 用两个内存 `AtomicI64` 承载队列的健康语义：
//!
//! - `depth` = 库内 `delivered = 0` 的行数（未送达）
//! - `in_flight` = 库内 `in_flight = 1` 的行数（在途）
//!
//! 这两个计数器直接驱动 `len()`、队列深度指标、`MAX_IN_FLIGHT` 反压阈值与
//! `queue_health` 判据。一旦与库内真值漂移，现场表现是
//! 「采集正常但数据越积越多」/「队列显示为空但库里还有数据」，而且**没有任何告警**。
//!
//! 本模块用**独立连接**直接查库取真值，在每一步操作后对账，
//! 把「计数器漂移」这类只在长跑中才显形的问题提前到 CI 秒级暴露。

#![cfg(not(feature = "encryption"))]

use std::path::PathBuf;
use std::sync::Arc;

use rusqlite::Connection;

use super::SqliteStorage;
use crate::engine::metrics::Metrics;
use crate::engine::model::DataPoint;
use crate::queue::storage::QueueStorage;

/// 测试夹具：真实 SQLite 后端 + 一个独立连接用于读取库内真值。
struct Fixture {
    storage: SqliteStorage,
    db_path: PathBuf,
    /// tempdir 必须活到 fixture 析构，否则 SQLite 文件会被提前删除。
    _dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("queue.sqlite");
        let metrics = Arc::new(Metrics::new());
        let storage = SqliteStorage::new(db_path.to_str().unwrap(), metrics).expect("sqlite init");
        Self {
            storage,
            db_path,
            _dir: dir,
        }
    }

    fn query_i64(&self, sql: &str) -> i64 {
        let conn = Connection::open(&self.db_path).expect("open db");
        conn.query_row(sql, [], |r| r.get(0)).expect("query")
    }

    /// 库内未送达行数（`depth` 语义真值）。
    fn db_unsent(&self) -> i64 {
        self.query_i64("SELECT COUNT(*) FROM data_queue WHERE delivered = 0")
    }

    /// 库内在途行数（`in_flight` 语义真值）。
    fn db_in_flight(&self) -> i64 {
        self.query_i64("SELECT COUNT(*) FROM data_queue WHERE in_flight = 1")
    }

    /// 库内总行数。
    fn db_total(&self) -> i64 {
        self.query_i64("SELECT COUNT(*) FROM data_queue")
    }

    /// 内存计数器 ↔ 库内真值对账。
    fn assert_consistent(&self, ctx: &str) {
        let mem_depth = self.storage.len() as i64;
        let mem_in_flight = self.storage.in_flight_count();
        let db_unsent = self.db_unsent();
        let db_in_flight = self.db_in_flight();
        assert_eq!(
            mem_depth, db_unsent,
            "[{}] depth 漂移：内存={} 库内未送达={}",
            ctx, mem_depth, db_unsent
        );
        assert_eq!(
            mem_in_flight, db_in_flight,
            "[{}] in_flight 漂移：内存={} 库内在途={}",
            ctx, mem_in_flight, db_in_flight
        );
    }

    /// 入队 n 个点，返回实际插入行数。
    fn enqueue(&self, n: usize) -> usize {
        let points: Vec<DataPoint> = (0..n)
            .map(|i| {
                let v = i as f64;
                DataPoint::good(&format!("p{}", i), "dev1", v, v, "u16")
            })
            .collect();
        self.storage.enqueue_batch(&points).expect("enqueue")
    }

    /// 入队 n 个指定时间戳的点（用于触发 retention 淘汰）。
    fn enqueue_at(&self, n: usize, ts: i64) -> usize {
        let points: Vec<DataPoint> = (0..n)
            .map(|i| {
                let v = i as f64;
                let mut p = DataPoint::good(&format!("p{}", i), "dev1", v, v, "u16");
                p.timestamp = ts;
                p.source_ts = ts;
                p
            })
            .collect();
        self.storage.enqueue_batch(&points).expect("enqueue")
    }

    /// 出队 n 个点，返回其 id。
    fn dequeue_ids(&self, n: usize) -> Vec<i64> {
        self.storage
            .dequeue_batch(n)
            .expect("dequeue")
            .into_iter()
            .map(|i| i.id)
            .collect()
    }

    /// 直接改库（仅测试用）：把指定 id 的 `dequeued_at` 改成旧时间，
    /// 用来构造 stale in_flight 场景。
    fn set_dequeued_at(&self, ids: &[i64], ts: i64) {
        let conn = Connection::open(&self.db_path).expect("open db");
        for id in ids {
            conn.execute(
                "UPDATE data_queue SET dequeued_at = ?1 WHERE id = ?2",
                rusqlite::params![ts, id],
            )
            .expect("update dequeued_at");
        }
    }
}

// ===== 基础不变量 =====

/// stale in_flight 判定阈值（与生产 `INFLIGHT_TIMEOUT_MS` 一致）。
/// `cleanup_stale_in_flight(cutoff)` 的入参是**截止时间**而不是「当前时间」——
/// 传 now 会把刚出队的行也判成 stale。
fn stale_cutoff() -> i64 {
    DataPoint::now() - 5 * 60 * 1000
}

#[test]
fn enqueue_makes_depth_match_db() {
    let f = Fixture::new();
    assert_eq!(f.enqueue(5), 5);
    f.assert_consistent("enqueue");
    assert_eq!(f.storage.len(), 5);
    assert_eq!(f.storage.in_flight_count(), 0);
}

#[test]
fn dequeue_then_ack_once_keeps_counters_exact() {
    let f = Fixture::new();
    f.enqueue(5);
    let ids = f.dequeue_ids(3);
    f.assert_consistent("dequeue");
    assert_eq!(f.storage.in_flight_count(), 3);
    assert_eq!(f.storage.len(), 5);

    f.storage.ack_batch(&ids).expect("ack");
    f.assert_consistent("ack");
    assert_eq!(f.storage.len(), 2);
    assert_eq!(f.storage.in_flight_count(), 0);
}

#[test]
fn release_returns_rows_to_dequeueable_state() {
    let f = Fixture::new();
    f.enqueue(5);
    let ids = f.dequeue_ids(5);
    f.storage.release_in_flight(&ids).expect("release");
    f.assert_consistent("release");
    assert_eq!(f.storage.in_flight_count(), 0);
    // 释放后必须能被重新出队（否则数据滞留不投递）。
    assert_eq!(f.dequeue_ids(5).len(), 5);
    f.assert_consistent("re-dequeue");
}

// ===== 幂等性：重复 ack / 重复 release =====

#[test]
fn duplicate_ack_must_not_double_dec_depth() {
    // 生产里 ack 有两条独立通道：publish 通道与 outbox replay 通道
    // （`replay_ack_converged`）。同一批 queue row id 被两条通道各 ack 一次时，
    // 第二次 ack 在库里已无可更新行，但内存计数器若仍按 ids.len() 扣减就会漂移。
    let f = Fixture::new();
    f.enqueue(5);
    let ids = f.dequeue_ids(3);
    f.storage.ack_batch(&ids).expect("first ack");
    f.assert_consistent("first ack");

    f.storage.ack_batch(&ids).expect("second ack");
    f.assert_consistent("duplicate ack");
    assert_eq!(f.storage.len(), 2, "重复 ack 不得重复扣减 depth");
    assert_eq!(f.storage.in_flight_count(), 0);
}

#[test]
fn duplicate_release_must_not_double_dec_in_flight() {
    let f = Fixture::new();
    f.enqueue(5);
    let ids = f.dequeue_ids(5);
    f.storage.release_in_flight(&ids[..2]).expect("release");
    f.assert_consistent("release");
    assert_eq!(f.storage.in_flight_count(), 3);

    f.storage.release_in_flight(&ids[..2]).expect("duplicate release");
    f.assert_consistent("duplicate release");
    assert_eq!(
        f.storage.in_flight_count(),
        3,
        "重复 release 不得重复扣减 in_flight"
    );
}

#[test]
fn ack_of_unknown_ids_must_not_move_counters() {
    // ack 的 ids 里混入了已被 retention/淘汰删除的行（或纯伪造 id）时，
    // 库内没有任何行被更新，内存计数器却按 ids.len() 扣减 → 深度虚低。
    let f = Fixture::new();
    f.enqueue(3);
    f.storage.ack_batch(&[9_999_999]).expect("ack unknown");
    f.assert_consistent("ack unknown id");
    assert_eq!(f.storage.len(), 3);
    assert_eq!(f.storage.in_flight_count(), 0);
}

#[test]
fn release_of_never_dequeued_rows_must_not_move_in_flight() {
    let f = Fixture::new();
    f.enqueue(3);
    let ids = f.dequeue_ids(3);
    f.storage.release_in_flight(&ids[..1]).expect("release");
    assert_eq!(f.storage.in_flight_count(), 2);

    // 对从未出队的行 release：库内 in_flight 本来就是 0，不应影响计数。
    f.storage
        .release_in_flight(&[9_999_998, 9_999_999])
        .expect("release unknown");
    f.assert_consistent("release unknown id");
    assert_eq!(f.storage.in_flight_count(), 2);
}

// ===== stale in_flight 回收 =====

#[test]
fn stale_inflight_reclaim_matches_db() {
    let f = Fixture::new();
    f.enqueue(5);
    let ids = f.dequeue_ids(5);
    f.assert_consistent("dequeue");
    assert_eq!(f.storage.in_flight_count(), 5);

    // 把出队时间改到 10 分钟前 → 超过 5 分钟 stale 阈值。
    let old = DataPoint::now() - 10 * 60 * 1000;
    f.set_dequeued_at(&ids, old);

    let released = f
        .storage
        .cleanup_stale_in_flight(stale_cutoff())
        .expect("cleanup");
    assert_eq!(released, 5);
    f.assert_consistent("stale reclaim");
    assert_eq!(f.storage.in_flight_count(), 0);
}

#[test]
fn stale_inflight_reclaim_keeps_unaffected_rows() {
    let f = Fixture::new();
    f.enqueue(5);
    let ids = f.dequeue_ids(5);
    // 只把前 2 条改旧。
    let old = DataPoint::now() - 10 * 60 * 1000;
    f.set_dequeued_at(&ids[..2], old);

    let released = f
        .storage
        .cleanup_stale_in_flight(stale_cutoff())
        .expect("cleanup");
    assert_eq!(released, 2);
    f.assert_consistent("partial stale reclaim");
    assert_eq!(f.storage.in_flight_count(), 3);
}

// ===== retention 淘汰 =====

#[test]
fn retention_prune_keeps_depth_consistent() {
    let f = Fixture::new();
    // 默认保留 30 天 → 造 40 天前的时间戳。
    let old_ts = DataPoint::now() - 40 * 24 * 3600 * 1000;
    f.enqueue_at(4, old_ts);
    f.enqueue_at(3, DataPoint::now());
    f.assert_consistent("enqueue");
    assert_eq!(f.storage.len(), 7);

    f.storage.maintenance().expect("maintenance");
    f.assert_consistent("after maintenance");
    assert_eq!(f.storage.len(), 3, "过期行必须从 depth 中扣除");
    assert_eq!(f.db_total(), 3);
}

// ===== 随机操作序列对账（长跑语义的前置体检）=====

#[test]
fn random_operation_sequence_keeps_counters_consistent() {
    let f = Fixture::new();
    // 固定种子的 xorshift，保证失败可复现。
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };

    let mut dequeued: Vec<i64> = Vec::new();
    let mut acked: Vec<i64> = Vec::new();

    for step in 0..400 {
        let op = next() % 100;
        match op {
            // 入队
            0..=29 => {
                let n = (next() % 5 + 1) as usize;
                f.enqueue(n);
            }
            // 出队
            30..=59 => {
                let n = (next() % 5 + 1) as usize;
                let ids = f.dequeue_ids(n);
                dequeued.extend(ids);
            }
            // ack 一批（可能含已 ack 过的 id —— 幂等路径）
            60..=79 => {
                let n = (next() % 4 + 1) as usize;
                let ids = pick(&mut next, &dequeued, n);
                if !ids.is_empty() {
                    f.storage.ack_batch(&ids).expect("ack");
                    acked.extend(ids);
                }
            }
            // release 一批（可能含已 release 过的 id —— 幂等路径）
            80..=92 => {
                let n = (next() % 4 + 1) as usize;
                let ids = pick(&mut next, &dequeued, n);
                if !ids.is_empty() {
                    f.storage.release_in_flight(&ids).expect("release");
                }
            }
            // 混合：ack 已 ack 过的 + release 已 release 过的
            _ => {
                let n = (next() % 3 + 1) as usize;
                let ids = pick(&mut next, &acked, n);
                if !ids.is_empty() {
                    f.storage.ack_batch(&ids).expect("re-ack");
                }
                let n = (next() % 3 + 1) as usize;
                let ids = pick(&mut next, &dequeued, n);
                if !ids.is_empty() {
                    f.storage.release_in_flight(&ids).expect("re-release");
                }
            }
        }
        f.assert_consistent(&format!("step {} op {}", step, op));
    }
}

/// 从 `pool` 里取最多 `n` 个 id（允许重复，用于构造幂等路径）。
fn pick<F: FnMut() -> u64>(next: &mut F, pool: &[i64], n: usize) -> Vec<i64> {
    if pool.is_empty() {
        return Vec::new();
    }
    (0..n)
        .map(|_| {
            let idx = (next() % pool.len() as u64) as usize;
            pool[idx]
        })
        .collect()
}
