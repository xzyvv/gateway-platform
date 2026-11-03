//! 内置模拟消费者路由组
//!
//! 路由清单：
//! - GET  /api/sim_consumer/status -> get_sim_consumer_status（读）
//! - POST /api/sim_consumer/start  -> start_sim_consumer（写）
//! - POST /api/sim_consumer/stop   -> stop_sim_consumer（写）
//!
//! 鉴权策略：读走 `require_auth_if_anon_disabled`，写走 `require_auth`。
//! 启停会直接改变队列数据的去向（被消费掉且不外发），属运行期控制面操作，
//! 必须有 token，且 API 密钥需持 `config:write`（见 `middleware::auth`
//! 的 `required_write_scope` 归类）。

use std::sync::Arc;

use axum::{
    middleware::from_fn_with_state,
    routing::{get, post},
    Router,
};

use super::handlers::{get_sim_consumer_status, start_sim_consumer, stop_sim_consumer};
use crate::web::types::{require_auth, require_auth_if_anon_disabled, AppState};

pub fn sim_consumer_routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    let read = Router::new()
        .route("/api/sim_consumer/status", get(get_sim_consumer_status))
        .route_layer(from_fn_with_state(
            state.clone(),
            require_auth_if_anon_disabled,
        ));

    let write = Router::new()
        .route("/api/sim_consumer/start", post(start_sim_consumer))
        .route("/api/sim_consumer/stop", post(stop_sim_consumer))
        .route_layer(from_fn_with_state(state.clone(), require_auth));

    Router::new().merge(read).merge(write)
}
