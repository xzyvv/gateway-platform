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

/// 事件通道容量（DD-25）：有界队列，前端消费不过来时由生产者 `await` 背压，
/// 不再像 `unbounded` 那样把内存吃到不可控。
pub const SSE_CHANNEL_CAPACITY: usize = 256;

/// 调试台 SSE 统一使用有界通道；生产者必须 `send().await`（背压）而不是丢弃。
pub fn sse_channel<T>() -> (mpsc::Sender<T>, mpsc::Receiver<T>) {
    mpsc::channel(SSE_CHANNEL_CAPACITY)
}

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
pub fn sse_response<T: IntoSseEvent>(rx: mpsc::Receiver<T>) -> Response {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// DD-25：通道必须有界；写满后 `try_send` 返回 Full，生产者只能背压等待。
    ///
    /// futures 的 `channel(n)` 容量为 `n + 发送端个数`（每端额外一个槽位），
    /// 因此这里断言「写入次数不超过 capacity + 1」且最终一定触达 Full。
    #[test]
    fn sse_channel_is_bounded() {
        let (mut tx, _rx) = sse_channel::<u8>();
        let mut sent = 0usize;
        loop {
            match tx.try_send(0) {
                Ok(()) => {
                    sent += 1;
                    assert!(
                        sent <= SSE_CHANNEL_CAPACITY + 1,
                        "通道必须有界，写入次数 {sent} 已超过容量上限"
                    );
                }
                Err(err) if err.is_full() => break,
                Err(_) => panic!("非容量原因导致写入失败"),
            }
        }
        assert!(
            sent >= SSE_CHANNEL_CAPACITY,
            "容量内必须可写，实际只写入 {sent} 条"
        );
    }
}
