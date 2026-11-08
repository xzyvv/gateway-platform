//! 虚拟点位 CRUD + 重算
//!
//! 持久化到 GatewayConfig.virtual_points
//!
//! `last_value` / `error` 来自 core 的虚拟点位引擎（真实求值结果）：
//! 公式在处理管道中按采集周期求值，派生量参与广播/入队/北向上报。
//! 「重算」端点触发一次同步求值，立即返回最新结果。
//!
//! 写操作一律走 `reload_then_save`（而非纯元数据路径）：虚拟点位规则集
//! 由 core 引擎的 `ArcSwap` 承载，配置落盘后必须热重载才能让新公式生效，
//! 否则会退化成「改了不生效、必须重启」的老问题。

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::web::types::{ApiError, ApiResponse, AppState};
use gateway_core::config::VirtualPoint;

#[derive(Deserialize)]
pub(crate) struct VirtualPointPayload {
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub device_id: Option<String>,
    pub formula: String,
    #[serde(default)]
    pub unit: String,
    #[serde(default = "default_decimals")]
    pub decimals: u8,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_decimals() -> u8 {
    2
}
fn default_enabled() -> bool {
    true
}

#[derive(Serialize, ts_rs::TS)]
#[ts(export)]
pub struct VirtualPointView {
    pub id: String,
    pub name: String,
    pub device_id: String,
    pub formula: String,
    pub unit: String,
    pub decimals: u8,
    pub enabled: bool,
    /// 最近一次计算值（启用且无错时给出）
    pub last_value: Option<f64>,
    /// 最近一次计算错误（无错为 null）
    pub error: Option<String>,
    #[ts(type = "number")]
    pub created_at: i64,
}

impl VirtualPointView {
    /// 构造视图：合并引擎侧的真实求值状态。
    ///
    /// 引擎状态优先于配置推导：`disabled` / 公式错误 / 引用缺失等都以引擎
    /// 的实际状态为准，避免界面显示「正常」而实际从未求值成功。
    fn build(v: &VirtualPoint, state: Option<&crate::web::traits::VirtualPointStateView>) -> Self {
        let (last_value, error) = match state {
            Some(s) => (s.value, s.error.clone()),
            None => (
                None,
                if v.enabled {
                    Some("尚未求值（等待采集数据）".into())
                } else {
                    Some("disabled".into())
                },
            ),
        };
        Self {
            id: v.id.clone(),
            name: v.name.clone(),
            device_id: v.device_id.clone().unwrap_or_default(),
            formula: v.formula.clone(),
            unit: v.unit.clone(),
            decimals: v.decimals,
            enabled: v.enabled,
            last_value,
            error,
            created_at: v.created_at,
        }
    }
}

/// 列出所有虚拟点位
pub(crate) async fn list_virtual_points(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<VirtualPointView>>>, ApiError> {
    let cfg = state.config.load_full();
    let vps = cfg.virtual_points.clone().unwrap_or_default();
    let states = state.north.gateway.virtual_point_states();
    let views: Vec<VirtualPointView> = vps
        .iter()
        .map(|v| VirtualPointView::build(v, states.get(&v.id)))
        .collect();
    Ok(Json(ApiResponse::ok(views)))
}

/// 创建虚拟点位
pub(crate) async fn create_virtual_point(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<VirtualPointPayload>,
) -> Result<Json<ApiResponse<VirtualPointView>>, ApiError> {
    let _guard = state.config.config_reload_lock.lock().await;
    let old = state.config.load_full();
    let mut new_cfg = old.as_ref().clone();
    let vps = new_cfg.virtual_points.get_or_insert_with(Vec::new);
    let id = payload
        .id
        .unwrap_or_else(|| format!("vp_{}", chrono::Utc::now().timestamp_millis()));
    if vps.iter().any(|x| x.id == id) {
        return Err(ApiError::bad_request(format!(
            "virtual point {} already exists",
            id
        )));
    }
    let now = chrono::Utc::now().timestamp_millis();
    let vp = VirtualPoint {
        id: id.clone(),
        name: payload.name,
        device_id: payload.device_id,
        formula: payload.formula,
        unit: payload.unit,
        decimals: payload.decimals,
        enabled: payload.enabled,
        created_at: now,
    };
    vps.push(vp.clone());
    // 热重载：规则集变更后立即生效（core 引擎 ArcSwap 原子替换）
    let _ = crate::web::config_apply::reload_then_save(&state, &old, &new_cfg).await?;
    state.platform.audit.log(
        "virtual_point.create",
        &vp.id,
        &format!("{} = {}", vp.name, vp.formula),
        "api",
    );
    let states = state.north.gateway.virtual_point_states();
    Ok(Json(ApiResponse::ok(VirtualPointView::build(
        &vp,
        states.get(&vp.id),
    ))))
}

/// 更新虚拟点位
pub(crate) async fn update_virtual_point(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<VirtualPointPayload>,
) -> Result<Json<ApiResponse<VirtualPointView>>, ApiError> {
    let _guard = state.config.config_reload_lock.lock().await;
    let old = state.config.load_full();
    let mut new_cfg = old.as_ref().clone();
    let vps = new_cfg.virtual_points.get_or_insert_with(Vec::new);
    let pos = vps
        .iter()
        .position(|x| x.id == id)
        .ok_or_else(|| ApiError::not_found(format!("virtual point {} not found", id)))?;
    vps[pos] = VirtualPoint {
        id: id.clone(),
        name: payload.name,
        device_id: payload.device_id,
        formula: payload.formula,
        unit: payload.unit,
        decimals: payload.decimals,
        enabled: payload.enabled,
        created_at: vps[pos].created_at,
    };
    let vp = vps[pos].clone();
    // 热重载：公式改动立即生效
    let _ = crate::web::config_apply::reload_then_save(&state, &old, &new_cfg).await?;
    state.platform.audit.log(
        "virtual_point.update",
        &vp.id,
        &format!("{} = {}", vp.name, vp.formula),
        "api",
    );
    let states = state.north.gateway.virtual_point_states();
    Ok(Json(ApiResponse::ok(VirtualPointView::build(
        &vp,
        states.get(&vp.id),
    ))))
}

/// 删除虚拟点位
pub(crate) async fn delete_virtual_point(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let _guard = state.config.config_reload_lock.lock().await;
    let old = state.config.load_full();
    let mut new_cfg = old.as_ref().clone();
    let vps = new_cfg.virtual_points.get_or_insert_with(Vec::new);
    let pos = vps
        .iter()
        .position(|x| x.id == id)
        .ok_or_else(|| ApiError::not_found(format!("virtual point {} not found", id)))?;
    vps.remove(pos);
    // 热重载：删除后引擎内规则同步移除
    let _ = crate::web::config_apply::reload_then_save(&state, &old, &new_cfg).await?;
    state
        .platform
        .audit
        .log("virtual_point.delete", &id, "删除虚拟点位", "api");
    Ok(Json(ApiResponse::ok(())))
}

/// 重算虚拟点位（触发一次**同步真实求值**）
///
/// 求值由 core 的虚拟点位引擎执行：以点位实时值缓存为输入，按虚拟点位
/// 的 `decimals` 舍入后返回。点不存在返回 404。
pub(crate) async fn recalc_virtual_point(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<VirtualPointView>>, ApiError> {
    let cfg = state.config.load_full();
    let vps = cfg.virtual_points.clone().unwrap_or_default();
    let vp = vps
        .iter()
        .find(|x| x.id == id)
        .ok_or_else(|| ApiError::not_found(format!("virtual point {} not found", id)))?;
    let st = state
        .north
        .gateway
        .recalc_virtual_point(&id)
        .ok_or_else(|| ApiError::not_found(format!("virtual point {} not found", id)))?;
    Ok(Json(ApiResponse::ok(VirtualPointView::build(
        vp,
        Some(&st),
    ))))
}
