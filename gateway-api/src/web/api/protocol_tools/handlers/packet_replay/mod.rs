//! 报文回放 handler（静态知识库）
//! V1：返回预置的回放会话列表（含报文序列）。未来支持上传/保存真实会话。
//!
//! 子模块划分：
//! - `sessions`：预置会话静态数据（`build_sessions`）
//! - `replay_run`：会话真实回放（请求配对、单帧判定、阻塞汇总与 handler）
//! - `engine`：回放引擎事件流与后台任务（SSE 逐帧推送）

mod engine;
mod replay_run;
mod sessions;
#[cfg(test)]
mod tests;

use crate::web::middleware::{AuthedApiKey, AuthedMainToken};
use crate::web::types::{ApiResponse, AppState};
use axum::extract::{Extension, Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use ts_rs::TS;

use super::replay_store;
use sessions::build_sessions;

pub(crate) use replay_run::execute_replay;

/// 预置演示会话是否启用（`web.demo_replay_sessions`，默认 false）
fn demo_sessions(state: &AppState) -> bool {
    state
        .config
        .load_full()
        .web
        .as_ref()
        .map(|w| w.demo_replay_sessions)
        .unwrap_or(false)
}

/// 按开关取预置会话：生产默认不返回演示数据，避免与真实会话混淆。
fn builtin_sessions_for(demo: bool) -> Vec<ReplaySession> {
    if demo {
        build_sessions()
    } else {
        Vec::new()
    }
}

fn builtin_sessions(state: &AppState) -> Vec<ReplaySession> {
    builtin_sessions_for(demo_sessions(state))
}

/// 回放报文
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReplayPacket {
    pub id: String,
    #[ts(type = "number")]
    pub time_ms: i64,
    pub direction: String,
    pub hex: String,
    pub parsed: String,
}

/// 回放会话
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReplaySession {
    pub id: String,
    pub name: String,
    pub device_name: String,
    pub duration_ms: i32,
    pub packet_count: i32,
    #[ts(type = "number")]
    pub created_at_ms: i64,
    /// 会话归属（DD-26）：主 token 保存为 `"admin"`，API 密钥保存为 `key.id`；
    /// 升级前的历史会话为 null，只对管理员可见。保存时可省略，服务端按调用者盖章。
    #[serde(default)]
    #[ts(optional)]
    pub owner: Option<String>,
    pub packets: Vec<ReplayPacket>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 调用者身份（DD-26）：回放会话必须归属到具体用户/密钥，互相不可见、不可删。
#[derive(Debug, Clone)]
pub(crate) struct Caller {
    /// 归属标识：主 token 为 `"admin"`，API 密钥为 `key.id`
    pub(crate) owner: String,
    /// 管理员（主 token 或带 `*` / `admin` scope 的密钥）可管理全部会话
    pub(crate) admin: bool,
}

impl Caller {
    /// 是否可见/可管理该会话。
    ///
    /// 历史会话 `owner=None` 只对管理员可见——升级后宁可让管理员先看到，
    /// 也不能让任一 API 密钥读到别人的会话。
    pub(crate) fn can_view(&self, s: &ReplaySession) -> bool {
        if self.admin {
            return true;
        }
        matches!(s.owner.as_deref(), Some(o) if !o.is_empty() && o == self.owner)
    }
}

pub(crate) fn caller_of(
    main: Option<Extension<AuthedMainToken>>,
    key: Option<Extension<AuthedApiKey>>,
) -> Caller {
    if main.is_some() {
        return Caller {
            owner: "admin".into(),
            admin: true,
        };
    }
    match key {
        Some(Extension(k)) => Caller {
            admin: k.0.scopes.iter().any(|s| s == "*" || s == "admin"),
            owner: k.0.id,
        },
        // 匿名读（allow_anon_read）没有身份：既看不到任何持久化会话，也管理不了
        None => Caller {
            owner: String::new(),
            admin: false,
        },
    }
}

fn forbidden_owner() -> crate::web::types::ApiError {
    crate::web::types::ApiError {
        status: axum::http::StatusCode::FORBIDDEN,
        code: crate::web::error_codes::API_KEY_SCOPE,
        message: "只能管理自己保存的回放会话".to_string(),
    }
}

pub(crate) async fn list_replay_sessions(
    State(state): State<Arc<AppState>>,
    main: Option<Extension<AuthedMainToken>>,
    key: Option<Extension<AuthedApiKey>>,
) -> Result<Json<ApiResponse<Vec<ReplaySession>>>, crate::web::types::ApiError> {
    // 首次调用时从磁盘加载持久化会话（幂等，call_once 保证只做一次）
    let cfg_dir = state
        .config
        .config_loader
        .path()
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .to_path_buf();
    replay_store::ensure_loaded(&cfg_dir);

    let caller = caller_of(main, key);
    let mut sessions = builtin_sessions(&state); // 仅在 demo 开关开启时含预置模板
                                                 // DD-26：只返回属于调用者的会话（管理员可见全部）
    sessions.extend(
        replay_store::list(&cfg_dir)
            .into_iter()
            .filter(|s| caller.can_view(s)),
    );
    Ok(Json(ApiResponse::ok(sessions)))
}

pub(crate) async fn get_replay_session(
    State(state): State<Arc<AppState>>,
    main: Option<Extension<AuthedMainToken>>,
    key: Option<Extension<AuthedApiKey>>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<Option<ReplaySession>>>, crate::web::types::ApiError> {
    let cfg_dir = state
        .config
        .config_loader
        .path()
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .to_path_buf();
    replay_store::ensure_loaded(&cfg_dir);

    let caller = caller_of(main, key);
    // 先查持久化（不可见的会话按不存在处理，不泄露他人会话是否存在），找不到回预置
    let s = replay_store::get(&cfg_dir, &id)
        .filter(|s| caller.can_view(s))
        .or_else(|| builtin_sessions(&state).into_iter().find(|s| s.id == id));
    Ok(Json(ApiResponse::ok(s)))
}

/// 保存或更新回放会话（body 为完整 ReplaySession；id 已存在则替换）
///
/// DD-26：归属由服务端按调用者盖章，忽略 body 里的 `owner`——否则任何人
/// 都能把自己的会话写成别人的名字来嫁祸/越权。
pub(crate) async fn save_replay_session(
    State(state): State<Arc<AppState>>,
    main: Option<Extension<AuthedMainToken>>,
    key: Option<Extension<AuthedApiKey>>,
    Json(session): Json<ReplaySession>,
) -> Result<Json<ApiResponse<ReplaySession>>, crate::web::types::ApiError> {
    let cfg_dir = state
        .config
        .config_loader
        .path()
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .to_path_buf();
    replay_store::ensure_loaded(&cfg_dir);

    let caller = caller_of(main, key);
    // 预置会话（demo 开关开启时的 s1/s2）不允许覆盖——id 匹配则返回 400
    let builtin: Vec<ReplaySession> = builtin_sessions(&state);
    let builtin_ids: Vec<&str> = builtin.iter().map(|s| s.id.as_str()).collect();
    if builtin_ids.contains(&session.id.as_str()) {
        return Err(crate::web::types::ApiError::bad_request(
            "预置回放会话不可覆盖（请换一个 id 或先删预置后保存新的）",
        ));
    }
    // DD-26：不能覆盖别人的会话
    if let Some(existing) = replay_store::get(&cfg_dir, &session.id) {
        if !caller.can_view(&existing) {
            return Err(forbidden_owner());
        }
    }

    let mut session = session;
    session.owner = Some(caller.owner.clone());
    replay_store::upsert(&cfg_dir, session.clone());
    Ok(Json(ApiResponse::ok(session)))
}

/// 删除持久化回放会话（预置不可删，返回 400 提示）
pub(crate) async fn delete_replay_session(
    State(state): State<Arc<AppState>>,
    main: Option<Extension<AuthedMainToken>>,
    key: Option<Extension<AuthedApiKey>>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<()>>, crate::web::types::ApiError> {
    let cfg_dir = state
        .config
        .config_loader
        .path()
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .to_path_buf();
    replay_store::ensure_loaded(&cfg_dir);

    let builtin: Vec<ReplaySession> = builtin_sessions(&state);
    let builtin_ids: Vec<&str> = builtin.iter().map(|s| s.id.as_str()).collect();
    if builtin_ids.contains(&id.as_str()) {
        return Err(crate::web::types::ApiError::bad_request(
            "预置回放会话不可删除",
        ));
    }
    // DD-26：不能删别人的会话（不存在则保持 no-op 成功，与旧行为一致）
    let caller = caller_of(main, key);
    if let Some(existing) = replay_store::get(&cfg_dir, &id) {
        if !caller.can_view(&existing) {
            return Err(forbidden_owner());
        }
    }
    replay_store::delete(&cfg_dir, &id);
    Ok(Json(ApiResponse::ok(())))
}
