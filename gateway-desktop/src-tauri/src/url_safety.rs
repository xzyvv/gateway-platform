//! 出站 URL 安全校验（SSRF / 本地资源 / 危险 scheme 防护）
//!
//! Bug-D2 (fix)：原 `forward_webhook` 命令**无任何 URL 校验**——攻击者绕过前端
//! 直接 `invoke('forward_webhook', { url: 'http://169.254.169.254/...' })` 即可
//! 打到云元数据端点窃取凭证；同时该命令复用了 `state.http`（`danger_accept_invalid_certs(true)`），
//! TLS 完全不校验，攻击者注册钉子证书即可 MITM webhook 流量。
//! SSRF + TLS 跳过双重暴露。
//!
//! 本模块提供同步、纯字符串层的 URL 校验（与前端 `gateway-web/src/utils/urlSafety.ts` 语义对齐，
//! 防止两边口径漂移），由 `forward_webhook` 在 Rust 端强制调用。
//!
//! **不做 DNS 解析后 IP 校验**——Rust 端难拦截 DNS rebinding（无法获取内部
//! `tokio::net::lookup_host` 的解析结果），且 SSRF 真正的出网点不在本进程而在对端
//! webserver，需网络层 ACL 协同。
//!
//! 不引入 ipnet crate（gateway-core 已通过 hyper 间接依赖，但 desktop 独立），
//! IPv4 段判定手工实现。

/// 出站 URL 校验错误
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlSafetyError {
    InvalidUrl(String),
    SchemeNotAllowed(String),
    HostBlocked { host: String, reason: String },
}

/// URL 安全选项
#[derive(Debug, Clone, Default)]
pub struct UrlSafetyOptions {
    /// 用户显式信任的 host 列表（精确匹配或后缀匹配 `.trusted.example.com`）
    pub trusted_hosts: Vec<String>,
    /// 是否允许私有 IP。默认 false；调试环境可临时开启。**生产必须保持 false**。
    pub allow_private_ips: bool,
}

/// 黑名单 host 字面量（lowercase）
const BLOCKED_HOSTS: &[&str] = &[
    "localhost",
    "0.0.0.0",
    "::",
    "::1",
    "metadata.google.internal",
    "169.254.169.254", // AWS / GCP / Azure 元数据
    "metadata",
    "metadata.internal",
];

/// IPv6 保留段前缀
const IPV6_BLOCKED_PREFIXES: &[&str] = &[
    "::",      // unspecified
    "::1",     // loopback
    "fe80:",   // link-local
    "fc",      // ULA
    "fd",      // ULA
    "::ffff:", // IPv4-mapped（保守全部 deny）
];

/// 私有 / 链路本地 / CGNAT IPv4 判定（手工解析避免 ipnet 依赖）
fn is_private_or_reserved_ipv4(ip: &str) -> Option<&'static str> {
    let parts: Vec<&str> = ip.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut octets = [0u8; 4];
    for (i, p) in parts.iter().enumerate() {
        // 严格 ASCII 数字解析，拒绝 "01" / "0x01" 等绕过
        if p.is_empty() || p.len() > 3 || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        octets[i] = p.parse::<u8>().ok()?;
    }
    let [a, b, _c, _d] = octets;
    match a {
        0 => Some("unspecified (0.0.0.0/8)"),
        10 => Some("private (10.0.0.0/8)"),
        127 => Some("loopback (127.0.0.0/8)"),
        172 if (16..=31).contains(&b) => Some("private (172.16.0.0/12)"),
        192 if b == 168 => Some("private (192.168.0.0/16)"),
        169 if b == 254 => Some("link-local (169.254.0.0/16)"),
        100 if (64..=127).contains(&b) => Some("CGNAT (100.64.0.0/10)"),
        224..=239 => Some("multicast (224.0.0.0/4)"),
        240..=255 => Some("reserved (240.0.0.0/4)"),
        _ => None,
    }
}

