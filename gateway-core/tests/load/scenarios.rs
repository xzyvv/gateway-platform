//! T2a 负载场景（全部 `#[ignore]`）。
//!
//! 运行：`cargo test --test load -- --ignored --test-threads=1`
//! 规模由环境变量控制（见 `FarmSpec::from_env`）。

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::chaos_harness::StorageBackend;
use super::farm::{Farm, FarmSpec};
use super::north_mqtt::MqttNorth;
use super::report::{sample, Report};

fn soak_secs() -> u64 {
    std::env::var("LOAD_SOAK_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30)
}

/// 构建农场 + 真 MQTT 北向（broker 桩 + 真客户端），启动后跑 `secs` 秒并采样。
///
/// 返回 (增量报告, 农场, 北向)。农场已 stop、北向已停机。
async fn run_load(spec: FarmSpec, secs: u64) -> (Report, Farm, MqttNorth) {
    let mut farm = Farm::build(spec).await;

    // 第 5 层：真 MqttClient 与 pipeline 共用同一 DataQueue；
    // ConversionManager 消费北向专用队列（其 publish_batch 为空实现，不会双发）。
    let north_rx = farm.north_rx.take().expect("north feed receiver");
    let north = MqttNorth::start(farm.queue.clone(), farm.metrics.clone(), north_rx).await;

    farm.start();
    let t0 = sample(&farm.metrics, &north, farm.queue.len().await as i64);

    // 周期性采样真实队列积压峰值（`metrics.queue_depth` 在 memory 后端不更新）
    let peak = Arc::new(AtomicI64::new(0));
    let sampler = {
        let queue = farm.queue.clone();
        let peak = peak.clone();
        let ticks = (secs * 2).max(1);
        tokio::spawn(async move {
            for _ in 0..ticks {
                tokio::time::sleep(Duration::from_millis(500)).await;
                peak.fetch_max(queue.len().await as i64, Ordering::Relaxed);
            }
        })
    };

    tokio::time::sleep(Duration::from_secs(secs)).await;
    sampler.abort();

    let t1 = sample(&farm.metrics, &north, farm.queue.len().await as i64);
    farm.stop().await;
    north.stop().await;

    let report = Report::between(t0, t1, peak.load(Ordering::Relaxed));
    (report, farm, north)
}

/// L1：串口农场满载 soak（默认 4 总线 × 75 点 = 300 点 / 4800 baud）。
///
/// 断言稳态：采集持续在跑、无失败、无解码错误、数据真实经 MQTT broker 上送、
/// 北向吞吐跟上采集速率（P0-1/P0-2 后北向不再是"每周期一批"的瓶颈）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "T2a 负载/soak，手动运行：cargo test --test load -- --ignored --test-threads=1"]
async fn load_serial_farm_soak() {
    let spec = FarmSpec::from_env();
    let secs = soak_secs();
    let total = spec.total_points();
    let (report, farm, north) = run_load(spec, secs).await;

    println!(
        "规模: {} 总线 × {} 点 = {} 点, {} baud, interval={}s",
        farm.spec.buses, farm.spec.points_per_bus, total, farm.spec.baud, farm.spec.interval_secs
    );
    println!("{}", report.render());

    assert!(report.collect_delta > 0, "采集事务必须发生");
    assert_eq!(report.failures_delta, 0, "稳态不应有采集失败");
    assert_eq!(report.decode_delta, 0, "稳态不应有解码错误");

    // 端到端证据：真 MQTT 客户端出队 → 真 broker 桩实收 PUBLISH
    let received = north.batches();
    println!(
        "broker 实收 PUBLISH: {} 条, {} 点",
        received,
        north.points()
    );
    assert!(received > 0, "数据必须经真 MQTT broker 上送");
    assert!(report.north_points_delta > 0, "北向必须实收点位");
    assert!(
        report.north_points_delta <= report.collect_delta,
        "北向实收点数不应超过采集点数：采集 {} vs 实收 {}",
        report.collect_delta,
        report.north_points_delta
    );

    // P0-1/P0-2：北向不再是"每周期一批 × 单批 ≤100 条"的人为瓶颈。
    // 旧行为（publish_interval=1s + 单批 100 条）把北向硬锁在 100 点/s，
    // 远低于农场产出（~285 点/s），队列净积压持续增长——该断言已随 P0 反转：
    // 现在北向必须跟上采集速率，瓶颈回到串口采集侧。
    assert!(
        report.north_point_rate() >= report.collect_rate() * 0.85,
        "北向速率应跟随采集速率（不再受单批 100 条 / 每秒一轮约束）：采集 {:.1} 点/s vs 北向 {:.1} 点/s",
        report.collect_rate(),
        report.north_point_rate()
    );

    // 积压证据：北向跟上采集后，队列积压应保持**有界**（峰值不超过两个采集
    // 周期的产出量），而不是像旧行为那样持续增长到数千条。
    let backlog = farm.queue.len().await;
    println!("队列积压: {} 条（峰值 {}）", backlog, report.queue_len_peak);
    assert!(
        report.queue_len_peak <= (total as i64) * 2,
        "北向跟上采集后队列积压峰值应保持有界，峰值 {} 超过 2 个采集周期产出量 {}",
        report.queue_len_peak,
        total as i64 * 2
    );
    assert!(
        backlog <= total * 2,
        "北向跟上采集后队列终态积压应保持有界，实际 {}",
        backlog
    );
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
    let (report, _farm, north) = run_load(spec, 12).await;

    println!("{}", report.render());
    assert!(
        report.collect_delta > 0,
        "采集循环必须持续推进（无永久卡死）"
    );
    assert!(
        north.points() > 0,
        "静默期过后必须恢复并经 MQTT broker 成功上送数据"
    );
}
