//! 点位运维路由组
//!
//! 路由清单：
//! - GET    /api/points/virtual                    -> list_virtual_points
//! - POST   /api/points/virtual                    -> create_virtual_point
//! - PUT    /api/points/virtual/{id}               -> update_virtual_point
//! - DELETE /api/points/virtual/{id}               -> delete_virtual_point
//! - POST   /api/points/virtual/{id}/recalc        -> recalc_virtual_point
//! - POST   /api/points/find-replace/search        -> find_replace_search
//! - POST   /api/points/find-replace/apply         -> find_replace_apply
//! - GET    /api/points/offset                      -> offset_list
//! - POST   /api/points/offset/apply                -> offset_apply
//! - GET    /api/points/clone/devices               -> clone_devices
//! - POST   /api/points/clone/apply                 -> clone_apply
//!
//! License gate（v2.0）：调试/分析/批量功能全部免费，不挂中间件。
//! 配额校验（max_points / max_protocols / northbound_channels）不在此路由层做，
//! 而是由保存路径 `web::config_apply::reload_then_save` 统一前置校验，
//! 与 v1.2 中间件模式解耦。

use std::sync::Arc;

use axum::{
    middleware::from_fn_with_state,
    routing::{delete, get, post, put},
    Router,
};

use super::{
    clone_apply, clone_devices, create_virtual_point, delete_virtual_point, find_replace_apply,
    find_replace_search, list_virtual_points, offset_apply, offset_list, recalc_virtual_point,
    update_virtual_point,
};
use crate::web::types::{require_auth, require_auth_if_anon_disabled, AppState};

/// 构建点位运维路由组
pub fn points_ops_routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    // ===== virtual_points（基础能力：公式派生）=====
    let virtual_read = Router::new()
        .route("/api/points/virtual", get(list_virtual_points))
        .route_layer(from_fn_with_state(
            state.clone(),
            require_auth_if_anon_disabled,
        ));

    let virtual_write = Router::new()
        .route("/api/points/virtual", post(create_virtual_point))
        // F-13：recalc 是 POST 且有副作用（同步求值并回写派生点位），
        // 原先误放在读层（allow_anon_read=true 时匿名即可触发），现归入写层。
        .route("/api/points/virtual/:id/recalc", post(recalc_virtual_point))
        .route("/api/points/virtual/:id", put(update_virtual_point))
        .route("/api/points/virtual/:id", delete(delete_virtual_point))
        .route_layer(from_fn_with_state(state.clone(), require_auth));

    // ===== 批量运维（v2.0 全部免费，无中间件）=====
    let batch_read = Router::new()
        .route("/api/points/offset", get(offset_list))
        .route("/api/points/clone/devices", get(clone_devices))
        .route_layer(from_fn_with_state(
            state.clone(),
            require_auth_if_anon_disabled,
        ));

    let batch_write = Router::new()
        .route("/api/points/find-replace/search", post(find_replace_search))
        .route("/api/points/find-replace/apply", post(find_replace_apply))
        .route("/api/points/offset/apply", post(offset_apply))
        .route("/api/points/clone/apply", post(clone_apply))
        .route_layer(from_fn_with_state(state.clone(), require_auth));

    Router::new()
        .merge(virtual_read)
        .merge(virtual_write)
        .merge(batch_read)
        .merge(batch_write)
}
