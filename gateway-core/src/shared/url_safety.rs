//! 出站 URL 安全校验（SSRF 防护下沉）
//!
//! 缺陷 #2：把原先只存在于 gateway-api webhook 测试接口的 SSRF 策略下沉到
//! gateway-core/shared，供 webhook 上报客户端与 API 测试接口共用同一套口径。
//!
//! 语义以 API 侧为准：**默认放行私网 / 回环**（`block_private = false`），
//! 保存量厂区内网自测兼容；链路本地（含云元数据 169.254.169.254）、未指定、
//! 组播、0 段地址恒拒。在此基底上融合 desktop `url_safety.rs` 的域名黑名单
//! （localhost / metadata.* 等），堵住「名字即内网」的字面量通道。
//!
//! 不做 allow_hosts / trusted_hosts 白名单接线（过度设计）；`allow_hosts`
//! 仅作为可选参数保留，API 测试接口按需从配置传入。
//!
//! DNS 解析后复检（[`SsrfPolicy::check_resolved`]）为可选增强：DNS 解析与
//! 实际连接之间存在 TOCTOU（DNS rebinding）窗口；严格模式（block_private=true）
//! 以 `Policy::none()` 关闭重定向作为兜底，避免逐跳转向内网的二次绕过。

use std::net::{IpAddr, Ipv4Addr};

/// 逐跳复检时最多跟随的重定向跳数（与 API webhook 测试接口一致）。
pub const MAX_REDIRECTS: usize = 3;

/// 恒拒的主机名 / 域名 / IP 字面量黑名单（小写）。
///
/// 来自 desktop `url_safety.rs::BLOCKED_HOSTS`；与 IP 段判定互补——后者只认
/// 解析后的 `IpAddr`，这里在 DNS 解析前就拦住「名字本身就是元数据/本机」的
/// 目标，且对重定向逐跳同样生效。
const BLOCKED_HOST_LITERALS: &[&str] = &[
    "localhost",
    "0.0.0.0",
    "::",
    "::1",
    "169.254.169.254",
    "metadata",
    "metadata.internal",
    "metadata.google.internal",
];

/// 出站 URL 的 SSRF 策略。
#[derive(Clone, Debug, Default)]
pub struct SsrfPolicy {
    /// 允许的目标主机白名单（`host` 或 `host:port`）；空 = 不限制。
    /// 仅作为可选参数保留，本次不新增配置字段接线。
    pub allow_hosts: Vec<String>,
    /// 是否禁止私网 / 回环 / CGNAT / IPv6 ULA。默认 false（API 语义）。
    pub block_private: bool,
}

impl SsrfPolicy {
    pub fn new(allow_hosts: Vec<String>, block_private: bool) -> Self {
        Self {
            allow_hosts,
            block_private,
        }
    }

    /// 目标主机是否在白名单内（白名单为空视为不限制）。
    fn host_allowed(&self, url: &reqwest::Url) -> bool {
        if self.allow_hosts.is_empty() {
            return true;
        }
        let Some(host) = url.host_str() else {
            return false;
        };
        let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
        let host_port = match url.port_or_known_default() {
            Some(port) => format!("{}:{}", host, port),
            None => host.clone(),
        };
        self.allow_hosts.iter().any(|entry| {
            let entry = entry.trim().to_ascii_lowercase();
            !entry.is_empty() && (entry == host || entry == host_port)
        })
    }

