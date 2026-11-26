//! 边缘计算节点（聚合、死区、变化检测）

use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};

use crate::config::{AggregationConfig, PointConfig};
use crate::engine::model::DataPoint;
use crate::engine::model::Quality;

use super::edge::{aggregate, PipelineNode};

/// 补窗（catch-up）单次最多生成的空窗口数量。
///
/// 设备离线很久、或系统时钟前跳时，`当前窗口 - 上次窗口` 可达数天。若按 `window_ms`
/// 逐窗补齐，会一次性产出海量 DataPoint（1s 窗口离线一天 = 86400 个/点位），撑爆
/// 内存与下游队列。超过本上限即停止补窗，由调用方直接对齐到目标窗口边界，
/// 中间空洞不再逐窗生成占位样本（宁可丢细节，不可拖垮网关）。
///
/// 写入路径（`add_to_buffer`）与刷新路径（`flush_aggregated`）共用同一口径，
/// 避免两条路径的补窗上限不一致导致行为分裂。
const MAX_CATCHUP_WINDOWS: usize = 64;

/// 聚合窗口产出的数据点。
///
/// 聚合值是**派生值**，没有单一的「原始采集时刻」：这里取窗口右边界（与上报
/// `timestamp` 一致），语义为「本样本代表截至该时刻的窗口」。此前填 0 会让北向
/// 拿到 1970-01-01 的 source timestamp，客户端据此判定数据陈旧。
fn aggregated_point(point_id: &str, value: f64, window_end: i64) -> DataPoint {
    DataPoint {
        point_id: point_id.to_string(),
        device_id: String::new(),
        value,
        raw_value: value,
        raw_type: "aggregated".to_string(),
        quality: Quality::Good,
        quality_reason: String::new(),
        timestamp: window_end,
        // 派生值：source_ts 与上送时刻同取窗口右边界（见函数文档）
        source_ts: window_end,
        stale_count: 0,
        value_text: None,
    }
}

/// 空窗口占位点（`Bad` / `empty_window`），时间口径同 [`aggregated_point`]。
fn empty_window_point(point_id: &str, window_end: i64) -> DataPoint {
    DataPoint {
        point_id: point_id.to_string(),
        device_id: String::new(),
        value: 0.0,
        raw_value: 0.0,
        raw_type: "empty_window".to_string(),
        quality: Quality::Bad,
        quality_reason: "aggregate:EMPTY_WINDOW".to_string(),
        timestamp: window_end,
        source_ts: window_end,
        stale_count: 0,
        value_text: None,
    }
}

struct AggBuffer {
    values: Vec<(i64, f64)>, // (timestamp, value)
    window_start: i64,
    /// 上一个（或多个）窗口的待输出聚合值队列（窗口切换时 push，等 flush 取走）。
    pending: Vec<(f64, i64)>, // (aggregate_value, window_start_of_old_window)
}

/// 聚合节点
///
/// 收集所有 Good 样本到窗口缓冲区，窗口结束时输出聚合值。
/// 聚合模式下原始值不直接上传（`process` 返回 `None`），由 `flush_aggregated` 输出。
///
/// 实现了 `PipelineNode` 的 `flush_aggregated` / `flush_all`，
/// 可作为标准节点接入 `EdgeProcessor` 的统一节点循环。
pub struct AggregationNode {
    buffers: Mutex<HashMap<String, AggBuffer>>,
}

impl AggregationNode {
    pub fn new() -> Self {
        Self {
            buffers: Mutex::new(HashMap::new()),
        }
    }

