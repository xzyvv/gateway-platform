//! 批量地址偏移
//!
//! 修改 PointConfig.address_num，热重载

use std::sync::Arc;

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};

use super::find_replace::reload_points;
use crate::web::types::{ApiError, ApiResponse, AppState};

#[derive(Serialize, ts_rs::TS)]
#[ts(export)]
pub struct OffsetPointView {
    pub id: String,
    pub name: String,
    pub old_addr: u16,
    pub new_addr: u16,
    pub data_type: String,
    pub selected: bool,
}

#[derive(Deserialize)]
pub(crate) struct ApplyOffsetRequest {
    pub ids: Vec<String>,
    /// 地址偏移量，可正可负
    pub offset: i64,
}

pub(crate) async fn offset_list(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<OffsetPointView>>>, ApiError> {
    let cfg = state.config.load_full();
    let views: Vec<OffsetPointView> = cfg
        .points
        .iter()
        .map(|p| OffsetPointView {
            id: p.id.clone(),
            name: p.name.clone().unwrap_or_else(|| p.id.clone()),
            old_addr: p.address_num.unwrap_or(0),
            new_addr: p.address_num.unwrap_or(0),
            data_type: p.data_type.clone().unwrap_or_else(|| "f32".into()),
            selected: false,
        })
        .collect();
    Ok(Json(ApiResponse::ok(views)))
}

pub(crate) async fn offset_apply(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ApplyOffsetRequest>,
) -> Result<Json<ApiResponse<serde_json::Value>>, ApiError> {
    let _guard = state.config.config_reload_lock.lock().await;
    let old = state.config.load_full();
    let mut new_cfg = old.as_ref().clone();
    let mut count = 0usize;
    for p in &mut new_cfg.points {
        if !req.ids.contains(&p.id) {
            continue;
        }
        let cur = p.address_num.unwrap_or(0) as i64;
        let next = (cur + req.offset).max(0).min(u16::MAX as i64) as u16;
        if p.address_num != Some(next) {
            p.address_num = Some(next);
            count += 1;
        }
    }
    reload_points(&state, &old, &new_cfg).await?;
    state.platform.audit.log(
        "point.offset",
        &format!("{} 个点位", req.ids.len()),
        &format!("address_num 偏移 {:+}，实际更新 {} 个", req.offset, count),
        "api",
    );
    Ok(Json(ApiResponse::ok(
        serde_json::json!({ "updated": count }),
    )))
}
