//! 故障注入时间线。
//!
//! 所有事件按总时长的百分比生成，因此同一套编排既能用于 90s 冒烟，
//! 也能用于 24h 正式浸泡（24h 下各窗口自动放大到分钟级）。
//!
//! 编排顺序刻意如此：
//! 1. 单从站静默 → 验证 `self_check_ok` 增长、端口**不**重开；
//! 2. 整总线静默 → 验证 `self_check_fail` + 端口重开；
//! 3. 慢响应（接近超时）→ 验证延迟抖动不会误判成掉线；
//! 4. 慢响应（超过超时）→ 验证超时归因；
//! 5. 异常码 → 验证“合法异常帧 = 端口健康”。
//!
//! 每个故障窗口都会记入 `events.jsonl`，判定阶段据此排除故障期与恢复期。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::port::SoakControl;

/// 单个注入动作。
#[derive(Clone, Debug)]
pub enum Action {
    SlaveSilent { bus: usize, slave: u8, on: bool },
    BusDown { bus: usize, on: bool },
    Slow { bus: usize, ms: u64 },
    Exception { bus: usize, on: bool },
}

#[derive(Clone, Debug)]
pub struct Step {
    pub at: Duration,
    pub label: String,
    pub action: Action,
}

/// 一个故障窗口（用于判定时排除）。
#[derive(Clone, Debug)]
pub struct FaultWindow {
    pub start: Duration,
    pub end: Duration,
    pub label: String,
}

#[derive(Clone, Debug)]
pub struct Timeline {
    pub steps: Vec<Step>,
    pub windows: Vec<FaultWindow>,
}

impl Timeline {
    /// 按总时长生成编排。
    pub fn for_duration(
        total: Duration,
        buses: usize,
        slaves_per_bus: usize,
        bus_timeout_ms: u64,
    ) -> Self {
        let total_secs = total.as_secs_f64();
        let mut steps = Vec::new();
        let mut windows = Vec::new();

        // (起始比例, 持续比例, 最小持续秒, 构造 (on/off, label))
        let mut push_window = |start_frac: f64,
                               dur_frac: f64,
                               min_secs: f64,
                               label: &str,
                               mk: &dyn Fn(bool) -> Action| {
            let start = Duration::from_secs_f64(total_secs * start_frac);
            let dur = Duration::from_secs_f64((total_secs * dur_frac).max(min_secs));
            windows.push(FaultWindow {
                start,
                end: start + dur,
                label: label.to_string(),
            });
            steps.push(Step {
                at: start,
                label: format!("{label}: 开始"),
                action: mk(true),
            });
            steps.push(Step {
                at: start + dur,
                label: format!("{label}: 结束"),
                action: mk(false),
            });
        };

        // 1) 单设备静默（设备级；端口健康）
        let bus0 = 0usize;
        let slave0 = 1u8.min(slaves_per_bus.max(1) as u8);
        push_window(0.10, 0.02, 8.0, "单从站静默", &|on| {
            Action::SlaveSilent {
                bus: bus0,
                slave: slave0,
                on,
            }
        });

        // 2) 整总线静默（端口级）
        let bus1 = if buses > 1 { 1 } else { 0 };
        push_window(0.28, 0.015, 6.0, "整总线静默", &|on| Action::BusDown {
            bus: bus1,
            on,
        });

        // 3) 慢响应但未超时（0.6 × timeout）
        let bus2 = if buses > 2 { 2 } else { 0 };
        push_window(0.45, 0.04, 10.0, "慢响应(未超时)", &|on| {
            Action::Slow {
                bus: bus2,
                ms: if on { bus_timeout_ms * 3 / 5 } else { 0 },
            }
        });

        // 4) 慢响应超过超时（1.3 × timeout）
        push_window(0.60, 0.015, 6.0, "慢响应(超时)", &|on| Action::Slow {
            bus: bus2,
            ms: if on { bus_timeout_ms * 13 / 10 } else { 0 },
        });

        // 5) 异常码（合法帧）
        push_window(0.78, 0.02, 8.0, "异常码应答", &|on| {
            Action::Exception { bus: bus2, on }
        });

        steps.sort_by_key(|s| s.at);
        windows.sort_by_key(|a| a.start);
        Self { steps, windows }
    }

    /// 当前是否处于故障窗口内（含 60s 恢复期）。
    pub fn in_fault_or_recovery(&self, elapsed: Duration, recovery: Duration) -> bool {
        self.windows
            .iter()
            .any(|w| elapsed >= w.start && elapsed <= w.end + recovery)
    }
}

