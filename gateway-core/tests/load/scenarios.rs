//! T2a 负载场景（全部 `#[ignore]`）。
//!
//! 运行：`cargo test --test load -- --ignored --test-threads=1`
//! 规模由环境变量控制（见 `FarmSpec::from_env`）。

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gateway_core::north::ConversionManager;

use super::chaos_harness::StorageBackend;
use super::farm::{Farm, FarmSpec};
use super::north_stub::CountingSink;
use super::report::{sample, Report};

fn soak_secs() -> u64 {
    std::env::var("LOAD_SOAK_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30)
}

/// 构建农场 + 北向计数桩 + ConversionManager，启动后跑 `secs` 秒并采样。
///
/// 返回 (增量报告, 农场, 计数桩)。农场已 stop，ConversionManager 任务已 abort。
async fn run_load(spec: FarmSpec, secs: u64) -> (Report, Farm, Arc<CountingSink>) {
    let farm = Farm::build(spec).await;
    let sink = CountingSink::new();

    let cm = ConversionManager::new(farm.realtime_bus.clone());
    cm.register_connector(sink.clone() as Arc<dyn gateway_core::north::NorthConnector>);
    let cm_handle = tokio::spawn(async move { cm.run().await });

    farm.start();
    let t0 = sample(&farm.metrics, &sink);

    // 周期性采样队列深度峰值（独立于采集线程）
    let peak = Arc::new(AtomicI64::new(0));
    let sampler = {
        let metrics = farm.metrics.clone();
        let peak = peak.clone();
        let ticks = (secs * 2).max(1);
        tokio::spawn(async move {
            for _ in 0..ticks {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let d = metrics.queue_depth.load(Ordering::Relaxed);
                peak.fetch_max(d, Ordering::Relaxed);
            }
        })
    };

    tokio::time::sleep(Duration::from_secs(secs)).await;
    sampler.abort();

    let t1 = sample(&farm.metrics, &sink);
    farm.stop().await;
    cm_handle.abort();

    let report = Report::between(t0, t1, peak.load(Ordering::Relaxed));
    (report, farm, sink)
}

/// L1：串口农场满载 soak（默认 4 总线 × 75 点 = 300 点 / 4800 baud）。
///
/// 断言稳态：采集持续在跑、无失败、无解码错误、数据真实扇出到北向、队列不无界增长。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "T2a 负载/soak，手动运行：cargo test --test load -- --ignored --test-threads=1"]
async fn load_serial_farm_soak() {
    let spec = FarmSpec::from_env();
    let secs = soak_secs();
    let total = spec.total_points();
    let (report, farm, _sink) = run_load(spec, secs).await;

    println!(
        "规模: {} 总线 × {} 点 = {} 点, {} baud, interval={}s",
        farm.spec.buses, farm.spec.points_per_bus, total, farm.spec.baud, farm.spec.interval_secs
    );
    println!("{}", report.render());

    assert!(report.collect_delta > 0, "采集事务必须发生");
    assert_eq!(report.failures_delta, 0, "稳态不应有采集失败");
    assert_eq!(report.decode_delta, 0, "稳态不应有解码错误");
    assert!(report.sink_points_delta > 0, "数据必须扇出到北向桩");
    assert!(
        report.queue_depth_peak <= total as i64 * 4,
        "队列深度应受控，实际峰值 {}",
        report.queue_depth_peak
    );

    // 1A 证据：采集数据确实经 ProcessingPipeline 落到 DataQueue（内存队列无消费者，应持续增长）
    let queued = farm.queue.len().await;
    println!("入队总量: {}", queued);
    assert!(queued > 0, "数据必须经管道入队，实际 {}", queued);
}

/// L2：波特率吞吐保真。同规模、不同波特率，高波特率吞吐应显著更高，
/// 证明串口节流确实约束了吞吐（节流真的生效）。
///
/// 负载刻意压到“单周期线路耗时 > 采集周期”，让波特率成为瓶颈而非采集周期。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "T2a 保真校验，手动运行：cargo test --test load -- --ignored --test-threads=1"]
async fn load_baud_throughput_fidelity() {
    let base = FarmSpec {
        buses: 1,
        points_per_bus: 500,
        baud: 4800,
        interval_secs: 1,
        slave_id: 1,
        silent_first: 0,
        storage: StorageBackend::from_env("LOAD_STORAGE"),
    };

    let mut slow_spec = base.clone();
    slow_spec.baud = 4800;
    let (slow, _f1, _s1) = run_load(slow_spec, 12).await;

    let mut fast_spec = base;
    fast_spec.baud = 115200;
    let (fast, _f2, _s2) = run_load(fast_spec, 12).await;

    println!("--- 4800 baud ---\n{}", slow.render());
    println!("--- 115200 baud ---\n{}", fast.render());

    assert!(slow.collect_delta > 0 && fast.collect_delta > 0);
    assert!(
        fast.collect_delta as f64 >= slow.collect_delta as f64 * 1.3,
        "高波特率吞吐应显著高于低波特率：4800={} vs 115200={}",
        slow.collect_delta,
        fast.collect_delta
    );
}

/// L3：瞬时静默恢复。每总线前 2 次事务静默（触发超时/失败），
/// 之后恢复应答；断言链路最终续传、数据仍能上送、无永久卡死。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "T2a 恢复场景，手动运行：cargo test --test load -- --ignored --test-threads=1"]
async fn load_transient_silence_recovery() {
    let spec = FarmSpec {
        buses: 1,
        points_per_bus: 20,
        baud: 4800,
        interval_secs: 1,
        slave_id: 1,
        silent_first: 2,
        storage: StorageBackend::from_env("LOAD_STORAGE"),
    };
    let (report, _farm, sink) = run_load(spec, 12).await;

    println!("{}", report.render());
    assert!(
        report.collect_delta > 0,
        "采集循环必须持续推进（无永久卡死）"
    );
    assert!(sink.points() > 0, "静默期过后必须恢复并成功上送数据");
}
