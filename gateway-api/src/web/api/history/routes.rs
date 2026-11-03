//! 历史查询 / CSV 导出路由组
//!
//! 路由清单：
//! - GET    /api/history            -> history
//! - GET    /api/history/export     -> history_export_csv
//!
//! 设备模板（`/api/templates`）不属历史域，已迁到 `api/devices/routes.rs`。
//!
//! 鉴权策略：GET 类 allow_anon_read=false 时要求 token。

use std::sync::Arc;

use axum::{middleware::from_fn_with_state, routing::get, Router};

use super::{history, history_export_csv};
use crate::web::types::{require_auth_if_anon_disabled, AppState};

/// 构建历史查询路由组
pub fn history_routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/history", get(history))
        .route("/api/history/export", get(history_export_csv))
        .route_layer(from_fn_with_state(state, require_auth_if_anon_disabled))
}
