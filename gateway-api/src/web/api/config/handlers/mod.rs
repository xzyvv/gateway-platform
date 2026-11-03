//! 配置 API handlers 子模块入口
//!
//! 从原 handlers.rs（376 行单文件）拆分为目录，按职责分文件：
//! - devices.rs：设备 CRUD
//! - points.rs：测点 CRUD
//! - config.rs：配置导入导出 + 热重载
//!
//! 共享辅助（apply_reload / with_devices_mut）放本文件，
//! 子模块通过 `use super::*;` 拿到共享 import 与辅助函数。

// 重新导出父模块（config）的 import，供子 handler 通过 use super::* 引用
pub(crate) use super::*;

pub mod bus_control;
pub mod config;
pub mod devices;
pub mod points;
pub mod project;
pub mod types;

// 将各子模块的 handler 函数提升到 handlers 命名空间
pub(crate) use bus_control::*;
pub(crate) use config::*;
pub(crate) use devices::*;
pub(crate) use points::*;
pub(crate) use project::*;
pub(crate) use types::*;

// ===== 内部辅助 =====

pub(crate) async fn apply_reload(
    state: &Arc<AppState>,
    old: &GatewayConfig,
    new: GatewayConfig,
) -> Result<crate::web::config_apply::ReloadOutcome, ApiError> {
    // D-06：统一走 config_apply 的唯一写回路径（expand → reload → store → save）
    let (outcome, _effective) =
        crate::web::config_apply::reload_then_save(state, old, &new).await?;
    Ok(outcome)
}

/// 在 devices 字段（Option<Vec<_>>）上执行 mutate，自动初始化 None 为空 Vec。
pub(crate) fn with_devices_mut<F, R>(cfg: &mut GatewayConfig, f: F) -> R
where
    F: FnOnce(&mut Vec<DeviceConfig>) -> R,
{
    let devices = cfg.devices.get_or_insert_with(Vec::new);
    f(devices)
}

/// 敏感导出（含明文凭据）只允许**主 token**。
///
/// 只读 HTTP 方法不触发 API Key 的 scope 校验（见 `middleware::is_write_method`），
/// 因此导出端点若默认返回明文，任何只读 API Key 都能取走 MQTT 凭据 / 密码哈希 /
/// OTA 签名私钥。默认行为改为脱敏；仅当调用方显式要求明文且持主 token 时放行，
/// 并落审计日志。API Key 一律拒绝（即便持有 `*:write`）。
pub(crate) fn ensure_primary_token(
    state: &Arc<AppState>,
    headers: &axum::http::HeaderMap,
    audit_action: &str,
) -> Result<(), ApiError> {
    use crate::web::middleware::{api_token, extract_token_from_headers, token_matches};
    let provided = extract_token_from_headers(headers)
        .ok_or_else(|| ApiError::unauthorized("missing api token"))?;
    if token_matches(&provided, &api_token(state)) {
        state.platform.audit.log(
            audit_action,
            "gateway",
            "sensitive export with plaintext secrets",
            "api",
        );
        Ok(())
    } else {
        Err(ApiError {
            status: axum::http::StatusCode::FORBIDDEN,
            code: 8005,
            message: "plaintext export requires the primary api token".to_string(),
        })
    }
}