    /// IP 是否受限；返回拒绝原因（`None` 表示放行）。
    fn ip_blocked_reason(&self, ip: IpAddr) -> Option<String> {
        // IPv4 映射的 IPv6（::ffff:169.254.169.254）按 IPv4 判定，避免绕过
        let ip = match ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
            other => other,
        };
        match ip {
            IpAddr::V4(v4) => {
                // 恒拒：链路本地 / 未指定 / 组播 / 广播 / 0 段
                let always = v4.is_link_local()
                    || v4.is_unspecified()
                    || v4.is_multicast()
                    || v4.is_broadcast()
                    || v4.octets()[0] == 0;
                if always {
                    return Some(format!("不允许访问受限地址 {}", ip));
                }
                if self.block_private && (v4.is_loopback() || v4.is_private() || is_cgnat(v4)) {
                    return Some(format!(
                        "block_private=true：不允许访问私网/回环地址 {}",
                        ip
                    ));
                }
            }
            IpAddr::V6(v6) => {
                let always =
                    v6.is_unicast_link_local() || v6.is_unspecified() || v6.is_multicast();
                if always {
                    return Some(format!("不允许访问受限地址 {}", ip));
                }
                if self.block_private && (v6.is_loopback() || v6.is_unique_local()) {
                    return Some(format!(
                        "block_private=true：不允许访问私网/回环地址 {}",
                        ip
                    ));
                }
            }
        }
        None
    }

    /// 同步字面量校验：scheme / 域名黑名单 / 主机白名单 / IP 字面量。
    ///
    /// 重定向逐跳也调用本方法（见 [`redirect_policy`]），纯函数、零 I/O。
    /// 注：`reqwest::Url` 按 WHATWG 规范把 `0177.0.0.1` / `0x7f.0.0.1` 等
    /// 非标准写法归一化为 `127.0.0.1`，随后由下面的 IpAddr 判定拦截。
    pub fn check_literal(&self, url: &reqwest::Url) -> Result<(), String> {
        match url.scheme() {
            "http" | "https" => {}
            other => return Err(format!("仅支持 http/https，当前为 {}", other)),
        }
        let Some(host) = url.host_str() else {
            return Err("URL 缺少主机名".to_string());
        };
        let bare = host.trim_matches(['[', ']']);
        let lower = bare.to_ascii_lowercase();
        if BLOCKED_HOST_LITERALS.contains(&lower.as_str()) {
            return Err(format!("目标主机 {} 在出站黑名单内", lower));
        }
        if !self.host_allowed(url) {
            return Err(format!("目标主机 {} 不在 allow_hosts 白名单内", host));
        }
        if let Ok(ip) = bare.parse::<IpAddr>() {
            if let Some(reason) = self.ip_blocked_reason(ip) {
                return Err(reason);
            }
        }
        Ok(())
    }

    /// 便捷入口：解析字符串后做字面量校验（send_once 的 outbox 复检用）。
    pub fn check_url_str(&self, url: &str) -> Result<(), String> {
        let parsed = reqwest::Url::parse(url).map_err(|e| format!("URL 解析失败: {}", e))?;
        self.check_literal(&parsed)
    }

    /// DNS 解析后复检（仅 `block_private=true` 时调用）。
    ///
    /// 防「域名解析到私网/链路本地」绕过字面量检查。注意 DNS 解析与 reqwest
    /// 实际连接之间存在 TOCTOU 窗口（DNS rebinding），严格模式另以
    /// `Policy::none()` 关闭重定向兜底。
    pub async fn check_resolved(&self, url: &reqwest::Url) -> Result<(), String> {
        let Some(host) = url.host_str() else {
            return Err("URL 缺少主机名".to_string());
        };
        let host = host.trim_matches(['[', ']']);
        if host.parse::<IpAddr>().is_ok() {
            return Ok(()); // 字面量已在 check_literal 校验
        }
        let port = url.port_or_known_default().unwrap_or(80);
        let addrs = tokio::net::lookup_host((host, port))
            .await
            .map_err(|e| format!("目标域名解析失败: {}", e))?;
        let mut any = false;
        for addr in addrs {
            any = true;
            if let Some(reason) = self.ip_blocked_reason(addr.ip()) {
                return Err(format!("{}（解析结果 {}）", reason, addr.ip()));
            }
        }
        if !any {
            return Err(format!("目标域名 {} 未解析到任何地址", host));
        }
        Ok(())
    }
}

/// CGNAT 段 100.64.0.0/10（`Ipv4Addr::is_private` 不含此段）。
fn is_cgnat(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 100 && (o[1] & 0xC0) == 64
}

/// 由策略构造 reqwest 重定向策略：逐跳复检 + 跳数上限；严格模式完全不跟随。
pub fn redirect_policy(policy: &SsrfPolicy) -> reqwest::redirect::Policy {
    if policy.block_private {
        return reqwest::redirect::Policy::none();
    }
    let hop = policy.clone();
    reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= MAX_REDIRECTS {
            attempt.stop()
        } else {
            match hop.check_literal(attempt.url()) {
                Ok(()) => attempt.follow(),
                Err(reason) => attempt.error(reason),
            }
        }
    })
}

/// Webhook run() URL 校验失败原因（区分解析失败 / SSRF 拒绝）。
pub enum WebhookUrlError {
    Parse(String),
    Blocked(String),
}

impl WebhookUrlError {
    /// (error 日志文案, last_error 文案)，供 webhook 按原口径上报。
    pub fn messages(&self, url: &str) -> (String, String) {
        match self {
            Self::Parse(e) => (
                format!("webhook url parse failed '{}': {}", url, e),
                format!("invalid webhook url: {}", e),
            ),
            Self::Blocked(r) => (
                format!("webhook url blocked by SSRF policy '{}': {}", url, r),
                format!("invalid webhook url (SSRF): {}", r),
            ),
        }
    }
}

