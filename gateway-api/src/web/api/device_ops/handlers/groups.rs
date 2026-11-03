//! 设备分组 CRUD handler
//!
//! 持久化到 GatewayConfig.device_groups
//! 错误码：6001 名称为空/重复 / 6002 不存在，HTTP 400（见 `ApiResponse::into_response` 的区间映射）
//!
//! 注意：handler 直接返回 `ApiResponse<T>` 而不是 `Json<ApiResponse<T>>`。
//! 包一层 `Json` 会走 axum 默认的 200 状态码，`ApiResponse::into_response` 里的
//! 业务码 → HTTP 状态映射就不会生效（历史上 6001/6002 因此恒为 200）。

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};
use serde::Deserialize;

use crate::web::types::{ApiError, ApiResponse, AppState};
use gateway_core::config::DeviceGroup;

/// 列表
pub(crate) async fn list_groups(
    State(state): State<Arc<AppState>>,
) -> Result<ApiResponse<Vec<DeviceGroup>>, ApiError> {
    let cfg = state.config.load_full();
    let groups = cfg.device_groups.clone().unwrap_or_default();
    Ok(ApiResponse::ok(groups))
}

/// 创建
#[derive(Deserialize)]
pub(crate) struct CreateGroupRequest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub device_ids: Vec<String>,
}

pub(crate) async fn create_group(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateGroupRequest>,
) -> Result<ApiResponse<DeviceGroup>, ApiError> {
    if payload.name.trim().is_empty() {
        return Ok(ApiResponse::err(6001, "组名不能为空"));
    }
    let _guard = state.config.config_reload_lock.lock().await;
    let old = state.config.load_full().as_ref().clone();
    let mut new_cfg = old.clone();
    let groups = new_cfg.device_groups.get_or_insert_with(Vec::new);
    if groups.iter().any(|g| g.name == payload.name) {
        return Ok(ApiResponse::err(
            6001,
            format!("组名 '{}' 已存在", payload.name),
        ));
    }
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let new_group = DeviceGroup {
        id: payload.id,
        name: payload.name,
        description: payload.description,
        color: payload.color,
        device_ids: payload.device_ids,
        created_at: now_ms,
    };
    groups.push(new_group.clone());
    let _ = crate::web::config_apply::reload_then_save(&state, &old, &new_cfg).await?;
    state.platform.audit.log(
        "group.create",
        &new_group.id,
        &format!(
            "{}（{} 台设备）",
            new_group.name,
            new_group.device_ids.len()
        ),
        "api",
    );
    Ok(ApiResponse::ok(new_group))
}

/// 更新（PATCH 部分字段）
#[derive(Deserialize)]
pub(crate) struct UpdateGroupRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub device_ids: Option<Vec<String>>,
}

pub(crate) async fn update_group(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<UpdateGroupRequest>,
) -> Result<ApiResponse<()>, ApiError> {
    let _guard = state.config.config_reload_lock.lock().await;
    let old = state.config.load_full().as_ref().clone();
    let mut new_cfg = old.clone();
    let groups = match new_cfg.device_groups.as_mut() {
        Some(g) => g,
        None => return Ok(ApiResponse::err(6002, "group not found")),
    };
    let found = groups.iter_mut().find(|g| g.id == id);
    let g = match found {
        Some(g) => g,
        None => return Ok(ApiResponse::err(6002, "group not found")),
    };
    if let Some(name) = payload.name {
        g.name = name;
    }
    if let Some(desc) = payload.description {
        g.description = desc;
    }
    if let Some(color) = payload.color {
        g.color = color;
    }
    if let Some(devs) = payload.device_ids {
        g.device_ids = devs;
    }
    let _ = crate::web::config_apply::reload_then_save(&state, &old, &new_cfg).await?;
    state
        .platform
        .audit
        .log("group.update", &id, "更新分组字段", "api");
    Ok(ApiResponse::ok(()))
}

pub(crate) async fn delete_group(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<ApiResponse<()>, ApiError> {
    let _guard = state.config.config_reload_lock.lock().await;
    let old = state.config.load_full().as_ref().clone();
    let mut new_cfg = old.clone();
    let groups = match new_cfg.device_groups.as_mut() {
        Some(g) => g,
        None => return Ok(ApiResponse::err(6002, "group not found")),
    };
    let before = groups.len();
    groups.retain(|g| g.id != id);
    if groups.len() == before {
        return Ok(ApiResponse::err(6002, "group not found"));
    }
    let _ = crate::web::config_apply::reload_then_save(&state, &old, &new_cfg).await?;
    state
        .platform
        .audit
        .log("group.delete", &id, "删除设备分组", "api");
    Ok(ApiResponse::ok(()))
}

/// 单设备加入分组
///
/// `device_id` 走请求体而非第二段路径：路由注册为 `POST /api/device-groups/{id}/devices`，
/// 前端 `deviceOps.ts::addDeviceToGroup` 也按 `{ device_id }` 提交。
/// 此前 handler 用 `Path<(String, String)>` 解两段路径，与路由只有一段参数不匹配，
/// axum 解包直接 400，端点不可用。
#[derive(Deserialize)]
pub(crate) struct AddDeviceToGroupRequest {
    pub device_id: String,
}

pub(crate) async fn add_device_to_group(
    State(state): State<Arc<AppState>>,
    Path(group_id): Path<String>,
    Json(payload): Json<AddDeviceToGroupRequest>,
) -> Result<ApiResponse<()>, ApiError> {
    let device_id = payload.device_id;
    let _guard = state.config.config_reload_lock.lock().await;
    let old = state.config.load_full().as_ref().clone();
    let mut new_cfg = old.clone();
    let groups = match new_cfg.device_groups.as_mut() {
        Some(g) => g,
        None => return Ok(ApiResponse::err(6002, "group not found")),
    };
    let g = match groups.iter_mut().find(|g| g.id == group_id) {
        Some(g) => g,
        None => return Ok(ApiResponse::err(6002, "group not found")),
    };
    if !g.device_ids.contains(&device_id) {
        g.device_ids.push(device_id.clone());
    }
    let _ = crate::web::config_apply::reload_then_save(&state, &old, &new_cfg).await?;
    state
        .platform
        .audit
        .log("group.device.add", &group_id, &device_id, "api");
    Ok(ApiResponse::ok(()))
}

pub(crate) async fn remove_device_from_group(
    State(state): State<Arc<AppState>>,
    Path((group_id, device_id)): Path<(String, String)>,
) -> Result<ApiResponse<()>, ApiError> {
    let _guard = state.config.config_reload_lock.lock().await;
    let old = state.config.load_full().as_ref().clone();
    let mut new_cfg = old.clone();
    let groups = match new_cfg.device_groups.as_mut() {
        Some(g) => g,
        None => return Ok(ApiResponse::err(6002, "group not found")),
    };
    let g = match groups.iter_mut().find(|g| g.id == group_id) {
        Some(g) => g,
        None => return Ok(ApiResponse::err(6002, "group not found")),
    };
    g.device_ids.retain(|d| d != &device_id);
    let _ = crate::web::config_apply::reload_then_save(&state, &old, &new_cfg).await?;
    state
        .platform
        .audit
        .log("group.device.remove", &group_id, &device_id, "api");
    Ok(ApiResponse::ok(()))
}
