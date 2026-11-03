//! 长跑主循环与测试入口。
//!
//! 两个 `#[ignore]` 入口由一个驱动脚本串起来：
//! 1. `soak_run`     —— 跑真实链路 + 故障编排 + 采样，写出 JSONL；
//! 2. `soak_verdict` —— 读产物做阈值判定，生成 `report.md`，硬失败则 panic。
//!
//! 分开的原因：判定必须能离线重算（改阈值不需要重跑 24h），
//! 且驱动脚本在两次 cargo 调用之间可以补采进程级指标。

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use super::farm::SoakFarm;
use super::sampler::{self, ProcSample, Sample, SampleSource};
use super::spec::SoakSpec;
use super::timeline::{Timeline, TimelineState};
use super::verdict::{self, Thresholds, RECOVERY_SECS};

/// 故障结束后的排除时长：期内产生的坏点仍可能在管道 / 队列中陆续北向送达，
/// 计入会把故障期的坏质量点算到稳态成功率头上。与判定阶段共用
/// `RECOVERY_SECS`，避免采样与判定两处口径漂移。
const RECOVERY_MS: u64 = (RECOVERY_SECS * 1000.0) as u64;

/// 执行一次浸泡运行（产物写入 `spec.out_dir`）。
pub async fn run(spec: SoakSpec) -> Result<()> {
    fs::create_dir_all(&spec.out_dir)
        .with_context(|| format!("创建输出目录失败: {:?}", spec.out_dir))?;
    let metrics_path = spec.out_dir.join("metrics.jsonl");
    let events_path = spec.out_dir.join("events.jsonl");
    let proc_path = spec.out_dir.join("proc.jsonl");
    // 清掉上一轮产物，避免追加写入把两轮数据混在一起
    for p in [&metrics_path, &events_path, &proc_path] {
        let _ = fs::remove_file(p);
    }
    fs::write(spec.out_dir.join("pid.txt"), std::process::id().to_string())
        .context("写 pid.txt 失败")?;

    println!(
        "[soak] 启动：{} 点 / {} 总线 × {} 从站 × {} 点 / 周期 {}s / 时长 {}s / 输出 {:?}",
        spec.actual_points(),
        spec.buses,
        spec.slaves_per_bus,
        spec.points_per_slave,
        spec.interval_secs,
        spec.duration.as_secs(),
        spec.out_dir
    );

    let started = Instant::now();
    let farm = SoakFarm::build(spec.clone()).await;
    let timeline = Timeline::for_duration(
        spec.duration,
        spec.buses,
        spec.slaves_per_bus,
        spec.bus_timeout_ms,
    );
    for w in &timeline.windows {
        println!(
            "[soak] 计划故障窗口：{}  {:.0}s ~ {:.0}s",
            w.label,
            w.start.as_secs_f64(),
            w.end.as_secs_f64()
        );
    }

    let state = Arc::new(TimelineState::new(Duration::from_millis(RECOVERY_MS)));
    let timeline_handle = super::timeline::spawn(
        farm.controls.clone(),
        timeline.clone(),
        state.clone(),
        &events_path,
    );

    let source = SampleSource {
        metrics: farm.metrics.clone(),
        sink: farm.sink.clone(),
        drained: farm.drained.clone(),
        controls: farm.controls.clone(),
        timeline: state.clone(),
    };
    let sampler_handle = sampler::spawn(source, spec.sample, started, &metrics_path);

    farm.start();

    // 长跑按整秒 sleep，但每 30s 打印一次进度，便于驱动脚本/人工观察
    let mut slept = Duration::ZERO;
    let tick = Duration::from_secs(30);
    while slept < spec.duration {
        let remain = spec.duration - slept;
        let step = remain.min(tick);
        tokio::time::sleep(step).await;
        slept += step;
        println!(
            "[soak] 进度 {:.1}% ({:.0}s / {:.0}s)",
            slept.as_secs_f64() / spec.duration.as_secs_f64() * 100.0,
            slept.as_secs_f64(),
            spec.duration.as_secs_f64()
        );
    }

    timeline_handle.abort();
    sampler_handle.abort();

    let slave_snapshot = farm.slave_snapshot();
    let drained = farm.drained.load(std::sync::atomic::Ordering::Relaxed);
    farm.stop().await;

    let final_json = serde_json::json!({
        "ts_ms": super::timeline::unix_ms(),
        "duration_secs": started.elapsed().as_secs_f64(),
        "points": spec.actual_points(),
        "buses": spec.buses,
        "slaves_per_bus": spec.slaves_per_bus,
        "points_per_slave": spec.points_per_slave,
        "interval_secs": spec.interval_secs,
        "drained": drained,
        "slaves": slave_snapshot,
    });
    fs::write(
        spec.out_dir.join("final.json"),
        serde_json::to_vec_pretty(&final_json)?,
    )?;

    println!(
        "[soak] 结束：实际运行 {:.0}s，产物目录 {:?}",
        started.elapsed().as_secs_f64(),
        spec.out_dir
    );
    Ok(())
}

/// 读取产物并判定；硬失败时返回错误（测试入口会因此失败）。
pub fn check(spec: &SoakSpec) -> Result<verdict::Verdict> {
    let samples: Vec<Sample> = verdict::read_jsonl(&spec.out_dir.join("metrics.jsonl"))?;
    let proc: Vec<ProcSample> = verdict::read_jsonl(&spec.out_dir.join("proc.jsonl"))?;
    // 判定用的时间线由 spec 重新生成（纯函数，结果确定）
    let timeline = Timeline::for_duration(
        spec.duration,
        spec.buses,
        spec.slaves_per_bus,
        spec.bus_timeout_ms,
    );
    let thresholds = Thresholds::default();
    let v = verdict::evaluate(spec, &thresholds, &samples, &proc, &timeline)?;
    let md = verdict::render_report(spec, &v, &samples, &spec.out_dir)?;
    println!("{}", md);
    Ok(v)
}

/// 供测试入口调用：硬失败转成 panic，让 cargo 退出码非 0。
pub fn assert_passed(v: &verdict::Verdict) {
    if !v.passed() {
        panic!("浸泡判定未通过：\n{}", v.hard_failures.join("\n"));
    }
}

/// 清理运行产物中的大文件（驱动脚本可选调用）。
pub fn truncate_artifacts(dir: &Path) {
    for name in ["metrics.jsonl", "events.jsonl", "proc.jsonl"] {
        let _ = fs::write(dir.join(name), "");
    }
}
