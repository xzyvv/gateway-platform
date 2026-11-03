//! 实时数据 WebSocket / 诊断日志流 / 诊断命令执行路由组
//!
//! 路由清单：
//! - GET  /api/realtime           -> realtime_ws
//! - GET  /api/diagnostics/logs   -> diagnostics_logs_ws
//! - POST /api/diagnostics/exec   -> diagnostics_exec
//!
//! 鉴权策略：GET 类 allow_anon_read=false 时要求 token；
//! `POST /api/diagnostics/exec` 另需 `require_admin_or_exec_scope`（主 token 或持
//! `diagnostics:exec` scope 的密钥），其余 POST 类始终要求 token。

use std::sync::Arc;

use axum::{
    middleware::from_fn_with_state,
    routing::{get, post},
    Router,
};

use super::{diagnostics_exec, diagnostics_logs_ws, realtime_ws};
use crate::web::types::{require_admin_or_exec_scope, require_auth_if_anon_disabled, AppState};

/// 构建实时数据 / 诊断流路由组
pub fn realtime_routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    let read = Router::new()
        .route("/api/realtime", get(realtime_ws))
        .route("/api/diagnostics/logs", get(diagnostics_logs_ws))
        .route_layer(from_fn_with_state(
            state.clone(),
            require_auth_if_anon_disabled,
        ));

    let write = Router::new()
        .route("/api/diagnostics/exec", post(diagnostics_exec))
        .route_layer(from_fn_with_state(state, require_admin_or_exec_scope));

    Router::new().merge(read).merge(write)
}
