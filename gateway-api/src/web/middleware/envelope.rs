//! P1-1：统一响应信封（router 层兜底）+ 提取器纯文本错误规整。

use axum::response::{IntoResponse, Response};

use crate::web::error::ApiError;

/// 明确豁免「统一信封」的路径清单（**后端唯一维护点**）。
///
/// 前端 `gateway-web/src/api/http.ts` 的 `BARE_RESPONSE_PATHS` 与之一一对应，
/// 新增/删除豁免项必须两处同步。
///
/// 为什么这些端点不能包信封：
/// - `/health/live`、`/health/ready`：K8s / 负载均衡探针按裸 JSON 读取，
///   外面再包一层会让探针拿不到 `status` 字段（就绪探针直接失效）；
/// - `/api/metrics`：Prometheus 抓取端点返回 `text/plain` 指标，
///   包成 JSON 抓取端会解析失败；
/// - `/api/history/export`：CSV 文件下载，前端按 blob 消费；
/// - `/api/auth/login`：前端 token 契约是裸 `{"token": "..."}`（见 `api/auth.ts`），
///   包信封会破坏登录链路；
/// - `/api/diagnostics/logs/download`：日志文本/附件下载。
fn is_envelope_exempt(path: &str) -> bool {
    if path == "/health" || path.starts_with("/health/") {
        return true;
    }
    matches!(
        path,
        "/api/metrics"
            | "/api/history/export"
            | "/api/auth/login"
            | "/api/diagnostics/logs/download"
    )
}

/// 统一响应信封中间件（P1-1）。
///
/// 全站 24 个 JSON 端点里原本有 13 个直接返回裸对象，前端只能靠
/// 「猜 body 里有没有 code/data/message」解包，一旦某个 handler 漏改就静默失配。
/// 这里在 router 层兜底：凡是 **2xx + application/json + 尚未是信封** 的响应，
/// 统一包成 `{"code":0,"message":"success","data":<原 body>}`，
/// 新增 handler 忘记写 `ApiResponse` 也不会再破坏契约，无需逐个 handler 改造。
///
/// 刻意**不**包装的情况：
/// - 非 2xx：错误响应走 `ApiError` / `ApiResponse::err`，前端错误分支按
///   `{error|message}` 读取，再包一层会把真实错误码掩盖成 `code:0`；
/// - 非 JSON：SSE（`text/event-stream`）、Prometheus 文本、CSV、二进制下载；
/// - 豁免路径（见 [`is_envelope_exempt`]）；
/// - 已是信封的响应（幂等，避免双层 `data.data`）。
///
/// 该中间件必须挂在 `CompressionLayer` **内层**：先在未压缩的原始 body 上做
/// JSON 解析与包装，再由外层压缩，否则需要先解压才能识别信封。
pub(crate) async fn envelope(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    use axum::body::Body;
    use axum::http::header::CONTENT_TYPE;

    let path = req.uri().path().to_string();
    let resp = next.run(req).await;

    if is_envelope_exempt(&path) {
        return resp;
    }
    // BUG-C/D：axum 的 `Json` / `Query` 提取器在解析失败时直接返回 400/415/422 +
    // 纯文本 body（如 "Failed to deserialize query string: ..."），不经过
    // `ApiError`，前端拿到的是裸字符串，既读不到 message 也没有业务码。
    // 这里只对 **text/plain 的 4xx** 补一次信封（JSON 错误响应本就是信封形态，
    // 归 ApiError 负责，不要重复包装）。
    if !resp.status().is_success() {
        return normalize_plain_error(resp).await;
    }
    let is_json = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if !is_json {
        return resp;
    }

    // 缓冲上限 64 MiB：网关最大的 JSON 响应是历史查询（默认 1000 行），远低于此。
    // 超限时无法回退原始 body，按 500 明确失败，而不是原样透出一个未包装响应。
    const MAX_ENVELOPE_BYTES: usize = 64 * 1024 * 1024;
    let (mut parts, body) = resp.into_parts();
    let bytes = match axum::body::to_bytes(body, MAX_ENVELOPE_BYTES).await {
        Ok(b) => b,
        Err(_) => {
            return ApiError::internal("response body exceeds envelope buffer limit")
                .into_response();
        }
    };

    // 非 JSON 对象（数组 / 标量 / 空 body / 非法 JSON）也要包：前端只按 data 读取。
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    let already_envelope = value.get("code").is_some_and(|c| c.is_number())
        && value.get("message").is_some_and(|m| m.is_string())
        && value.get("data").is_some();
    if already_envelope {
        return Response::from_parts(parts, Body::from(bytes));
    }

    let wrapped = serde_json::json!({
        "code": 0,
        "message": "success",
        "data": value,
    });
    let Ok(out) = serde_json::to_vec(&wrapped) else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    // body 长度已变，旧的 Content-Length 必须替换（外层 CompressionLayer 若压缩会自行改写）
    parts.headers.remove(axum::http::header::CONTENT_LENGTH);
    if let Ok(v) = axum::http::HeaderValue::from_str(&out.len().to_string()) {
        parts.headers.insert(axum::http::header::CONTENT_LENGTH, v);
    }
    Response::from_parts(parts, Body::from(out))
}

