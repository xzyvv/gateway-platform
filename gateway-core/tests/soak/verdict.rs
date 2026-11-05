//! 阈值判定与报告生成。
//!
//! 判定只读运行产物（`metrics.jsonl` + `proc.jsonl`），可在跑完后离线重算：
//! ```text
//! SOAK_OUT_DIR=soak-out cargo test --test soak -- --ignored --exact soak_verdict --nocapture
//! ```
//!
//! 判定分两层：
//! - **硬失败**：崩溃、泄漏、任务死亡、采集成功率跌破下限——任一命中即不可上线；
//! - **软失败**：趋势性异常（RSS 斜率、重连频率、解码错误）——需持续观察。
//!
//! 故障窗口与其后 60s 恢复期的采样不参与稳态判定（见 `Timeline::windows`）。

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{anyhow, Result};

use super::sampler::{ProcSample, Sample};
use super::spec::SoakSpec;
use super::timeline::Timeline;

/// 故障结束后的排除时长（秒）。采样阶段用它标记 `phase`，判定阶段用它排除
/// 恢复期采样——必须同源，否则会出现「采样已标 steady、判定仍排除」这类错位。
pub const RECOVERY_SECS: f64 = 60.0;

/// 默认阈值。与已确认的现场模型对齐（2000 点 / 5s / 24h）。
#[derive(Clone, Debug)]
pub struct Thresholds {
    /// 稳态聚合采集成功率下限
    pub min_success_rate: f64,
    /// 单设备成功率下限
    pub min_device_success_rate: f64,
    /// 单设备参与判定的最小请求数（样本不足不判定）
    pub min_device_requests: u64,
    /// RSS 允许增长（字节）
    pub max_rss_growth: i64,
    /// RSS 允许增长倍数（相对基线）
    pub max_rss_growth_ratio: f64,
    /// 峰值 RSS 允许倍数
    pub max_rss_peak_ratio: f64,
    /// 后 6h RSS 斜率（字节/小时）
    pub max_rss_slope_per_hour: f64,
    /// 句柄允许增量
    pub max_handle_growth: i64,
    /// 队列 p99 相对于“每周期点位数”的倍数
    pub queue_p99_cycles: f64,
    /// 队列峰值相对于“每周期点位数”的倍数
    pub queue_max_cycles: f64,
    /// 北向一致性下限
    pub min_north_ratio: f64,
    /// 稳态每小时允许的端口重开次数
    pub max_reopen_per_hour: f64,
    /// 故障后队列回落观察窗（秒）
    pub recovery_secs: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            min_success_rate: 0.999,
            min_device_success_rate: 0.95,
            min_device_requests: 100,
            max_rss_growth: 24 * 1024 * 1024,
            max_rss_growth_ratio: 1.5,
            max_rss_peak_ratio: 2.0,
            max_rss_slope_per_hour: 1.0 * 1024.0 * 1024.0,
            max_handle_growth: 20,
            queue_p99_cycles: 2.0,
            queue_max_cycles: 10.0,
            min_north_ratio: 0.999,
            max_reopen_per_hour: 2.0,
            recovery_secs: RECOVERY_SECS,
        }
    }
}

/// 单条判定结果。
#[derive(Clone, Debug)]
pub struct CheckRow {
    pub name: String,
    pub observed: String,
    pub threshold: String,
    pub hard: bool,
    pub passed: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Verdict {
    pub rows: Vec<CheckRow>,
    pub hard_failures: Vec<String>,
    pub soft_failures: Vec<String>,
    pub notes: Vec<String>,
}

impl Verdict {
    pub fn passed(&self) -> bool {
        self.hard_failures.is_empty()
    }

