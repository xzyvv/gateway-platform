//! Web API 中间件集合：鉴权 / 速率限制 / 请求 ID / 统一响应信封 / 请求分类标记。
//!
//! 原单文件 `middleware.rs`（约 1.4k 行）按职责拆为子模块，对外路径
//! `crate::web::middleware::*` 保持不变（由本文件统一 re-export），
//! 路由与 handler 的引用无需改动。

mod auth;
mod category;
mod envelope;
mod rate_limit;
mod request_id;

pub use rate_limit::RateLimiter;

pub(crate) use auth::{
    allow_anon_read, api_token, extract_token_from_headers, extract_token_from_ws_protocol,
    extract_token_from_ws_query, require_admin_or_exec_scope, require_auth,
    require_auth_if_anon_disabled, token_matches,
};
pub(crate) use category::{
    tag_config_entry, tag_config_management, tag_diagnostic, tag_protocol_tool,
};
pub(crate) use envelope::envelope;
pub(crate) use rate_limit::rate_limit;
pub(crate) use request_id::request_id;
pub use request_id::{next_request_id, sanitize_incoming_request_id, RequestId, REQUEST_ID_HEADER};
