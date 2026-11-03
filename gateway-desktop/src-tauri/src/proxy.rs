//! 网关 HTTP 代理
//!
//! 职责：前端通过 invoke('proxy_request') 调用本模块，由 Rust 端发起
//! HTTP 请求到激活的网关，绕开 webview 的 CORS 限制。错误经结构化
//! ProxyError 返回，前端可凭 code 做差异化处理。

use std::collections::HashMap;
use std::sync::Arc;

use crate::state::AppState;

/// Q6: 结构化代理错误，让前端可按 code 区分网络/超时/状态码/响应过大等场景。
///
/// 成功路径仍返回 [`ProxyResponse`]；所有错误路径统一返回 `ProxyError`，
/// 前端可凭 `code` 做差异化处理（弹认证框 / 重试 / 提示超大 / 走导出通道）。
#[derive(Debug, serde::Serialize)]
pub struct ProxyError {
    /// 错误码：
    /// - `TIMEOUT` 请求超时
    /// - `NETWORK` 连接/网络错误（含 DNS、流式中断等）
    /// - `HTTP_STATUS` 非 2xx 响应（`status` 字段有值）
    /// - `BODY_TOO_LARGE` 响应体超 16 MiB 上限
    /// - `BUILD_CLIENT` HTTP 客户端构建失败（当前在 AppState::load 兜底处理，proxy_request 不会触发）
    /// - `DESERIALIZE` 响应体反序列化失败（当前按文本读取，不会触发）
    /// - `NO_ACTIVE_GATEWAY` 未选择/找不到激活网关
    /// - `UNSUPPORTED_METHOD` 不支持的 HTTP 方法
    pub code: String,
    /// 人类可读错误描述
    pub message: String,
    /// HTTP 状态码（仅 `HTTP_STATUS` 类错误有值）
    pub status: Option<u16>,
    /// 错误来源：底层错误链 to_string()
    pub source: String,
    /// HTTP_STATUS 错误的响应体（前端可读取后端结构化错误详情，限 64 KiB）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct ProxyResponse {
    pub status: u16,
    pub body: String,
}

