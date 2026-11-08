//! 设备域路由组：`/api/devices*` + `/api/io/*` + `/api/templates`
//!
//! **`/api/devices*` 前缀的唯一注册点**。handler 分散在三个子域
//! （`config` 的设备 CRUD、`device_ops` 的运维接口、本模块的 control/topology/
//! point-quality），但路由只在本文件出现一次，所以 grep 任一
//! `/api/devices/...` 路径只会命中这里，再由处理器名直达实现文件。
//!
//! 路由清单：
//! - GET    /api/devices                          -> list_devices（config）
//! - POST   /api/devices                          -> create_device（config）
//! - PUT    /api/devices/{id}                     -> update_device（config）
//! - DELETE /api/devices/{id}                     -> delete_device（config）
//! - GET    /api/devices/{id}/points              -> list_points（config）
//! - POST   /api/devices/{id}/points              -> create_point（config）
//! - GET    /api/devices/assets                   -> list_assets
//! - GET    /api/devices/{id}/asset               -> get_asset
//! - PUT    /api/devices/{id}/asset               -> update_asset (upsert)
//! - GET    /api/devices/health                   -> list_health
//! - GET    /api/devices/shadows                  -> list_shadows
//! - GET    /api/devices/control/writable         -> list_writable_points
//! - GET    /api/devices/control/commands         -> list_commands
//! - POST   /api/devices/control/send             -> send_command
//! - GET    /api/devices/topology                 -> device_topology
//! - GET    /api/devices/point-quality            -> list_point_quality
//! - GET    /api/devices/point-quality/stats      -> point_quality_stats
//! - GET    /api/devices/backoff                  -> list_point_backoff
//! - GET    /api/io/status                        -> io_status
//! - POST   /api/io/{index}                       -> control_io
//! - GET    /api/templates                        -> list_templates
//! - POST   /api/templates                        -> save_template
//! - DELETE /api/templates/{name}                 -> delete_template
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
//! D-03：原 `POST /api/discover` 与 `POST /api/protocol-tools/slave-scan` 等价，
//! 已删除（前端改调 slave-scan 阻塞模式）。
//!
//! 鉴权与分类标记**必须按子 Router 分别施加**，不要合并成一个大组：
//! - 设备 CRUD：读 `require_auth_if_anon_disabled`、写 `require_auth`，
//!   且都打 `ConfigEntry` 分类标记 —— 与收口前在 `config` 路由组里的行为一致，
//!   指标/审计的数字才能跨版本连续比对；
//! - 设备运维（assets/health/shadows/topology/quality/backoff/control）：
//!   读匿名可读、写必须 token，**无**分类标记（沿用 `device_ops` 原状）；
//! - IO / 模板：同运维组，无分类标记（沿用 `devices` 原状）。

use std::sync::Arc;

use axum::{
    middleware::{from_fn, from_fn_with_state},
    routing::{delete, get, post, put},
    Router,
};

use super::{
    control_io, delete_template, device_topology, io_status, list_commands, list_point_backoff,
    list_point_quality, list_templates, list_writable_points, point_quality_stats, save_template,
    send_command,
};
// 设备 CRUD 与运维 handler 分属 `config` / `device_ops` 子域，
// 由 `web::api` 的扁平重导出统一暴露（见 `web/api/mod.rs`）。
use crate::web::api::{
    add_device_to_group, create_device, create_group, create_point, delete_device, delete_group,
    get_asset, list_assets, list_devices, list_groups, list_health, list_points, list_shadows,
    remove_device_from_group, update_asset, update_device, update_group,
};
use crate::web::types::{require_auth, require_auth_if_anon_disabled, AppState};

/// 构建设备域路由组
pub fn devices_routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    // === 设备 CRUD（原 config 路由组，保留 ConfigEntry 分类标记）===
    let crud_read = Router::new()
        .route("/api/devices", get(list_devices))
        .route("/api/devices/:id/points", get(list_points))
        .route_layer(from_fn_with_state(
            state.clone(),
            require_auth_if_anon_disabled,
        ))
        .route_layer(from_fn(crate::web::middleware::tag_config_entry));

    let crud_write = Router::new()
        .route("/api/devices", post(create_device))
        .route("/api/devices/:id", put(update_device).delete(delete_device))
        .route("/api/devices/:id/points", post(create_point))
        .route_layer(from_fn_with_state(state.clone(), require_auth))
        .route_layer(from_fn(crate::web::middleware::tag_config_entry));

    // === 设备运维（原 device_ops 路由组，无分类标记）===
    let ops_read = Router::new()
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

    let ops_write = Router::new()
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

    // === IO 控制 / 设备模板（本模块自有）===
    let io_read = Router::new()
        .route("/api/io/status", get(io_status))
        .route("/api/templates", get(list_templates))
        .route_layer(from_fn_with_state(
            state.clone(),
            require_auth_if_anon_disabled,
        ));

    let io_write = Router::new()
        .route("/api/io/:index", post(control_io))
        .route("/api/templates", post(save_template))
        .route("/api/templates/:name", delete(delete_template))
        .route_layer(from_fn_with_state(state, require_auth));

    Router::new()
        .merge(crud_read)
        .merge(crud_write)
        .merge(ops_read)
        .merge(ops_write)
        .merge(io_read)
        .merge(io_write)
}
