//! 设备资产 CRUD handler
//!
//! 持久化到 GatewayConfig.device_assets
//! 一设备一资产：id = device_id，PUT 即 upsert

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};
use serde::Deserialize;

use crate::web::types::{ApiError, ApiResponse, AppState};
use gateway_core::config::DeviceAsset;

#[derive(Deserialize)]
pub(crate) struct UpdateAssetRequest {
    #[serde(default)]
    pub asset_no: String,
    #[serde(default)]
    pub vendor: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub purchased_at: String,
    #[serde(default)]
    pub calibrated_at: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

pub(crate) async fn list_assets(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<DeviceAsset>>>, ApiError> {
    let cfg = state.config.load_full();
    let assets = cfg.device_assets.clone().unwrap_or_default();
    Ok(Json(ApiResponse::ok(assets)))
}

pub(crate) async fn get_asset(
    State(state): State<Arc<AppState>>,
    Path(device_id): Path<String>,
) -> Result<Json<ApiResponse<DeviceAsset>>, ApiError> {
    let cfg = state.config.load_full();
    let asset = cfg
        .device_assets
        .as_ref()
        .and_then(|a| a.iter().find(|x| x.device_id == device_id).cloned())
        .ok_or_else(|| ApiError::not_found("asset not found"))?;
    Ok(Json(ApiResponse::ok(asset)))
}

/// PUT /api/devices/{id}/asset — upsert（不存在则创建）
pub(crate) async fn update_asset(
    State(state): State<Arc<AppState>>,
    Path(device_id): Path<String>,
    Json(payload): Json<UpdateAssetRequest>,
) -> Result<Json<ApiResponse<DeviceAsset>>, ApiError> {
    let _guard = state.config.config_reload_lock.lock().await;
    let old = state.config.load_full().as_ref().clone();
    let mut new_cfg = old.clone();
    let assets = new_cfg.device_assets.get_or_insert_with(Vec::new);
    let asset = DeviceAsset {
        device_id: device_id.clone(),
        asset_no: payload.asset_no,
        vendor: payload.vendor,
        model: payload.model,
        location: payload.location,
        owner: payload.owner,
        purchased_at: payload.purchased_at,
        calibrated_at: payload.calibrated_at,
        tags: payload.tags,
    };
    let pos = assets.iter().position(|x| x.device_id == device_id);
    match pos {
        Some(i) => assets[i] = asset.clone(),
        None => assets.push(asset.clone()),
    }
    let _ = crate::web::config_apply::reload_then_save(&state, &old, &new_cfg).await?;
    state.platform.audit.log(
        "asset.update",
        &device_id,
        &format!(
            "asset_no={}, vendor={}, model={}",
            asset.asset_no, asset.vendor, asset.model
        ),
        "api",
    );
    Ok(Json(ApiResponse::ok(asset)))
}
