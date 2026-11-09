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

    /// 入队「同一批点位多拍」，模拟 1s 采样 / 5s 上报形成的积压形态。
    /// 同一 `(point_id, timestamp)` 会被去重索引吞掉，故每一拍显式错开 timestamp。
    fn enqueue_same_points(&self, point_count: usize, shots: usize) -> usize {
        let mut total = 0;
        for shot in 0..shots {
            let points: Vec<DataPoint> = (0..point_count)
                .map(|i| {
                    let v = (shot * 10 + i) as f64;
                    let mut p = DataPoint::good(&format!("p{}", i), "dev1", v, v, "u16");
                    p.timestamp = 1_000 + shot as i64;
                    p.source_ts = 1_000 + shot as i64;
                    p
                })
                .collect();
            total += self.storage.enqueue_batch(&points).expect("enqueue");
        }
        total
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

    f.storage
        .release_in_flight(&ids[..2])
        .expect("duplicate release");
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

// ===== snapshot 上报（每周期折叠一次 + 每点位只出队最新一行） =====

#[test]
fn snapshot_dequeue_returns_latest_row_per_point() {
    let f = Fixture::new();
    assert_eq!(f.enqueue_same_points(3, 5), 15);
    assert_eq!(f.db_total(), 15);

    // 每个上报周期开头折叠一次：3 个点位各留最新 1 行，其余 12 行标 delivered。
    assert_eq!(
        f.storage.collapse_snapshot().expect("collapse").superseded,
        12
    );
    assert_eq!(f.db_unsent(), 3);
    f.assert_consistent("snapshot collapse");

    let items = f
        .storage
        .dequeue_snapshot_batch(10, i64::MAX)
        .expect("snapshot dequeue");
    assert_eq!(items.len(), 3, "每个点位只应取 1 行");

    // 最新一拍（shot=4）的 value = 4*10 + i
    let mut values: Vec<f64> = items.iter().map(|i| i.value).collect();
    values.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    assert_eq!(values, vec![40.0, 41.0, 42.0]);

    // 3 条在途（仍计入 depth：depth 语义是 delivered=0）
    assert_eq!(f.db_unsent(), 3);
    assert_eq!(f.storage.in_flight_count(), 3);
    f.assert_consistent("snapshot dequeue");
}

#[test]
fn snapshot_dequeue_limit_keeps_unsent_latest() {
    let f = Fixture::new();
    assert_eq!(f.enqueue_same_points(4, 3), 12);

    // 先折叠：4 个点位各留最新 1 行，其余 8 行标 delivered。
    assert_eq!(
        f.storage.collapse_snapshot().expect("collapse").superseded,
        8
    );

    let items = f
        .storage
        .dequeue_snapshot_batch(2, i64::MAX)
        .expect("snapshot dequeue");
    assert_eq!(items.len(), 2);

    // 4 个点位各保留最新行：2 行在途 + 2 行待发
    assert_eq!(f.db_unsent(), 4);
    assert_eq!(f.db_total(), 12);
    assert_eq!(f.storage.in_flight_count(), 2);
    f.assert_consistent("snapshot dequeue limited");
}

/// 折叠必须幂等，且不得动「在途」行（那些行等 ACK，标 delivered 会丢数据）。
#[test]
fn snapshot_collapse_is_idempotent_and_skips_in_flight() {
    let f = Fixture::new();
    assert_eq!(f.enqueue_same_points(2, 4), 8);

    // 8 行 → 每点位留 1 行，折叠 6 行。
    assert_eq!(
        f.storage.collapse_snapshot().expect("collapse").superseded,
        6
    );
    // 同上送周期重复调用不应再折叠任何行（否则 depth 会被重复扣减）。
    assert_eq!(
        f.storage
            .collapse_snapshot()
            .expect("second collapse")
            .superseded,
        0,
        "已折叠的行不得重复计数"
    );
    f.assert_consistent("snapshot collapse idempotent");

    // 出队（in_flight=1）后再折叠：在途行不能被标成 delivered。
    let ids: Vec<i64> = f
        .storage
        .dequeue_snapshot_batch(10, i64::MAX)
        .expect("snapshot dequeue")
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert_eq!(ids.len(), 2);
    assert_eq!(
        f.storage
            .collapse_snapshot()
            .expect("collapse with in-flight")
            .superseded,
        0,
        "在途行不受折叠影响"
    );
    assert_eq!(f.storage.in_flight_count(), 2);
    assert_eq!(f.db_in_flight(), 2);
    f.assert_consistent("snapshot collapse skips in-flight");
}

/// 折叠返回的 cutoff 必须钉住本周期边界：排空期间新到的样本不能同周期上行。
#[test]
fn snapshot_dequeue_ignores_rows_newer_than_cutoff() {
    let f = Fixture::new();
    assert_eq!(f.enqueue_same_points(2, 2), 4);

    let collapse = f.storage.collapse_snapshot().expect("collapse");
    assert_eq!(collapse.superseded, 2, "每个点位各折叠 1 行");
    assert!(collapse.cutoff > 0, "空表以外的折叠必须给出上界");

    // 折叠与排空之间新到一拍（timestamp 错开避开同毫秒去重索引，id > cutoff）。
    let fresh: Vec<DataPoint> = (0..2)
        .map(|i| {
            let v = (20 + i) as f64;
            let mut p = DataPoint::good(&format!("p{}", i), "dev1", v, v, "u16");
            p.timestamp = 2_000;
            p.source_ts = 2_000;
            p
        })
        .collect();
    assert_eq!(f.storage.enqueue_batch(&fresh).expect("enqueue"), 2);

    // 本周期只发 cutoff 以内的最新行：2 行，且 value 是第 2 拍（shot=1）。
    let items = f
        .storage
        .dequeue_snapshot_batch(10, collapse.cutoff)
        .expect("snapshot dequeue with cutoff");
    assert_eq!(items.len(), 2, "cutoff 之后新到的样本必须留到下个周期");
    let mut values: Vec<f64> = items.iter().map(|i| i.value).collect();
    values.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    assert_eq!(values, vec![10.0, 11.0]);

    // 下个周期：折叠 + 新 cutoff，才能取到第 3 拍。
    let next = f.storage.collapse_snapshot().expect("next collapse");
    let items = f
        .storage
        .dequeue_snapshot_batch(10, next.cutoff)
        .expect("snapshot dequeue next cycle");
    assert_eq!(items.len(), 2);
    let mut values: Vec<f64> = items.iter().map(|i| i.value).collect();
    values.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    assert_eq!(values, vec![20.0, 21.0]);
    f.assert_consistent("snapshot cutoff");
}

#[test]
fn stream_dequeue_still_returns_every_sample() {
    let f = Fixture::new();
    f.enqueue_same_points(2, 5);

    let items = f.storage.dequeue_batch(10).expect("dequeue");
    assert_eq!(items.len(), 10, "stream 模式必须保持逐样本上行");
    f.assert_consistent("stream dequeue");
}

/// 已知边界（`docs/design/snapshot-upload.md` §3.5）：折叠只以**未送达行**判定「最新」，
/// 已送达 / 在途的更新值不参与判定。因此慢 ACK 期间旧行被 release 回未送达后，会在
/// 更新值之后被再次上行一次——快照语义在失败重试路径上退化为 at-least-once。
///
/// 本用例固化该行为：若有人把折叠口径改成「全表 MAX(id)」，这里会失败并提醒同步
/// 更新设计文档与现场语义说明。
#[test]
fn snapshot_slow_ack_can_resend_stale_row_after_newer_delivered() {
    let f = Fixture::new();

    // 周期 1：只有旧值 1.0 入队 → 折叠保留 → 出队后在途（PubAck 迟迟不回）。
    let mut p = DataPoint::good("p0", "dev1", 1.0, 1.0, "u16");
    p.timestamp = 1_000;
    p.source_ts = 1_000;
    assert_eq!(f.storage.enqueue_batch(&[p]).expect("enqueue v1"), 1);
    let c1 = f.storage.collapse_snapshot().expect("collapse 1");
    let old_id = f
        .storage
        .dequeue_snapshot_batch(10, c1.cutoff)
        .expect("dequeue v1")
        .into_iter()
        .map(|i| i.id)
        .next()
        .expect("v1 必须被出队");

    // 周期 2：排空期间新值 2.0 入队 → 折叠保留 2.0（1.0 在途，不参与判定）→
    // 出队并 ACK 成功（先于 1.0 完成）。
    let mut p = DataPoint::good("p0", "dev1", 2.0, 2.0, "u16");
    p.timestamp = 2_000;
    p.source_ts = 2_000;
    assert_eq!(f.storage.enqueue_batch(&[p]).expect("enqueue v2"), 1);
    let c2 = f.storage.collapse_snapshot().expect("collapse 2");
    let new_ids: Vec<i64> = f
        .storage
        .dequeue_snapshot_batch(10, c2.cutoff)
        .expect("dequeue v2")
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert_eq!(new_ids.len(), 1, "2.0 必须在 1.0 在途时单独上行");
    f.storage.ack_batch(&new_ids).expect("ack v2");

    // 周期 3：1.0 的 PubAck 超时 → release 回未送达。折叠口径只看未送达行，
    // 已送达的 2.0 不参与判定，于是 1.0 再次成为「最新未送达」。
    f.storage.release_in_flight(&[old_id]).expect("release v1");
    assert_eq!(
        f.storage
            .collapse_snapshot()
            .expect("collapse 3")
            .superseded,
        0,
        "已知边界：旧行不会被折叠（更新值已送达，不参与判定）"
    );
    let stale: Vec<f64> = f
        .storage
        .dequeue_snapshot_batch(10, i64::MAX)
        .expect("dequeue stale")
        .iter()
        .map(|i| i.value)
        .collect();
    assert_eq!(stale, vec![1.0], "已知边界：旧值会在新值之后重发一次");
    f.assert_consistent("snapshot slow ack");
}
