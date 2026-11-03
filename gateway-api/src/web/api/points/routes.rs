//! 点位即时读取 / 写入 / 批量操作路由组
//!
//! 路由清单：
//! - GET  /api/points/:id/read -> point_read_now
//! - POST /api/points/write    -> points_write_unified（B2 唯一写入口：1 条走单写、≥2 条走批写）
//! - POST /api/points/batch    -> points_batch（R3 合并：裸数组=upsert，{action}=upsert|edit）
//!
//! 第 1 期冗余收口：原 `POST /api/points/:id/write` 与 `POST /api/points/write-batch`
//! 与统一写入口语义重复（内部复用同一 `write_points_core`），前端已全部切到
//! `POST /api/points/write`，两个旧端点已删除。
//!
//! 鉴权策略：`/api/points/:id/read` 会真实发起一次现场采集（占用总线），
//! 与纯读缓存不同，统一放在写层（`require_auth`），与同语义的
//! `POST /api/diagnostics/task-diag/:point_id` 保护等级一致（F-13）。
//! 其余写类始终要求 token。

use std::sync::Arc;

use axum::{
    middleware::from_fn_with_state,
    routing::{get, post},
    Router,
};

use super::{point_read_now, points_batch, points_write_unified};
use crate::web::types::{require_auth, AppState};

/// 构建点位即时读取 / 批量操作路由组
pub fn points_routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    let write = Router::new()
        // F-13：`:id/read` 有真实总线副作用，匿名可触发等同放开一次现场采集，
        // 因此从读层移入写层（require_auth）。
        .route("/api/points/:id/read", get(point_read_now))
        // B2 唯一写入口：前端三个写入口页统一调用本路由（单/批分发由后端决定）
        .route("/api/points/write", post(points_write_unified))
        .route("/api/points/batch", post(points_batch))
        .route_layer(from_fn_with_state(state, require_auth));

    Router::new().merge(write)
}
