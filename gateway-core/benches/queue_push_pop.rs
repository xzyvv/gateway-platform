//! 队列 push/pop/ack 性能基准测试
//!
//! 覆盖真实队列路径（此前实现只压了 metrics 原子计数，未触及队列本身）：
//! - `MemoryStorage`：`QueueStorage` 内存后端，入队 / 出队+ACK 的纯数据结构成本
//! - `SqliteStorage`：SQLite WAL 持久化后端，稳态 入队→出队→ACK 往返成本（含磁盘 IO）
//! - `DataQueue`：生产路径（mpsc 通道 + 单 owner actor）的异步端到端往返开销
//!
//! 运行：`cargo bench --bench queue_push_pop`

use criterion::{
    black_box, criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput,
};
use std::sync::Arc;

use gateway_core::engine::metrics::Metrics;
use gateway_core::engine::model::DataPoint;
use gateway_core::queue::{DataQueue, MemoryStorage, QueueStorage, SqliteStorage};

/// 构造 n 个正常质量的数据点（队列基准的通用输入）
fn make_points(n: usize) -> Vec<DataPoint> {
    (0..n)
        .map(|i| DataPoint::good(&format!("pt_{i}"), "dev_1", i as f64, i as f64, "u16"))
        .collect()
}

/// 内存后端：批量入队成本
///
/// 用 `iter_batched` 每次以全新后端做入队，避免队列跨迭代无限增长污染测量。
fn bench_memory_enqueue(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_enqueue");
    for &k in &[1usize, 64, 1024] {
        let points = make_points(k);
        group.throughput(Throughput::Elements(k as u64));
        group.bench_with_input(BenchmarkId::from_parameter(k), &k, |b, _| {
            b.iter_batched(
                MemoryStorage::new,
                |s| {
                    let n = s.enqueue_batch(&points).expect("enqueue_batch");
                    black_box(n)
                },
                BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

/// 内存后端：批量出队 + ACK 成本（预填充 k 条，测量取走并确认的路径）
fn bench_memory_dequeue_ack(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_dequeue_ack");
    for &k in &[1usize, 64, 1024] {
        let points = make_points(k);
        group.throughput(Throughput::Elements(k as u64));
        group.bench_with_input(BenchmarkId::from_parameter(k), &k, |b, _| {
            b.iter_batched(
                || {
                    let s = MemoryStorage::new();
                    s.enqueue_batch(&points).expect("seed");
                    s
                },
                |s| {
                    let items = s.dequeue_batch(k).expect("dequeue_batch");
                    let ids: Vec<i64> = items.iter().map(|i| i.id).collect();
                    s.ack_batch(&ids).expect("ack_batch");
                    black_box(items.len())
                },
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

/// SQLite 持久化后端：稳态 入队→出队→ACK 往返（每轮自清空，队列深度恒定）
///
/// 这是生产环境的真实成本（含 WAL 写入、DELETE、read_pool 读取）。
fn bench_sqlite_cycle(c: &mut Criterion) {
    let dir = tempfile::tempdir().expect("create tempdir");
    let db_path = dir.path().join("queue-bench.db");
    let metrics = Arc::new(Metrics::new());
    let storage = SqliteStorage::new(db_path.to_str().expect("utf8 db path"), metrics)
        .expect("open sqlite queue");

    let mut group = c.benchmark_group("sqlite_cycle");
    for &k in &[1usize, 64, 256] {
        let points = make_points(k);
        group.throughput(Throughput::Elements(k as u64));
        group.bench_with_input(BenchmarkId::from_parameter(k), &k, |b, _| {
            b.iter(|| {
                storage.enqueue_batch(&points).expect("enqueue_batch");
                let items = storage.dequeue_batch(k).expect("dequeue_batch");
                let ids: Vec<i64> = items.iter().map(|i| i.id).collect();
                storage.ack_batch(&ids).expect("ack_batch");
                black_box(items.len())
            });
        });
    }
    group.finish();
    drop(dir);
}

/// 生产路径：`DataQueue` 异步门面（mpsc 通道 + actor）端到端往返
///
/// 测量含通道往返与任务调度的真实编排开销，而非仅后端数据结构成本。
fn bench_dataqueue_roundtrip(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("build tokio runtime");

    let mut group = c.benchmark_group("dataqueue_roundtrip");
    for &k in &[1usize, 64, 256] {
        let q = rt
            .block_on(async { DataQueue::new_memory() })
            .expect("create data queue");
        let points = make_points(k);
        group.throughput(Throughput::Elements(k as u64));
        group.bench_with_input(BenchmarkId::from_parameter(k), &k, |b, _| {
            b.iter(|| {
                rt.block_on(async {
                    q.enqueue(points.clone()).await.expect("enqueue");
                    let items = q.dequeue(k).await.expect("dequeue");
                    let ids: Vec<i64> = items.iter().map(|i| i.id).collect();
                    q.ack(ids).await.expect("ack");
                    black_box(items.len())
                })
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_memory_enqueue,
    bench_memory_dequeue_ack,
    bench_sqlite_cycle,
    bench_dataqueue_roundtrip,
);
criterion_main!(benches);