    fn add_to_buffer(&self, dp: &DataPoint, agg: &AggregationConfig) {
        let window_ms = (agg.window * 1000) as i64;
        let aligned_start = if window_ms > 0 {
            dp.timestamp - (dp.timestamp % window_ms)
        } else {
            dp.timestamp
        };
        let mut buffers = self.buffers.lock();
        let buffer = buffers
            .entry(dp.point_id.clone())
            .or_insert_with(|| AggBuffer {
                values: Vec::new(),
                window_start: aligned_start,
                pending: Vec::new(),
            });

        if dp.timestamp - buffer.window_start >= window_ms {
            // Bug 34: handle old window and all intermediate empty windows
            if !buffer.values.is_empty() {
                let agg_value = aggregate(&buffer.values, agg.function);
                buffer.pending.push((agg_value, buffer.window_start));
            }
            // Push Bad placeholders for intermediate empty windows
            if window_ms > 0 {
                let mut ws = buffer.window_start + window_ms;
                let mut catchup_windows: usize = 0;
                while ws < aligned_start {
                    // Bug 168: 写入路径的补窗同样必须有上限。原实现只给 `flush_aggregated`
                    // 加了限制，而本函数在「样本时间戳跨越多个窗口」时（设备离线后首个
                    // 恢复样本、系统时钟前跳、或点位被临时禁用后重新启用）会一次性把
                    // 整个缺口逐窗 push 进 `pending`。`pending` 在下一次 flush 时会被
                    // 逐条展开成 DataPoint，因此这里的内存放大倍数等于缺口窗口数，
                    // 且对每个点位各算一份，属于典型的无界内存增长。
                    if catchup_windows >= MAX_CATCHUP_WINDOWS {
                        tracing::warn!(
                            "aggregation {}: catch-up window limit ({}) reached while buffering, \
                             skipping remaining gap of {} windows",
                            dp.point_id,
                            MAX_CATCHUP_WINDOWS,
                            (aligned_start - ws) / window_ms
                        );
                        break;
                    }
                    catchup_windows += 1;
                    buffer.pending.push((f64::NAN, ws));
                    ws += window_ms;
                }
            }
            buffer.window_start = aligned_start;
            buffer.values.clear();
        }
        buffer.values.push((dp.timestamp, dp.value));
    }
}

impl Default for AggregationNode {
    fn default() -> Self {
        Self::new()
    }
}

