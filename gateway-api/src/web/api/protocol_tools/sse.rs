//! SSE 流式响应共享件（p0-1b 回放 / p0-2 从站扫描 共用）
//!
//! 内容协商与响应组装同一套语义：
//! - `Accept` 含 `text/event-stream` → 流式；否则调用方走阻塞 JSON（契约 p0-1b §2 / p0-2 §2）；
//! - 响应头补 `X-Accel-Buffering: no` 防中间代理缓冲；5s 心跳注释行防 60s 空闲超时断连。

use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures::{channel::mpsc, StreamExt};
use std::convert::Infallible;

/// SSE 心跳间隔（契约 §3.6）：axum 实际产出 `: hb` 注释行
pub const SSE_KEEPALIVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// 可转为 SSE 事件的事件流元素（回放 `ReplayEvent` / 扫描 `ScanEvent` 各自实现）。
/// `Send` 约束：Sse 响应要求流本身可跨线程（axum 0.7 `IntoResponse` 的隐含边界）。
pub trait IntoSseEvent: Send + 'static {
    fn into_sse(self) -> Event;
}

/// Accept 头内容协商：含 text/event-stream 即走流式模式
pub fn accepts_sse(headers: &HeaderMap) -> bool {
    headers
        .get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("text/event-stream"))
        .unwrap_or(false)
}

/// 组装 SSE 响应：Content-Type/Cache-Control 由 Sse 自带，
/// 补 X-Accel-Buffering: no 防 nginx 等中间代理缓冲（契约 §3）
pub fn sse_response<T: IntoSseEvent>(rx: mpsc::UnboundedReceiver<T>) -> Response {
    let stream = rx.map(|ev| Ok::<_, Infallible>(ev.into_sse()));
    let sse =
        Sse::new(stream).keep_alive(KeepAlive::new().interval(SSE_KEEPALIVE_INTERVAL).text("hb"));
    let mut resp = sse.into_response();
    resp.headers_mut().insert(
        axum::http::HeaderName::from_static("x-accel-buffering"),
        axum::http::HeaderValue::from_static("no"),
    );
    resp
}
