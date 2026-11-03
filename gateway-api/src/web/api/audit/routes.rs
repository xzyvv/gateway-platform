//! 审计日志查询路由组
//!
//! 路由清单：
//! - GET /api/audit/logs -> list_audit_logs
//!
//! 鉴权策略：GET 类，`allow_anon_read=false` 时要求 token。

use std::sync::Arc;

use axum::{middleware::from_fn_with_state, routing::get, Router};

use super::list_audit_logs;
use crate::web::types::{require_auth_if_anon_disabled, AppState};

/// 构建审计日志查询路由组
pub fn audit_routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/audit/logs", get(list_audit_logs))
        .route_layer(from_fn_with_state(state, require_auth_if_anon_disabled))
}
