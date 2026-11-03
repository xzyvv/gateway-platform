//! 采集统计报表 handler
//!
//! 数据全部来自 [`ScopedCollectMetrics`]：采集循环里每次真实读取的「成功/失败 + 耗时」
//! 按总线与点位累计（最近 31 天日桶）。此前用 `hash(设备ID + 时间片)` 生成成功率、
//! 延迟、读数、错误分布，页面数字与设备实际运行无关，已全部删除。
//!
//! 语义说明：
//! - 窗口内**没有任何采集**时，成功率与平均延迟返回 `null`（前端渲染「—」），
//!   不用 0 或 100 冒充；
//! - 计数类字段（total_reads / success / failed / data_points）为 0 表示「窗口内确实无采集」，
//!   是真实值而非占位；
//! - 错误类型分布取全局错误计数器（进程启动以来），按三类真实分类汇总。

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use gateway_core::config::{DeviceConfig, GatewayConfig, PointConfig};
use gateway_core::engine::metrics::{Metrics, ScopeStats};

use crate::web::types::{ApiError, ApiResponse, AppState};

/// 单设备统计
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DeviceStat {
    pub device_id: String,
    pub device_name: String,
    pub protocol: String,
    pub total_reads: i32,
    pub success: i32,
    pub failed: i32,
    /// 成功率（%）；窗口内无采集为 null
    pub success_rate: Option<f64>,
    /// 平均延迟（ms）；窗口内无采集为 null
    pub avg_latency_ms: Option<i32>,
    pub data_points: i32,
}

/// 单总线统计
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct BusStat {
    pub bus: String,
    pub protocol: String,
    pub total_reads: i32,
    pub success: i32,
    pub failed: i32,
    pub success_rate: Option<f64>,
    pub avg_latency_ms: Option<i32>,
    pub device_count: i32,
    /// 在线设备数（最近一次采集成功的设备数）
    pub online_devices: i32,
    /// 超时率（%）；窗口内无采集为 null
    pub timeout_rate: Option<f64>,
    /// 最近一次失败的错误码（如 TIMEOUT / CRC / EXC_02）；窗口内无失败为 null
    pub last_error_code: Option<String>,
    /// 重连次数（自进程启动以来累计）
    pub reconnect_count: i32,
}

/// 错误类型分布
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ErrorDistItem {
    pub error_type: String,
    pub count: i32,
    /// 占比（%）
    pub percentage: f64,
}

/// 汇总指标
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct StatsSummary {
    pub total_reads: i32,
    pub success_rate: Option<f64>,
    pub avg_latency_ms: Option<i32>,
    pub total_data_points: i32,
}

/// 采集统计报表
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct StatsReport {
    /// 周期：day/week/month
    pub period: String,
    /// 起始时间戳（毫秒）
    #[ts(type = "number")]
    pub start_at_ms: i64,
    /// 结束时间戳（毫秒）
    #[ts(type = "number")]
    pub end_at_ms: i64,
    pub device_stats: Vec<DeviceStat>,
    pub bus_stats: Vec<BusStat>,
    pub error_dist: Vec<ErrorDistItem>,
    pub summary: StatsSummary,
}

#[derive(Deserialize)]
pub(crate) struct StatsQuery {
    /// 周期：day/week/month，默认 day
    #[serde(default = "default_period")]
    pub period: String,
}

fn default_period() -> String {
    "day".to_string()
}