impl PipelineNode for AggregationNode {
    fn name(&self) -> &'static str {
        "aggregation"
    }

    fn process(&self, point: &PointConfig, dp: DataPoint) -> Option<DataPoint> {
        // 仅对 Good 样本聚合；失败数据直接透传给后续节点
        if dp.quality != Quality::Good {
            return Some(dp);
        }
        if let Some(agg) = &point.aggregation {
            self.add_to_buffer(&dp, agg);
            return None; // 聚合模式下原始值不直接上传
        }
        Some(dp)
    }

    /// 刷新产物（聚合值）重入时直接放行
    ///
    /// 聚合值已是最终结果：若走 `process`，`point.aggregation` 仍在 → 会被重新塞回
    /// 窗口缓冲并返回 `None`（该值当轮丢失），下一轮 flush 又把它输出、再次重入，
    /// 形成每秒重复上报同一值的循环。这里直接 `Some(dp)`，只让后续节点
    /// （死区/变化上传）继续处理。
    fn process_flushed(&self, _point: &PointConfig, dp: DataPoint) -> Option<DataPoint> {
        Some(dp)
    }

    /// 刷新所有到期的聚合窗口，返回聚合后的数据点
    fn flush_aggregated(&self, points: &[PointConfig]) -> Vec<DataPoint> {
        let mut result = Vec::new();
        let now = DataPoint::now();
        let point_map: HashMap<&str, &PointConfig> =
            points.iter().map(|p| (p.id.as_str(), p)).collect();

        let mut buffers = self.buffers.lock();
        let point_ids: Vec<String> = buffers.keys().cloned().collect();

        for pid in point_ids {
            if let Some(point) = point_map.get(pid.as_str()) {
                if let Some(agg) = &point.aggregation {
                    let window_ms = (agg.window * 1000) as i64;
                    if let Some(buffer) = buffers.get_mut(&pid) {
                        // 1. 取出窗口切换时暂存的旧窗口聚合值
                        // Bug 34: NAN values represent Bad empty-window placeholders
                        let pending: Vec<(f64, i64)> = std::mem::take(&mut buffer.pending);
                        for (agg_value, old_window_start) in pending {
                            let window_end = old_window_start + window_ms;
                            result.push(if agg_value.is_nan() {
                                empty_window_point(&pid, window_end)
                            } else {
                                aggregated_point(&pid, agg_value, window_end)
                            });
                        }
                        // 2. 检查当前窗口是否超时，循环处理所有过期窗口
                        // Bug 34: 原代码只处理一个窗口，跨多窗口时中间窗口丢失
                        // Bug 167: 补窗循环必须有迭代上限——若网关离线很久或系统时钟前跳，
                        // `now - window_start` 可达数天，按 window_ms 逐窗补齐会一次性产出
                        // 海量 DataPoint，撑爆内存与下游队列。超出上限直接跳出，由下方
                        // “对齐到当前窗口边界”收尾，中间空洞不再逐窗生成占位样本。
                        let mut catchup_windows: usize = 0;
                        while window_ms > 0 && now - buffer.window_start >= window_ms {
                            if catchup_windows >= MAX_CATCHUP_WINDOWS {
                                tracing::warn!(
                                    "aggregation {}: catch-up window limit ({}) reached, skipping remaining gap",
                                    pid,
                                    MAX_CATCHUP_WINDOWS
                                );
                                break;
                            }
                            catchup_windows += 1;
                            let window_end = buffer.window_start + window_ms;
                            if !buffer.values.is_empty() {
                                // 有数据：正常聚合输出
                                let agg_value = aggregate(&buffer.values, agg.function);
                                result.push(aggregated_point(&pid, agg_value, window_end));
                                buffer.values.clear();
                            } else {
                                // 空窗口（设备离线无数据）：输出 Bad 占位样本
                                result.push(empty_window_point(&pid, window_end));
                            }
                            buffer.window_start += window_ms;
                        }
                        // 对齐到当前窗口边界
                        if window_ms > 0 {
                            buffer.window_start = now - (now % window_ms);
                        } else {
                            buffer.window_start = now;
                        }
                    }
                } else {
                    // Bug 35: aggregation config removed, clean up stale buffer
                    buffers.remove(&pid);
                }
            } else {
                // Bug 35: point removed from config, clean up stale buffer
                buffers.remove(&pid);
            }
        }
        result
    }

    /// 强制刷新所有聚合缓冲区（用于退出前）
    fn flush_all(&self, points: &[PointConfig]) -> Vec<DataPoint> {
        let mut result = Vec::new();
        let point_map: HashMap<&str, &PointConfig> =
            points.iter().map(|p| (p.id.as_str(), p)).collect();

        let mut buffers = self.buffers.lock();
        for (pid, buffer) in buffers.iter_mut() {
            if let Some(point) = point_map.get(pid.as_str()) {
                if let Some(agg) = &point.aggregation {
                    let window_ms = (agg.window * 1000) as i64;
                    let pending: Vec<(f64, i64)> = std::mem::take(&mut buffer.pending);
                    for (agg_value, old_window_start) in pending {
                        let window_end = old_window_start + window_ms;
                        result.push(if agg_value.is_nan() {
                            // `pid` 来自 `buffers.iter_mut()`，已是 `&String`
                            empty_window_point(pid, window_end)
                        } else {
                            aggregated_point(pid, agg_value, window_end)
                        });
                    }
                    if !buffer.values.is_empty() {
                        let agg_value = aggregate(&buffer.values, agg.function);
                        result.push(aggregated_point(pid, agg_value, DataPoint::now()));
                    }
                }
            }
        }
        buffers.clear();
        result
    }
}

/// 死区过滤节点
///
/// 值变化小于死区阈值时过滤掉，减少无效上传。
pub struct DeadbandNode {
    last_values: Mutex<HashMap<String, f64>>,
}

