//! Web API 审计日志查询端点（A14）
//!
//! 提供 GET /api/audit/logs 接口，支持按 action/target/source 子串过滤、
//! 时间范围（ts_from ~ ts_to 闭区间）筛选、limit 条数上限。

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    response::{IntoResponse, Json},
};

use crate::web::types::{ApiError, AppState};
use gateway_core::engine::audit::{AuditEntry, AuditFilter};

#[derive(serde::Deserialize)]
pub struct AuditQuery {
    /// action 子串匹配（大小写不敏感）
    pub action: Option<String>,
    /// target 子串匹配
    pub target: Option<String>,
    /// source 子串匹配（操作人/来源）
    pub source: Option<String>,
    /// 起始时间（毫秒 epoch，闭区间）
    pub ts_from: Option<i64>,
    /// 截止时间（毫秒 epoch，闭区间）
    pub ts_to: Option<i64>,
    /// 返回条数上限（默认 200，最大 1000）
    pub limit: Option<usize>,
}

mod handlers;
mod routes;

pub(crate) use handlers::*;
pub(crate) use routes::audit_routes;
