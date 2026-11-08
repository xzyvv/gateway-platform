//! 设备健康评分 handler
//!
//! 数据全部来自真实运行时：
//! - 采集成功率 / 平均延迟 / 错误次数：最近 24 小时内该设备点位的累计采集结果
//!   （[`ScopedCollectMetrics`] 日桶，由采集循环逐次写入）；
//! - 离线次数：调度器点位状态里的 `offline_episodes`（「正常 → 首次失败」的累计次数，
//!   成功不清零）；
//! - 最后在线：该设备所有点位 `last_good_ts` 的最大值。
//!
//! 此前实现用 `hash(设备ID + 时间片)` 生成成功率、延迟、错误数、离线数，且把「当前时间」
//! 当作最后在线时间，页面数字与设备实际运行状态无关，已全部删除。
//!
//! 24 小时内没有任何采集时，`score` / `success_rate` / `avg_latency_ms` / `last_seen_at`
//! 均为 `null`（前端渲染「—」），等级为 `unknown`，不用 0 分或 100 分冒充。

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use serde::Serialize;
use ts_rs::TS;

use gateway_core::config::{DeviceConfig, PointConfig};
use gateway_core::engine::metrics::ScopeStats;

use crate::web::api::point_owner::PointOwnerIndex;
use crate::web::types::{ApiError, ApiResponse, AppState, DeviceStatus};

/// 健康评分窗口：最近 24 小时
const HEALTH_WINDOW_MS: i64 = 86_400_000;

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DeviceHealth {
    pub device_id: String,
    pub device_name: String,
    pub protocol: String,
    /// 所属总线名
    pub bus: String,
    /// 健康分 0-100；窗口内无采集为 null
    pub score: Option<i32>,
    /// excellent / good / warning / critical / unknown
    pub level: String,
    /// 成功率（%）；窗口内无采集为 null
    pub success_rate: Option<f64>,
    /// 平均延迟（ms）；窗口内无采集为 null
    pub avg_latency_ms: Option<i32>,
    /// 窗口内采集失败次数
    pub error_count: i32,
    /// 累计离线次数（正常 → 首次失败的次数）
    pub offline_count: i32,
    /// 好质量点位占比（%）；设备无点位时为 null
    pub good_point_ratio: Option<f64>,
    /// 点位连续失败次数之和（当前快照）
    pub consecutive_failures: i32,
    /// 点位质量码分布（point-quality 统计的唯一口径）
    pub quality_breakdown: QualityBreakdown,
    /// 最后成功采集时间（毫秒时间戳）；从未成功为 null
    #[ts(type = "number")]
    pub last_seen_at: Option<i64>,
    pub details: Vec<HealthDetail>,
}

/// 点位质量码分布（四桶口径与 `/api/devices/point-quality/stats` 一致）
///
/// - `unknown` 桶包含「数据过期但连续失败 < 3 次」的点位（质量码 `old`），
///   与 [`QualityStats`] 的 `unknown` 含义保持不变，避免前端解释口径变化。
#[derive(Debug, Clone, Copy, Default, Serialize, TS)]
#[ts(export)]
pub struct QualityBreakdown {
    pub good: i32,
    pub uncertain: i32,
    pub bad: i32,
    pub unknown: i32,
    pub total: i32,
}

/// 点位质量码（唯一口径，health / point-quality 两处共用）
///
/// 返回 `good` / `uncertain` / `bad` / `old`：
/// - scheduler 快照 quality 为 `good` → good；
/// - `stale` → uncertain（数据未更新但设备可通）；
/// - 其余按连续失败次数分：≥3 → bad，否则 old（未采集到但仍可重试）。
pub(crate) fn quality_code(raw: &str, consecutive_failures: u32) -> &'static str {
    match raw {
        "good" => "good",
        "stale" => "uncertain",
        _ if consecutive_failures >= 3 => "bad",
        _ => "old",
    }
}

