//! IO 控制 / 设备模板路由组
//!
//! 路由清单：
//! - GET  /api/io/status            -> io_status
//! - POST /api/io/:index            -> control_io
//! - GET  /api/templates            -> list_templates
//! - POST /api/templates            -> save_template
//! - DELETE /api/templates/:name    -> delete_template
//!
//! D-03：原 `POST /api/discover` 与 `POST /api/protocol-tools/slave-scan` 等价，
//! 已删除（前端改调 slave-scan 阻塞模式）。
//!
//! 设备模板原寄居在 `api/history`（模板属设备域），已迁入本路由组，URL 不变。
//!
//! 鉴权策略：GET 类 allow_anon_read=false 时要求 token；写类始终要求 token。

use std::sync::Arc;

use axum::{
    middleware::from_fn_with_state,
    routing::{delete, get, post},
    Router,
};

use super::{control_io, delete_template, io_status, list_templates, save_template};
use crate::web::types::{require_auth, require_auth_if_anon_disabled, AppState};

/// 构建 IO 控制与设备模板路由组
pub fn devices_routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    let read = Router::new()
        .route("/api/io/status", get(io_status))
        .route("/api/templates", get(list_templates))
        .route_layer(from_fn_with_state(
            state.clone(),
            require_auth_if_anon_disabled,
        ));

    let write = Router::new()
        .route("/api/io/:index", post(control_io))
        .route("/api/templates", post(save_template))
        .route("/api/templates/:name", delete(delete_template))
        .route_layer(from_fn_with_state(state, require_auth));

    Router::new().merge(read).merge(write)
}
