//! P0-3: 认证 / 安全策略映射
//!
//! 把 `bus.opcua` 配置（认证方式 / 安全策略 / 安全模式 / 证书路径）
//! 翻译成 async-opcua 客户端侧的安全描述与 `IdentityToken`。

use opcua::client::{IdentityToken, Password};
use opcua::crypto::SecurityPolicy;
use opcua::types::{MessageSecurityMode, UAString, UserTokenPolicy, UserTokenType};

use crate::config::OpcUaClientConfig;
use crate::south::protocol::{CollectorError, CollectorResult};

// ===== P0-3: 认证 / 安全策略映射 =====

/// 安全策略：配置校验保证只可能是 `none` / `basic256sha256`，其余值按缺失处理。
pub(super) fn security_policy_of(auth: Option<&OpcUaClientConfig>) -> SecurityPolicy {
    match auth
        .map(|a| a.security_policy.to_ascii_lowercase())
        .as_deref()
    {
        Some("basic256sha256") => SecurityPolicy::Basic256Sha256,
        _ => SecurityPolicy::None,
    }
}

pub(super) fn security_mode_of(auth: Option<&OpcUaClientConfig>) -> MessageSecurityMode {
    match auth
        .map(|a| a.security_mode.to_ascii_lowercase())
        .as_deref()
    {
        Some("sign") => MessageSecurityMode::Sign,
        Some("sign_and_encrypt") => MessageSecurityMode::SignAndEncrypt,
        _ => MessageSecurityMode::None,
    }
}

/// 端点描述里的用户令牌策略：仅用于声明期望的认证方式；
/// 实际可用性由 Server 返回的端点列表（`user_identity_tokens`）决定。
pub(super) fn user_token_policy_of(auth: Option<&OpcUaClientConfig>) -> UserTokenPolicy {
    match auth.map(|a| a.auth.to_ascii_lowercase()).as_deref() {
        Some("username") => UserTokenPolicy {
            policy_id: UAString::from("username"),
            token_type: UserTokenType::UserName,
            ..UserTokenPolicy::anonymous()
        },
        Some("certificate") => UserTokenPolicy {
            policy_id: UAString::from("certificate"),
            token_type: UserTokenType::Certificate,
            ..UserTokenPolicy::anonymous()
        },
        _ => UserTokenPolicy::anonymous(),
    }
}

/// 按配置构造 `IdentityToken`；配置缺失必填字段时返回可读错误（配置校验已兜底，
/// 这里防止运行时构造出空凭据导致 Server 拒绝且原因不明）。
pub(super) fn build_identity_token(
    auth: Option<&OpcUaClientConfig>,
) -> CollectorResult<IdentityToken> {
    let Some(cfg) = auth else {
        return Ok(IdentityToken::Anonymous);
    };
    match cfg.auth.to_ascii_lowercase().as_str() {
        "username" => {
            let username = cfg.username.as_deref().unwrap_or("").trim();
            let password = cfg.password.as_deref().unwrap_or("").trim();
            if username.is_empty() || password.is_empty() {
                return Err(CollectorError::IoError(
                    "OPC UA username auth requires both username and password".into(),
                ));
            }
            Ok(IdentityToken::UserName(
                username.to_string(),
                Password::new(password.to_string()),
            ))
        }
        "certificate" => {
            let cert = cfg.client_cert_path.as_deref().unwrap_or("").trim();
            let key = cfg.client_key_path.as_deref().unwrap_or("").trim();
            if cert.is_empty() || key.is_empty() {
                return Err(CollectorError::IoError(
                    "OPC UA certificate auth requires client_cert_path and client_key_path".into(),
                ));
            }
            IdentityToken::new_x509_path(cert, key).map_err(|e| {
                CollectorError::IoError(format!(
                    "OPC UA load client certificate failed (cert={}, key={}): {}",
                    cert, key, e
                ))
            })
        }
        _ => Ok(IdentityToken::Anonymous),
    }
}