/// IPv6 私有 / 链路本地判定（仅做前缀匹配）
fn is_private_or_reserved_ipv6(ip: &str) -> Option<&'static str> {
    let lower = ip.to_lowercase();
    for prefix in IPV6_BLOCKED_PREFIXES {
        if lower.starts_with(prefix) {
            return Some(match *prefix {
                "::" => "IPv6 unspecified",
                "::1" => "IPv6 loopback",
                "fe80:" => "IPv6 link-local (fe80::/10)",
                "fc" | "fd" => "IPv6 ULA (fc00::/7)",
                "::ffff:" => "IPv4-mapped IPv6 (::ffff:0:0/96)",
                _ => "IPv6 reserved",
            });
        }
    }
    None
}

/// IP 字面命中私有段？
fn ip_literal_blocked_reason(ip: &str) -> Option<String> {
    if ip.contains(':') {
        is_private_or_reserved_ipv6(ip).map(|s| s.to_string())
    } else {
        is_private_or_reserved_ipv4(ip).map(|s| s.to_string())
    }
}

/// host 字符串层黑名单判定
fn host_literal_blocked(host: &str) -> Option<String> {
    let stripped = if host.starts_with('[') && host.ends_with(']') {
        &host[1..host.len() - 1]
    } else {
        host
    };
    let lower = stripped.to_lowercase();

    for blocked in BLOCKED_HOSTS {
        if lower == *blocked {
            return Some(format!("blocked host literal: {}", blocked));
        }
    }

    if let Some(reason) = ip_literal_blocked_reason(&lower) {
        return Some(reason);
    }

    None
}

/// trusted_hosts 是否豁免该 host
fn is_trusted(host: &str, trusted: &[String]) -> bool {
    let lower = host.to_lowercase();
    trusted.iter().any(|t| {
        let t = t.to_lowercase();
        if t == lower {
            return true;
        }
        if let Some(dot_suffix) = t.strip_prefix('.') {
            // ".example.com" 后缀匹配 "x.example.com" 或 "example.com"
            return lower.ends_with(&format!(".{}", dot_suffix)) || lower == dot_suffix;
        }
        false
    })
}