/// 代理 HTTP 请求到当前激活的网关
/// method: GET/POST/PUT/DELETE
/// path: 相对路径，如 /api/config
/// body: 请求体（可选）
/// headers: 额外请求头
#[tauri::command]
pub async fn proxy_request(
    state: tauri::State<'_, Arc<AppState>>,
    method: String,
    path: String,
    body: Option<serde_json::Value>,
    headers: Option<HashMap<String, String>>,
) -> Result<ProxyResponse, ProxyError> {
    let cfg = state.config.read().await;
    let active_name = cfg.active.clone().ok_or_else(|| ProxyError {
        code: "NO_ACTIVE_GATEWAY".to_string(),
        message: "未选择激活的网关".to_string(),
        status: None,
        source: String::new(),

        body: None,
    })?;
    let gateway = cfg
        .gateways
        .iter()
        .find(|g| g.name == active_name)
        .ok_or_else(|| ProxyError {
            code: "NO_ACTIVE_GATEWAY".to_string(),
            message: format!("激活的网关 '{}' 不存在", active_name),
            status: None,
            source: String::new(),

            body: None,
        })?;

    let base = gateway.url.trim_end_matches('/').to_string();
    let url = format!("{}{}", base, path);

    let client = &state.http;
    let method_upper = method.to_uppercase();
    // opt-5: 对幂等方法（GET/HEAD）的连接/超时失败重试，工业现场网关可能瞬时抖动。
    // 非幂等方法（POST/PUT/DELETE）不重试，避免重复写入导致配置/数据重复。
    let is_idempotent = matches!(method_upper.as_str(), "GET" | "HEAD");
    let max_attempts: usize = if is_idempotent { 3 } else { 1 };

    let mut resp_opt: Option<reqwest::Response> = None;
    // 保留最后一次 reqwest::Error 用于重试耗尽时归类（TIMEOUT / NETWORK）
    let mut last_err: Option<reqwest::Error> = None;
    for attempt in 0..max_attempts {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(200 * attempt as u64)).await;
        }
        let mut req = match method_upper.as_str() {
            "GET" => client.get(&url),
            "POST" => client.post(&url),
            "PUT" => client.put(&url),
            "DELETE" => client.delete(&url),
            other => {
                return Err(ProxyError {
                    code: "UNSUPPORTED_METHOD".to_string(),
                    message: format!("不支持的方法: {}", other),
                    status: None,
                    source: String::new(),

                    body: None,
                })
            }
        };
        // 借用 headers/body（重试需重建请求，不能 move）
        if let Some(hdrs) = &headers {
            for (k, v) in hdrs {
                req = req.header(k, v);
            }
        }
        if let Some(b) = &body {
            req = req.json(b);
        }
        match req.send().await {
            Ok(r) => {
                resp_opt = Some(r);
                break;
            }
            Err(e) if is_idempotent && (e.is_connect() || e.is_timeout()) => {
                tracing::warn!(
                    "proxy_request {} {} 失败，重试 {}/{}: {}",
                    method_upper,
                    path,
                    attempt + 1,
                    max_attempts,
                    e
                );
                last_err = Some(e);
                continue;
            }
            Err(e) => return Err(classify_reqwest_error(&e)),
        }
    }
    let resp = match resp_opt {
        Some(r) => r,
        None => {
            // 仅幂等重试耗尽时走到此分支：last_err 必为 connect/timeout
            return Err(last_err
                .map(|e| {
                    let mut err = classify_reqwest_error(&e);
                    err.message = format!(
                        "请求网关失败（重试 {} 次耗尽）: {}",
                        max_attempts.saturating_sub(1),
                        err.message
                    );
                    err
                })
                .unwrap_or_else(|| ProxyError {
                    code: "NETWORK".to_string(),
                    message: format!(
                        "请求网关失败（重试 {} 次耗尽）",
                        max_attempts.saturating_sub(1)
                    ),
                    status: None,
                    source: String::new(),

                    body: None,
                }));
        }
    };

    let status = resp.status().as_u16();

    // Q6: 非 2xx 返回结构化 HTTP_STATUS 错误，携带响应体供前端读取后端错误详情
    if !resp.status().is_success() {
        // 读取错误响应体（限 64 KiB），供前端读取后端结构化错误详情
        let err_bytes = resp.bytes().await.unwrap_or_default();
        const MAX_ERR_BODY: usize = 64 * 1024;
        let err_body = if err_bytes.len() > MAX_ERR_BODY {
            String::from_utf8_lossy(&err_bytes[..MAX_ERR_BODY]).into_owned()
        } else {
            String::from_utf8_lossy(&err_bytes).into_owned()
        };
        return Err(ProxyError {
            code: "HTTP_STATUS".to_string(),
            message: format!("网关返回非成功状态码 {}", status),
            status: Some(status),
            source: String::new(),
            body: Some(err_body),
        });
    }

    // F6: 响应体大小上限可配置（AppState.max_response_bytes，默认 64 KiB，
    // 通过 GATEWAY_MAX_RESPONSE_BYTES 环境变量设置）。超限返回 BODY_TOO_LARGE
    // 错误，引导前端走专门的导出通道（流式 / chunked）。
    let max_response_bytes = state.max_response_bytes;
    let mut total_read: usize = 0;
    let mut text = String::new();
    use futures_util::StreamExt;
    let mut stream = resp.bytes_stream();
    while let Some(chunk_res) = stream.next().await {
        // 流式读取阶段的中断本质是网络错误，归类为 TIMEOUT / NETWORK
        let chunk = chunk_res.map_err(|e| classify_reqwest_error(&e))?;
        total_read = total_read.saturating_add(chunk.len());
        if total_read > max_response_bytes {
            return Err(ProxyError {
                code: "BODY_TOO_LARGE".to_string(),
                message: format!(
                    "响应体超过 {} 字节上限，请走专门的导出通道",
                    max_response_bytes
                ),
                status: None,
                source: String::new(),

                body: None,
            });
        }
        text.push_str(&String::from_utf8_lossy(&chunk));
    }

    Ok(ProxyResponse { status, body: text })
}

/// 收集 `std::error::Error` 的 cause 链为 "top: cause1: cause2" 字符串
fn error_chain(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut cur = e.source();
    while let Some(cause) = cur {
        s.push_str(": ");
        s.push_str(&cause.to_string());
        cur = cause.source();
    }
    s
}

/// 将 reqwest::Error 归类为 [`ProxyError`]：
/// 超时 → `TIMEOUT`；其余 send 失败（连接/DNS/请求体发送/流式中断）→ `NETWORK`
fn classify_reqwest_error(e: &reqwest::Error) -> ProxyError {
    let code = if e.is_timeout() { "TIMEOUT" } else { "NETWORK" };
    ProxyError {
        code: code.to_string(),
        message: e.to_string(),
        status: None,
        source: error_chain(e),

        body: None,
    }
}
