//! 配置导入 + 热重载 handler（从原 handlers.rs 拆分）
//!
//! 路由：
//! - PUT  /api/config/import  -> import_config
//! - POST /api/config/reload   -> reload_config
//!
//! D-01：原 `GET /api/config/export` 与 `GET /api/config` 载荷逐字节等价，
//! 仅多 `include_secrets` 分支，已合并进 `GET /api/config`（见 system handlers）。

use super::*;
use gateway_core::engine::error::{GatewayError, GatewayResult};
// ===== 配置导入 + 热重载 =====

/// PUT /api/config/import — 批量覆盖配置
pub async fn import_config(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ConfigImport>,
) -> Result<ApiResponse<serde_json::Value>, ApiError> {
    if let Err(e) = validate_config(&payload.config) {
        return Ok(ApiResponse::err(4003, e.message()));
    }
    // D2: 串行化「读→改→写」
    let _guard = state.config.config_reload_lock.lock().await;
    let old = state.config.load_full();
    // 审计所需的规模数字必须在 apply_reload 移走 payload.config 之前取好
    let device_count = payload
        .config
        .devices
        .as_ref()
        .map(|d| d.len())
        .unwrap_or(0);
    let point_count = payload.config.points.len();
    let bus_count = payload.config.buses.len();
    let outcome = apply_reload(&state, &old, payload.config).await?;
    // E1: 整份配置导入是覆盖式写操作，单独落审计（通用 config.update 看不出「导入」语义）
    state.platform.audit.log(
        "config.import",
        "gateway",
        &format!(
            "devices={} points={} buses={}",
            device_count, point_count, bus_count
        ),
        "api",
    );
    let mut resp = serde_json::json!({ "status": "ok" });
    outcome.apply_to(&mut resp);
    Ok(ApiResponse::ok(resp))
}

/// POST /api/config/reload — 触发热重载（重新加载磁盘配置 + 增量应用）
pub async fn reload_config(
    State(state): State<Arc<AppState>>,
    Json(_payload): Json<ReloadRequest>,
) -> Result<Json<ApiResponse<serde_json::Value>>, ApiError> {
    // D2: 串行化「读→改→写」
    let _guard = state.config.config_reload_lock.lock().await;
    let old = state.config.load_full();
    // 从磁盘重新加载最新内容（避免内存与文件不一致）。
    // 使用 Q18 新增的 reload() 而非 load()：文件缺失时返回错误，
    // 而非静默生成默认配置——对显式 reload 端点更合理。
    let new = state
        .config
        .config_loader
        .reload()
        .map_err(|e| ApiError::internal(format!("reload failed: {}", e)))?;
    let outcome = apply_reload(&state, &old, new).await?;
    let mut resp = serde_json::json!({ "status": "ok" });
    outcome.apply_to(&mut resp);
    Ok(Json(ApiResponse::ok(resp)))
}

/// 校验请求体：与导入接口一致，使用 `{ "config": ... }` 信封。
///
/// `templates` 为模板库中的模板名清单（可选）：给出时后端才能判定「设备引用的模板
/// 是否存在」；缺省或空表示模板库不可用，跳过模板类检查（旧前端行为）。
#[derive(Debug, serde::Deserialize)]
pub struct ConfigValidateRequest {
    pub config: GatewayConfig,
    #[serde(default)]
    pub templates: Vec<String>,
}

/// POST /api/config/validate — 只校验不落盘。
///
/// 校验规则全部来自 `GatewayConfig::validate_report_with`（单一来源），
/// 前端不再维护独立的规则副本；返回 `issues` 带 level/code/field/message，
/// 前端据 code 映射中文类别与修复建议、据 field 前缀推导配置页签实现「定位」。
pub async fn validate_config_report(
    Json(payload): Json<ConfigValidateRequest>,
) -> Json<ApiResponse<serde_json::Value>> {
    let issues = payload.config.validate_report_with(&payload.templates);
    let errors = issues.iter().filter(|i| i.level == "error").count();
    let warnings = issues.iter().filter(|i| i.level == "warning").count();
    let infos = issues.iter().filter(|i| i.level == "info").count();
    Json(ApiResponse::ok(serde_json::json!({
        "valid": errors == 0,
        "errors": errors,
        "warnings": warnings,
        "infos": infos,
        "issues": issues,
    })))
}

/// 配置导入请求校验（import_config 用）
fn validate_config(config: &GatewayConfig) -> GatewayResult<()> {
    // 基本校验：至少有一个设备
    if config.devices.as_ref().is_none_or(|d| d.is_empty()) {
        return Err(GatewayError::Config("至少需要一个设备".into()));
    }
    Ok(())
}

/// POST /api/config/backup — 手动备份当前配置（在网关侧落一个可回滚的版本快照）
///
/// 与 `GET /api/config` 的区别：后者只把配置内容回给前端由其自行保存；
/// backup 则在网关侧写入版本仓库，可通过 `POST /api/config/versions/:id/rollback` 恢复。
pub async fn backup_config(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<serde_json::Value>>, ApiError> {
    let cfg = state.config.load_full();
    let vid = state
        .config
        .version_store
        .save_version(&cfg, "manual backup")
        .map_err(|e| ApiError::internal(format!("backup failed: {}", e)))?;
    state.platform.audit.log(
        "config.backup",
        "gateway",
        &format!("version v{}", vid),
        "api",
    );
    Ok(Json(ApiResponse::ok(serde_json::json!({
        "status": "ok",
        "version_id": vid,
        "points": cfg.points.len(),
        "devices": cfg.devices.as_ref().map(|d| d.len()).unwrap_or(0),
    }))))
}
