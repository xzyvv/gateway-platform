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

use crate::web::types::{ApiResponse, AppState};
use axum::extract::{Path, State};
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
    pub packets: Vec<ReplayPacket>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub(crate) async fn list_replay_sessions(
    State(state): State<Arc<AppState>>,
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

    let mut sessions = builtin_sessions(&state); // 仅在 demo 开关开启时含预置模板
    sessions.extend(replay_store::list());
    Ok(Json(ApiResponse::ok(sessions)))
}

pub(crate) async fn get_replay_session(
    State(state): State<Arc<AppState>>,
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

    // 先查持久化，找不到回预置（与 execute_replay 同口径）
    let s = replay_store::get(&id)
        .or_else(|| builtin_sessions(&state).into_iter().find(|s| s.id == id));
    Ok(Json(ApiResponse::ok(s)))
}

/// 保存或更新回放会话（body 为完整 ReplaySession；id 已存在则替换）
pub(crate) async fn save_replay_session(
    State(state): State<Arc<AppState>>,
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

    // 预置会话（demo 开关开启时的 s1/s2）不允许覆盖——id 匹配则返回 400
    let builtin: Vec<ReplaySession> = builtin_sessions(&state);
    let builtin_ids: Vec<&str> = builtin.iter().map(|s| s.id.as_str()).collect();
    if builtin_ids.contains(&session.id.as_str()) {
        return Err(crate::web::types::ApiError::bad_request(
            "预置回放会话不可覆盖（请换一个 id 或先删预置后保存新的）",
        ));
    }

    replay_store::upsert(&cfg_dir, session.clone());
    Ok(Json(ApiResponse::ok(session)))
}

/// 删除持久化回放会话（预置不可删，返回 400 提示）
pub(crate) async fn delete_replay_session(
    State(state): State<Arc<AppState>>,
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
    replay_store::delete(&cfg_dir, &id);
    Ok(Json(ApiResponse::ok(())))
}