impl DeadbandNode {
    pub fn new() -> Self {
        Self {
            last_values: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for DeadbandNode {
    fn default() -> Self {
        Self::new()
    }
}

impl PipelineNode for DeadbandNode {
    fn name(&self) -> &'static str {
        "deadband"
    }

    fn process(&self, point: &PointConfig, dp: DataPoint) -> Option<DataPoint> {
        if point.deadband <= 0.0 {
            return Some(dp);
        }
        // Bug 75 修复：NaN/Inf 比较总为 false（NaN < anything = false, NaN != NaN = true），
        // 原实现会让 NaN 绕过死区检查并写入 last_values，污染后续所有比较。
        // NaN/Inf 视为异常值：直接通过 + 不更新 last_values，避免污染。
        if !dp.value.is_finite() {
            return Some(dp);
        }
        let mut last_values = self.last_values.lock();
        if let Some(last_val) = last_values.get(&dp.point_id).copied() {
            // last_val 也可能是上一轮被错误写入的 NaN，需防御
            if last_val.is_finite() && (dp.value - last_val).abs() < point.deadband {
                return None; // 变化小于死区，过滤
            }
        }
        last_values.insert(dp.point_id.clone(), dp.value);
        Some(dp)
    }

    /// 质量恢复样本：放行并把基线更新为恢复值，避免下一条同值样本被误判为「变化」。
    fn process_recovered(&self, _point: &PointConfig, dp: DataPoint) -> Option<DataPoint> {
        if dp.value.is_finite() {
            self.last_values
                .lock()
                .insert(dp.point_id.clone(), dp.value);
        }
        Some(dp)
    }

    /// DD-16：点位下线后淘汰死区基线，避免基线表随点位增删单调增长。
    fn prune(&self, active: &HashSet<String>) {
        self.last_values.lock().retain(|id, _| active.contains(id));
    }
}

/// 变化上传节点
///
/// 仅在值发生变化时上传，值不变则过滤。变化阈值优先复用点位死区（`deadband`），
/// 死区为 0 时才退化为「完全相同才过滤」——`f64::EPSILON` 量级对工程量纲没有
/// 实际过滤意义，变化上报的带宽收益实际来自死区配置。
pub struct ChangeNode {
    last_values: Mutex<HashMap<String, f64>>,
}

impl ChangeNode {
    pub fn new() -> Self {
        Self {
            last_values: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for ChangeNode {
    fn default() -> Self {
        Self::new()
    }
}

impl PipelineNode for ChangeNode {
    fn name(&self) -> &'static str {
        "on_change"
    }

    fn process(&self, point: &PointConfig, dp: DataPoint) -> Option<DataPoint> {
        if !point.on_change {
            return Some(dp);
        }
        // Bug 76 修复：NaN/Inf 与任何值比较都返回 false（包括 NaN != NaN），
        // 原实现会让 NaN 绕过变化检查并写入 last_values，导致后续比较全失效。
        // NaN/Inf 视为变化：直接通过，但**不写入基线**（I-21，见下方 insert 处）。
        let is_nan = !dp.value.is_finite();
        // 变化阈值复用死区：死区节点在前，正常路径下这里不会触发；但允许
        // 单独使用本节点（自定义管道）时也能按死区过滤，避免 f64::EPSILON
        // 让「变化上报」退化成「每样本都上报」。
        let threshold = if point.deadband > 0.0 {
            point.deadband
        } else {
            f64::EPSILON
        };
        let mut last_values = self.last_values.lock();
        if let Some(last_val) = last_values.get(&dp.point_id).copied() {
            if !is_nan && last_val.is_finite() && (dp.value - last_val).abs() < threshold {
                return None; // 值未变化（或在死区内），过滤
            }
        }
        // I-21：NaN/Inf 放行但不写基线。把非有限值当基线后，下一条有限样本
        // 与 NaN 比较恒为 false，会被判成「变化」而每条都上报；且基线从此
        // 失去参照意义。死区节点同样只在有限值时才更新基线，两处口径必须一致。
        if dp.value.is_finite() {
            last_values.insert(dp.point_id.clone(), dp.value);
        }
        Some(dp)
    }

    /// 质量恢复样本：放行并更新基线（NaN/Inf 也记账，与其 `process` 行为一致）。
    fn process_recovered(&self, _point: &PointConfig, dp: DataPoint) -> Option<DataPoint> {
        self.last_values
            .lock()
            .insert(dp.point_id.clone(), dp.value);
        Some(dp)
    }

    /// DD-16：点位下线后淘汰变化上传基线，避免基线表随点位增删单调增长。
    fn prune(&self, active: &HashSet<String>) {
        self.last_values.lock().retain(|id, _| active.contains(id));
    }
}
