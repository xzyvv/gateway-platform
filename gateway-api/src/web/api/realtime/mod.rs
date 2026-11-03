//! Web API 实时端点：WebSocket 实时数据、日志流、诊断执行

use std::sync::Arc;

use axum::{extract::State, response::Json};
use serde::Deserialize;

use crate::web::types::{ApiError, AppState};

use std::time::Duration;

const WS_PING_INTERVAL: Duration = Duration::from_secs(30);
const WS_IDLE_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Deserialize)]
pub(crate) struct ExecRequest {
    // 默认构建（无 remote-shell）下 handler 不使用该字段，仅为保持请求体契约不变。
    #[cfg_attr(not(feature = "remote-shell"), allow(dead_code))]
    command: String,
}

mod handlers;
mod routes;

pub(crate) use handlers::*;
pub(crate) use routes::realtime_routes;
