//! 实时数据 WebSocket 路由组
//!
//! 路由清单：
//! - GET  /api/realtime -> realtime_ws
//!
//! `/api/diagnostics/logs`（WS tail）与 `/api/diagnostics/exec`（白名单 shell）已迁到
//! `api/diagnostics/routes.rs` —— `/api/diagnostics*` 前缀只能有一个注册点。
//!
//! 鉴权策略：`allow_anon_read=false` 时要求 token。

use std::sync::Arc;

use axum::{middleware::from_fn_with_state, routing::get, Router};

use super::realtime_ws;
use crate::web::types::{require_auth_if_anon_disabled, AppState};

/// 构建实时数据路由组
pub fn realtime_routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/realtime", get(realtime_ws))
        .route_layer(from_fn_with_state(state, require_auth_if_anon_disabled))
}