/// 共享故障状态：采样器据此标记 `phase`。
///
/// 判定不能只躲开「故障正在发生」的采样：故障期内产生的坏质量点会滞留在
/// 管道 / 队列里，在故障结束后仍陆续北向送达。若把这些采样算作稳态，
/// 稳态成功率会被故障期的坏点污染（实测 240s 跑中非 Good 点从 50 累积到 550，
/// 且一直不回落）。因此状态需覆盖「故障窗口 + 恢复期」。
pub struct TimelineState {
    active: AtomicUsize,
    /// 最近一次故障解除的时刻（含恢复期，到此刻之前都算故障态）
    quiet_until_ms: AtomicU64,
    /// 故障解除后仍需排除的恢复时长
    recovery_ms: u64,
}

impl TimelineState {
    pub fn new(recovery: Duration) -> Self {
        Self {
            active: AtomicUsize::new(0),
            quiet_until_ms: AtomicU64::new(0),
            recovery_ms: recovery.as_millis() as u64,
        }
    }

    /// 当前是否处于「故障窗口或恢复期」。
    pub fn is_faulting(&self) -> bool {
        if self.active.load(Ordering::Relaxed) > 0 {
            return true;
        }
        let until = self.quiet_until_ms.load(Ordering::Relaxed);
        until > 0 && unix_ms() < until
    }

    fn enter(&self) {
        self.active.fetch_add(1, Ordering::Relaxed);
    }

    fn leave(&self) {
        // 允许下溢到 0，但绝不产生负值
        let mut cur = self.active.load(Ordering::Relaxed);
        while cur > 0 {
            match self.active.compare_exchange_weak(
                cur,
                cur - 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => cur = actual,
            }
        }
        // 每次解除都把排除期往后推，覆盖所有重叠窗口
        self.quiet_until_ms.store(
            unix_ms().saturating_add(self.recovery_ms),
            Ordering::Relaxed,
        );
    }
}

/// 启动时间线执行任务：按 `at` 依次应用动作，并写 `events.jsonl`。
pub fn spawn(
    controls: Vec<Arc<SoakControl>>,
    timeline: Timeline,
    state: Arc<TimelineState>,
    events_path: &Path,
) -> tokio::task::JoinHandle<()> {
    let events_path = events_path.to_path_buf();
    // 用单调时钟对齐，避免系统时间跳变影响编排
    let started = Instant::now();
    let steps = timeline.steps;
    tokio::spawn(async move {
        let mut idx = 0usize;
        let mut ticker = tokio::time::interval(Duration::from_millis(500));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            let elapsed = started.elapsed();
            while idx < steps.len() && steps[idx].at <= elapsed {
                let step = &steps[idx];
                apply(&controls, &state, &step.action);
                write_event(&events_path, elapsed, &step.label);
                // 控制台同步可见（`--nocapture` 下驱动脚本会落盘）
                println!("[soak][{:>8.1}s] {}", elapsed.as_secs_f64(), step.label);
                idx += 1;
            }
            if idx >= steps.len() {
                break;
            }
        }
    })
}

fn apply(controls: &[Arc<SoakControl>], state: &TimelineState, action: &Action) {
    match action {
        Action::SlaveSilent { bus, slave, on } => {
            if let Some(c) = controls.get(*bus) {
                c.silence_slave(*slave, *on);
                toggle_state(state, *on);
            }
        }
        Action::BusDown { bus, on } => {
            if let Some(c) = controls.get(*bus) {
                c.set_bus_down(*on);
                toggle_state(state, *on);
            }
        }
        Action::Slow { bus, ms } => {
            if let Some(c) = controls.get(*bus) {
                let was = c_slow_active(c);
                c.set_slow(*ms);
                if *ms > 0 && !was {
                    state.enter();
                } else if *ms == 0 && was {
                    state.leave();
                }
            }
        }
        Action::Exception { bus, on } => {
            if let Some(c) = controls.get(*bus) {
                c.set_exception(*on);
                toggle_state(state, *on);
            }
        }
    }
}

fn c_slow_active(c: &SoakControl) -> bool {
    c.slow_ms_is_active()
}

fn toggle_state(state: &TimelineState, on: bool) {
    if on {
        state.enter();
    } else {
        state.leave();
    }
}

fn write_event(path: &Path, elapsed: Duration, label: &str) {
    let line = serde_json::json!({
        "ts_ms": unix_ms(),
        "elapsed_secs": elapsed.as_secs_f64(),
        "label": label,
    });
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{}", line);
    }
}

pub fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
