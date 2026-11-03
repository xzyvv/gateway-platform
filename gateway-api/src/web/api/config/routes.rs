//! 设备/测点 CRUD + 配置导入 + 热重载路由组
//!
//! 路由清单（共 18 个 handler，对应 18 条路由）：
//! - GET    /api/devices                -> list_devices
//! - POST   /api/devices                -> create_device
//! - PUT    /api/devices/{id}           -> update_device
//! - DELETE /api/devices/{id}           -> delete_device
//! - GET    /api/devices/{id}/points    -> list_points
//! - POST   /api/devices/{id}/points    -> create_point
//! - PUT    /api/points/{id}            -> update_point
//! - DELETE /api/points/{id}            -> delete_point（软删除进回收站）
//! - GET    /api/points/trash           -> list_point_trash
//! - DELETE /api/points/trash           -> purge_all_point_trash
//! - DELETE /api/points/trash/{id}      -> purge_point
//! - POST   /api/points/{id}/restore    -> restore_point
//! - POST   /api/config/backup          -> backup_config
//! - PUT    /api/config/import           -> import_config
//! - POST   /api/config/reload           -> reload_config
//! - POST   /api/config/validate         -> validate_config_report
//!
//! F1: 按 配置入口(CRUD) vs 配置管理(import/reload) 拆路由 + 中间件

use std::sync::Arc;

use axum::{
    middleware::{from_fn, from_fn_with_state},
    routing::{delete, get, post, put},
    Router,
};

use super::{
    backup_config, create_device, create_point, delete_device, delete_point, export_project,
    import_config, import_project, list_devices, list_paused_buses, list_point_trash, list_points,
    pause_bus_collect, purge_all_point_trash, purge_point, reload_config, restore_point,
    resume_bus_collect, update_device, update_point, validate_config_report,
};
use crate::web::types::{require_auth, require_auth_if_anon_disabled, AppState};

/// F1: Config entry (CRUD) + config management (import/export/reload) routes
pub fn config_routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    // === Config entry: device/point CRUD (read-only) ===
    let crud_read = Router::new()
        .route("/api/devices", get(list_devices))
        .route("/api/devices/:id/points", get(list_points))
        // 静态段 /api/points/trash 必须能与 /api/points/:id 共存：
        // axum 0.7 的 matchit 静态段优先级高于参数段，此处另一 router 亦仅注册 /:id 的 PUT/DELETE
        .route("/api/points/trash", get(list_point_trash))
        // E1：当前被暂停采集的总线（调试页刷新后仍能看到状态）
        .route("/api/buses/paused", get(list_paused_buses))
        .route_layer(from_fn_with_state(
            state.clone(),
            require_auth_if_anon_disabled,
        ))
        .route_layer(from_fn(crate::web::middleware::tag_config_entry));

    // === Config entry: device/point CRUD (write) ===
    let crud_write = Router::new()
        .route("/api/devices", post(create_device))
        .route("/api/devices/:id", put(update_device).delete(delete_device))
        .route("/api/devices/:id/points", post(create_point))
        .route("/api/points/:id", put(update_point).delete(delete_point))
        // 回收站：清空 / 单条彻底删除 / 恢复（写操作，始终要求 token）
        .route("/api/points/trash", delete(purge_all_point_trash))
        .route("/api/points/trash/:id", delete(purge_point))
        .route("/api/points/:id/restore", post(restore_point))
        // E1：调试需独占总线时暂停/恢复该总线的采集（幂等，可反复调用）
        .route("/api/buses/:name/pause", post(pause_bus_collect))
        .route("/api/buses/:name/resume", post(resume_bus_collect))
        .route_layer(from_fn_with_state(state.clone(), require_auth))
        .route_layer(from_fn(crate::web::middleware::tag_config_entry));

    // === Config management: import/reload (always require token) ===
    let mgmt = Router::new()
        .route("/api/config/backup", post(backup_config))
        .route("/api/config/import", put(import_config))
        .route("/api/config/reload", post(reload_config))
        // P4：只校验不落盘（前端校验规则唯一来源）
        .route("/api/config/validate", post(validate_config_report))
        // 工程文件（三层子集 + 信封）：与整份 config 导入导出并存，语义不同
        .route("/api/project/export", get(export_project))
        .route("/api/project/import", post(import_project))
        .route_layer(from_fn_with_state(state.clone(), require_auth))
        .route_layer(from_fn(crate::web::middleware::tag_config_management));

    Router::new().merge(crud_read).merge(crud_write).merge(mgmt)
}
