//! 网关健康检测与 webhook 转发
//!
//! 职责：
//! 1. 启动时检测激活网关 /health/live 可达性（避免登录后才发现连不上）
//! 2. 切网关前探测任意 URL 可达性
//! 3. webhook 转发：Rust 端发起 POST，避免 webview 直连外部 HTTP（CSP 收紧后必需）

use std::sync::Arc;

use serde::Serialize;

use crate::state::AppState;

/// opt-6: 激活网关健康状态（启动检测用）
#[derive(Debug, Serialize)]
pub struct GatewayHealth {
    /// 网关 /health/live 是否可达
    pub reachable: bool,
    /// 探测延迟（毫秒）
    pub latency_ms: u64,
    /// 失败原因（可达时为 None）
    pub error: Option<String>,
}

/// opt-6: 启动时检测激活网关健康状态。
///
/// 前端 bootstrap 后调用：若不可达，提示用户检查地址或扫描局域网发现可达网关，
/// 避免"登录后才发现网关连不上"的死锁体验。复用 AppState.http（带 TLS 跳过 + 重试预算）。
#[tauri::command]
pub async fn check_gateway_health(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<GatewayHealth, String> {
    let cfg = state.config.read().await;
    let active_name = cfg.active.clone();
    let gateway = active_name
        .as_ref()
        .and_then(|name| cfg.gateways.iter().find(|g| g.name == *name).cloned());
    drop(cfg);

    let gateway = match gateway {
        Some(g) => g,
        None => {
            return Ok(GatewayHealth {
                reachable: false,
                latency_ms: 0,
                error: Some("未配置激活网关".to_string()),
            })
        }
    };
    let base = gateway.url.trim_end_matches('/');
    let start = std::time::Instant::now();
    match state.http.get(format!("{}/health/live", base)).send().await {
        Ok(r) if r.status().is_success() => Ok(GatewayHealth {
            reachable: true,
            latency_ms: start.elapsed().as_millis() as u64,
            error: None,
        }),
        Ok(r) => Ok(GatewayHealth {
            reachable: false,
            latency_ms: start.elapsed().as_millis() as u64,
            error: Some(format!("网关返回状态码 {}", r.status())),
        }),
        Err(e) => {
            let kind = if e.is_connect() {
                "连接失败"
            } else if e.is_timeout() {
                "超时"
            } else {
                "请求失败"
            };
            Ok(GatewayHealth {
                reachable: false,
                latency_ms: start.elapsed().as_millis() as u64,
                error: Some(format!("{}: {}", kind, e)),
            })
        }
    }
}

/// 检测任意 URL 的健康状态（切网关前探测目标可达性）。
/// 与 check_gateway_health 区别：后者检测已激活网关，本函数接受任意 URL。
/// 复用 AppState.http（带 TLS 跳过 + 重试），避免 webview 直连 HTTP
#[tauri::command]
pub async fn check_url_health(
    state: tauri::State<'_, Arc<AppState>>,
    url: String,
) -> Result<GatewayHealth, String> {
    let base = url.trim_end_matches('/');
    let start = std::time::Instant::now();
    match state.http.get(format!("{}/health/live", base)).send().await {
        Ok(r) if r.status().is_success() => Ok(GatewayHealth {
            reachable: true,
            latency_ms: start.elapsed().as_millis() as u64,
            error: None,
        }),
        Ok(r) => Ok(GatewayHealth {
            reachable: false,
            latency_ms: start.elapsed().as_millis() as u64,
            error: Some(format!("状态码 {}", r.status())),
        }),
        Err(e) => Ok(GatewayHealth {
            reachable: false,
            latency_ms: start.elapsed().as_millis() as u64,
            error: Some(format!("{}", e)),
        }),
    }
}

/// 转发 webhook POST 请求到用户配置的外部 URL。
/// 避免 webview 直连外部 HTTP（CSP 收紧后必需），由 Rust 端发出请求
///
/// Bug-D2 (fix)：原实现直接 `state.http.post(&url).send()` 有两个严重问题：
/// 1. **零 URL 校验**——攻击者绕过前端直接 `invoke('forward_webhook', { url: ... })`
///    可打到 `http://169.254.169.254/...` 云元数据端点、`file:///etc/passwd`、
///    `http://192.168.1.1/admin` 内网管理页等。
/// 2. **TLS 跳过**——`state.http` 是 `danger_accept_invalid_certs(true)`，攻击者
///    注册钉子证书即可 MITM webhook 流量（修改告警接收内容 / 注入恶意 payload）。
///
/// 修复：
/// 1. 入口处调 `validate_outbound_url`（与前端 urlSafety.ts 语义对齐），
///    拦截 localhost / 私有 IP / 元数据端点 / 非 http(s) scheme。
/// 2. 使用独立的 `state.webhook_http` 客户端（**强制 TLS 校验**，不跳证书），
///    webhook 接收方是公网（Slack / Lark / 自建 webhook），不存在工业自签证书场景。
#[tauri::command]
pub async fn forward_webhook(
    state: tauri::State<'_, Arc<AppState>>,
    url: String,
    body: String,
) -> Result<(), String> {
    // Bug-D2: SSRF 黑名单拦截
    let opts = crate::url_safety::UrlSafetyOptions::default();
    if let Err(e) = crate::url_safety::validate_outbound_url(&url, &opts) {
        tracing::warn!("forward_webhook blocked: url={} reason={:?}", url, e);
        return Err(format!("webhook url rejected by security policy: {:?}", e));
    }

    // Bug-D2: 使用专用的 webhook_http 客户端（强制 TLS 校验）
    state
        .webhook_http
        .post(&url)
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| format!("webhook 发送失败: {}", e))?;
    Ok(())
}
