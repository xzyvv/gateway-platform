//! Web API 历史端点：历史查询、CSV 导出
//!
//! 设备模板管理（`/api/templates`）不属历史域，已迁到 `api/devices`。

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    response::{IntoResponse, Json},
};

use crate::web::types::{ApiError, AppState, HistoryQuery};

mod handlers;
mod routes;

pub(crate) use handlers::*;
pub(crate) use routes::history_routes;