/// 把一个点位（可选快照）计入质量分布
pub(crate) fn acc_quality(bd: &mut QualityBreakdown, state: Option<&DeviceStatus>) {
    bd.total += 1;
    let cf = state.map(|v| v.consecutive_failures).unwrap_or(0);
    let raw = state.map(|v| v.quality.as_str()).unwrap_or("bad");
    match quality_code(raw, cf) {
        "good" => bd.good += 1,
        "uncertain" => bd.uncertain += 1,
        "bad" => bd.bad += 1,
        _ => bd.unknown += 1,
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct HealthDetail {
    pub label: String,
    pub value: String,
    pub score: i32,
}

fn level_from_score(s: i32) -> &'static str {
    if s >= 90 {
        "excellent"
    } else if s >= 70 {
        "good"
    } else if s >= 50 {
        "warning"
    } else {
        "critical"
    }
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn clamp_i32(v: u64) -> i32 {
    v.min(i32::MAX as u64) as i32
}

/// 评分算法（输入全部是真实值）：成功率权重最高，延迟、错误数、掉线频次依次扣分。
fn score_from(success_rate: f64, latency_ms: i32, errors: i32, offline: i32) -> i32 {
    let mut score = 100.0_f64;
    score -= (100.0 - success_rate) * 0.6;
    score -= f64::from(latency_ms.clamp(0, 300) / 10);
    score -= f64::from(errors.clamp(0, 10)) * 2.0;
    score -= f64::from(offline.clamp(0, 4)) * 5.0;
    score.round().clamp(0.0, 100.0) as i32
}

/// 设备点位的累计统计（合并窗口内所有点位）
fn sum_points(ids: &[String], snapshot: &HashMap<String, ScopeStats>) -> ScopeStats {
    let mut acc = ScopeStats::default();
    for id in ids {
        if let Some(s) = snapshot.get(id) {
            acc.total += s.total;
            acc.success += s.success;
            acc.failed += s.failed;
            acc.duration_sum_us += s.duration_sum_us;
            acc.last_success_ms = acc.last_success_ms.max(s.last_success_ms);
            acc.last_failure_ms = acc.last_failure_ms.max(s.last_failure_ms);
        }
    }
    acc
}

fn expand_points(state: &AppState) -> Vec<PointConfig> {
    let cfg = state.config.load_full();
    cfg.devices
        .as_ref()
        .filter(|d| !d.is_empty())
        .and_then(|d| state.config.templates.expand_devices(d).ok())
        .unwrap_or_default()
}

fn build_health(
    device: &DeviceConfig,
    point_ids: &[String],
    point_snapshot: &HashMap<String, ScopeStats>,
    point_states: &HashMap<String, DeviceStatus>,
) -> DeviceHealth {
    let stats = sum_points(point_ids, point_snapshot);

    // 离线次数 / 最后在线 / 质量分布：来自调度器真实点位状态
    let mut offline: u32 = 0;
    let mut last_seen = stats.last_success_ms;
    let mut cf_sum: i32 = 0;
    let mut breakdown = QualityBreakdown::default();
    for id in point_ids {
        let st = point_states.get(id);
        if let Some(v) = st {
            offline = offline.saturating_add(v.offline_episodes);
            last_seen = last_seen.max(v.last_good_ts);
            cf_sum = cf_sum.saturating_add(clamp_i32(v.consecutive_failures as u64));
        }
        acc_quality(&mut breakdown, st);
    }
    let good_point_ratio = if breakdown.total > 0 {
        Some((f64::from(breakdown.good) / f64::from(breakdown.total) * 1000.0).round() / 10.0)
    } else {
        None
    };

    let success_rate = stats.success_rate();
    let avg_latency = stats.avg_latency_ms().map(|v| v.round() as i32);
    let error_count = clamp_i32(stats.failed);
    let offline_count = offline.min(i32::MAX as u32) as i32;

    let score = match (success_rate, avg_latency) {
        (Some(rate), Some(latency)) => Some(score_from(rate, latency, error_count, offline_count)),
        _ => None,
    };
    let level = score.map(level_from_score).unwrap_or("unknown").to_string();

    let rate_text = success_rate.map_or_else(|| "—".to_string(), |v| format!("{v:.1}%"));
    let lat_text = avg_latency.map_or_else(|| "—".to_string(), |v| format!("{v} ms"));
    let details = vec![
        HealthDetail {
            label: "采集成功率（24h）".into(),
            value: rate_text,
            score: success_rate.map_or(0, |v| (v * 0.6).round() as i32),
        },
        HealthDetail {
            label: "平均延迟（24h）".into(),
            value: lat_text,
            score: avg_latency.map_or(0, |v| (30 - v.clamp(0, 300) / 10).max(0)),
        },
        HealthDetail {
            label: "采集失败（24h）".into(),
            value: format!("{} 次", error_count),
            score: (20 - error_count.clamp(0, 10) * 2).max(0),
        },
        HealthDetail {
            label: "好质量点位占比（当前）".into(),
            value: good_point_ratio.map_or_else(|| "—".to_string(), |v| format!("{v:.1}%")),
            score: good_point_ratio.map_or(0, |v| (v * 0.2).round() as i32),
        },
        HealthDetail {
            label: "离线次数（累计）".into(),
            value: format!("{} 次", offline_count),
            score: (20 - offline_count.clamp(0, 4) * 5).max(0),
        },
    ];

    DeviceHealth {
        device_id: if device.id.is_empty() {
            device.name.clone()
        } else {
            device.id.clone()
        },
        device_name: device.name.clone(),
        protocol: device.protocol.clone(),
        bus: device.bus.clone(),
        score,
        level,
        success_rate,
        avg_latency_ms: avg_latency,
        error_count,
        offline_count,
        good_point_ratio,
        consecutive_failures: cf_sum,
        quality_breakdown: breakdown,
        last_seen_at: (last_seen > 0).then_some(last_seen),
        details,
    }
}

/// 构造全部设备的健康数据（列表/详情/汇总共用，避免多处重复计算）
///
/// 这是设备评分的**唯一口径**：`/api/devices/point-quality/stats` 基于
/// 本函数结果或其逐点同源实现派生；排行视图已合并进前端健康评分页，不再单独出端点。
pub(crate) fn build_all(state: &AppState, now: i64) -> Vec<DeviceHealth> {
    let cfg = state.config.load_full();
    let devices: &[DeviceConfig] = cfg.devices.as_deref().unwrap_or(&[]);
    if devices.is_empty() {
        return Vec::new();
    }
    let expanded = expand_points(state);
    let point_snapshot = state
        .platform
        .metrics
        .scoped
        .point_snapshot(now - HEALTH_WINDOW_MS);
    let point_states: HashMap<String, DeviceStatus> = state
        .south
        .scheduler
        .device_status()
        .into_iter()
        .map(|v| (v.point_id.clone(), v))
        .collect();
    // 点位归属索引：替代「每台设备线性扫描全部点位」的 O(D×P) 查找
    let owner_index = PointOwnerIndex::from_config_points(&cfg.points, &expanded, devices);

    devices
        .iter()
        .map(|d| {
            let ids = owner_index.point_ids_of(d);
            build_health(d, &ids, &point_snapshot, &point_states)
        })
        .collect()
}

pub(crate) async fn list_health(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<DeviceHealth>>>, ApiError> {
    Ok(Json(ApiResponse::ok(build_all(&state, now_ms()))))
}
