//! F-11：采集任务真实时序快照（供 `/api/diagnostics/schedule-timeline`）
//!
//! 背景：此前运维页的「甘特图」用配置里的采集间隔 + 平均延迟**前端推算**任务条，
//! 还自行算出「检测到 N 处时间重叠」的结论——视觉精确、结论却是编造的。
//! 本模块在采集循环里记录**每组的真实执行窗口**（起止 epoch 毫秒），
//! 由 API 汇总成时间线；重叠结论直接来自真实样本，不再由前端推测。
//!
//! 设计约束（稳定性优先）：
//! - 每条 group 只保留最近 `MAX_SAMPLES_PER_GROUP` 条样本，内存上界固定；
//! - 记录发生在采集循环的**尾部**（结果已交给 pipeline），不改变采集语义；
//! - 锁为 parking_lot 短临界区，不跨 await，不参与 scheduler 的 LOCK ORDER 嵌套。

use std::collections::HashMap;
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;

/// 每组保留的真实样本上限（10s 窗口、1s 周期下足够覆盖多次执行）
pub const MAX_SAMPLES_PER_GROUP: usize = 32;

/// 单次快照返回的重叠区间上限
const MAX_OVERLAPS: usize = 64;

/// 单次执行样本（epoch 毫秒）
#[derive(Debug, Clone)]
struct RawSample {
    start_ms: i64,
    end_ms: i64,
}

#[derive(Debug, Default)]
struct GroupRec {
    protocol: String,
    bus: String,
    interval_ms: u64,
    runs: u64,
    samples: Vec<RawSample>,
}

/// 单次执行的对外形态
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct TimelineSample {
    /// 开始时刻（epoch 毫秒）
    #[ts(type = "number")]
    pub start_ms: i64,
    /// 结束时刻（epoch 毫秒）
    #[ts(type = "number")]
    pub end_ms: i64,
    /// 本次耗时（毫秒）
    #[ts(type = "number")]
    pub duration_ms: u64,
    /// 本次耗时是否超过该组采集间隔（超时即背靠背占用总线）
    pub overrun: bool,
}

/// 一个采集组的时序
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct TimelineGroup {
    /// 组键 `protocol:bus`
    pub group: String,
    /// 协议名
    pub protocol: String,
    /// 总线名
    pub bus: String,
    /// 该组当前采集间隔（毫秒，退避后周期）
    #[ts(type = "number")]
    pub interval_ms: u64,
    /// 本进程内的累计执行次数
    #[ts(type = "number")]
    pub runs: u64,
    /// 窗口内平均耗时（毫秒）
    #[ts(type = "number")]
    pub avg_duration_ms: u64,
    /// 窗口内最大耗时（毫秒）
    #[ts(type = "number")]
    pub max_duration_ms: u64,
    /// 窗口内真实执行样本（按时间升序）
    pub samples: Vec<TimelineSample>,
}

/// 同一总线上不同组的真实时间重叠
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct TimelineOverlap {
    pub bus: String,
    /// 重叠区间起点（epoch 毫秒）
    #[ts(type = "number")]
    pub start_ms: i64,
    /// 重叠区间终点（epoch 毫秒）
    #[ts(type = "number")]
    pub end_ms: i64,
    /// 重叠时长（毫秒）
    #[ts(type = "number")]
    pub overlap_ms: u64,
    pub a_group: String,
    pub b_group: String,
}

/// `/api/diagnostics/schedule-timeline` 响应体
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ScheduleTimeline {
    /// 生成时刻（epoch 毫秒）
    #[ts(type = "number")]
    pub generated_at_ms: i64,
    /// 统计窗口（毫秒）
    #[ts(type = "number")]
    pub window_ms: u64,
    /// 采集调度是否在运行
    pub running: bool,
    /// 采集组数
    pub group_count: usize,
    pub groups: Vec<TimelineGroup>,
    pub overlaps: Vec<TimelineOverlap>,
}

impl ScheduleTimeline {
    /// 空快照：trait 默认实现（mock 调度器 / 无时序能力）用
    pub fn empty(window_ms: u64) -> Self {
        Self {
            generated_at_ms: now_ms(),
            window_ms,
            running: false,
            group_count: 0,
            groups: Vec::new(),
            overlaps: Vec::new(),
        }
    }
}

/// 采集时序注册表（每进程一份，挂在 Scheduler 上）
#[derive(Debug, Default)]
pub struct Timeline {
    groups: Mutex<HashMap<String, GroupRec>>,
}