fn days_for(period: &str) -> i64 {
    match period {
        "week" => 7,
        "month" => 30,
        _ => 1,
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn clamp_i32(v: u64) -> i32 {
    v.min(i32::MAX as u64) as i32
}

fn accumulate(acc: &mut ScopeStats, s: &ScopeStats) {
    acc.total += s.total;
    acc.success += s.success;
    acc.failed += s.failed;
    acc.duration_sum_us += s.duration_sum_us;
    acc.last_success_ms = acc.last_success_ms.max(s.last_success_ms);
    acc.last_failure_ms = acc.last_failure_ms.max(s.last_failure_ms);
}

/// 某设备的全部点位 ID：显式点位按 `device_id` 匹配，模板展开点位按 `"{设备名}_"` 前缀匹配。
fn device_point_ids(
    device: &DeviceConfig,
    cfg_points: &[PointConfig],
    expanded_points: &[PointConfig],
) -> Vec<String> {
    let prefix = format!("{}_", device.name);
    let mut ids: Vec<String> = Vec::new();
    for p in cfg_points.iter().chain(expanded_points.iter()) {
        let by_id = !device.id.is_empty() && p.device_id.as_deref() == Some(device.id.as_str());
        let by_prefix = !device.name.is_empty() && p.id.starts_with(&prefix);
        if by_id || by_prefix {
            ids.push(p.id.clone());
        }
    }
    ids
}

fn sum_points(ids: &[String], snapshot: &HashMap<String, ScopeStats>) -> ScopeStats {
    let mut acc = ScopeStats::default();
    for id in ids {
        if let Some(s) = snapshot.get(id) {
            accumulate(&mut acc, s);
        }
    }
    acc
}

/// 全部参与采集的点位（显式点位 + 模板展开点位），按 id 去重。
fn all_point_ids(cfg: &GatewayConfig, expanded: &[PointConfig]) -> Vec<String> {
    let mut ids: Vec<String> = Vec::with_capacity(cfg.points.len() + expanded.len());
    for p in cfg.points.iter().chain(expanded.iter()) {
        if !ids.contains(&p.id) {
            ids.push(p.id.clone());
        }
    }
    ids
}

/// 真实错误分类：串口 IO / 协议解码 / 其余采集失败（= 总失败 − 已归类）。
fn build_error_dist(metrics: &Metrics) -> Vec<ErrorDistItem> {
    use std::sync::atomic::Ordering;

    let serial = metrics.collect.serial_errors.load(Ordering::Relaxed);
    let decode = metrics
        .collect
        .protocol_decode_errors
        .load(Ordering::Relaxed);
    let other = metrics
        .collect
        .collect_failures
        .load(Ordering::Relaxed)
        .saturating_sub(serial.saturating_add(decode));

    let categories: [(&str, u64); 3] = [
        ("串口 IO 错误", serial),
        ("协议解码错误", decode),
        ("其他采集失败", other),
    ];
    let total: u64 = categories.iter().map(|(_, c)| *c).sum();
    if total == 0 {
        return Vec::new();
    }
    categories
        .iter()
        .filter(|(_, count)| *count > 0)
        .map(|(name, count)| ErrorDistItem {
            error_type: (*name).to_string(),
            count: clamp_i32(*count),
            percentage: ((*count as f64 / total as f64) * 1000.0).round() / 10.0,
        })
        .collect()
}

fn generate_report(
    period: &str,
    days: i64,
    cfg: &GatewayConfig,
    expanded: &[PointConfig],
    metrics: &Metrics,
) -> StatsReport {
    let now = now_ms();
    let start = now - days * 86_400_000;
    let point_snapshot = metrics.scoped.point_snapshot(start);
    let bus_snapshot = metrics.scoped.bus_snapshot(start);

    // ===== 设备统计（按设备汇总其点位的真实采集结果）=====
    let devices = cfg.devices.clone().unwrap_or_default();
    let mut bus_device_counts: HashMap<String, i32> = HashMap::new();
    let device_stats: Vec<DeviceStat> = devices
        .iter()
        .map(|d| {
            let ids = device_point_ids(d, &cfg.points, expanded);
            // 该设备占用的总线（去重）→ 每个总线的设备数 +1
            let mut buses: Vec<String> = ids
                .iter()
                .filter_map(|id| point_bus(id, &cfg.points, expanded))
                .collect();
            buses.sort_unstable();
            buses.dedup();
            for bus in buses {
                *bus_device_counts.entry(bus).or_insert(0) += 1;
            }
            let s = sum_points(&ids, &point_snapshot);
            DeviceStat {
                device_id: if d.id.is_empty() {
                    d.name.clone()
                } else {
                    d.id.clone()
                },
                device_name: d.name.clone(),
                protocol: d.protocol.clone(),
                total_reads: clamp_i32(s.total),
                success: clamp_i32(s.success),
                failed: clamp_i32(s.failed),
                success_rate: s.success_rate(),
                avg_latency_ms: s.avg_latency_ms().map(|v| v.round() as i32),
                data_points: clamp_i32(s.success),
            }
        })
        .collect();

    // ===== 总线统计（key 为 BusConfig.port_name，与采集循环记录一致）=====
    let bus_stats: Vec<BusStat> = cfg
        .buses
        .iter()
        .map(|b| {
            let display = b
                .tcp_addr
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or(b.port_name.as_str())
                .to_string();
            let s = bus_snapshot.get(&b.port_name).copied().unwrap_or_default();
            let device_count = bus_device_counts.get(&b.port_name).copied().unwrap_or(0);
            BusStat {
                bus: display,
                protocol: b.protocol.clone(),
                total_reads: clamp_i32(s.total),
                success: clamp_i32(s.success),
                failed: clamp_i32(s.failed),
                success_rate: s.success_rate(),
                avg_latency_ms: s.avg_latency_ms().map(|v| v.round() as i32),
                device_count,
                online_devices: clamp_i32(u64::from(s.online_devices)),
                timeout_rate: s.timeout_rate(),
                last_error_code: s.last_error_code.map(|c| c.code()),
                reconnect_count: clamp_i32(s.reconnects),
            }
        })
        .collect();

    // ===== 汇总（覆盖全部参与采集的点位）=====
    let all_ids = all_point_ids(cfg, expanded);
    let overall = sum_points(&all_ids, &point_snapshot);

    StatsReport {
        period: period.to_string(),
        start_at_ms: start,
        end_at_ms: now,
        device_stats,
        bus_stats,
        error_dist: build_error_dist(metrics),
        summary: StatsSummary {
            total_reads: clamp_i32(overall.total),
            success_rate: overall.success_rate(),
            avg_latency_ms: overall.avg_latency_ms().map(|v| v.round() as i32),
            total_data_points: clamp_i32(overall.success),
        },
    }
}

fn point_bus(
    point_id: &str,
    cfg_points: &[PointConfig],
    expanded: &[PointConfig],
) -> Option<String> {
    cfg_points
        .iter()
        .chain(expanded.iter())
        .find(|p| p.id == point_id)
        .map(|p| p.bus.clone())
}

pub(crate) async fn get_stats_report(
    State(state): State<Arc<AppState>>,
    Query(q): Query<StatsQuery>,
) -> Result<Json<ApiResponse<StatsReport>>, ApiError> {
    let cfg = state.config.load_full();
    let period = match q.period.as_str() {
        "week" => "week",
        "month" => "month",
        _ => "day",
    };
    let days = days_for(period);

    // 模板模式：devices 在启动/热重载时展开为点位，state 里的 cfg.points 不含它们。
    // 展开失败不阻断报表：设备行会退化为 cfg.points 中的显式点位统计。
    let expanded = cfg
        .devices
        .as_ref()
        .filter(|d| !d.is_empty())
        .and_then(|d| state.config.templates.expand_devices(d).ok())
        .unwrap_or_default();

    Ok(Json(ApiResponse::ok(generate_report(
        period,
        days,
        &cfg,
        &expanded,
        &state.platform.metrics,
    ))))
}
