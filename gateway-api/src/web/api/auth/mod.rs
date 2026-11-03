//! Web 登录端点：用户名/密码校验 -> 返回 API Token

use crate::web::types::{ApiError, AppState};
use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Deserialize)]
pub(crate) struct LoginRequest {
    pub(crate) username: String,
    pub(crate) password: String,
}

#[derive(Serialize)]
pub(crate) struct LoginResponse {
    pub(crate) token: String,
}

#[derive(Deserialize)]
pub(crate) struct ChangePasswordRequest {
    /// #11: 当前密码——服务端据此二次校验（此前仅前端"再登录一次"代偿，服务端无这道关）。
    pub(crate) old_password: String,
    pub(crate) new_password: String,
}

mod handlers;
mod routes;

pub(crate) use handlers::*;
pub(crate) use routes::auth_routes;

#[derive(Serialize)]
pub(crate) struct WhoAmIResponse {
    pub(crate) username: String,
    pub(crate) using_default_password: bool,
    /// 当前配置的安全风险项（默认凭据 / 匿名只读 / 开放 CORS / 全网卡监听）
    /// 前端登录后据此提示一次；空数组表示无风险项
    pub(crate) security_warnings: Vec<String>,
    /// 调用者凭证类型：`primary` = Web 登录换取的主 api_token（等同管理员）；
    /// `api_key` = API 密钥。前端据此决定管理类入口的显隐，取代原先「前端恒为 admin」。
    pub(crate) credential: String,
    /// 主 token 为 `["*"]`；API 密钥为其 scopes 列表（只读密钥可能为空/仅 `*:read`）。
    pub(crate) scopes: Vec<String>,
}
