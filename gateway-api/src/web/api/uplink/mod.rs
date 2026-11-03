//! Web API：上行模板配置 + 调试（§17 网关模板化上行）

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Json,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::web::types::AppState;

mod handlers;
mod routes;

#[derive(Serialize)]
pub(crate) struct ApiOk<T: Serialize> {
    #[serde(rename = "code")]
    pub code: u32,
    #[serde(rename = "message")]
    pub message: &'static str,
    #[serde(rename = "data")]
    pub data: T,
}

impl<T: Serialize> ApiOk<T> {
    fn new(data: T) -> Self {
        Self {
            code: 0,
            message: "success",
            data,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct ApiErr {
    #[serde(rename = "code")]
    pub code: u32,
    #[serde(rename = "message")]
    pub message: String,
}

impl ApiErr {
    fn new(code: u32, msg: impl Into<String>) -> Self {
        Self {
            code,
            message: msg.into(),
        }
    }
}

type Resp = (StatusCode, Json<serde_json::Value>);

fn ok<T: Serialize>(data: T) -> Resp {
    (
        StatusCode::OK,
        // 序列化失败时返回 Null 避免请求处理路径 panic(稳定性优先于返回错误细节)
        Json(serde_json::to_value(ApiOk::new(data)).unwrap_or(serde_json::Value::Null)),
    )
}

fn err(status: StatusCode, code: u32, msg: impl Into<String>) -> Resp {
    (
        status,
        // D2: 与 ok() 一致——序列化失败返回 Null,避免请求处理路径 panic。
        Json(serde_json::to_value(ApiErr::new(code, msg)).unwrap_or(serde_json::Value::Null)),
    )
}

#[derive(Serialize)]
pub(crate) struct UplinkConfigDto {
    pub enabled: bool,
    pub template: String,
    pub encoding: String,
    pub max_bytes: u32,
    pub updated_at: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct SaveUplinkConfigRequest {
    pub enabled: bool,
    pub template: String,
    #[serde(default = "default_encoding")]
    pub encoding: String,
    #[serde(default = "default_max_bytes")]
    pub max_bytes: u32,
}

fn default_encoding() -> String {
    "json_lines".into()
}
fn default_max_bytes() -> u32 {
    4096
}

#[derive(Deserialize)]
pub(crate) struct ImportTemplateRequest {
    pub template: String,
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Serialize)]
pub(crate) struct TemplateVarDto {
    pub name: String,
    #[serde(rename = "type")]
    pub var_type: &'static str,
    pub description: &'static str,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
}

#[derive(Deserialize)]
pub(crate) struct TestUplinkRequest {
    pub template: String,
    #[serde(default)]
    pub vars: std::collections::HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub send: bool,
}

#[derive(Serialize)]
pub(crate) struct TestUplinkResponse {
    pub ok: bool,
    pub rendered: String,
    pub bytes: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publish: Option<PublishResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// 测试历史记录 id（仅 send=true 时返回）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_id: Option<i64>,
}

#[derive(Serialize)]
pub(crate) struct PublishResult {
    pub topic: String,
    pub qos: u8,
    pub delivered: bool,
    pub latency_ms: u64,
}

// ===== 测试历史 =====

#[derive(Deserialize)]
pub(crate) struct ListHistoryQuery {
    #[serde(default = "default_history_limit")]
    pub limit: usize,
}

fn default_history_limit() -> usize {
    50
}

pub(crate) use handlers::*;
pub(crate) use routes::uplink_routes;

#[cfg(test)]
mod tests;