/// 解析 URL 并做字面量 SSRF 校验（webhook run() 用）。
pub fn check_webhook_url(url: &str) -> Result<reqwest::Url, WebhookUrlError> {
    let parsed = reqwest::Url::parse(url).map_err(|e| WebhookUrlError::Parse(e.to_string()))?;
    SsrfPolicy::default()
        .check_literal(&parsed)
        .map_err(WebhookUrlError::Blocked)?;
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> reqwest::Url {
        reqwest::Url::parse(s).expect("test url")
    }

    fn strict() -> SsrfPolicy {
        SsrfPolicy {
            block_private: true,
            ..Default::default()
        }
    }

    // ---- API 侧迁移的 4 例 ----

    #[test]
    fn blocks_link_local_and_unspecified_always() {
        let p = SsrfPolicy::default();
        assert!(p
            .check_literal(&url("http://169.254.169.254/latest/meta-data"))
            .is_err());
        assert!(p.check_literal(&url("http://[fe80::1]/")).is_err());
        assert!(p.check_literal(&url("http://0.0.0.0:8080/")).is_err());
        assert!(p
            .check_literal(&url("http://[::ffff:169.254.169.254]/"))
            .is_err());
        assert!(p.check_literal(&url("ftp://example.com/")).is_err());
    }

    #[test]
    fn allows_private_targets_by_default() {
        let p = SsrfPolicy::default();
        assert!(p.check_literal(&url("http://10.1.2.3:8080/hook")).is_ok());
        assert!(p.check_literal(&url("http://127.0.0.1:9000/")).is_ok());
        assert!(p.check_literal(&url("http://192.168.1.10/")).is_ok());
    }

    #[test]
    fn block_private_rejects_loopback_and_private() {
        let p = strict();
        for u in &[
            "http://127.0.0.1/",
            "http://10.0.0.1/",
            "http://172.16.5.4/",
            "http://192.168.0.2/",
            "http://100.64.1.1/",
            "http://[::1]/",
            "http://[fd00::1]/",
        ] {
            assert!(p.check_literal(&url(u)).is_err(), "{u} 应被严格模式拒绝");
        }
        assert!(p.check_literal(&url("http://8.8.8.8/")).is_ok());
    }

    #[test]
    fn allowlist_limits_hosts() {
        let p = SsrfPolicy::new(
            vec!["ERP.local".to_string(), "10.1.2.3:8080".to_string()],
            false,
        );
        assert!(p.check_literal(&url("http://erp.local/hook")).is_ok());
        assert!(p.check_literal(&url("http://10.1.2.3:8080/hook")).is_ok());
        assert!(p.check_literal(&url("http://10.1.2.3:9090/hook")).is_err());
        assert!(p.check_literal(&url("http://evil.example.com/")).is_err());
    }

    // ---- desktop 侧迁移（按 core 默认放行私网语义调整）----

    #[test]
    fn scheme_whitelist() {
        assert!(SsrfPolicy::default()
            .check_url_str("https://api.example.com/webhook")
            .is_ok());
        assert!(SsrfPolicy::default()
            .check_url_str("http://api.example.com/webhook")
            .is_ok());
        assert!(SsrfPolicy::default()
            .check_url_str("file:///etc/passwd")
            .is_err());
        assert!(SsrfPolicy::default()
            .check_url_str("ftp://example.com/x")
            .is_err());
    }

    #[test]
    fn blocked_host_literals() {
        let p = SsrfPolicy::default();
        for u in &[
            "http://localhost:8080/x",
            "http://metadata/",
            "http://metadata.internal/",
            "http://metadata.google.internal/",
        ] {
            assert!(p.check_url_str(u).is_err(), "{u} 应被域名黑名单拒绝");
        }
    }

    #[test]
    fn rfc1918_outside_range_ok() {
        let p = SsrfPolicy::default();
        assert!(p.check_literal(&url("http://172.32.0.1/x")).is_ok());
        assert!(p.check_literal(&url("http://172.15.0.1/x")).is_ok());
    }

    #[test]
    fn malformed_and_empty_host_rejected() {
        let p = SsrfPolicy::default();
        assert!(p.check_url_str("not a url").is_err());
        assert!(p.check_url_str("http://").is_err());
    }

    // ---- 补充绕过用例 ----

    #[test]
    fn ipv4_mapped_normalized_and_blocked() {
        // 恒拒：映射回 169.254.169.254 后按 IPv4 链路本地判定
        assert!(SsrfPolicy::default()
            .check_literal(&url("http://[::ffff:169.254.169.254]/"))
            .is_err());
        // 严格模式：映射回私网同样按 IPv4 判定
        assert!(strict()
            .check_literal(&url("http://[::ffff:192.168.1.1]/"))
            .is_err());
    }

    #[test]
    fn cgnat_blocked_in_strict_mode() {
        assert!(strict().check_literal(&url("http://100.64.1.1/")).is_err());
        assert!(strict().check_literal(&url("http://100.127.255.255/")).is_err());
        // 100.128.0.0 不在 CGNAT 段
        assert!(strict().check_literal(&url("http://100.128.0.1/")).is_ok());
    }

    #[test]
    fn octal_and_hex_host_cannot_bypass() {
        // reqwest::Url 按 WHATWG 规范归一化，归一化结果仍受 IP 判定约束
        assert_eq!(
            url("http://0177.0.0.1/x").host_str(),
            Some("127.0.0.1")
        );
        assert_eq!(url("http://0x7f.0.0.1/x").host_str(), Some("127.0.0.1"));
        assert!(strict().check_url_str("http://0177.0.0.1/x").is_err());
        assert!(strict().check_url_str("http://0x7f.0.0.1/x").is_err());
    }
}
