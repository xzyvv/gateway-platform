//! 认证路由组（登录 / 身份校验 / 改密）
//!
//! 路由清单：
//! - POST /api/auth/login    -> login（公开，无需鉴权）
//! - GET  /api/auth/whoami   -> whoami（始终要求 token）
//! - POST /api/auth/password -> change_password（始终要求 token）
//!
//! 鉴权策略：login 不挂中间件；whoami/password 始终挂 `require_auth`
//! （whoami 的语义是"确认手上的 token 是否有效"，即使 allow_anon_read=true 也必须鉴权）。

use std::sync::Arc;

use axum::{
    middleware::from_fn_with_state,
    routing::{get, post},
    Router,
};

use super::{change_password, login, whoami};
use crate::web::types::{require_auth, AppState};

/// 构建认证路由组（公开登录 + 受保护的 whoami/password）
pub fn auth_routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    let public = Router::new().route("/api/auth/login", post(login));

    let protected = Router::new()
        .route("/api/auth/whoami", get(whoami))
        .route("/api/auth/password", post(change_password))
        .route_layer(from_fn_with_state(state, require_auth));

    Router::new().merge(public).merge(protected)
}