impl Timeline {
    /// 记录一次真实执行（由采集循环尾部调用）
    pub fn record(
        &self,
        key: &str,
        protocol: &str,
        bus: &str,
        interval: Duration,
        start_ms: i64,
        end_ms: i64,
    ) {
        let mut map = self.groups.lock();
        let rec = map.entry(key.to_string()).or_default();
        rec.protocol = protocol.to_string();
        rec.bus = bus.to_string();
        rec.interval_ms = interval.as_millis() as u64;
        rec.runs = rec.runs.saturating_add(1);
        rec.samples.push(RawSample {
            start_ms,
            end_ms: end_ms.max(start_ms),
        });
        if rec.samples.len() > MAX_SAMPLES_PER_GROUP {
            rec.samples.remove(0);
        }
    }

    /// 重新开始一轮运行期（`Scheduler::start()` 时清空，避免重启前样本混入）
    pub fn clear(&self) {
        self.groups.lock().clear();
    }

    /// 生成时间线快照：只保留窗口内的真实样本，并按总线计算真实重叠
    pub fn snapshot(&self, window_ms: u64) -> ScheduleTimeline {
        let now = now_ms();
        let since = now.saturating_sub(window_ms as i64);

        let mut groups: Vec<TimelineGroup> = Vec::new();
        let mut per_bus: HashMap<String, Vec<(String, RawSample)>> = HashMap::new();

        let map = self.groups.lock();
        for (key, rec) in map.iter() {
            let samples: Vec<RawSample> = rec
                .samples
                .iter()
                .filter(|s| s.end_ms >= since)
                .cloned()
                .collect();
            if samples.is_empty() {
                continue;
            }
            let durations: Vec<u64> = samples
                .iter()
                .map(|s| (s.end_ms - s.start_ms).max(0) as u64)
                .collect();
            let sum: u64 = durations.iter().sum();
            groups.push(TimelineGroup {
                group: key.clone(),
                protocol: rec.protocol.clone(),
                bus: rec.bus.clone(),
                interval_ms: rec.interval_ms,
                runs: rec.runs,
                avg_duration_ms: sum / durations.len() as u64,
                max_duration_ms: durations.iter().copied().max().unwrap_or(0),
                samples: samples
                    .iter()
                    .map(|s| {
                        let duration_ms = (s.end_ms - s.start_ms).max(0) as u64;
                        TimelineSample {
                            start_ms: s.start_ms,
                            end_ms: s.end_ms,
                            duration_ms,
                            overrun: rec.interval_ms > 0 && duration_ms > rec.interval_ms,
                        }
                    })
                    .collect(),
            });
            let entry = per_bus.entry(rec.bus.clone()).or_default();
            for s in &samples {
                entry.push((key.clone(), s.clone()));
            }
        }
        drop(map);

        groups.sort_by(|a, b| a.bus.cmp(&b.bus).then_with(|| a.group.cmp(&b.group)));

        // 真实重叠：同一总线上「不同组」的执行窗口相交。同一总线在物理层串行，
        // 若不同组窗口相交，说明该组耗时已超出调度预算（是真问题，不是推测）。
        // 全配对比较（样本量受 MAX_SAMPLES_PER_GROUP 约束，规模可控），
        // 输出条数封顶，避免极端配置把响应体撑大。
        let mut overlaps: Vec<TimelineOverlap> = Vec::new();
        for (bus, mut list) in per_bus {
            list.sort_by_key(|(_, s)| s.start_ms);
            'pair: for i in 0..list.len() {
                for j in (i + 1)..list.len() {
                    let (a_group, a) = &list[i];
                    let (b_group, b) = &list[j];
                    if a_group == b_group {
                        continue;
                    }
                    let start = a.start_ms.max(b.start_ms);
                    let end = a.end_ms.min(b.end_ms);
                    if end > start {
                        if overlaps.len() >= MAX_OVERLAPS {
                            break 'pair;
                        }
                        overlaps.push(TimelineOverlap {
                            bus: bus.clone(),
                            start_ms: start,
                            end_ms: end,
                            overlap_ms: (end - start) as u64,
                            a_group: a_group.clone(),
                            b_group: b_group.clone(),
                        });
                    }
                }
            }
        }
        overlaps.sort_by_key(|o| (o.bus.clone(), o.start_ms));

        ScheduleTimeline {
            generated_at_ms: now,
            window_ms,
            running: false,
            group_count: groups.len(),
            groups,
            overlaps,
        }
    }
}

/// 当前 epoch 毫秒
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
