//! License 路由组
//!
//! 路由清单：
//! - GET  /api/license          -> get_license
//! - POST /api/license/import   -> import_license
//!
//! 鉴权策略：GET 类 allow_anon_read=false 时要求 token；POST 类始终要求 token。

use std::sync::Arc;

use axum::{
    middleware::from_fn_with_state,
    routing::{get, post},
    Router,
};

use super::{get_license, import_license};
use crate::web::types::{require_auth, require_auth_if_anon_disabled, AppState};

/// 构建 License 路由组
pub fn license_routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    let read = Router::new()
        .route("/api/license", get(get_license))
        .route_layer(from_fn_with_state(
            state.clone(),
            require_auth_if_anon_disabled,
        ));

    let write = Router::new()
        .route("/api/license/import", post(import_license))
        .route_layer(from_fn_with_state(state, require_auth));

    Router::new().merge(read).merge(write)
}
