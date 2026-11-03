//! 会话真实回放（阻塞模式）
//!
//! 请求配对、单帧判定、阻塞汇总与 HTTP handler 均在此；
//! 逐帧推送（SSE）的引擎与事件定义见 `engine`。

use crate::web::api::protocol_tools::{
    accepts_sse, acquire_bus, audit_send, error_response, sse_response,
};
use crate::web::debug_bus::{
    clamp_interval_ms, clamp_timeout_ms, normalize_hex, BusErrorKind, MAX_REPLAY_DURATION,
};
use crate::web::types::{ApiResponse, AppState};
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::{channel::mpsc, StreamExt};
use serde::Serialize;
use std::sync::Arc;
use ts_rs::TS;

use super::engine::{spawn_replay, ReplayAudit, ReplayEvent};
use super::now_ms;
use super::{ReplayPacket, ReplaySession};

/// 回放执行请求（契约 §4）
#[derive(Debug, serde::Deserialize)]
pub struct ReplayExecuteBody {
    /// 目标总线 `port_name`（会话本身不携带总线，执行时由用户指定）
    pub bus: String,
    /// 单帧等待超时（ms），默认 1500、上限 10000
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// 相邻帧下发间隔（ms），默认 200、下限 20
    #[serde(default)]
    pub interval_ms: Option<u64>,
    /// 首个 mismatch/error 后是否停止，默认 false
    #[serde(default)]
    pub stop_on_mismatch: bool,
}

/// 单帧回放结果
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ReplayFrameResult {
    pub packet_id: String,
    pub request_hex: String,
    /// 会话中紧随该请求的录制响应；录制中无配对响应时为 ""
    pub expected_response_hex: String,
    /// 本次真实收到的响应；无响应时为 ""
    pub actual_response_hex: String,
    /// match / mismatch / timeout / error
    pub status: String,
    pub latency_ms: i32,
    /// 帧级失败分类（契约 §5）；正常时为 null
    pub error: Option<String>,
}

/// 回放执行结果
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ReplayExecuteResult {
    pub session_id: String,
    pub bus: String,
    #[ts(type = "number")]
    pub started_at_ms: i64,
    #[ts(type = "number")]
    pub finished_at_ms: i64,
    pub total: i32,
    /// 实际响应与录制响应字节一致（JSON 字段名按契约 §4 为 `match`）
    #[serde(rename = "match")]
    #[ts(rename = "match")]
    pub match_count: i32,
    /// 有响应但与录制不同（JSON 字段名按契约 §4 为 `mismatch`）
    #[serde(rename = "mismatch")]
    #[ts(rename = "mismatch")]
    pub mismatch_count: i32,
    /// 发送失败（含总线不可用）
    pub failed: i32,
    pub results: Vec<ReplayFrameResult>,
}

/// 按契约 §4 配对：每条 request 的期望响应 = 会话中其后最近的一条 response。
///
/// 中间夹着其它请求也不影响配对（同会话视为同设备）；遇到的第一条 response
/// 即为该请求的应答，符合现场「一问一答」的实际时序。
pub(super) fn pair_requests(session: &ReplaySession) -> Vec<(ReplayPacket, Option<ReplayPacket>)> {
    let mut out = Vec::new();
    for (i, p) in session.packets.iter().enumerate() {
        if p.direction != "request" {
            continue;
        }
        let expected = session.packets[i + 1..]
            .iter()
            .find(|q| q.direction == "response")
            .cloned();
        out.push((p.clone(), expected));
    }
    out
}

/// 单帧判定（契约 §4 比较规则）
///
/// 特别处理「录制时本就无响应」：期望为空串时，本次仍无响应 → 行为一致，记 match；
/// 本次反而收到响应 → 设备行为已变化，记 mismatch。这条规则让「离线故障」会话
/// 回放后能正确反映设备是否已恢复，而不是一律报 mismatch。
pub(super) fn judge_frame(
    expected: &str,
    actual: &str,
    kind: Option<&str>,
) -> (&'static str, Option<String>) {
    let exp = normalize_hex(expected);
    if let Some(k) = kind {
        // 录制时本就无响应（期望为空）：本次仍无响应 → 行为一致，记 match。
        // 这条必须放在「超时→timeout」之前，否则离线会话回放会被整体记成失败。
        if k == "timeout" && exp.is_empty() {
            return ("match", None);
        }
        return if k == "timeout" {
            ("timeout", Some(k.to_string()))
        } else {
            ("error", Some(k.to_string()))
        };
    }
    let act = normalize_hex(actual);
    if exp == act {
        ("match", None)
    } else {
        ("mismatch", None)
    }
}

