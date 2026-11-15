//! OPC UA Server 配置（F2）

use serde::{Deserialize, Serialize};

/// OPC UA Server 用户名/密码用户
///
/// 落盘时 `password` 由 `crypto/secret.rs` 统一加密为 `enc:` 形式，
/// API 返回时由 `redact_config_secrets` 替换为占位符。
#[derive(Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, ts_rs::TS)]
#[ts(export)]
pub struct OpcuaUserConfig {
    /// 登录用户名（同时作为 OPC UA user token id，必须非空且唯一）
    pub username: String,
    /// 登录密码（内存中为明文，落盘前加密）
    pub password: String,
}

/// OPC UA Server 配置
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, ts_rs::TS)]
#[ts(export)]
pub struct OpcuaConfig {
    /// 是否启用 OPC UA Server
    #[serde(default)]
    pub enabled: bool,

    /// 监听端点，格式 opc.tcp://host:port（默认 opc.tcp://0.0.0.0:4840）
    #[serde(default = "default_opcua_endpoint")]
    pub endpoint: String,

    /// 是否在已配置证书的情况下仍开放匿名/明文 None 端点（默认 false）
    ///
    /// 默认关闭：配置了证书后只暴露 Basic256Sha256/SignAndEncrypt，避免遗留
    /// 无需凭据、无加密的读取通道。仅本地调试可显式开启。
    #[serde(default)]
    pub allow_anonymous: bool,

    /// 服务端证书 PEM 路径（可选）
    ///
    /// 配置后使用该证书替代全局设备证书；必须与 [`Self::key_path`] 同时配置。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cert_path: Option<String>,

    /// 服务端私钥 PEM 路径（可选）
    ///
    /// 与 [`Self::cert_path`] 成对出现；配置后启动时校验文件存在，缺失则拒绝启动。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_path: Option<String>,

    /// 是否自动信任未登记在信任列表中的客户端证书（默认 true，兼容旧行为）
    ///
    /// 生产环境建议关闭，并通过 pki 目录维护受信客户端证书；关闭后未受信
    /// 客户端在 SignAndEncrypt 握手阶段即被拒绝。
    #[serde(default = "default_true")]
    pub trust_client_certs: bool,

    /// 用户名/密码认证用户列表
    ///
    /// 非空时仅这些用户可登录，匿名是否放行由 `allow_anonymous` 决定。
    /// 为空时必须显式 `allow_anonymous = true`，否则启用后没有任何可用的
    /// 登录方式，配置校验会拒绝启动（不再隐式放行匿名）。
    /// 旧配置（缺该字段且用户为空）在加载/热重载路径由
    /// `sanitize_legacy_opcua_anonymous` 兜底放行匿名并告警，避免升级后网关起不来；
    /// Web 保存路径仍严格拒绝。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub users: Vec<OpcuaUserConfig>,

    /// 是否允许 OPC UA 客户端写回点位（默认 false）
    ///
    /// 开启后只有点位配置 `writable = true` 的节点可被写入，写入会转成
    /// 南向采集器的真实写命令（与 API 写入口共用同一白名单语义）。
    #[serde(default)]
    pub write_enabled: bool,
}

/// 用户密码不得随 `{:?}` 进入日志。
impl std::fmt::Debug for OpcuaUserConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        crate::config::fmt_redacted_debug(self, f)
    }
}

impl Default for OpcuaConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: default_opcua_endpoint(),
            allow_anonymous: false,
            cert_path: None,
            key_path: None,
            trust_client_certs: true,
            users: Vec::new(),
            write_enabled: false,
        }
    }
}

impl OpcuaConfig {
    /// 跨字段语义校验（validator 无法表达的部分）。
    ///
    /// 返回 `None` 表示通过；否则返回可直接展示给运维的中文错误。
    pub fn semantic_error(&self) -> Option<String> {
        if self.endpoint.trim().is_empty() {
            return Some("endpoint 不能为空".to_string());
        }
        match (&self.cert_path, &self.key_path) {
            (Some(c), Some(k)) => {
                if c.trim().is_empty() || k.trim().is_empty() {
                    return Some("cert_path/key_path 不能为空字符串".to_string());
                }
            }
            (Some(_), None) | (None, Some(_)) => {
                return Some("cert_path 与 key_path 必须同时配置或同时留空".to_string());
            }
            (None, None) => {}
        }
        // 收紧旧行为：未配置用户时不再隐式放行匿名。启用后既无用户也未放行
        // 匿名，端点将没有任何可登录方式（客户端一律 BadIdentityTokenRejected），
        // 属于必然不可用的配置，启动/保存时直接拦截。
        if self.enabled && !self.allow_anonymous && self.users.is_empty() {
            return Some(
                "已启用 OPC UA 但未配置任何用户且未放行匿名，客户端将无法登录：\
                 请至少配置一个用户名/密码用户，或显式开启 allow_anonymous"
                    .to_string(),
            );
        }
        let mut seen = std::collections::HashSet::new();
        for (i, u) in self.users.iter().enumerate() {
            if u.username.trim().is_empty() {
                return Some(format!("users[{i}].username 不能为空"));
            }
            if u.username.len() > 64 {
                return Some(format!(
                    "users[{i}].username 长度 {} 超过 64 字符",
                    u.username.len()
                ));
            }
            if u.password.is_empty() {
                return Some(format!("users[{i}].password 不能为空"));
            }
            if !seen.insert(u.username.as_str()) {
                return Some(format!("users[{i}].username \"{}\" 重复", u.username));
            }
        }
        None
    }
}

