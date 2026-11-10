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
    Json(mut payload): Json<ConfigImport>,
) -> Result<ApiResponse<serde_json::Value>, ApiError> {
    // 导入老版本备份时，「mqtt + 模拟消费者」双开的组合按存量策略降级，
    // 与磁盘加载路径保持一致——恢复流程不该被 400 卡死。降级说明随响应返回。
    let downgraded = sanitize_imported_config(&mut payload.config);
    if let Err(e) = validate_config(&payload.config) {
        return Ok(ApiResponse::err(4003, e.message()));
    }
    // D2: 串行化「读→改→写」
    let _guard = state.config.config_reload_lock.lock().await;
    let old = state.config.load_full();
    // 审计所需的规模数字必须在 reload_then_save 消费配置之前取好
    let device_count = payload
        .config
        .devices
        .as_ref()
        .map(|d| d.len())
        .unwrap_or(0);
    let point_count = payload.config.points.len();
    let bus_count = payload.config.buses.len();
    let (outcome, effective) =
        crate::web::config_apply::reload_then_save(&state, &old, &payload.config).await?;
    // 导入是覆盖式写操作，必须与 POST /api/config 一样留版本快照，
    // 否则导入前的配置在版本库里没有落点，无法回滚。
    // 快照失败只降级为告警：配置已落盘生效，不应因历史归档失败而回滚本次导入。
    match state
        .config
        .version_store
        .save_version(&effective, "config import")
    {
        Ok(vid) => tracing::info!("config version v{} saved after import", vid),
        Err(e) => tracing::warn!("failed to save config version after import: {}", e),
    }
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
    if let Some(note) = downgraded {
        resp["sanitized"] = serde_json::json!([note]);
        tracing::warn!("config import downgraded: {}", note);
    }
    Ok(ApiResponse::ok(resp))
}

/// 导入路径的存量兜底：双消费者组合降级为「只留 MQTT」。
///
/// 与加载路径（`ConfigLoader::load/reload`）共用同一套降级逻辑，保证
/// 「从老版本备份恢复」和「升级后启动」两种场景表现一致：都是关掉模拟消费者，
/// 而不是把用户挡在门外。返回降级说明（未降级时为 `None`）。
fn sanitize_imported_config(config: &mut GatewayConfig) -> Option<String> {
    if gateway_core::config::sanitize_legacy_consumer_exclusivity(config) {
        Some(
            "sim_consumer.enabled 已由 true 降级为 false：它与 mqtt.enabled 互斥，\
             两个消费者会分流同一批队列数据导致静默丢数（如需用模拟消费者请先停用 MQTT）"
                .to_string(),
        )
    } else {
        None
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with_consumers(mqtt: bool, sim: bool) -> GatewayConfig {
        let mut cfg = GatewayConfig::default();
        cfg.mqtt.enabled = mqtt;
        cfg.sim_consumer.enabled = sim;
        cfg
    }

    /// 导入老版本备份（双开）要降级而不是 400——恢复流程不能被挡死。
    #[test]
    fn import_downgrades_legacy_double_consumer() {
        let mut cfg = cfg_with_consumers(true, true);
        let note = sanitize_imported_config(&mut cfg);
        assert!(note.is_some(), "双开时必须给出降级说明");
        assert!(!cfg.sim_consumer.enabled, "模拟消费者应被关掉");
        assert!(
            cfg.mqtt.enabled,
            "MQTT 保留：它是生产链路，模拟消费者只是排障工具"
        );
    }

    /// 单消费者场景不动配置。
    #[test]
    fn import_keeps_single_consumer_untouched() {
        let mut only_mqtt = cfg_with_consumers(true, false);
        assert!(sanitize_imported_config(&mut only_mqtt).is_none());
        assert!(only_mqtt.mqtt.enabled);

        let mut only_sim = cfg_with_consumers(false, true);
        assert!(sanitize_imported_config(&mut only_sim).is_none());
        assert!(only_sim.sim_consumer.enabled, "只有模拟消费者时不该降级");
    }
}
