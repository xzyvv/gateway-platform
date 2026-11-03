//! 周期指标采样。
//!
//! 每个采样点写出 `metrics.jsonl` 一行；判定阶段只依赖该文件 + 驱动脚本采到的
//! `proc.jsonl`，因此报告可以在跑完后离线重算，不需要复跑。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gateway_core::engine::metrics::Metrics;
use serde::Serialize;
use tokio::task::JoinHandle;

use super::port::SoakControl;
use super::timeline::{unix_ms, TimelineState};

/// 一次采样快照。字段名即 JSON 键名，判定模块按同一结构反序列化。
#[derive(Clone, Debug, Serialize, serde::Deserialize)]
pub struct Sample {
    pub ts_ms: u64,
    pub elapsed_secs: f64,
    /// steady | fault
    pub phase: String,
    pub collect_total: u64,
    pub collect_fail: u64,
    pub decode_errors: u64,
    pub serial_errors: u64,
    pub queue_depth: i64,
    pub sink_points: u64,
    pub sink_batches: u64,
    /// 北向收到的 Good 质量点数（北向一致性分母）
    pub good_points: u64,
    pub drained: u64,
    pub panics: u64,
    pub watchdog: u64,
    pub probe_ok: u64,
    pub probe_fail: u64,
    pub probe_skip: u64,
    pub reopen: u64,
    /// 每总线：[(从站号, 累计请求数, 累计应答数)]
    pub slaves: Vec<Vec<(u8, u64, u64)>>,
}

/// 采样所需的共享句柄集合。
pub struct SampleSource {
    pub metrics: Arc<Metrics>,
    pub sink: Arc<crate::soak::farm::QualitySink>,
    pub drained: Arc<AtomicU64>,
    pub controls: Vec<Arc<SoakControl>>,
    pub timeline: Arc<TimelineState>,
}

impl SampleSource {
    pub fn snapshot(&self, started: Instant) -> Sample {
        let m = &self.metrics;
        Sample {
            ts_ms: unix_ms(),
            elapsed_secs: started.elapsed().as_secs_f64(),
            phase: if self.timeline.is_faulting() {
                "fault".to_string()
            } else {
                "steady".to_string()
            },
            collect_total: m.collect.collect_total.load(Ordering::Relaxed),
            collect_fail: m.collect.collect_failures.load(Ordering::Relaxed),
            decode_errors: m.collect.protocol_decode_errors.load(Ordering::Relaxed),
            serial_errors: m.collect.serial_errors.load(Ordering::Relaxed),
            queue_depth: m.queue_depth.load(Ordering::Relaxed),
            sink_points: self.sink.points(),
            sink_batches: self.sink.batches(),
            good_points: self.sink.good(),
            drained: self.drained.load(Ordering::Relaxed),
            panics: m.lifecyle.task_panics.load(Ordering::Relaxed),
            watchdog: m.lifecyle.watchdog_timeouts.load(Ordering::Relaxed),
            probe_ok: m.bus.self_check_ok.load(Ordering::Relaxed),
            probe_fail: m.bus.self_check_fail.load(Ordering::Relaxed),
            probe_skip: m.bus.self_check_skipped.load(Ordering::Relaxed),
            reopen: m.bus.port_reopen_by_self_check.load(Ordering::Relaxed),
            slaves: self.controls.iter().map(|c| c.snapshot()).collect(),
        }
    }
}

/// 启动采样任务。
pub fn spawn(
    source: SampleSource,
    interval: Duration,
    started: Instant,
    out_path: &Path,
) -> JoinHandle<()> {
    let out_path = out_path.to_path_buf();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            // 首次 tick 立即触发，保证有 t0 基线
            ticker.tick().await;
            let s = source.snapshot(started);
            if let Ok(line) = serde_json::to_string(&s) {
                if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&out_path) {
                    let _ = writeln!(f, "{}", line);
                }
            }
        }
    })
}

/// 进程级采样（由驱动脚本写入），判定阶段读取。
#[derive(Clone, Debug, Serialize, serde::Deserialize)]
pub struct ProcSample {
    pub ts_ms: u64,
    pub pid: u32,
    pub rss_bytes: i64,
    pub handles: i64,
}

/// `AtomicI64` 便捷读取（供判定/诊断复用）。
pub fn read_i64(v: &AtomicI64) -> i64 {
    v.load(Ordering::Relaxed)
}
