//! 上行模板配置保存/导入/导出 handler

use super::*;
use crate::web::config_apply::reload_then_save;
use crate::web::services::validate_template;
use gateway_core::config::PointConfig;

pub async fn get_uplink_config(State(state): State<Arc<AppState>>) -> Resp {
    let cfg = state.config.load();
    let mqtt = &cfg.mqtt;
    let dto = UplinkConfigDto {
        enabled: mqtt
            .payload_template
            .as_ref()
            .is_some_and(|s| !s.trim().is_empty()),
        template: mqtt.payload_template.clone().unwrap_or_default(),
        encoding: "json_lines".into(),
        max_bytes: 4096,
        updated_at: Some(Utc::now().to_rfc3339()),
    };
    ok(dto)
}

/// 抽出 save 逻辑供 save_uplink_config / import_template 共用。
async fn do_save_uplink_config(
    state: &Arc<AppState>,
    req: SaveUplinkConfigRequest,
    audit_action: &str,
) -> Resp {
    let template_str = req.template.clone();
    let template_len = template_str.len();
    let payload_template = if req.enabled && !template_str.trim().is_empty() {
        if let Err(e) = validate_template(&template_str) {
            return err(
                StatusCode::BAD_REQUEST,
                40001,
                format!("模板编译失败：{}", e),
            );
        }
        Some(template_str)
    } else {
        None
    };

    let _guard = state.config.config_reload_lock.lock().await;

    let old_cfg = state.config.load();
    let new_cfg = {
        let mut cfg = (*old_cfg).clone();
        cfg.mqtt.payload_template = payload_template;
        cfg
    };

    // V-8：统一走 reload_then_save —— 结构校验 + 模板展开校验 + license 配额 + 热重载 + 落盘 +
    // 更新内存。任一步失败即中止，不再绕过配额与校验规则；
    // reload_config 失败会作为 ApiError 返回，不会出现「配置已落盘但运行态未更新却提示成功」。
    let (outcome, _effective) = match reload_then_save(state, &old_cfg, &new_cfg).await {
        Ok(o) => o,
        Err(e) => return err(e.status, e.code, e.message),
    };
    if outcome.notes.is_empty() {
        tracing::debug!("uplink config hot reload: no components changed");
    } else {
        tracing::info!("uplink config hot reload notes: {:?}", outcome.notes);
    }

    // 取 `gateway.mqtt()` 的**实时**值：热重载可能替换/移除 MQTT 客户端，
    // `state.north.mqtt` 是启动期快照，重载后会指向已被替换的旧客户端。
    if let Some(mqtt) = state.north.gateway.mqtt() {
        let points: Vec<PointConfig> = new_cfg.points.clone();
        mqtt.set_point_configs(points);
    }

    // 持久化配置版本快照（含模板），支持前端 GET /api/config/versions 回滚到上一版模板。
    // E-6：配置已生效却没有可回滚版本，不能只 warn —— 必须显式上报 5xx。
    match state
        .config
        .version_store
        .save_version(&new_cfg, audit_action)
    {
        Ok(vid) => tracing::info!(
            "uplink config version v{} saved (action={})",
            vid,
            audit_action
        ),
        Err(e) => {
            tracing::error!("failed to save uplink config version: {}", e);
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                50003,
                format!("配置已保存并生效，但版本快照写入失败：{}", e),
            );
        }
    }

    state.platform.audit.log(
        "uplink_template_update",
        "mqtt.payload_template",
        &format!(
            "action={}, enabled={}, template_len={}",
            audit_action, req.enabled, template_len
        ),
        "web_api",
    );

    let dto = UplinkConfigDto {
        enabled: new_cfg.mqtt.payload_template.is_some(),
        template: new_cfg.mqtt.payload_template.clone().unwrap_or_default(),
        encoding: req.encoding,
        max_bytes: req.max_bytes,
        updated_at: Some(Utc::now().to_rfc3339()),
    };
    ok(dto)
}

pub async fn save_uplink_config(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SaveUplinkConfigRequest>,
) -> Resp {
    do_save_uplink_config(&state, req, "uplink template update").await
}

// ===== 模板导入/导出 =====

/// 导出当前模板（细粒度：只返回模板字符串与 enabled 状态，不带其他配置）
pub async fn export_template(State(state): State<Arc<AppState>>) -> Resp {
    let cfg = state.config.load();
    let template = cfg.mqtt.payload_template.clone().unwrap_or_default();
    let enabled = !template.trim().is_empty();
    ok(serde_json::json!({
        "template": template,
        "enabled": enabled,
        "encoding": "json_lines",
        "max_bytes": 4096,
        "exported_at": Utc::now().to_rfc3339(),
    }))
}

/// 导入模板：编译校验 + 写盘 + 热重载 + 版本快照（复用 save 逻辑）
pub async fn import_template(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ImportTemplateRequest>,
) -> Resp {
    do_save_uplink_config(
        &state,
        import_request_to_save(req),
        "uplink template import",
    )
    .await
}

/// 将导入请求转换为内部 save 请求（填充默认 encoding/max_bytes，便于测试）
pub(crate) fn import_request_to_save(req: ImportTemplateRequest) -> SaveUplinkConfigRequest {
    SaveUplinkConfigRequest {
        template: req.template,
        enabled: req.enabled,
        encoding: default_encoding(),
        max_bytes: default_max_bytes(),
    }
}
