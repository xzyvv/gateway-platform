//! 实时 API 处理函数（WebSocket / 诊断）

use super::*;
#[cfg(feature = "remote-shell")]
use axum::response::IntoResponse;

pub(crate) async fn diagnostics_logs_ws(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    uri: axum::http::Uri,
    ws: axum::extract::ws::WebSocketUpgrade,
) -> axum::response::Response {
    use crate::web::error::ApiError;
    use crate::web::middleware::{
        allow_anon_read, api_token, extract_token_from_headers, extract_token_from_ws_protocol,
        extract_token_from_ws_query, token_matches,
    };
    use axum::extract::ws::Message;
    use axum::response::IntoResponse;
    use futures::StreamExt;

    // A6: WebSocket 升级前校验 token，避免未授权客户端订阅诊断日志流。
    // B1 修复：token 提取顺序为 Header > Sec-WebSocket-Protocol 子协议 > URL query。
    //   - Header 路径：Tauri / 自定义客户端走 Authorization: Bearer。
    //   - 子协议路径：浏览器 WebSocket 走 `bearer.<token>` 子协议（**不再落 URL 日志**）。
    //   - query 路径：仅在 `allow_anon_read=true` 时接受（向后兼容旧客户端）。
    //     生产部署建议强制要求前两种，避免 token 写入反代 access log。
    let ws_proto_token = extract_token_from_ws_protocol(&headers);
    let provided_token = extract_token_from_headers(&headers).or_else(|| ws_proto_token.clone());

    if !allow_anon_read(&state) {
        match provided_token {
            Some(token) if token_matches(&token, &api_token(&state)) => {}
            Some(_) => return ApiError::unauthorized("invalid api token").into_response(),
            None => return ApiError::unauthorized(
                "websocket authentication required; pass Authorization: Bearer <token> header or `bearer.<token>` Sec-WebSocket-Protocol subprotocol"
            ).into_response(),
        }
    } else if provided_token.is_none() {
        // 匿名读模式：仍接受 query token 兼容旧客户端（生产环境应关闭 allow_anon_read）
        if extract_token_from_ws_query(&uri).is_none() {
            // 匿名读开启且无任何 token：继续（不强制登录）
        }
    }

    // B1-2 修复：浏览器要求服务端在 101 响应中回选客户端提供的子协议——
    // Chrome/Firefox 对「客户端提供了 Sec-WebSocket-Protocol 但服务端未回选」的握手
    // 一律判定失败（前端表现为实时/日志流永远「重连中」）。axum 默认不回选，
    // 必须显式调用 protocols()。仅当子协议 token 本身有效时回选，
    // 与「该连接经此子协议完成鉴权」的语义保持一致。
    let ws = match ws_proto_token.filter(|t| token_matches(t, &api_token(&state))) {
        Some(t) => ws.protocols([format!("bearer.{t}")]),
        None => ws,
    };

    let mut rx = state.platform.diagnostics.subscribe_logs();

    ws.on_upgrade(move |mut socket| async move {
        let mut ping_ticker = tokio::time::interval(Duration::from_secs(30));
        // Bug 97: 增加 idle timeout ticker——90s 内未收到客户端消息视为僵尸客户端。
        let mut idle_ticker = tokio::time::interval(Duration::from_secs(90));
        idle_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last_activity = tokio::time::Instant::now();
        loop {
            tokio::select! {
                result = rx.recv() => {
                    match result {
                        Ok(line) => {
                            if socket.send(Message::Text(line)).await.is_err() {
                                break;
                            }
                            last_activity = tokio::time::Instant::now();
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => break,
                    }
                }
                _ = ping_ticker.tick() => {
                    if socket.send(Message::Ping(Vec::new())).await.is_err() {
                        break;
                    }
                }
                _ = idle_ticker.tick() => {
                    if last_activity.elapsed() >= Duration::from_secs(90) {
                        tracing::debug!("diagnostics logs ws idle timeout, closing");
                        let _ = socket.send(Message::Close(None)).await;
                        break;
                    }
                }
                msg = socket.next() => {
                    last_activity = tokio::time::Instant::now();
                    match msg {
                        Some(Ok(Message::Ping(p))) => {
                            if socket.send(Message::Pong(p)).await.is_err() { break; }
                        }
                        Some(Ok(Message::Close(_))) | None => break,
                        _ => {}
                    }
                }
            }
        }
    })
    .into_response()
}

#[cfg(feature = "remote-shell")]
pub(crate) async fn diagnostics_exec(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ExecRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let diagnostics = state.platform.diagnostics.clone();
    let cmd = req.command.clone();
    // Bug 99: 命令字符串长度上限 4 KB。
    const MAX_CMD_LEN: usize = 4096;
    if cmd.len() > MAX_CMD_LEN {
        return Err(ApiError::bad_request(format!(
            "command too long ({} > {})",
            cmd.len(),
            MAX_CMD_LEN
        )));
    }
    let result = diagnostics.exec_command(&cmd).await;

    // 审计：命令执行属于高权限操作，成功/失败都留痕（detail 截断到 200 字符，
    // 避免超长命令把审计文件撑大；完整命令仍可从网关日志追溯）。
    let detail: String = cmd.chars().take(200).collect();
    state.platform.audit.log(
        "diagnostics.exec",
        "gateway",
        &format!(
            "cmd={} status={}",
            detail,
            if result.is_ok() { "ok" } else { "failed" }
        ),
        "api",
    );

    match result {
        Ok(output) => Ok(Json(serde_json::json!({
            "status": "ok",
            "output": output,
        }))),
        Err(e) => Err(ApiError::bad_request(e.message())),
    }
}

/// 默认构建（未开启 `remote-shell`）下的降级实现：保留路由与鉴权，直接返回明确的未启用错误。
#[cfg(not(feature = "remote-shell"))]
pub(crate) async fn diagnostics_exec(
    State(_state): State<Arc<AppState>>,
    Json(_req): Json<ExecRequest>,
) -> Result<axum::http::StatusCode, ApiError> {
    Err(ApiError::bad_request(
        "remote shell is disabled in this build (rebuild with --features remote-shell)".to_string(),
    ))
}

pub(crate) async fn realtime_ws(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    uri: axum::http::Uri,
    ws: axum::extract::ws::WebSocketUpgrade,
) -> axum::response::Response {
    use crate::web::error::ApiError;
    use crate::web::middleware::{
        allow_anon_read, api_token, extract_token_from_headers, extract_token_from_ws_protocol,
        extract_token_from_ws_query, token_matches,
    };
    use axum::extract::ws::Message;
    use axum::response::IntoResponse;
    use futures::StreamExt;

    // A6: WebSocket 升级前校验 token，避免未授权客户端订阅实时数据流。
    // 实时数据可能含敏感计量读数，必须鉴权。
    // B1 修复：token 提取顺序为 Header > Sec-WebSocket-Protocol 子协议 > URL query。
    //   - Header 路径：Tauri / 自定义客户端走 Authorization: Bearer。
    //   - 子协议路径：浏览器 WebSocket 走 `bearer.<token>` 子协议（**不再落 URL 日志**）。
    //   - query 路径：仅在 `allow_anon_read=true` 时接受（向后兼容旧客户端）。
    //
    // 实时数据流（与日志流不同）即使 allow_anon_read=true 仍要求 token——
    // 实时数据是最高敏感级别，不接受匿名读。
    //
    // Bug T5: 严格模式（allow_anon_read=false）下不得再接受 URL query 形式的 token——
    // query string 会落入反向代理 / 网关的 access log，导致 token 泄露。仅在匿名读
    // 模式保留 query 路径以兼容旧客户端；严格模式一律拒绝 query token（客户端应改用
    // Authorization 头或 `bearer.<token>` 子协议）。
    let anon = allow_anon_read(&state);
    let ws_proto_token = extract_token_from_ws_protocol(&headers);
    let provided_token = extract_token_from_headers(&headers)
        .or_else(|| ws_proto_token.clone())
        .or_else(|| {
            if anon {
                extract_token_from_ws_query(&uri)
            } else {
                None
            }
        });

    // P2 修复：原条件 `if !anon || provided_token.is_some()` 在 anon=true 且未带 token 时
    // `!anon` 为 false、`provided_token.is_some()` 为 false，整段鉴权被跳过——与上方注释
    // （实时流即使 allow_anon_read=true 仍要求 token）直接矛盾，任何匿名客户端可直接订阅
    // 实时数据流（最高敏感级别）。注意路由层 `require_auth_if_anon_disabled` 在匿名模式下
    // 会直接放行，handler 内的这段校验才是实时流真正的闸门，不能依赖外层中间件。
    // 改为无条件校验：allow_anon_read 只放宽"取 token 的位置"（额外接受 query 形式），
    // 不放宽"是否需要 token"，故实时流一律要求有效 token。
    match provided_token {
        Some(token) if token_matches(&token, &api_token(&state)) => {}
        Some(_) => return ApiError::unauthorized("invalid api token").into_response(),
        None => return ApiError::unauthorized(
            "websocket authentication required; pass Authorization: Bearer <token> header or `bearer.<token>` Sec-WebSocket-Protocol subprotocol"
        ).into_response(),
    }

    // B1-2 修复：同 diagnostics_logs_ws——浏览器要求 101 响应回选其提供的子协议，
    // 否则判定握手失败（实时数据页永远「重连中」）。仅子协议 token 有效时回选。
    let ws = match ws_proto_token.filter(|t| token_matches(t, &api_token(&state))) {
        Some(t) => ws.protocols([format!("bearer.{t}")]),
        None => ws,
    };

    let realtime_bus = state.south.realtime_bus.clone();

    ws.on_upgrade(move |mut socket| async move {
        // #9: 一次性长订阅。旧实现每轮 select! 都调用 realtime_bus.latest()
        // （内部 subscribe + 等 1ms + 丢弃），会导致 (1) socket.send 期间发布的批次
        // 丢失；(2) 每个连接约 1000 次/秒地创建/销毁订阅者，空转消耗嵌入式网关 CPU。
        // 改为在循环外订阅一次，循环内直接 rx.recv() 并处理 Lagged。
        let mut rx = realtime_bus.subscribe();
        let mut subscribed: Option<std::collections::HashSet<String>> = None;
        let mut last_pong = tokio::time::Instant::now();
        let mut ping_ticker = tokio::time::interval(WS_PING_INTERVAL);
        ping_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut idle_ticker = tokio::time::interval(WS_IDLE_TIMEOUT);
        idle_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                data = rx.recv() => {
                    match data {
                        Ok(points) => {
                            let filtered: Vec<_> = match &subscribed {
                                Some(set) => points.iter().filter(|d| set.contains(&d.point_id)).cloned().collect(),
                                None => points.as_ref().clone(),
                            };
                            if !filtered.is_empty() {
                                if let Ok(text) = serde_json::to_string(&filtered) {
                                    if socket.send(Message::Text(text)).await.is_err() {
                                        break;
                                    }
                                }
                            }
                        }
                        // #9: 消费慢于生产导致广播溢出——跳过丢失的批次并告警，
                        // 与"静默少点"区分开。
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("realtime ws lagged, dropped {} batches", n);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = ping_ticker.tick() => {
                    if socket.send(Message::Ping(Vec::new())).await.is_err() {
                        break;
                    }
                }
                _ = idle_ticker.tick() => {
                    if last_pong.elapsed() >= WS_IDLE_TIMEOUT {
                        tracing::debug!("realtime ws idle timeout (no pong for {:?}), closing", WS_IDLE_TIMEOUT);
                        let _ = socket.send(Message::Close(None)).await;
                        break;
                    }
                }
                msg = socket.next() => {
                    match msg {
                        Some(Ok(Message::Ping(payload))) => {
                            if socket.send(Message::Pong(payload)).await.is_err() {
                                break;
                            }
                        }
                        Some(Ok(Message::Pong(_))) => {
                            last_pong = tokio::time::Instant::now();
                        }
                        Some(Ok(Message::Close(_))) | None => break,
                        Some(Ok(Message::Text(text))) => {
                            if let Ok(cmd) = serde_json::from_str::<serde_json::Value>(&text) {
                                match cmd.get("action").and_then(|a| a.as_str()) {
                                    Some("subscribe") => {
                                        if let Some(arr) = cmd.get("points").and_then(|p| p.as_array()) {
                                            const MAX_POINTS: usize = 10_000;
                                            const MAX_ID_LEN: usize = 256;
                                            let mut set: std::collections::HashSet<String> =
                                                std::collections::HashSet::with_capacity(arr.len().min(MAX_POINTS));
                                            for v in arr.iter().take(MAX_POINTS) {
                                                if let Some(s) = v.as_str() {
                                                    if s.len() <= MAX_ID_LEN {
                                                        set.insert(s.to_string());
                                                    }
                                                }
                                            }
                                            let size = set.len();
                                            subscribed = Some(set);
                                            tracing::debug!("ws subscribed to {} points", size);
                                        }
                                    }
                                    Some("unsubscribe") => {
                                        subscribed = None;
                                        tracing::debug!("ws unsubscribed, full stream resumed");
                                    }
                                    _ => {}
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }).into_response()
}
