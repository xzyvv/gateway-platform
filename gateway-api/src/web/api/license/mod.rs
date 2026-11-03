//! License API（§7）：授权状态查询 + 授权文件导入
//!
//! 路由：
//! - `GET /api/license`（read_routes，allow_anon_read=false 时要求 token）
//! - `POST /api/license/import`（write_routes，始终要求 token）
//!
//! 设计原则：
//! - GET 返回 LicenseSnapshot（授权状态 + 本机设备码，字段与 ts-rs 导出严格一致）
//! - 即使无 license 也返回 200 + present=false（不是错误状态，前端据此走 core 档默认视图）
//! - 导入失败返回 400 + 客户可读的中文原因（签名/指纹/格式），前端直接展示
//! - 有效期/时钟问题不算导入失败：文件是合法签发产物，落盘后状态降级由 GET 呈现
//!
//! 路由注册：见 [license_routes]（位于子模块 `routes`）。

use std::sync::Arc;

use axum::{
    extract::State,
    response::{IntoResponse, Response},
    Json,
};

use crate::web::error::ApiError;
use crate::web::types::AppState;
use gateway_core::license::{LicenseError, LicenseSnapshot};

mod handlers;
mod routes;

pub(crate) use handlers::*;
pub(crate) use routes::license_routes;

#[derive(serde::Deserialize)]
pub(crate) struct ImportRequest {
    /// .lic 文件全文（前端 FileReader 读文本后 JSON 传输，避免 multipart 依赖）
    pub content: String,
}
