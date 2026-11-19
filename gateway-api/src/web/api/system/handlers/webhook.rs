//! Webhook 测试 API
//!
//! 提供 webhook 连通性测试：向指定 URL 发送 HTTP 请求并返回响应详情。
//!
//! 路由：
//! - POST /api/webhook/test -> test_webhook

use super::*;
use gateway_core::shared::url_safety as core_url_safety;
use std::time::Instant;

/// Webhook 测试请求
#[derive(serde::Deserialize)]
pub struct WebhookTestRequest {
    pub url: String,
    #[serde(default = "default_method")]
    pub method: String,
    #[serde(default)]
    pub headers: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub body: String,
}

fn default_method() -> String {
    "POST".to_string()
}

/// 响应体读取上限：超出即截断并断开连接。
///
/// 修复（响应体无界）：原实现直接 `resp.text()`——对端返回多少就往内存里读多少，
/// 再整体塞进 JSON 返回。目标只要回一个大文件或流式响应，就能把网关内存打满。
const MAX_RESPONSE_BYTES: usize = 256 * 1024;

/// 统一的「请求未发出」响应（URL 非法 / 目标被拒）
fn rejected(status_text: &str, error: &str) -> WebhookTestResponse {
    WebhookTestResponse {
        ok: false,
        status: 0,
        status_text: status_text.to_string(),
        latency: 0,
        response_body: String::new(),
        response_headers: std::collections::HashMap::new(),
        error: Some(error.to_string()),
    }
}

/// SSRF 策略：由 `web.webhook_test_allow_hosts` / `web.webhook_test_block_private` 驱动。
///
/// 设计取舍：该接口的正当用途就是打厂区内网的 http 服务（含同机自测 webhook），
/// 因此「是否封私网」做成可配置项而不是硬编码——默认保持既有行为（不封私网），
/// 生产环境可置 `webhook_test_block_private=true` 收紧。
///
/// 缺陷 #2：具体判定逻辑已下沉至 `gateway_core::shared::url_safety`，本类型仅
/// 负责从配置构造策略并委托给 core 的纯函数，保证 API 测试接口与 webhook 上报
/// 客户端共用同一套 SSRF 口径（含逐跳重定向复检与域名黑名单）。
#[derive(Clone)]
struct SsrfPolicy(core_url_safety::SsrfPolicy);

impl SsrfPolicy {
    fn from_state(state: &Arc<AppState>) -> Self {
        let cfg = state.config.load_full();
        let web = cfg.web.as_ref();
        Self(core_url_safety::SsrfPolicy {
            allow_hosts: web
                .map(|w| w.webhook_test_allow_hosts.clone())
                .unwrap_or_default(),
            block_private: web.map(|w| w.webhook_test_block_private).unwrap_or(false),
        })
    }

    fn check_literal(&self, url: &reqwest::Url) -> Result<(), String> {
        self.0.check_literal(url)
    }

    async fn check_resolved(&self, url: &reqwest::Url) -> Result<(), String> {
        self.0.check_resolved(url).await
    }

    fn redirect_policy(&self) -> reqwest::redirect::Policy {
        core_url_safety::redirect_policy(&self.0)
    }

    fn block_private(&self) -> bool {
        self.0.block_private
    }
}

/// 分片读取响应体，最多 [`MAX_RESPONSE_BYTES`]；超限立即中断读取（断开连接）
async fn read_body_capped(resp: &mut reqwest::Response) -> String {
    let mut buf: Vec<u8> = Vec::new();
    let mut truncated = false;
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                let remain = MAX_RESPONSE_BYTES.saturating_sub(buf.len());
                if chunk.len() > remain {
                    buf.extend_from_slice(&chunk[..remain]);
                    truncated = true;
                    break;
                }
                buf.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(_) => break, // 读取中断：保留已读部分
        }
    }
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    if truncated {
        text.push_str(&format!(
            "\n…[响应体超过 {}KB 已截断]",
            MAX_RESPONSE_BYTES / 1024
        ));
    }
    text
}

/// Webhook 测试响应
#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct WebhookTestResponse {
    pub ok: bool,
    pub status: u16,
    pub status_text: String,
    pub latency: u64,
    pub response_body: String,
    pub response_headers: std::collections::HashMap<String, String>,
    pub error: Option<String>,
}

