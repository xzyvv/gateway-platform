//! [`SinkCore`] 生命周期内核的回归测试。
//!
//! 覆盖三条现场最要命的路径：
//! 1. **下游长期不可用** → `apply` 持续失败 → `on_failure` 触发 → 重试一次仍失败
//!    → 本批被丢弃、`sink_dropped` 递增、`sink_channel_depth` 必须回到 0
//! 2. **下游抖动** → `apply` 首次失败、重试成功 → 本批**不得**被丢弃
//! 3. **发送端与消费端的并发竞态** → `in_flight` 计数不得下溢
//!    （旧实现把 `fetch_add(1)` 放在 `try_send` 之后，消费端可能先把
//!    `in_flight` 从 0 减到 `usize::MAX` → debug 下 panic 掉整个 sink task）

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::engine::metrics::{Metrics, SinkId};
use crate::engine::model::DataPoint;
use crate::engine::shutdown::ShutdownToken;

use super::base::SinkCore;
use super::SINK_CHANNEL_CAPACITY;

/// 轮询等待条件成立（最多 5s）。
async fn wait_until<F: Fn() -> bool>(cond: F) -> bool {
    wait_for(cond, Duration::from_secs(5)).await
}

/// 轮询等待条件成立（最多 `timeout`）。
async fn wait_for<F: Fn() -> bool>(cond: F, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if cond() {
            return true;
        }
        if Instant::now() >= deadline {
            return cond();
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

fn point() -> DataPoint {
    DataPoint::good("p1", "dev1", 1.0, 1.0, "u16")
}

// ===== 下游长期不可用：apply 持续失败 =====

/// apply 持续失败 → on_failure 被调用 → 重试一次仍失败 → 本批丢弃并计数。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sink_core_apply_always_failing_drops_batch() {
    let metrics = Arc::new(Metrics::new());
    let apply_calls = Arc::new(AtomicU64::new(0));
    let failure_calls = Arc::new(AtomicU64::new(0));
    let ac = apply_calls.clone();
    let fc = failure_calls.clone();

    let core = SinkCore::new(
        SinkId::OpcUa,
        ShutdownToken::new(),
        metrics.clone(),
        move |_| {
            ac.fetch_add(1, Ordering::Relaxed);
            Err(())
        },
        move || {
            fc.fetch_add(1, Ordering::Relaxed);
        },
    );

    core.send(vec![point()]);

    assert!(
        wait_until(|| metrics.sink_dropped_total(SinkId::OpcUa) == 1).await,
        "apply 二次失败后本批必须计入 dropped（实际 dropped={}）",
        metrics.sink_dropped_total(SinkId::OpcUa)
    );
    assert_eq!(
        apply_calls.load(Ordering::Relaxed),
        2,
        "apply 应被调用两次（原一次 + on_failure 后重试一次）"
    );
    assert_eq!(
        failure_calls.load(Ordering::Relaxed),
        1,
        "on_failure 应被调用一次"
    );
    assert_eq!(
        metrics
            .lifecyle
            .sink_reconnect_attempts
            .load(Ordering::Relaxed),
        1
    );
    assert_eq!(
        metrics
            .lifecyle
            .sink_reconnect_exhausted
            .load(Ordering::Relaxed),
        1
    );
}

/// 丢弃分支必须把 `sink_channel_depth` gauge 归零。
///
/// 旧实现只减了 `in_flight` 计数、没更新 gauge，导致深度只涨不落
/// （每丢一批偏 1）→ 现场看通道深度永远不回落，误判成"背压"。
///
/// 第一次 apply 故意慢 50ms：确保消费端的 gauge 写入一定发生在生产端
/// 写入之后，避免"最后写入者"不确定带来的偶发失败。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sink_core_dropped_batch_resets_channel_depth_gauge() {
    let metrics = Arc::new(Metrics::new());
    let first = Arc::new(AtomicBool::new(true));
    let f = first.clone();
    let core = SinkCore::new(
        SinkId::OpcUa,
        ShutdownToken::new(),
        metrics.clone(),
        move |_| {
            if f.swap(false, Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(())
        },
        || {},
    );

    core.send(vec![point()]);
    assert!(
        wait_until(|| metrics.sink_dropped_total(SinkId::OpcUa) == 1).await,
        "批次未被丢弃"
    );
    assert!(
        wait_for(
            || metrics.sink_channel_depth(SinkId::OpcUa) == 0,
            Duration::from_secs(2)
        )
        .await,
        "丢弃后通道深度 gauge 必须回到 0（实际 {}）",
        metrics.sink_channel_depth(SinkId::OpcUa)
    );
}

// ===== 下游抖动：apply 首次失败、重试成功 =====

/// apply 首次失败 → on_failure → 重试成功 → 本批**不得**被丢弃。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sink_core_apply_recovers_after_on_failure_is_not_dropped() {
    let metrics = Arc::new(Metrics::new());
    let apply_calls = Arc::new(AtomicU64::new(0));
    let failure_calls = Arc::new(AtomicU64::new(0));
    let ac = apply_calls.clone();
    let fc = failure_calls.clone();

    let core = SinkCore::new(
        SinkId::OpcUa,
        ShutdownToken::new(),
        metrics.clone(),
        move |_| {
            if ac.fetch_add(1, Ordering::Relaxed) == 0 {
                Err(())
            } else {
                Ok(())
            }
        },
        move || {
            fc.fetch_add(1, Ordering::Relaxed);
        },
    );

    core.send(vec![point()]);

    assert!(
        wait_until(|| metrics
            .lifecyle
            .sink_reconnect_attempts
            .load(Ordering::Relaxed)
            == 1)
        .await,
        "apply 失败后必须触发一次重连"
    );
    // 留出时间确认没有后续丢弃
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        metrics.sink_dropped_total(SinkId::OpcUa),
        0,
        "重试成功后本批不得被丢弃"
    );
    assert_eq!(
        metrics
            .lifecyle
            .sink_reconnect_exhausted
            .load(Ordering::Relaxed),
        0
    );
    assert_eq!(apply_calls.load(Ordering::Relaxed), 2);
    assert_eq!(failure_calls.load(Ordering::Relaxed), 1);
}