    fn row(&mut self, name: &str, observed: String, threshold: &str, hard: bool, passed: bool) {
        if !passed {
            if hard {
                self.hard_failures
                    .push(format!("{name}: {observed}（阈值 {threshold}）"));
            } else {
                self.soft_failures
                    .push(format!("{name}: {observed}（阈值 {threshold}）"));
            }
        }
        self.rows.push(CheckRow {
            name: name.to_string(),
            observed,
            threshold: threshold.to_string(),
            hard,
            passed,
        });
    }
}

/// 执行判定。
pub fn evaluate(
    spec: &SoakSpec,
    thresholds: &Thresholds,
    samples: &[Sample],
    proc: &[ProcSample],
    timeline: &Timeline,
) -> Result<Verdict> {
    if samples.is_empty() {
        return Err(anyhow!("metrics.jsonl 为空，无法判定"));
    }
    let mut v = Verdict::default();
    let cycles = spec.points_per_cycle().max(1) as f64;

    // ---------- 1. 稳定性：panic / watchdog ----------
    let panics_delta = delta(samples, |s| s.panics);
    let watchdog_delta = delta(samples, |s| s.watchdog);

    v.row(
        "任务 panic",
        format!("+{}", panics_delta),
        "= 0",
        true,
        panics_delta == 0,
    );
    v.row(
        "watchdog 超时",
        format!("+{}", watchdog_delta),
        "= 0",
        true,
        watchdog_delta == 0,
    );

    // ---------- 2. 稳态窗口切分 ----------
    let steady_pairs: Vec<(&Sample, &Sample)> = samples
        .windows(2)
        .filter(|w| w[0].phase == "steady" && w[1].phase == "steady")
        .map(|w| (&w[0], &w[1]))
        .collect();

    if steady_pairs.is_empty() {
        v.notes
            .push("没有足够的稳态采样对，成功率/队列/北向判定被跳过".to_string());
    }

    // 注意：成功率判定一律以「从站请求/应答」为准，不使用 `collect_failures`
    // （它按点记账，与按帧的链路口径量纲不同），见第 4 节。

    // ---------- 3. 从站请求 / 应答累计（唯一可信的链路应答真值） ----------
    let mut dev_req: HashMap<(usize, u8), u64> = HashMap::new();
    let mut dev_ans: HashMap<(usize, u8), u64> = HashMap::new();
    for (a, b) in &steady_pairs {
        for (bus, sa) in a.slaves.iter().enumerate() {
            let sb = match b.slaves.get(bus) {
                Some(x) => x,
                None => continue,
            };
            for (i, (slave, req_a, ans_a)) in sa.iter().enumerate() {
                let Some((_, req_b, ans_b)) = sb.get(i) else {
                    continue;
                };
                if slave != &sb[i].0 {
                    continue;
                }
                let dr = req_b.saturating_sub(*req_a);
                let da = ans_b.saturating_sub(*ans_a);
                if dr == 0 {
                    continue;
                }
                *dev_req.entry((bus, *slave)).or_default() += dr;
                *dev_ans.entry((bus, *slave)).or_default() += da;
            }
        }
    }

    // ---------- 4. 稳态链路应答率 ----------
    // 以从站桩的 (请求数, 应答数) 为地面真值——它直接数串口上真实的帧交换，
    // 不经过任何指标口径，不会被 `collect_failures` 的记账方式影响。
    let wire_req: u64 = dev_req.values().sum();
    let wire_ans: u64 = dev_ans.values().sum();
    if wire_req > 0 {
        let rate = wire_ans as f64 / wire_req as f64;
        v.row(
            "稳态链路应答率",
            format!("{:.4}%（{} / {} 次请求）", rate * 100.0, wire_ans, wire_req),
            &format!("≥ {:.1}%", thresholds.min_success_rate * 100.0),
            true,
            rate >= thresholds.min_success_rate,
        );
    } else {
        v.notes
            .push("从站请求数为 0（可能存在任务卡死），应答率判定被跳过".to_string());
        v.row("稳态采集活跃", "0 次请求".to_string(), "> 0", true, false);
    }

    // 交叉校验：`gateway_collect_failures` 按点记账（每个点位每次尝试 +1），
    // 而链路应答率按帧记账（每从站每周期 1 帧），两者量纲不同不能直接比，
    // 但「稳态有未应答帧却 collect_failures 全程不增长」一定是记账漏了。
    if wire_ans < wire_req {
        let sampled_fail = samples.last().map(|s| s.collect_fail).unwrap_or(0);
        if sampled_fail == 0 {
            v.soft_failures.push(format!(
                "稳态有 {} 次请求未获应答，但 gateway_collect_failures 全程为 0：采集失败记账口径可疑",
                wire_req - wire_ans
            ));
        }
    }

    // ---------- 4.5 总线活跃度（硬兜底） ----------
    // 稳态链路应答率对死总线不可见：serial_state=None 时 serial task 立即
    // 返回 BusClosed、不发串口帧，从站桩 exp 不增长，dev_req 中该总线无
    // 任何 (dr>0) 条目，被 dr==0 静默跳过，导致应答率仍显示 100%。
    // 此处直接按总线汇总稳态串口请求数，0 即判定该总线死亡。
    if !steady_pairs.is_empty() {
        for bus in 0..spec.buses {
            let bus_req: u64 = dev_req
                .iter()
                .filter(|((b, _), _)| *b == bus)
                .map(|(_, v)| *v)
                .sum();
            v.row(
                &format!("总线活跃度 bus{}", bus),
                format!("{} 次请求", bus_req),
                "> 0",
                true,
                bus_req > 0,
            );
        }
    }

    // ---------- 5. 单设备成功率 ----------
    let mut worst: Option<((usize, u8), f64, u64)> = None;
    for ((bus, slave), req) in &dev_req {
        if *req < thresholds.min_device_requests {
            continue;
        }
        let ans = *dev_ans.get(&(*bus, *slave)).unwrap_or(&0);
        let r = ans as f64 / *req as f64;
        if worst.is_none_or(|(_, wr, _)| r < wr) {
            worst = Some(((*bus, *slave), r, *req));
        }
    }
    match worst {
        Some((id, r, req)) => v.row(
            "单设备最低成功率",
            format!(
                "bus{} slave{} = {:.4}%（{} 次）",
                id.0,
                id.1,
                r * 100.0,
                req
            ),
            &format!("≥ {:.0}%", thresholds.min_device_success_rate * 100.0),
            true,
            r >= thresholds.min_device_success_rate,
        ),
        None => v.notes.push("单设备样本不足，未判定".to_string()),
    }

    // ---------- 6. 队列深度 ----------
    if !steady_pairs.is_empty() {
        let mut depths: Vec<i64> = samples
            .iter()
            .filter(|s| s.phase == "steady")
            .map(|s| s.queue_depth)
            .collect();
        depths.sort_unstable();
        let max = *depths.last().unwrap_or(&0);
        // 最近秩法取 p99；样本极少时退化为最大值
        let p99 = if depths.is_empty() {
            0
        } else {
            depths[(depths.len() - 1) * 99 / 100]
        };
        v.row(
            "队列峰值",
            max.to_string(),
            &format!(
                "≤ {:.0}（{} 周期）",
                cycles * thresholds.queue_max_cycles,
                thresholds.queue_max_cycles
            ),
            true,
            max as f64 <= cycles * thresholds.queue_max_cycles,
        );
        v.row(
            "队列 p99",
            p99.to_string(),
            &format!(
                "≤ {:.0}（{} 周期）",
                cycles * thresholds.queue_p99_cycles,
                thresholds.queue_p99_cycles
            ),
            false,
            p99 as f64 <= cycles * thresholds.queue_p99_cycles,
        );
    }

    // ---------- 7. 北向一致性 ----------
    // 分母用 `collect_total`：它对每个点位的每次尝试都 +1（含失败点），语义准确。
    let total_delta = steady_pairs.iter().fold(0u64, |acc, (a, b)| {
        acc + b.collect_total.saturating_sub(a.collect_total)
    });
    let sink_delta = steady_pairs.iter().fold(0u64, |acc, (a, b)| {
        acc + b.sink_points.saturating_sub(a.sink_points)
    });
    if total_delta > 0 {
        let ratio = sink_delta as f64 / total_delta as f64;
        v.row(
            "北向送达率",
            format!("{:.4}%（{} / {}）", ratio * 100.0, sink_delta, total_delta),
            &format!("≥ {:.1}%", thresholds.min_north_ratio * 100.0),
            true,
            ratio >= thresholds.min_north_ratio,
        );
    } else {
        v.notes
            .push("稳态采集增量为 0，北向送达率未判定".to_string());
    }

    // 送达的点里 Good 质量占比：正常应≈链路应答率；若明显偏低说明管道在污染质量码。
    let good_delta = steady_pairs.iter().fold(0u64, |acc, (a, b)| {
        acc + b.good_points.saturating_sub(a.good_points)
    });
    if sink_delta > 0 {
        let ratio = good_delta as f64 / sink_delta as f64;
        v.row(
            "北向 Good 质量占比",
            format!("{:.4}%（{} / {}）", ratio * 100.0, good_delta, sink_delta),
            &format!("≥ {:.1}%", thresholds.min_success_rate * 100.0),
            true,
            ratio >= thresholds.min_success_rate,
        );
    } else {
        v.notes.push("北向无数据，Good 质量占比未判定".to_string());
    }

    // ---------- 8. 自检 / 重开 / 解码错误（稳态） ----------
    // 必须用 steady_pairs 逐对求和，不能用 steady_slice 首尾差：
    // probe_fail / reopen / decode_errors 都是单调累计计数器，故障窗口内的
    // 增量会留在计数器上。若取「首个稳态样本 → 末个稳态样本」的差，会把
    // 两个稳态样本之间所有故障窗口的增量一并算入稳态，导致「稳态自检失败」
    // 在任何含整总线静默/慢响应超时窗口的跑中恒不达标记。
    let probe_fail_delta: u64 = steady_pairs
        .iter()
        .map(|(a, b)| b.probe_fail.saturating_sub(a.probe_fail))
        .sum();
    let reopen_delta: u64 = steady_pairs
        .iter()
        .map(|(a, b)| b.reopen.saturating_sub(a.reopen))
        .sum();
    let decode_delta: u64 = steady_pairs
        .iter()
        .map(|(a, b)| b.decode_errors.saturating_sub(a.decode_errors))
        .sum();

    v.row(
        "稳态自检失败",
        format!("+{}", probe_fail_delta),
        "= 0",
        false,
        probe_fail_delta == 0,
    );
    // 稳态时长 = 所有相邻稳态样本对的时长之和（排除故障窗口占用的时间）。
    // 用 steady_slice 首尾差会把故障窗口时长算进分母，压低重开速率。
    let steady_hours: f64 = steady_pairs
        .iter()
        .map(|(a, b)| b.elapsed_secs - a.elapsed_secs)
        .sum::<f64>()
        .max(1.0)
        / 3600.0;
    let reopen_rate = reopen_delta as f64 / steady_hours;
    v.row(
        "稳态端口重开",
        format!("{:.1} 次/小时（共 {}）", reopen_rate, reopen_delta),
        &format!("≤ {:.0} 次/小时", thresholds.max_reopen_per_hour),
        false,
        reopen_rate <= thresholds.max_reopen_per_hour,
    );
    v.row(
        "稳态解码错误",
        format!("+{}", decode_delta),
        "= 0",
        false,
        decode_delta == 0,
    );
    // 「自检帧是否触发过」需看全量采样（含故障期），不能用稳态增量——
    // 自检帧本就设计为在故障窗口（整总线静默/慢响应超时）内触发，
    // 稳态增量恒为 0 会导致误报「从未触发」。
    let total_probe = delta(samples, |s| s.probe_ok) + delta(samples, |s| s.probe_fail);
    if total_probe == 0 {
        v.notes
            .push("自检帧从未触发（单从站静默窗口可能太短，未达到 10 次连续静默阈值）".to_string());
    }

    // ---------- 8. 进程级：RSS / 句柄 / PID ----------
    if proc.len() < 2 {
        v.notes.push(
            "proc.jsonl 缺失或样本不足：RSS / 句柄判定被跳过（需用 scripts/soak.ps1 驱动）"
                .to_string(),
        );
    } else {
        let pid = proc[0].pid;
        let pid_changed = proc.iter().any(|p| p.pid != pid);
        v.row(
            "进程 PID",
            if pid_changed {
                "发生变化".to_string()
            } else {
                pid.to_string()
            },
            "恒定",
            true,
            !pid_changed,
        );

        // 基线取预热结束时刻的样本，避免把启动期分配算成“泄漏”
        let start_ts = proc[0].ts_ms;
        let warmup_ms = spec.warmup.as_millis() as u64;
        let base_idx = proc
            .iter()
            .position(|p| p.ts_ms >= start_ts.saturating_add(warmup_ms))
            .unwrap_or(0);
        let base_rss = proc[base_idx].rss_bytes;
        let max_rss = proc.iter().map(|p| p.rss_bytes).max().unwrap_or(0);
        let growth = max_rss - base_rss;
        let ratio = if base_rss > 0 {
            max_rss as f64 / base_rss as f64
        } else {
            1.0
        };
        v.row(
            "RSS 增长",
            format!("{:.1} MB（{:.2}×）", mb(growth), ratio),
            &format!(
                "≤ {:.0} MB 且 ≤ {:.1}×",
                mb(thresholds.max_rss_growth),
                thresholds.max_rss_growth_ratio
            ),
            true,
            growth <= thresholds.max_rss_growth && ratio <= thresholds.max_rss_growth_ratio,
        );
        v.row(
            "RSS 峰值",
            format!("{:.1} MB（{:.2}×）", mb(max_rss), ratio),
            &format!("≤ {:.1}×", thresholds.max_rss_peak_ratio),
            true,
            ratio <= thresholds.max_rss_peak_ratio,
        );

        // 斜率只有在观察窗口足够长时才有意义：短跑里启动期分配会被放大成几十 MB/h
        let (slope, slope_window_hours) = rss_slope_per_hour(proc, base_idx);
        if slope_window_hours >= 1.0 {
            v.row(
                "RSS 末段斜率",
                format!("{:.2} MB/h", mb_f(slope)),
                &format!("≤ {:.1} MB/h", mb_f(thresholds.max_rss_slope_per_hour)),
                false,
                slope <= thresholds.max_rss_slope_per_hour,
            );
        } else {
            v.notes.push(format!(
                "RSS 斜率观察窗仅 {:.1} 分钟，不足以判定趋势（需 ≥ 1h）",
                slope_window_hours * 60.0
            ));
        }

        let base_h = proc[base_idx].handles;
        let max_h = proc.iter().map(|p| p.handles).max().unwrap_or(0);
        v.row(
            "句柄增量",
            format!("+{}（基线 {}）", max_h - base_h, base_h),
            &format!("≤ +{}", thresholds.max_handle_growth),
            true,
            max_h - base_h <= thresholds.max_handle_growth,
        );
    }

    // ---------- 9. 故障后恢复（软） ----------
    for w in &timeline.windows {
        let deadline = w.end.as_secs_f64() + thresholds.recovery_secs;
        let after = samples
            .iter()
            .filter(|s| s.elapsed_secs >= deadline)
            .take(6)
            .map(|s| s.queue_depth)
            .max();
        if let Some(q) = after {
            v.row(
                &format!("故障后队列回落（{}）", w.label),
                format!("{}", q),
                &format!("≤ {:.0}", cycles * thresholds.queue_p99_cycles),
                false,
                (q as f64) <= cycles * thresholds.queue_p99_cycles,
            );
        }
    }

    Ok(v)
}

fn delta(samples: &[Sample], f: impl Fn(&Sample) -> u64) -> u64 {
    match (samples.first(), samples.last()) {
        (Some(a), Some(b)) => f(b).saturating_sub(f(a)),
        _ => 0,
    }
}

/// 返回 (斜率 bytes/h, 实际观察窗口小时数)。
///
/// `base_idx` 是预热结束时刻的样本下标。斜率必须**从预热结束后算起**：
/// 否则短于 6h 的运行会回退到 `proc.first()`（进程启动瞬间），
/// 把启动期一次性分配摊进斜率 → 恒定误报「泄漏」。
///
/// 另：用**最小二乘拟合**而非首尾两点差值。RSS 是锯齿状曲线，
/// 首尾两点会被噪声主导（同一份数据：两点差值 +2.24 MB/h，
/// 最小二乘 +0.46 MB/h，中位数分段 +0.18 MB/h），
/// 两点差值会把正常运行的进程判成泄漏。
fn rss_slope_per_hour(proc: &[ProcSample], base_idx: usize) -> (f64, f64) {
    let last = match proc.last() {
        Some(p) => p,
        None => return (0.0, 0.0),
    };
    let window_ms = 6.0 * 3600.0 * 1000.0;
    // 末段窗口起点：优先取「距末样本 ≥ 6h 的最早样本」；
    // 若运行不足 6h，则从预热结束处开始（而非进程启动处）。
    let start_idx = proc
        .iter()
        .position(|p| (last.ts_ms.saturating_sub(p.ts_ms)) as f64 >= window_ms)
        .map(|i| i.max(base_idx))
        .unwrap_or(base_idx);
    let window = &proc[start_idx..];
    if window.len() < 2 || last.ts_ms <= window[0].ts_ms {
        return (0.0, 0.0);
    }

    // 最小二乘：slope = Σ(x-x̄)(y-ȳ) / Σ(x-x̄)²
    let n = window.len() as f64;
    let t0 = window[0].ts_ms as f64;
    let xs: Vec<f64> = window
        .iter()
        .map(|p| (p.ts_ms as f64 - t0) / 3_600_000.0)
        .collect();
    let ys: Vec<f64> = window.iter().map(|p| p.rss_bytes as f64).collect();
    let mean_x = xs.iter().sum::<f64>() / n;
    let mean_y = ys.iter().sum::<f64>() / n;
    let mut num = 0.0;
    let mut den = 0.0;
    for (x, y) in xs.iter().zip(ys.iter()) {
        let dx = x - mean_x;
        num += dx * (y - mean_y);
        den += dx * dx;
    }
    if den <= 0.0 {
        return (0.0, 0.0);
    }
    let hours = (last.ts_ms - window[0].ts_ms) as f64 / 3_600_000.0;
    (num / den, hours)
}

fn mb(bytes: i64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn mb_f(bytes: f64) -> f64 {
    bytes / (1024.0 * 1024.0)
}

/// 读取 JSONL 为 `Vec<T>`（空行忽略）。
pub fn read_jsonl<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(path)?;
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        out.push(serde_json::from_str(line)?);
    }
    Ok(out)
}