fn default_opcua_endpoint() -> String {
    "opc.tcp://0.0.0.0:4840".to_string()
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::{OpcuaConfig, OpcuaUserConfig};

    /// 契约：旧配置（不含 allow_anonymous 字段）反序列化后必须默认为 false
    /// （既向后兼容，又默认不开放匿名明文端点）
    #[test]
    fn allow_anonymous_defaults_false_for_legacy_config() {
        let cfg: OpcuaConfig =
            serde_json::from_str(r#"{"enabled":true,"endpoint":"opc.tcp://0.0.0.0:4840"}"#)
                .expect("legacy config parse");
        assert!(
            !cfg.allow_anonymous,
            "缺省 allow_anonymous 必须为 false（不开放匿名 None 端点）"
        );
        // 新增字段的向后兼容默认值：信任客户端证书（旧行为）、不启用写回、无用户
        assert!(cfg.trust_client_certs, "旧配置应保持信任客户端证书行为");
        assert!(!cfg.write_enabled, "写回默认必须关闭");
        assert!(cfg.users.is_empty(), "旧配置应无用户名认证用户");
        assert_eq!(cfg.cert_path, None);
        assert_eq!(cfg.key_path, None);
        // 收紧后的行为：这种「启用 + 无用户 + 不放行匿名」的旧配置曾经隐式放行
        // 匿名，现在必须被校验拦截（运维需补用户或显式开启 allow_anonymous）。
        assert!(
            cfg.semantic_error().is_some(),
            "启用但无用户且不放行匿名的旧配置必须被拒绝"
        );
    }

    /// 契约：启用 OPC UA 时，登录方式必须至少有一种（用户 或 显式匿名）
    #[test]
    fn enabled_requires_users_or_explicit_anonymous() {
        let cfg = OpcuaConfig {
            enabled: true,
            ..Default::default()
        };
        assert!(cfg.semantic_error().is_some(), "无用户且不放行匿名必须拒绝");

        let cfg = OpcuaConfig {
            enabled: true,
            allow_anonymous: true,
            ..Default::default()
        };
        assert!(cfg.semantic_error().is_none(), "显式放行匿名应通过");

        let cfg = OpcuaConfig {
            enabled: true,
            users: vec![OpcuaUserConfig {
                username: "op".into(),
                password: "p".into(),
            }],
            ..Default::default()
        };
        assert!(cfg.semantic_error().is_none(), "配置了用户应通过");

        // 未启用时不做登录方式校验（配置节可先保留，服务不开）
        let cfg = OpcuaConfig::default();
        assert!(cfg.semantic_error().is_none());
    }

    /// 契约：证书路径必须成对配置
    #[test]
    fn cert_and_key_must_be_paired() {
        let cfg = OpcuaConfig {
            cert_path: Some("server.pem".into()),
            ..Default::default()
        };
        assert!(cfg.semantic_error().is_some());

        let cfg = OpcuaConfig {
            cert_path: Some("server.pem".into()),
            key_path: Some("server.key".into()),
            ..Default::default()
        };
        assert!(cfg.semantic_error().is_none());
    }

    /// 契约：用户名校验（空、超长、重复）
    #[test]
    fn user_names_are_unique_and_non_empty() {
        let make = |name: &str, pass: &str| OpcuaUserConfig {
            username: name.to_string(),
            password: pass.to_string(),
        };

        let cfg = OpcuaConfig {
            users: vec![make("", "p")],
            ..Default::default()
        };
        assert!(cfg.semantic_error().is_some(), "空用户名必须拒绝");

        let cfg = OpcuaConfig {
            users: vec![make("op", "")],
            ..Default::default()
        };
        assert!(cfg.semantic_error().is_some(), "空密码必须拒绝");

        let cfg = OpcuaConfig {
            users: vec![make("op", "a"), make("op", "b")],
            ..Default::default()
        };
        assert!(cfg.semantic_error().is_some(), "重复用户名必须拒绝");

        let cfg = OpcuaConfig {
            users: vec![make("op", "p")],
            ..Default::default()
        };
        assert!(cfg.semantic_error().is_none());
    }
}
