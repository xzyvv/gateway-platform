//! 点位数据质量 handler
//!
//! 路由：
//! - GET /api/devices/point-quality       -> list_point_quality
//! - GET /api/devices/point-quality/stats  -> point_quality_stats
//! - GET /api/devices/backoff             -> list_point_backoff

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::State;
use axum::Json;

use gateway_core::config::{DeviceConfig, PointConfig};

use crate::web::api::device_ops::{acc_quality, quality_code, QualityBreakdown};
use crate::web::types::{ApiError, ApiResponse, AppState};

use super::super::model::{PointBackoff, PointQuality, QualityStats};

fn map_quality(q: &str, failures: u32) -> String {
    // 质量码判定唯一口径在 health.rs，本页只做展示映射
    quality_code(q, failures).to_string()
}

/// 点位 / 设备 id 索引：替代原先循环内的 `iter().find(..)` 线性查找（O(N²)）
///
/// 快照行数 = 点位数（可达数千），每次请求都线性扫描配置会明显拖慢接口。
fn build_index<'a>(
    points: &'a [PointConfig],
    devices: Option<&'a [DeviceConfig]>,
) -> (
    HashMap<&'a str, &'a PointConfig>,
    HashMap<&'a str, &'a DeviceConfig>,
) {
    let point_by_id = points
        .iter()
        .map(|p| (p.id.as_str(), p))
        .collect::<HashMap<_, _>>();
    let device_by_id = devices
        .unwrap_or(&[])
        .iter()
        .map(|d| (d.id.as_str(), d))
        .collect::<HashMap<_, _>>();
    (point_by_id, device_by_id)
}

/// GET /api/devices/point-quality — 点位质量列表
pub(crate) async fn list_point_quality(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<PointQuality>>>, ApiError> {
    let cfg = state.config.load_full();
    let statuses = state.south.scheduler.device_status();
    let now = chrono::Utc::now().timestamp_millis();
    let (point_by_id, device_by_id) = build_index(&cfg.points, cfg.devices.as_deref());

    let mut result: Vec<PointQuality> = Vec::with_capacity(statuses.len());
    for s in &statuses {
        let pid = s.point_id.as_str();
        let cf = s.consecutive_failures;
        let quality_raw = s.quality.as_str();
        let last_good = s.last_good_value;
        // 最后成功采集时刻用 scheduler 快照的真实 last_good_ts（0 = 从未成功，
        // 回退 now 避免前端把时间渲染成 1970）；此前恒为 now 属桩行为。
        let last_good_ts = if s.last_good_ts > 0 {
            s.last_good_ts
        } else {
            now
        };

        let point = point_by_id.get(pid).copied();
        let point_name = point
            .and_then(|p| p.name.clone())
            .unwrap_or_else(|| pid.to_string());
        // device_id 优先取 scheduler 快照（真值源）：模板化配置下点位由设备模板
        // 展开生成，不在 cfg.points 里，用配置反查会得到空串 → 前端按设备聚合
        // 「最后成功采集」时全部落空。cfg.points 仅作无快照时的兜底。
        let device_id = if !s.device_id.is_empty() {
            s.device_id.clone()
        } else {
            point.and_then(|p| p.device_id.clone()).unwrap_or_default()
        };
        let device_name = device_by_id
            .get(device_id.as_str())
            .map(|d| d.name.clone())
            .unwrap_or_else(|| device_id.clone());

        result.push(PointQuality {
            point_id: pid.to_string(),
            point_name,
            device_id,
            device_name,
            value: last_good,
            quality: map_quality(quality_raw, cf),
            consecutive_failures: cf,
            last_update_at: last_good_ts,
        });
    }

    Ok(Json(ApiResponse::ok(result)))
}

/// GET /api/devices/backoff — 采集退避状态列表（§7.3 退避可视化）
///
/// 暴露 Scheduler device_states 的退避字段：连续失败次数、当前/基础采集间隔、
/// 上次成功值与时间戳、质量码。供前端"采集退避"页轮询展示。
pub(crate) async fn list_point_backoff(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<PointBackoff>>>, ApiError> {
    let cfg = state.config.load_full();
    let statuses = state.south.scheduler.device_status();
    let (point_by_id, device_by_id) = build_index(&cfg.points, cfg.devices.as_deref());

    let mut result: Vec<PointBackoff> = Vec::with_capacity(statuses.len());
    for s in &statuses {
        let pid = s.point_id.as_str();
        let cf = s.consecutive_failures;
        let current_ms = s.current_interval_ms;
        let base_ms = s.base_interval_ms;
        let stale_count = s.stale_count;
        let last_good = s.last_good_value;
        let last_good_ts = s.last_good_ts;
        let quality_raw = s.quality.as_str();

        let point = point_by_id.get(pid).copied();
        let point_name = point
            .and_then(|p| p.name.clone())
            .unwrap_or_else(|| pid.to_string());
        // 同 list_point_quality：device_id / bus 优先用 scheduler 快照，
        // 模板展开点位不在 cfg.points 中，配置反查只能拿到空值。
        let device_id = if !s.device_id.is_empty() {
            s.device_id.clone()
        } else {
            point.and_then(|p| p.device_id.clone()).unwrap_or_default()
        };
        let device_name = device_by_id
            .get(device_id.as_str())
            .map(|d| d.name.clone())
            .unwrap_or_else(|| device_id.clone());
        let bus = if !s.bus.is_empty() {
            s.bus.clone()
        } else {
            point.map(|p| p.bus.clone()).unwrap_or_default()
        };

        result.push(PointBackoff {
            point_id: pid.to_string(),
            point_name,
            device_id,
            device_name,
            bus,
            base_interval_ms: base_ms,
            current_interval_ms: current_ms,
            backoff_multiplier: current_ms.checked_div(base_ms).unwrap_or(1) as u32,
            consecutive_failures: cf,
            stale_count,
            last_good_value: last_good,
            last_good_ts,
            quality: map_quality(quality_raw, cf),
            backing_off: cf >= 3,
        });
    }

    Ok(Json(ApiResponse::ok(result)))
}

/// GET /api/devices/point-quality/stats — 质量统计
///
/// R1 合并：计数逻辑与健康评分共用 [`acc_quality`]（同一质量码口径），
/// 不再在本文件里自维护一套 `match` 分支。
pub(crate) async fn point_quality_stats(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<QualityStats>>, ApiError> {
    let statuses = state.south.scheduler.device_status();
    let mut bd = QualityBreakdown::default();
    for s in &statuses {
        acc_quality(&mut bd, Some(s));
    }
    Ok(Json(ApiResponse::ok(QualityStats {
        good: bd.good.max(0) as u32,
        bad: bd.bad.max(0) as u32,
        stale: bd.uncertain.max(0) as u32,
        unknown: bd.unknown.max(0) as u32,
        total: bd.total.max(0) as u32,
    })))
}
