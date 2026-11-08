//! 设备影子 handler
//!
//! 期望值来自 [GatewayConfig::devices]（device.interval）；实测值来自 scheduler
//! 点位状态快照（last_good_value / last_good_ts / quality）。历史 V1 用
//! `hash ^ 时间片` 伪随机编造——同一设备两次刷新值乱跳，比报错更有误导性。

use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use serde::Serialize;
use ts_rs::TS;

use gateway_core::config::DeviceConfig;

use crate::web::types::{ApiError, ApiResponse, AppState, DeviceStatus};

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DeviceShadow {
    pub device_id: String,
    pub device_name: String,
    pub protocol: String,
    pub expected_interval_ms: i64,
    pub last_value_hint: Option<String>,
    pub last_quality: String,
    pub last_update_at: i64,
    pub points: Vec<ShadowPoint>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ShadowPoint {
    pub point_id: String,
    pub quality: String,
    pub last_value: Option<String>,
}

/// 快照质量码 → 影子三值（前端 mapQuality 只认 good/uncertain，其余归 bad）
fn map_snapshot_quality(s: &DeviceStatus) -> &'static str {
    match s.quality.as_str() {
        "good" => "good",
        "stale" => "uncertain",
        _ => "bad",
    }
}

fn build_shadow(dev: &DeviceConfig, statuses: &[DeviceStatus]) -> DeviceShadow {
    let interval_ms = dev.interval.unwrap_or(5) as i64 * 1000;
    // 该设备的点位快照：优先 device_id 匹配；模板展开点位不在 cfg.points，
    // 按 "{设备名}_" 前缀兜底（与 topology / health 同口径）
    let prefix = format!("{}_", dev.name);
    let dev_points: Vec<&DeviceStatus> = statuses
        .iter()
        .filter(|s| {
            let by_id = !dev.id.is_empty() && s.device_id == dev.id;
            let by_prefix = !dev.name.is_empty() && s.point_id.starts_with(&prefix);
            by_id || by_prefix
        })
        .collect();

    let points: Vec<ShadowPoint> = dev_points
        .iter()
        .map(|s| ShadowPoint {
            point_id: s.point_id.clone(),
            quality: map_snapshot_quality(s).to_string(),
            last_value: s.last_good_value.map(|v| v.to_string()),
        })
        .collect();

    // 设备级聚合：最近一次成功（last_good_ts 最大）决定 value / update_at；
    // 质量取最差（bad > uncertain > good）。无点位快照（未采集）→ uncertain + 0，
    // 前端把 last_update_at=0 显示为「—」（诚实表达暂无数据，而非编造）。
    let mut latest_ts: i64 = 0;
    let mut latest_val: Option<String> = None;
    let mut worst = "good";
    for s in &dev_points {
        let ts = s.last_good_ts;
        if ts > latest_ts {
            latest_ts = ts;
            latest_val = s.last_good_value.map(|v| v.to_string());
        }
        worst = match (worst, map_snapshot_quality(s)) {
            ("bad", _) | (_, "bad") => "bad",
            ("uncertain", _) | (_, "uncertain") => "uncertain",
            _ => "good",
        };
    }

    DeviceShadow {
        device_id: dev.id.clone(),
        device_name: dev.name.clone(),
        protocol: dev.protocol.clone(),
        expected_interval_ms: interval_ms,
        last_value_hint: latest_val,
        last_quality: if dev_points.is_empty() {
            "uncertain".to_string()
        } else {
            worst.to_string()
        },
        last_update_at: latest_ts,
        points,
    }
}

pub(crate) async fn list_shadows(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<DeviceShadow>>>, ApiError> {
    let cfg = state.config.load_full();
    let statuses = state.south.scheduler.device_status();
    let list: Vec<DeviceShadow> = cfg
        .devices
        .as_ref()
        .map(|d| d.iter().map(|x| build_shadow(x, &statuses)).collect())
        .unwrap_or_default();
    Ok(Json(ApiResponse::ok(list)))
}
