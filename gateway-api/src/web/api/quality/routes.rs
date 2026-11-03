//! 数据质量规则路由组
//!
//! 路由清单：
//! - GET    /api/quality/rules      -> list_quality_rules（读）
//! - POST   /api/quality/rules      -> create_quality_rule（写）
//! - PUT    /api/quality/rules/:id  -> update_quality_rule（写，局部更新）
//! - DELETE /api/quality/rules/:id  -> delete_quality_rule（写）
//!
//! 鉴权策略：读走 `require_auth_if_anon_disabled`，写走 `require_auth`。
//! 写操作成功后重新安装网关处理管道的规则快照（规则真实生效，非仅落库）。

use std::sync::Arc;

use axum::{
    middleware::from_fn_with_state,
    routing::{get, post, put},
    Router,
};

use super::handlers::{
    create_quality_rule, delete_quality_rule, list_quality_rules, update_quality_rule,
};
use crate::web::types::{require_auth, require_auth_if_anon_disabled, AppState};

pub fn quality_routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    let read = Router::new()
        .route("/api/quality/rules", get(list_quality_rules))
        .route_layer(from_fn_with_state(
            state.clone(),
            require_auth_if_anon_disabled,
        ));

    let write = Router::new()
        .route("/api/quality/rules", post(create_quality_rule))
        .route(
            "/api/quality/rules/:id",
            put(update_quality_rule).delete(delete_quality_rule),
        )
        .route_layer(from_fn_with_state(state.clone(), require_auth));

    Router::new().merge(read).merge(write)
}