/// 阻塞模式（v1 行为）：收集事件流汇总为一次性 JSON 响应。
/// 即使服务端提前收尾（硬上限/总线掉线），也按 v1 语义返回 200 + 已执行帧的部分结果。
async fn collect_replay_blocking(mut rx: mpsc::UnboundedReceiver<ReplayEvent>) -> Response {
    let mut session_id = String::new();
    let mut bus = String::new();
    let mut started_at_ms = 0i64;
    let mut frames: Vec<ReplayFrameResult> = Vec::new();
    while let Some(ev) = rx.next().await {
        match ev {
            ReplayEvent::Started {
                session_id: s,
                bus: b,
                started_at_ms: t,
                ..
            } => {
                session_id = s;
                bus = b;
                started_at_ms = t;
            }
            ReplayEvent::Frame(f) => frames.push(f),
            ReplayEvent::Summary(r) => return Json(ApiResponse::ok(*r)).into_response(),
            ReplayEvent::Aborted { .. } | ReplayEvent::Failed { .. } => break,
        }
    }
    // 服务端提前收尾：用已收集的帧重建汇总（与 v1 提前 break 后返回部分结果一致）
    let mut match_count = 0;
    let mut mismatch_count = 0;
    let mut failed = 0;
    for f in &frames {
        match f.status.as_str() {
            "match" => match_count += 1,
            "mismatch" => mismatch_count += 1,
            _ => failed += 1,
        }
    }
    Json(ApiResponse::ok(ReplayExecuteResult {
        session_id,
        bus,
        started_at_ms,
        finished_at_ms: now_ms(),
        total: frames.len() as i32,
        match_count,
        mismatch_count,
        failed,
        results: frames,
    }))
    .into_response()
}

pub(crate) async fn execute_replay(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    Json(body): Json<ReplayExecuteBody>,
) -> Result<Response, crate::web::types::ApiError> {
    let cfg_dir = state
        .config
        .config_loader
        .path()
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .to_path_buf();
    super::super::replay_store::ensure_loaded(&cfg_dir);

    // 先查持久化，找不到回预置（仅 demo 开关开启时才命中预置）
    let session = super::super::replay_store::get(&id)
        .or_else(|| {
            super::builtin_sessions(&state)
                .into_iter()
                .find(|s| s.id == id)
        })
        .ok_or_else(|| crate::web::types::ApiError::not_found(format!("回放会话 {} 不存在", id)))?;

    let bus_name = body.bus.trim().to_string();
    if bus_name.is_empty() {
        return Ok(error_response(BusErrorKind::InvalidFrame, "bus 不能为空"));
    }
    let timeout_ms = clamp_timeout_ms(body.timeout_ms);
    let interval_ms = clamp_interval_ms(body.interval_ms);

    // 总线准入与 v1 完全一致——准入失败在流建立之前返回，仍走 v1 JSON 错误体（契约 §2）
    let lease = match acquire_bus(&state, &bus_name).await {
        Ok(l) => l,
        Err(e) => {
            audit_send(
                &state,
                &bus_name,
                &format!("replay {} {}", id, e.kind.as_str()),
            );
            return Ok(error_response(e.kind, e.detail));
        }
    };

    let st = state.clone();
    let audit: ReplayAudit = Arc::new(move |action, bus, detail| {
        st.platform.audit.log(action, bus, detail, "api");
    });
    let rx = spawn_replay(
        audit,
        id,
        bus_name,
        lease,
        pair_requests(&session),
        timeout_ms,
        interval_ms,
        body.stop_on_mismatch,
        MAX_REPLAY_DURATION,
    );
    // P0-1b：同一 URL 按 Accept 内容协商（契约 §2）——请求 SSE 则逐帧推送，
    // 缺省保持 v1 阻塞 JSON 行为不变（既有契约测试不受影响）
    if accepts_sse(&headers) {
        Ok(sse_response(rx))
    } else {
        Ok(collect_replay_blocking(rx).await)
    }
}