/// POST /api/webhook/test — 发送 webhook 测试请求
pub(crate) async fn test_webhook(
    State(state): State<Arc<AppState>>,
    Json(req): Json<WebhookTestRequest>,
) -> Result<Json<ApiResponse<WebhookTestResponse>>, ApiError> {
    if !req.url.starts_with("http://") && !req.url.starts_with("https://") {
        return Ok(Json(ApiResponse::ok(rejected(
            "Invalid URL",
            "URL 必须以 http:// 或 https:// 开头",
        ))));
    }

    let url = reqwest::Url::parse(&req.url)
        .map_err(|e| ApiError::bad_request(format!("URL 解析失败: {}", e)))?;
    let policy = SsrfPolicy::from_state(&state);
    if let Err(reason) = policy.check_literal(&url) {
        // 被 SSRF 策略拒绝的目标同样落审计：便于事后发现「谁在尝试探测内网」
        state
            .platform
            .audit
            .log("webhook.test.blocked", &req.url, &reason, "api");
        return Ok(Json(ApiResponse::ok(rejected("Blocked Target", &reason))));
    }
    // 严格模式（block_private=true）额外做 DNS 解析复检，并关闭重定向跟随，
    // 避免通过「域名解析到内网」或「302 逐跳跳转」绕过策略。
    if policy.block_private() {
        if let Err(reason) = policy.check_resolved(&url).await {
            state
                .platform
                .audit
                .log("webhook.test.blocked", &req.url, &reason, "api");
            return Ok(Json(ApiResponse::ok(rejected("Blocked Target", &reason))));
        }
    }

    // 重定向策略复用 core 的统一实现：默认逐跳复检跳转目标（否则
    // http://<公网地址>/302 → http://<链路本地> 会绕过上面的检查），
    // 严格模式（block_private=true）完全不跟随。
    let redirect_policy = policy.redirect_policy();

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(redirect_policy)
        .build()
        .map_err(|e| ApiError::internal(format!("build client failed: {}", e)))?;

    let is_get = req.method.to_uppercase() == "GET";
    let method = if is_get {
        reqwest::Method::GET
    } else if req.method.to_uppercase() == "PUT" {
        reqwest::Method::PUT
    } else {
        reqwest::Method::POST
    };

    let mut builder = client.request(method, url);
    for (k, v) in &req.headers {
        builder = builder.header(k, v);
    }
    if !is_get && !req.body.is_empty() {
        builder = builder.body(req.body);
    }

    let start = Instant::now();
    let result = builder.send().await;
    let latency = start.elapsed().as_millis() as u64;

    match result {
        Ok(mut resp) => {
            let status = resp.status();
            let status_text = status.canonical_reason().unwrap_or("").to_string();
            let response_headers: std::collections::HashMap<String, String> = resp
                .headers()
                .iter()
                .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
                .collect();
            // 修复（响应体无界）：限长读取，超限截断并主动断开
            let response_body = read_body_capped(&mut resp).await;
            let ok = status.is_success();
            let status_u16 = status.as_u16();
            let error_msg = if ok {
                None
            } else {
                Some(format!("HTTP {} {}", status_u16, status_text))
            };
            // 对外发起任意 HTTP 请求属高危动作，落审计便于事后追查（含目标与结果码）
            state.platform.audit.log(
                "webhook.test",
                &req.url,
                &format!("HTTP {}, ok={}, {}ms", status_u16, ok, latency),
                "api",
            );

            Ok(Json(ApiResponse::ok(WebhookTestResponse {
                ok,
                status: status_u16,
                status_text,
                latency,
                response_body,
                response_headers,
                error: error_msg,
            })))
        }
        Err(e) => {
            state
                .platform
                .audit
                .log("webhook.test", &req.url, &format!("请求失败：{}", e), "api");
            Ok(Json(ApiResponse::ok(WebhookTestResponse {
                ok: false,
                status: 0,
                status_text: "Request Failed".to_string(),
                latency,
                response_body: String::new(),
                response_headers: std::collections::HashMap::new(),
                error: Some(e.to_string()),
            })))
        }
    }
}

#[cfg(test)]
mod tests {
    // 缺陷 #2：策略实现已下沉到 core；这里直接测 core 的策略，确保 API 侧
    // 原有的 allow_hosts / block_private 配置语义不丢失。
    use gateway_core::shared::url_safety::SsrfPolicy;

    fn url(s: &str) -> reqwest::Url {
        reqwest::Url::parse(s).expect("test url")
    }

    #[test]
    fn blocks_link_local_and_unspecified_always() {
        let p = SsrfPolicy::default();
        assert!(p
            .check_literal(&url("http://169.254.169.254/latest/meta-data"))
            .is_err());
        assert!(p.check_literal(&url("http://[fe80::1]/")).is_err());
        assert!(p.check_literal(&url("http://0.0.0.0:8080/")).is_err());
        // IPv4 映射的 IPv6 同样按 IPv4 判定
        assert!(p
            .check_literal(&url("http://[::ffff:169.254.169.254]/"))
            .is_err());
        // 非 http/https
        assert!(p.check_literal(&url("ftp://example.com/")).is_err());
    }

    #[test]
    fn allows_private_targets_by_default() {
        // 默认保持厂区内网自测的既有用法
        let p = SsrfPolicy::default();
        assert!(p.check_literal(&url("http://10.1.2.3:8080/hook")).is_ok());
        assert!(p.check_literal(&url("http://127.0.0.1:9000/")).is_ok());
        assert!(p.check_literal(&url("http://192.168.1.10/")).is_ok());
    }

    #[test]
    fn block_private_rejects_loopback_and_private() {
        let p = SsrfPolicy {
            allow_hosts: Vec::new(),
            block_private: true,
        };
        assert!(p.check_literal(&url("http://127.0.0.1/")).is_err());
        assert!(p.check_literal(&url("http://10.0.0.1/")).is_err());
        assert!(p.check_literal(&url("http://172.16.5.4/")).is_err());
        assert!(p.check_literal(&url("http://192.168.0.2/")).is_err());
        // CGNAT 100.64.0.0/10
        assert!(p.check_literal(&url("http://100.64.1.1/")).is_err());
        assert!(p.check_literal(&url("http://[::1]/")).is_err());
        assert!(p.check_literal(&url("http://[fd00::1]/")).is_err());
        // 公网地址仍放行
        assert!(p.check_literal(&url("http://8.8.8.8/")).is_ok());
    }

    #[test]
    fn allowlist_limits_hosts() {
        let p = SsrfPolicy {
            allow_hosts: vec!["ERP.local".to_string(), "10.1.2.3:8080".to_string()],
            block_private: false,
        };
        assert!(p.check_literal(&url("http://erp.local/hook")).is_ok());
        assert!(p.check_literal(&url("http://10.1.2.3:8080/hook")).is_ok());
        // 清单登记的是 host:port，则其他端口不放行
        assert!(p.check_literal(&url("http://10.1.2.3:9090/hook")).is_err());
        assert!(p.check_literal(&url("http://evil.example.com/")).is_err());
    }
}