/// 同步 URL 校验（不做 DNS 解析）。
///
/// 返回 `Ok(())` 表示 URL 通过校验；返回 `Err` 给出具体拒绝原因。
pub fn validate_outbound_url(url: &str, options: &UrlSafetyOptions) -> Result<(), UrlSafetyError> {
    let parsed = url::Url::parse(url).map_err(|e| UrlSafetyError::InvalidUrl(e.to_string()))?;

    match parsed.scheme() {
        "http" | "https" => {}
        other => return Err(UrlSafetyError::SchemeNotAllowed(other.to_string())),
    }

    let host_raw = parsed.host_str().unwrap_or("");
    let host = host_raw.trim();

    // 显式拒绝空 host：url crate 会接受 "http:///path" / "http://" 这类畸形 URL
    if host.is_empty() {
        return Err(UrlSafetyError::InvalidUrl("empty host".into()));
    }

    if is_trusted(host, &options.trusted_hosts) {
        return Ok(());
    }

    if !options.allow_private_ips {
        if let Some(reason) = host_literal_blocked(host) {
            return Err(UrlSafetyError::HostBlocked {
                host: host.to_string(),
                reason,
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts_allow_private() -> UrlSafetyOptions {
        UrlSafetyOptions {
            allow_private_ips: true,
            ..Default::default()
        }
    }

    fn opts_default() -> UrlSafetyOptions {
        UrlSafetyOptions::default()
    }

    #[test]
    fn valid_https_ok() {
        assert!(validate_outbound_url("https://api.example.com/webhook", &opts_default()).is_ok());
    }

    #[test]
    fn valid_http_ok() {
        assert!(validate_outbound_url("http://api.example.com/webhook", &opts_default()).is_ok());
    }

    #[test]
    fn scheme_file_blocked() {
        let err = validate_outbound_url("file:///etc/passwd", &opts_default()).unwrap_err();
        assert!(matches!(err, UrlSafetyError::SchemeNotAllowed(ref s) if s == "file"));
    }

    #[test]
    fn scheme_ftp_blocked() {
        assert!(matches!(
            validate_outbound_url("ftp://example.com/x", &opts_default()).unwrap_err(),
            UrlSafetyError::SchemeNotAllowed(_)
        ));
    }

    #[test]
    fn localhost_blocked() {
        let err = validate_outbound_url("http://localhost:8080/x", &opts_default()).unwrap_err();
        assert!(matches!(err, UrlSafetyError::HostBlocked { .. }));
    }

    #[test]
    fn cloud_metadata_blocked() {
        let err =
            validate_outbound_url("http://169.254.169.254/latest/meta-data/", &opts_default())
                .unwrap_err();
        assert!(matches!(err, UrlSafetyError::HostBlocked { .. }));
    }

    #[test]
    fn rfc1918_blocked() {
        for url in &[
            "http://10.0.0.1/x",
            "http://192.168.1.1/x",
            "http://172.16.0.1/x",
            "http://172.31.255.255/x",
        ] {
            assert!(
                validate_outbound_url(url, &opts_default()).is_err(),
                "{} 应该被拦截",
                url
            );
        }
    }

    #[test]
    fn rfc1918_allowed_when_opt_in() {
        assert!(validate_outbound_url("http://10.0.0.1/x", &opts_allow_private()).is_ok());
        assert!(validate_outbound_url("http://192.168.1.1/x", &opts_allow_private()).is_ok());
    }

    #[test]
    fn rfc1918_outside_range_ok() {
        // 172.32 / 172.15 不在 172.16/12 范围内
        assert!(validate_outbound_url("http://172.32.0.1/x", &opts_default()).is_ok());
        assert!(validate_outbound_url("http://172.15.0.1/x", &opts_default()).is_ok());
    }

    #[test]
    fn ipv6_loopback_blocked() {
        let err = validate_outbound_url("http://[::1]/x", &opts_default()).unwrap_err();
        assert!(matches!(err, UrlSafetyError::HostBlocked { .. }));
    }

    #[test]
    fn ipv4_mapped_blocked() {
        let err =
            validate_outbound_url("http://[::ffff:192.168.1.1]/x", &opts_default()).unwrap_err();
        assert!(matches!(err, UrlSafetyError::HostBlocked { .. }));
    }

    #[test]
    fn trusted_hosts_overrides_block() {
        let mut opts = opts_default();
        opts.trusted_hosts.push("internal.example.com".into());
        assert!(validate_outbound_url("https://internal.example.com/x", &opts).is_ok());
        // 非 trusted 仍被拦截
        assert!(validate_outbound_url("http://169.254.169.254/x", &opts).is_err());
    }

    #[test]
    fn trusted_hosts_suffix_match() {
        let mut opts = opts_default();
        opts.trusted_hosts.push(".trusted.example.com".into());
        assert!(validate_outbound_url("https://api.trusted.example.com/x", &opts).is_ok());
        assert!(validate_outbound_url("https://trusted.example.com/x", &opts).is_ok());
    }

    #[test]
    fn malformed_url_rejected() {
        assert!(matches!(
            validate_outbound_url("not a url", &opts_default()).unwrap_err(),
            UrlSafetyError::InvalidUrl(_)
        ));
    }

    #[test]
    fn empty_host_rejected() {
        // url crate 解析 "http://" 会得到空 host，校验函数必须拦截
        let err = validate_outbound_url("http://", &opts_default()).unwrap_err();
        // 不同 url 版本可能返回 InvalidUrl(host_str 空) 或 HostBlocked("")，
        // 两种拒绝结果都是预期的
        assert!(matches!(
            err,
            UrlSafetyError::InvalidUrl(_) | UrlSafetyError::HostBlocked { .. }
        ));
    }

    #[test]
    fn octal_ip_not_bypass() {
        // 防止 "0177.0.0.1"（八进制）等绕过
        assert!(validate_outbound_url("http://0177.0.0.1/x", &opts_default()).is_err());
    }
}
