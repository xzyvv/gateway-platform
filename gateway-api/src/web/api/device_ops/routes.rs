//! 设备运维路由组
//!
//! 路由清单（18 条路由）：
//! - GET    /api/device-groups                          -> list_groups
//! - POST   /api/device-groups                          -> create_group
//! - PUT    /api/device-groups/{id}                     -> update_group
//! - DELETE /api/device-groups/{id}                     -> delete_group
//! - POST   /api/device-groups/{id}/devices             -> add_device_to_group
//! - DELETE /api/device-groups/{id}/devices/{did}       -> remove_device_from_group
//! - GET    /api/devices/assets                         -> list_assets
//! - GET    /api/devices/{id}/asset                     -> get_asset
//! - PUT    /api/devices/{id}/asset                     -> update_asset (upsert)
//! - GET    /api/devices/health                         -> list_health
//! - GET    /api/devices/shadows                        -> list_shadows
//! - GET    /api/devices/control/writable               -> list_writable_points
//! - GET    /api/devices/control/commands               -> list_commands
//! - POST   /api/devices/control/send                   -> send_command
//! - GET    /api/devices/topology                       -> device_topology
//! - GET    /api/devices/point-quality                   -> list_point_quality
//! - GET    /api/devices/point-quality/stats             -> point_quality_stats
//! - GET    /api/devices/backoff                        -> list_point_backoff
//!
//! 第 1 期冗余收口（删除的重复端点）：
//! - `GET /api/devices/health/summary`：与 `/api/devices/health` 同源派生，
//!   前端改为从健康列表派生汇总；
//! - `GET /api/devices/{id}/health`：与列表同源（同一 `build_all`），
//!   前端改为在列表内查找；
//! - `GET /api/devices/{id}/shadow`：与 `/api/devices/shadows` 同源，
//!   前端改为在列表内查找；
//! - `GET /api/devices/topology/stats`：是 `/api/devices/topology` 的聚合派生，
//!   前端改为从拓扑节点派生。
//!
//! 鉴权策略：
//! - GET 类：allow_anon_read=false 时要求 token
//! - POST/PUT/DELETE 类：始终要求 token
//!
//! 说明：本路由组聚合 `device_ops`（groups/assets/health/shadows）与
//! `devices`（control/topology/point-quality）两个模块的 handler，
//! 形成"设备运维"统一入口。质量排行视图已并入前端健康评分页，后端不再单独出端点。

use std::sync::Arc;

use axum::{
    middleware::from_fn_with_state,
    routing::{delete, get, post, put},
    Router,
};

use crate::web::types::{require_auth, require_auth_if_anon_disabled, AppState};

// 本模块（device_ops）handler：groups / assets / health / shadows
use super::{
    add_device_to_group, create_group, delete_group, get_asset, list_assets, list_groups,
    list_health, list_shadows, remove_device_from_group, update_asset, update_group,
};

// 跨模块（devices）handler：control / topology / point-quality
use crate::web::api::devices::{
    device_topology, list_commands, list_point_backoff, list_point_quality, list_writable_points,
    point_quality_stats, send_command,
};

/// 构建设备运维路由组
pub fn device_ops_routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    let read = Router::new()
        .route("/api/device-groups", get(list_groups))
        .route("/api/devices/assets", get(list_assets))
        .route("/api/devices/:id/asset", get(get_asset))
        .route("/api/devices/health", get(list_health))
        .route("/api/devices/shadows", get(list_shadows))
        .route("/api/devices/control/writable", get(list_writable_points))
        .route("/api/devices/control/commands", get(list_commands))
        .route("/api/devices/topology", get(device_topology))
        .route("/api/devices/point-quality", get(list_point_quality))
        .route("/api/devices/point-quality/stats", get(point_quality_stats))
        .route("/api/devices/backoff", get(list_point_backoff))
        .route_layer(from_fn_with_state(
            state.clone(),
            require_auth_if_anon_disabled,
        ));

    let write = Router::new()
        .route("/api/device-groups", post(create_group))
        .route("/api/device-groups/:id", put(update_group))
        .route("/api/device-groups/:id", delete(delete_group))
        .route("/api/device-groups/:id/devices", post(add_device_to_group))
        .route(
            "/api/device-groups/:id/devices/:did",
            delete(remove_device_from_group),
        )
        .route("/api/devices/:id/asset", put(update_asset))
        .route("/api/devices/control/send", post(send_command))
        .route_layer(from_fn_with_state(state.clone(), require_auth));

    Router::new().merge(read).merge(write)
}
