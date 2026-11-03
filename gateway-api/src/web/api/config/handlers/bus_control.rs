//! E1：总线采集暂停 / 恢复（调试工具独占总线前的让路动作）
//!
//! 背景：协议会话 / 报文回放等调试能力要求「总线未被采集引擎占用」，
//! 而系统原本没有任何暂停采集的入口——用户点调试只会拿到
//! 「总线正在被采集引擎使用，请先暂停该总线的采集后再试」这条**做不到的指引**
//! （死路）。本模块提供最小可用的暂停/恢复：不修改配置、不换 collector，
//! 只把该总线的点位从调度中临时摘除，恢复时按完整点位集重建。

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};

use super::*;

/// POST /api/buses/:name/pause
///
/// 返回值里的 `paused_buses` 是当前全部暂停中的总线，前端可直接渲染状态，
/// 无需再额外轮询一次列表接口。
pub(crate) async fn pause_bus_collect(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<ApiResponse<serde_json::Value>>, ApiError> {
    let cfg = state.config.load_full();
    if !cfg.buses.iter().any(|b| b.port_name == name) {
        return Err(ApiError::not_found(format!(
            "总线 {name} 不在当前配置中，无法暂停采集"
        )));
    }
    // 点位集合必须与采集调度同一口径（含设备模板展开出的点位）
    let points = crate::web::config_apply::expand_points(&state, &cfg);
    state
        .south
        .scheduler
        .clone()
        .pause_bus(name.clone(), points)
        .await
        .map_err(ApiError::bad_request)?;
    let paused = state.south.scheduler.paused_buses();
    state
        .platform
        .audit
        .log("bus.pause", &name, "暂停总线采集", "api");
    Ok(Json(ApiResponse::ok(serde_json::json!({
        "bus": name,
        "paused": true,
        "paused_buses": paused,
    }))))
}

/// POST /api/buses/:name/resume
pub(crate) async fn resume_bus_collect(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<ApiResponse<serde_json::Value>>, ApiError> {
    let cfg = state.config.load_full();
    if !cfg.buses.iter().any(|b| b.port_name == name) {
        return Err(ApiError::not_found(format!(
            "总线 {name} 不在当前配置中，无法恢复采集"
        )));
    }
    let points = crate::web::config_apply::expand_points(&state, &cfg);
    state
        .south
        .scheduler
        .clone()
        .resume_bus(name.clone(), points)
        .await
        .map_err(ApiError::bad_request)?;
    let paused = state.south.scheduler.paused_buses();
    state
        .platform
        .audit
        .log("bus.resume", &name, "恢复总线采集", "api");
    Ok(Json(ApiResponse::ok(serde_json::json!({
        "bus": name,
        "paused": false,
        "paused_buses": paused,
    }))))
}

/// GET /api/buses/paused
///
/// 调试页刷新后需要知道「当前哪些总线被我暂停了」，否则用户会以为采集坏了。
pub(crate) async fn list_paused_buses(
    State(state): State<Arc<AppState>>,
) -> Json<ApiResponse<serde_json::Value>> {
    let paused = state.south.scheduler.paused_buses();
    Json(ApiResponse::ok(serde_json::json!({
        "paused_buses": paused,
    })))
}