/// 渲染 markdown 报告并写入 `report.md`。
pub fn render_report(
    spec: &SoakSpec,
    verdict: &Verdict,
    samples: &[Sample],
    out_dir: &Path,
) -> Result<String> {
    let fault_samples = samples.iter().filter(|s| s.phase == "fault").count();
    let mut md = String::new();
    md.push_str("# 浸泡测试报告\n\n");
    md.push_str(&format!(
        "- 规模：{} 点 / {} 总线 × {} 从站 × {} 点，周期 {}s  \n",
        spec.actual_points(),
        spec.buses,
        spec.slaves_per_bus,
        spec.points_per_slave,
        spec.interval_secs
    ));
    md.push_str(&format!(
        "- 时长：{}s（预热 {}s，采样 {}s）  \n",
        spec.duration.as_secs(),
        spec.warmup.as_secs(),
        spec.sample.as_secs()
    ));
    md.push_str(&format!(
        "- 理论采集速率：{:.1} 点/s  \n",
        spec.expected_points_per_sec()
    ));
    md.push_str(&format!(
        "- 采样点：{}（其中故障期 {}）  \n\n",
        samples.len(),
        fault_samples
    ));

    md.push_str(&format!(
        "## 结论：{}\n\n",
        if verdict.passed() {
            "**通过**"
        } else {
            "**未通过**"
        }
    ));

    md.push_str("| 判定项 | 实测 | 阈值 | 类型 | 结果 |\n|---|---|---|---|---|\n");
    for r in &verdict.rows {
        md.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            r.name,
            r.observed,
            r.threshold,
            if r.hard { "硬" } else { "软" },
            if r.passed { "通过" } else { "**失败**" }
        ));
    }

    if !verdict.hard_failures.is_empty() {
        md.push_str("\n### 硬失败\n\n");
        for f in &verdict.hard_failures {
            md.push_str(&format!("- {}\n", f));
        }
    }
    if !verdict.soft_failures.is_empty() {
        md.push_str("\n### 软失败\n\n");
        for f in &verdict.soft_failures {
            md.push_str(&format!("- {}\n", f));
        }
    }
    if !verdict.notes.is_empty() {
        md.push_str("\n### 说明\n\n");
        for n in &verdict.notes {
            md.push_str(&format!("- {}\n", n));
        }
    }

    fs::create_dir_all(out_dir)?;
    let path = out_dir.join("report.md");
    fs::write(&path, &md)?;
    Ok(md)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(ts_ms: u64, rss: i64) -> ProcSample {
        ProcSample {
            ts_ms,
            pid: 1234,
            rss_bytes: rss,
            handles: 100,
        }
    }

    const MB: i64 = 1024 * 1024;

    /// 短于 6h 的运行：斜率必须从**预热结束处**开始算，
    /// 不能把启动期的一次性分配摊进斜率（否则恒定误报泄漏）。
    #[test]
    fn short_run_slope_ignores_startup_allocation() {
        // 前 10 分钟：启动期一次性分配 +8MB（随后稳定）
        // 之后 2 小时：稳定在 20MB 不变
        let mut proc_samples: Vec<ProcSample> = Vec::new();
        let mut ts = 0u64;
        for i in 0..10 {
            proc_samples.push(sample(ts, 12 * MB + i * MB));
            ts += 60_000;
        }
        for _ in 0..120 {
            proc_samples.push(sample(ts, 20 * MB));
            ts += 60_000;
        }
        // 预热 10 分钟 → base_idx 指向 ts = 10min 的样本
        let base_idx = proc_samples
            .iter()
            .position(|p| p.ts_ms >= 600_000)
            .unwrap();
        let (slope, hours) = rss_slope_per_hour(&proc_samples, base_idx);
        assert!(
            slope.abs() < 0.01 * MB as f64,
            "稳定期斜率应接近 0，实际 {} bytes/h",
            slope
        );
        assert!(hours > 1.0, "观察窗应 > 1h，实际 {}", hours);
    }

    /// 真实泄漏必须被检出：稳定增长 1MB/h（每分钟采样，2 小时涨 2MB）。
    #[test]
    fn real_leak_is_detected() {
        let mut proc_samples: Vec<ProcSample> = Vec::new();
        let mut ts = 0u64;
        for i in 0..120 {
            // 每 60 分钟涨 1MB → 1 MB/h（线性爬升）
            let rss = 20 * MB + ((i as f64) * (MB as f64) / 60.0) as i64;
            proc_samples.push(sample(ts, rss));
            ts += 60_000;
        }
        let (slope, hours) = rss_slope_per_hour(&proc_samples, 0);
        let mb_per_h = slope / MB as f64;
        assert!(
            (mb_per_h - 1.0).abs() < 0.1,
            "泄漏斜率应约 1 MB/h，实际 {}",
            mb_per_h
        );
        assert!(hours > 1.0);
    }

    /// 锯齿状噪声不应被判成泄漏：均值稳定但首尾两点差异大。
    /// 首尾差值法在此会给出显著正斜率，最小二乘应接近 0。
    #[test]
    fn noisy_rss_is_not_misjudged_as_leak() {
        // 锯齿：0, +3, -2, +2.5, -1.5 MB 循环，均值不变
        let pattern = [0i64, 3, -2, 2, -1];
        let mut proc_samples: Vec<ProcSample> = Vec::new();
        let mut ts = 0u64;
        for i in 0..180 {
            proc_samples.push(sample(ts, (20 + pattern[i % pattern.len()]) * MB));
            ts += 60_000;
        }
        let (slope, _) = rss_slope_per_hour(&proc_samples, 0);
        assert!(
            slope.abs() < 0.1 * MB as f64,
            "锯齿噪声下斜率应接近 0，实际 {} bytes/h",
            slope
        );
    }

    /// 样本不足时不应产生斜率（避免除零 / 假阳性）。
    #[test]
    fn insufficient_samples_yield_zero_slope() {
        let proc_samples = vec![sample(0, 20 * MB)];
        let (slope, hours) = rss_slope_per_hour(&proc_samples, 0);
        assert_eq!(slope, 0.0);
        assert_eq!(hours, 0.0);
        assert_eq!(rss_slope_per_hour(&[], 0), (0.0, 0.0));
    }
}
