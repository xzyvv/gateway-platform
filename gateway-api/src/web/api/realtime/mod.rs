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
    // 默认构建（无 remote-shell）下不执行命令，但字段必须保留：
    // 请求体契约与开启该特性的构建一致，删掉会让同一份 JSON 解析结果不同。
    command: String,
}

mod handlers;
mod routes;

pub(crate) use handlers::*;
pub(crate) use routes::realtime_routes;
