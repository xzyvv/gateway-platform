//! 批量点位克隆
//!
//! 从源设备复制点位到目标设备。
//!
//! D-02：原 `GET /api/points/clone/sources` 与 `/clone/targets` 逐行等价
//! （同一份设备列表、同一份 point_count 统计，仅响应类型名不同），
//! 已合并为单一 `GET /api/point-tools/clone/devices`；前端源/目标两个下拉共用同一名单。

use std::sync::Arc;

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};

use super::find_replace::reload_points;
use crate::web::types::{ApiError, ApiResponse, AppState};

/// 可参与克隆的设备（源与目标共用同一列表）。
#[derive(Serialize, ts_rs::TS)]
#[ts(export)]
pub struct CloneDevice {
    pub id: String,
    pub name: String,
    pub point_count: usize,
}

#[derive(Deserialize)]
pub(crate) struct CloneApplyRequest {
    pub source_device_id: String,
    pub target_device_ids: Vec<String>,
    /// 克隆后点位名后缀（如 "_copy"），允许为空
    #[serde(default)]
    pub name_suffix: String,
}

/// GET /api/point-tools/clone/devices — 克隆源/目标设备名单
pub(crate) async fn clone_devices(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<CloneDevice>>>, ApiError> {
    let cfg = state.config.load_full();
    let devices = cfg.devices.clone().unwrap_or_default();
    let views = devices
        .iter()
        .map(|d| CloneDevice {
            id: d.id.clone(),
            name: d.name.clone(),
            point_count: cfg
                .points
                .iter()
                .filter(|p| p.device_id.as_deref() == Some(d.id.as_str()))
                .count(),
        })
        .collect();
    Ok(Json(ApiResponse::ok(views)))
}

pub(crate) async fn clone_apply(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CloneApplyRequest>,
) -> Result<Json<ApiResponse<serde_json::Value>>, ApiError> {
    let _guard = state.config.config_reload_lock.lock().await;
    let old = state.config.load_full();
    let mut new_cfg = old.as_ref().clone();

    // 1. 取源设备所有点位（克隆定义）
    let src_points: Vec<gateway_core::config::PointConfig> = new_cfg
        .points
        .iter()
        .filter(|p| p.device_id.as_deref() == Some(req.source_device_id.as_str()))
        .cloned()
        .collect();
    if src_points.is_empty() {
        return Err(ApiError::not_found(format!(
            "no points on device {}",
            req.source_device_id
        )));
    }

    // 2. 校验目标设备存在
    let devices = new_cfg.devices.clone().unwrap_or_default();
    for tid in &req.target_device_ids {
        if !devices.iter().any(|d| &d.id == tid) {
            return Err(ApiError::not_found(format!(
                "target device {} not found",
                tid
            )));
        }
    }

    // 3. 对每个目标设备，克隆源点位（重命名 + 调整 id）
    let mut total = 0usize;
    let suffix = if req.name_suffix.is_empty() {
        String::new()
    } else {
        req.name_suffix
    };
    for tid in &req.target_device_ids {
        for src in &src_points {
            let mut new_p = src.clone();
            // id = 源 id + "_" + 目标设备 id（保证唯一）
            new_p.id = format!("{}_{}", src.id, tid);
            if let Some(n) = &mut new_p.name {
                *n = format!("{}{}", n, suffix);
            }
            new_p.device_id = Some(tid.clone());
            // 跳过已存在
            if new_cfg.points.iter().any(|p| p.id == new_p.id) {
                continue;
            }
            new_cfg.points.push(new_p);
            total += 1;
        }
    }

    reload_points(&state, &old, &new_cfg).await?;
    state.platform.audit.log(
        "point.clone",
        &req.source_device_id,
        &format!(
            "克隆 {} 个点位 → {}",
            total,
            req.target_device_ids.join(",")
        ),
        "api",
    );
    Ok(Json(ApiResponse::ok(serde_json::json!({
        "total": total,
        "targets": req.target_device_ids,
    }))))
}