/// 把提取器拒绝（text/plain 4xx）规整成统一信封。
///
/// 语义：HTTP 状态码不变（前端仍按 4xx 走错误分支），body 改为
/// `{"code": <业务码>, "message": <原文>, "error": <原文>}`，与 `ApiError` 一致。
/// 业务码按状态码映射，便于前端/日志区分失败类型：
/// 400 → 4000（参数错误）、415 → 4150（Content-Type 不支持）、422 → 4220（结构不合法）。
/// 非 text/plain（如 handler 已返回 `ApiError` 的 JSON、无 body 的 405）原样返回。
async fn normalize_plain_error(resp: Response) -> Response {
    use axum::body::Body;
    use axum::http::header::CONTENT_TYPE;

    let is_plain = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/plain"));
    if !is_plain {
        return resp;
    }

    const MAX_ERROR_BYTES: usize = 1024 * 1024;
    let status = resp.status();
    let (mut parts, body) = resp.into_parts();
    let bytes = match axum::body::to_bytes(body, MAX_ERROR_BYTES).await {
        Ok(b) => b,
        // 读不出来就按 500 失败，避免把未知 body 原样透出
        Err(_) => return ApiError::internal("error body exceeds buffer limit").into_response(),
    };
    let raw = String::from_utf8_lossy(&bytes).trim().to_string();
    let message = if raw.is_empty() {
        status
            .canonical_reason()
            .unwrap_or("request failed")
            .to_string()
    } else {
        // 去掉 axum 的英文框架前缀，只留「哪个字段错、错在哪」这部分
        // （前端直接把 message 展示给用户，保留前缀等于把实现细节漏给现场）
        const PREFIXES: [&str; 4] = [
            "Failed to deserialize query string: ",
            "Failed to deserialize the JSON body into the target type: ",
            "Failed to parse the request body as JSON: ",
            "Failed to deserialize the JSON body: ",
        ];
        let mut msg = raw.as_str();
        for p in PREFIXES {
            if let Some(rest) = msg.strip_prefix(p) {
                msg = rest.trim();
                break;
            }
        }
        msg.to_string()
    };
    let code = match status.as_u16() {
        415 => 4150,
        422 => 4220,
        _ => 4000,
    };
    let out = serde_json::json!({
        "code": code,
        "message": message,
        "error": message,
    });
    let Ok(out) = serde_json::to_vec(&out) else {
        return ApiError::internal("serialize error envelope failed").into_response();
    };
    parts.headers.remove(axum::http::header::CONTENT_LENGTH);
    parts.headers.insert(
        CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/json"),
    );
    if let Ok(v) = axum::http::HeaderValue::from_str(&out.len().to_string()) {
        parts.headers.insert(axum::http::header::CONTENT_LENGTH, v);
    }
    Response::from_parts(parts, Body::from(out))
}
