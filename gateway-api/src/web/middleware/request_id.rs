//! 请求 ID（trace ID）中间件：生成 / 校验 / 回写响应头。

use axum::response::Response;

/// 请求 ID 响应头名。与 gateway-web 的 `api/http.ts`、运维排障脚本约定一致。
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// 进程内自增序号：与毫秒时间戳组合，保证同一毫秒内的并发请求也不撞号。
static REQUEST_ID_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// 生成请求 ID：13 位十六进制毫秒时间戳 + 8 位十六进制序号。
///
/// 不引入 uuid 依赖（workspace 未声明），时间戳前缀让 ID 天然可按时间排序，
/// 现场把 ID 贴给研发后可直接定位到对应时间段的日志。
pub fn next_request_id() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let seq = REQUEST_ID_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{:013x}{:08x}", millis, seq)
}

/// 校验调用方透传的 ID：只接受 ASCII 字母/数字与 `-_.`，长度 8..=128。
///
/// 透传可让前端为一次用户操作预生成 ID 并在多请求间串联，但直接回写响应头
/// 会带来响应头注入风险（换行等控制字符），故严格白名单校验，不合法即重新生成。
pub fn sanitize_incoming_request_id(raw: &str) -> Option<String> {
    let ok = (8..=128).contains(&raw.len())
        && raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.');
    if ok {
        Some(raw.to_string())
    } else {
        None
    }
}

/// 请求 ID 中间件：为每个请求确定 trace ID，写回响应头 `x-request-id`，
/// 并把 ID 挂进 request extensions（handler 可通过 `RequestId` 读取）。
///
/// 为什么放在最外层：即使请求被限速中间件拒绝、或 handler panic 被
/// CatchPanicLayer 兜住，响应仍带 ID，用户报障时“一个码”就能对上日志。
pub(crate) async fn request_id(
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let rid = req
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(sanitize_incoming_request_id)
        .unwrap_or_else(next_request_id);

    let method = req.method().clone();
    let path = req.uri().path().to_string();
    req.extensions_mut().insert(RequestId(rid.clone()));

    let started = std::time::Instant::now();
    let mut response = next.run(req).await;
    let elapsed_ms = started.elapsed().as_millis();

    if let Ok(value) = axum::http::HeaderValue::from_str(&rid) {
        response.headers_mut().insert(
            axum::http::HeaderName::from_static(REQUEST_ID_HEADER),
            value,
        );
    }

    let status = response.status();
    // 5xx 用 error 级别：抓日志时按级别过滤即可捞出所有需要研发介入的请求。
    // 其余按 debug，避免正常轮询刷爆 info 级日志。
    if status.is_server_error() {
        tracing::error!(
            request_id = %rid, method = %method, path = %path,
            status = status.as_u16(), elapsed_ms, "http 5xx"
        );
    } else {
        tracing::debug!(
            request_id = %rid, method = %method, path = %path,
            status = status.as_u16(), elapsed_ms, "http done"
        );
    }

    response
}

/// 挂在请求 extensions 上的请求 ID，handler 需要回写进业务响应体时读取。
#[derive(Debug, Clone)]
pub struct RequestId(pub String);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::error::ApiError;
    use axum::body::Body;
    use axum::response::IntoResponse;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    fn app() -> Router {
        let inner = Router::new().route("/ok", get(|| async { "ok" })).route(
            "/boom",
            get(|| async { ApiError::internal("boom").into_response() }),
        );
        inner.layer(axum::middleware::from_fn(request_id))
    }

    #[tokio::test]
    async fn generates_id_when_header_absent() {
        let resp = app()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/ok")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let rid = resp
            .headers()
            .get(REQUEST_ID_HEADER)
            .expect("每个响应都必须带 x-request-id")
            .to_str()
            .unwrap();
        assert_eq!(rid.len(), 21, "13 位毫秒 + 8 位序号");
        assert!(rid.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn echoes_valid_incoming_id() {
        let resp = app()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/ok")
                    .header(REQUEST_ID_HEADER, "web-abc12345")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.headers().get(REQUEST_ID_HEADER).unwrap(),
            "web-abc12345"
        );
    }

    #[tokio::test]
    async fn rejects_malformed_or_too_short_incoming_id() {
        for bad in ["", "short", "has space", "semi;colon"] {
            let resp = app()
                .oneshot(
                    axum::http::Request::builder()
                        .uri("/ok")
                        .header(REQUEST_ID_HEADER, bad)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let rid = resp
                .headers()
                .get(REQUEST_ID_HEADER)
                .unwrap()
                .to_str()
                .unwrap();
            assert_ne!(rid, bad, "非法透传 ID 必须被替换");
            assert_eq!(rid.len(), 21);
        }
    }

    #[tokio::test]
    async fn header_present_on_error_response() {
        let resp = app()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/boom")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        assert!(resp.headers().get(REQUEST_ID_HEADER).is_some());
    }

    #[test]
    fn incoming_id_allowlist() {
        assert!(sanitize_incoming_request_id("abcdefgh").is_some());
        assert!(sanitize_incoming_request_id("abc.def-ghi_jkl").is_some());
        assert!(sanitize_incoming_request_id("abc\ndef").is_none());
        assert!(sanitize_incoming_request_id(&"a".repeat(129)).is_none());
        assert!(sanitize_incoming_request_id("short").is_none());
    }
}
