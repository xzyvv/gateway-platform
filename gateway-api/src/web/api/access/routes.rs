//! 访问控制域路由：限流规则 / 访问日志 / API 密钥 / 计划任务
//!
//! 路由清单：
//! - GET    /api/access/rate-rules            -> list_rate_rules（读）
//! - POST   /api/access/rate-rules            -> create_rate_rule（写）
//! - PUT    /api/access/rate-rules/{id}       -> update_rate_rule（写）
//! - DELETE /api/access/rate-rules/:id        -> delete_rate_rule（写）
//! - GET    /api/access/rate-stats            -> rate_stats（读）
//! - GET    /api/access/rate-logs             -> rate_logs（读）
//! - GET    /api/access/api-keys              -> list_api_keys（读）
//! - POST   /api/access/api-keys              -> create_api_key（写）
//! - PUT    /api/access/api-keys/:id          -> update_api_key（写，启停）
//! - DELETE /api/access/api-keys/:id          -> delete_api_key（写）
//! - GET    /api/access/scheduled-tasks           -> list_sched_tasks（读）
//! - POST   /api/access/scheduled-tasks           -> create_sched_task（写）
//! - PUT    /api/access/scheduled-tasks/:id       -> update_sched_task（写）
//! - DELETE /api/access/scheduled-tasks/:id       -> delete_sched_task（写）
//! - POST   /api/access/scheduled-tasks/:id/run   -> run_sched_task_now（写）
//!
//! 鉴权分层与 config 域一致：读走 require_auth_if_anon_disabled，写走 require_auth。
//! API 密钥本身即本域管理对象，因此写操作必须经主 token 或带 :write scope 的密钥。

use std::sync::Arc;

use axum::{
    middleware::from_fn_with_state,
    routing::{get, post, put},
    Router,
};

use super::handlers::{
    create_api_key, create_rate_rule, create_sched_task, delete_api_key, delete_rate_rule,
    delete_sched_task, list_api_keys, list_rate_rules, list_sched_tasks, rate_logs, rate_stats,
    run_sched_task_now, update_api_key, update_rate_rule, update_sched_task,
};
use crate::web::types::{require_auth, require_auth_if_anon_disabled, AppState};

pub fn access_routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    let read = Router::new()
        .route("/api/access/rate-rules", get(list_rate_rules))
        .route("/api/access/rate-stats", get(rate_stats))
        .route("/api/access/rate-logs", get(rate_logs))
        .route("/api/access/api-keys", get(list_api_keys))
        .route("/api/access/scheduled-tasks", get(list_sched_tasks))
        .route_layer(from_fn_with_state(
            state.clone(),
            require_auth_if_anon_disabled,
        ));

    let write = Router::new()
        .route("/api/access/rate-rules", post(create_rate_rule))
        .route(
            "/api/access/rate-rules/:id",
            put(update_rate_rule).delete(delete_rate_rule),
        )
        .route("/api/access/api-keys", post(create_api_key))
        .route(
            "/api/access/api-keys/:id",
            put(update_api_key).delete(delete_api_key),
        )
        .route("/api/access/scheduled-tasks", post(create_sched_task))
        .route(
            "/api/access/scheduled-tasks/:id",
            put(update_sched_task).delete(delete_sched_task),
        )
        .route(
            "/api/access/scheduled-tasks/:id/run",
            post(run_sched_task_now),
        )
        .route_layer(from_fn_with_state(state.clone(), require_auth));

    Router::new().merge(read).merge(write)
}
