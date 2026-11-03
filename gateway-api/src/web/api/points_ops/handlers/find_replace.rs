//! 批量查找替换
//!
//! 直接修改 PointConfig.{name, address, unit}，热重载

use std::sync::Arc;

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};

use crate::web::types::{ApiError, ApiResponse, AppState};

#[derive(Deserialize)]
pub(crate) struct SearchRequest {
    pub keyword: String,
    /// name / address / unit / all
    #[serde(default = "default_scope")]
    pub scope: String,
}
fn default_scope() -> String {
    "all".into()
}

#[derive(Serialize, ts_rs::TS)]
#[ts(export)]
pub struct SearchResult {
    pub id: String,
    /// name / address / unit
    pub field: String,
    pub device_name: String,
    pub point_name: String,
    pub old_value: String,
}

#[derive(Deserialize)]
pub(crate) struct ReplaceRequest {
    pub ids: Vec<String>,
    pub field: String, // name / address / unit
    pub from: String,
    pub to: String,
}

pub(crate) async fn find_replace_search(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SearchRequest>,
) -> Result<Json<ApiResponse<Vec<SearchResult>>>, ApiError> {
    if req.keyword.is_empty() {
        return Ok(Json(ApiResponse::ok(Vec::new())));
    }
    let cfg = state.config.load_full();
    let mut out: Vec<SearchResult> = Vec::new();
    for p in &cfg.points {
        let dev = p.device_id.clone().unwrap_or_else(|| p.bus.clone());
        let mut try_match = |field: &str, val: &str| {
            if val.contains(req.keyword.as_str()) {
                out.push(SearchResult {
                    id: p.id.clone(),
                    field: field.into(),
                    device_name: dev.clone(),
                    point_name: p.name.clone().unwrap_or_else(|| p.id.clone()),
                    old_value: val.into(),
                });
            }
        };
        let want_all = req.scope == "all";
        if want_all || req.scope == "name" {
            if let Some(n) = &p.name {
                try_match("name", n);
            }
        }
        if want_all || req.scope == "address" {
            try_match("address", &p.address);
        }
        if want_all || req.scope == "unit" {
            if let Some(u) = &p.unit {
                try_match("unit", u);
            }
        }
    }
    Ok(Json(ApiResponse::ok(out)))
}

pub(crate) async fn find_replace_apply(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ReplaceRequest>,
) -> Result<Json<ApiResponse<serde_json::Value>>, ApiError> {
    let _guard = state.config.config_reload_lock.lock().await;
    let old = state.config.load_full().as_ref().clone();
    let mut new_cfg = old.clone();
    let mut count = 0usize;
    for p in &mut new_cfg.points {
        if !req.ids.contains(&p.id) {
            continue;
        }
        match req.field.as_str() {
            "name" => {
                if let Some(n) = &mut p.name {
                    if n.contains(req.from.as_str()) {
                        *n = n.replace(&req.from, &req.to);
                        count += 1;
                    }
                }
            }
            "address" => {
                if p.address.contains(req.from.as_str()) {
                    p.address = p.address.replace(&req.from, &req.to);
                    count += 1;
                }
            }
            "unit" => {
                if let Some(u) = &mut p.unit {
                    if u.contains(req.from.as_str()) {
                        *u = u.replace(&req.from, &req.to);
                        count += 1;
                    }
                }
            }
            _ => {
                return Err(ApiError::bad_request(format!(
                    "unknown field: {}",
                    req.field
                )))
            }
        }
    }
    // 修复（批量替换假成功）：原实现 `let _ = reload_points(...)` 吞掉错误，
    // 即便保存/校验/license 配额任一步失败（磁盘满、只读 rootfs、配额超限），
    // 接口仍返回 `updated: N` 的成功响应——前端提示「已替换 N 处」，而磁盘与运行时
    // 配置其实一处都没改。与同目录 offset.rs / clone.rs 保持一致：错误上抛，
    // 由 `ApiError` 映射为真实失败状态码。
    reload_points(&state, &old, &new_cfg).await?;
    state.platform.audit.log(
        "point.replace",
        &req.field,
        &format!("{} → {}（{} 处）", req.from, req.to, count),
        "api",
    );
    Ok(Json(ApiResponse::ok(
        serde_json::json!({ "updated": count }),
    )))
}

/// 共享：保存 + 热重载（virtual_points / offset / clone 共用）
pub(crate) async fn reload_points(
    state: &Arc<AppState>,
    old: &gateway_core::config::GatewayConfig,
    new_cfg: &gateway_core::config::GatewayConfig,
) -> Result<(), ApiError> {
    // 统一走 config_apply（expand → reload → store → save）
    crate::web::config_apply::reload_then_save(state, old, new_cfg)
        .await
        .map(|_| ())
}
