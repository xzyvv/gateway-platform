//! 上行模板配置 API 路由组
//!
//! 路由构建函数原定义在 `mod.rs`，现下沉至此，与其它业务子域保持一致的目录约定
//! （`<子域>/routes.rs` 定义 `*_routes`，由 `<子域>/mod.rs` 重导出）。

use std::sync::Arc;

use axum::{
    middleware::from_fn_with_state,
    routing::{delete, get, post},
    Router,
};

use super::{
    clear_test_history, export_template, get_uplink_config, import_template, list_template_vars,
    list_test_history, save_uplink_config, test_uplink,
};
use crate::web::types::{require_auth, require_auth_if_anon_disabled, AppState};

/// 构建上行模板配置路由组（按读/写分层鉴权，与 config_routes 同一约定）
pub fn uplink_routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    // 读接口：匿名可读开关开启时不强制 token
    let read = Router::new()
        .route("/api/uplink/config", get(get_uplink_config))
        .route("/api/uplink/vars", get(list_template_vars))
        .route("/api/uplink/template/export", get(export_template))
        .route("/api/uplink/test/history", get(list_test_history))
        .route_layer(from_fn_with_state(
            state.clone(),
            require_auth_if_anon_disabled,
        ));

    // 写接口：一律强制 token
    let write = Router::new()
        .route("/api/uplink/config", post(save_uplink_config))
        .route("/api/uplink/test", post(test_uplink))
        .route("/api/uplink/template/import", post(import_template))
        .route("/api/uplink/test/history", delete(clear_test_history))
        .route_layer(from_fn_with_state(state.clone(), require_auth));

    Router::new().merge(read).merge(write).with_state(state)
}