// ===== 通道满：背压退化成丢弃 =====

/// 下游卡住导致通道塞满 → 超出容量的批次必须被丢弃并计数。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sink_core_channel_full_drops_excess_batches() {
    let metrics = Arc::new(Metrics::new());
    let release = Arc::new(AtomicBool::new(false));
    let rel = release.clone();

    // apply 阻塞住，直到测试释放 —— 模拟下游卡死
    let core = SinkCore::new(
        SinkId::OpcUa,
        ShutdownToken::new(),
        metrics.clone(),
        move |_| {
            while !rel.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(())
        },
        || {},
    );

    for _ in 0..(SINK_CHANNEL_CAPACITY + 64) {
        core.send(vec![point()]);
    }

    assert!(
        wait_until(|| metrics.sink_dropped_total(SinkId::OpcUa) > 0).await,
        "通道塞满后超出容量的批次必须被丢弃（实际 dropped={}）",
        metrics.sink_dropped_total(SinkId::OpcUa)
    );
    release.store(true, Ordering::Relaxed);
}

// ===== 并发竞态：in_flight 不得下溢 =====

/// 发送端与消费端并发时 `in_flight` 计数不得下溢。
///
/// 旧实现把 `fetch_add(1)` 放在 `try_send` 成功之后：消费端可能在生产者
/// 自增之前就把计数从 0 减掉 → `usize::MAX` → debug 下紧接着的 `- 1`
/// 直接 panic 掉 sink task（通道关闭 → 后续数据全丢）。
///
/// 注意：通道容量有限（1024 批），突发发送必然产生正常的丢弃，
/// 因此本用例断言的是「每批都有交代（被消费或被丢弃）」+「sink task 仍然活着」，
/// 而不是"一批不丢"。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sink_core_in_flight_never_underflows_under_concurrent_send() {
    let metrics = Arc::new(Metrics::new());
    let consumed = Arc::new(AtomicU64::new(0));
    let c = consumed.clone();

    let core = SinkCore::new(
        SinkId::OpcUa,
        ShutdownToken::new(),
        metrics.clone(),
        move |_| {
            c.fetch_add(1, Ordering::Relaxed);
            Ok(())
        },
        || {},
    );

    const N: u64 = 20_000;
    for _ in 0..N {
        core.send(vec![point()]);
    }

    // 每批都必须有交代：被消费 或 被丢弃（通道满）
    assert!(
        wait_for(
            || consumed.load(Ordering::Relaxed) + metrics.sink_dropped_total(SinkId::OpcUa) == N,
            Duration::from_secs(10)
        )
        .await,
        "批次去向不明：consumed={} dropped={} 应合计 {N}",
        consumed.load(Ordering::Relaxed),
        metrics.sink_dropped_total(SinkId::OpcUa)
    );

    // sink task 必须还活着：再发一批，必须能被消费掉。
    // （若 in_flight 下溢导致 task panic，通道会关闭 → 这批只会被丢弃、不会消费）
    let before = consumed.load(Ordering::Relaxed);
    let mut alive = false;
    for _ in 0..50 {
        core.send(vec![point()]);
        if wait_for(
            || consumed.load(Ordering::Relaxed) > before,
            Duration::from_millis(200),
        )
        .await
        {
            alive = true;
            break;
        }
    }
    assert!(
        alive,
        "sink task 已死（in_flight 下溢导致 panic → 通道关闭）"
    );
}
