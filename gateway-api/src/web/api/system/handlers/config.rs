//! 系统 API 处理函数（从 system.rs 拆分）

use super::*;

pub(crate) fn redact_config_secrets(value: &mut serde_json::Value) {
    redact_secrets_at(value, "");
}

/// GET 响应脱敏：把敏感字段值替换为占位符 [`crate::web::config_apply::REDACTED_SECRET`]。
///
/// P0-2/P0-3：allow_anon_read 默认开启，匿名请求也能读到配置；
/// 这些字段在内存中已是明文（decrypt_config_secrets 解密），不外泄。
/// 占位值由保存路径还原（config_apply::restore_redacted_secrets）。
///
/// 字段清单是**唯一来源** [`gateway_core::config::sensitive_field_paths`]，
/// 与 `config_apply::restore_redacted_secrets` 共用；`webhook.headers` 为动态键
/// Map，按 [`gateway_core::config::SENSITIVE_SECRET_HEADERS_PATH`] 单独处理。
///
/// 在 `prefix` 下按清单脱敏；`prefix` 形如 `"/northbound"`（工程文件把 mqtt /
/// webhook / cellular 挂在 `/northbound` 下，带前缀复用同一清单）。
pub(crate) fn redact_secrets_at(value: &mut serde_json::Value, prefix: &str) {
    use crate::web::config_apply::REDACTED_SECRET;
    for rel in gateway_core::config::sensitive_field_paths() {
        let path = format!("{}{}", prefix, rel);
        if let Some(slot) = value.pointer_mut(&path) {
            if slot.is_string() {
                *slot = serde_json::Value::String(REDACTED_SECRET.to_string());
            }
        }
    }
    // Bug: webhook.headers 是 `HashMap<String, String>`（常装 Authorization 等凭据），
    // 不属于上面的「字符串字段」白名单，旧实现会明文回传。它是对象而非字符串，
    // 需逐项脱敏：保留键名、把值替换为占位符，保存时由
    // `config_apply::restore_redacted_secrets` 按键还原。
    let headers_path = format!(
        "{}{}",
        prefix,
        gateway_core::config::SENSITIVE_SECRET_HEADERS_PATH
    );
    if let Some(serde_json::Value::Object(headers)) = value.pointer_mut(&headers_path) {
        for (_name, v) in headers.iter_mut() {
            if v.is_string() {
                *v = serde_json::Value::String(REDACTED_SECRET.to_string());
            }
        }
    }
    // P0-3: OPC UA Client 凭据藏在 `buses[]` 数组项里，无法用静态 JSON Pointer 表达，
    // 与 webhook.headers 同理逐项脱敏；保存时由
    // `config_apply::restore_redacted_secrets` 按 `port_name` 匹配还原。
    //
    // 工程文件导出只有北向通道挂在 `/northbound` 下，`buses` 恒在根层级；
    // 因此除带前缀路径外，还要固定处理根 `/buses`（prefix 为空时两者相同，跳过重复）。
    redact_buses_secrets(value, &format!("{}/buses", prefix));
    if !prefix.is_empty() {
        redact_buses_secrets(value, "/buses");
    }
    // OPC UA Server 用户名/密码用户列表同理是动态数组：脱敏 `password`，
    // 保存时由 `config_apply::restore_redacted_secrets` 按 username 匹配还原。
    // 与 buses 相同，工程文件把 opcua 挂在 `/northbound` 下，完整配置在根层级。
    redact_opcua_users_secrets(value, &format!("{}/opcua/users", prefix));
    if !prefix.is_empty() {
        redact_opcua_users_secrets(value, "/opcua/users");
    }
}

/// 对指定 `opcua.users` 数组路径逐项脱敏 `password`（见 [`redact_secrets_at`]）。
///
/// `username` 不是凭据（同时是 OPC UA user token id），保留明文以便前端回显与
/// 保存时按用户名匹配还原密码。
fn redact_opcua_users_secrets(value: &mut serde_json::Value, users_path: &str) {
    use crate::web::config_apply::REDACTED_SECRET;
    if let Some(serde_json::Value::Array(users)) = value.pointer_mut(users_path) {
        for user in users.iter_mut() {
            if let Some(v) = user.get_mut("password") {
                if v.is_string() {
                    *v = serde_json::Value::String(REDACTED_SECRET.to_string());
                }
            }
        }
    }
}

/// 对指定 `buses` 数组路径逐项脱敏 OPC UA 用户名/密码（见 [`redact_secrets_at`]）。
fn redact_buses_secrets(value: &mut serde_json::Value, buses_path: &str) {
    use crate::web::config_apply::REDACTED_SECRET;
    if let Some(serde_json::Value::Array(buses)) = value.pointer_mut(buses_path) {
        for bus in buses.iter_mut() {
            let Some(opcua) = bus.get_mut("opcua").and_then(|v| v.as_object_mut()) else {
                continue;
            };
            for key in ["username", "password"] {
                if let Some(v) = opcua.get_mut(key) {
                    if v.is_string() {
                        *v = serde_json::Value::String(REDACTED_SECRET.to_string());
                    }
                }
            }
        }
    }
}

/// `GET /api/config` 查询参数（D-01：并入原 `/api/config/export` 的明文导出能力）。
#[derive(Debug, Default, serde::Deserialize)]
pub(crate) struct ConfigQuery {
    /// 是否返回明文凭据（默认 false）。为 true 时要求主 token 认证并落审计。
    #[serde(default)]
    pub include_secrets: bool,
}

/// GET /api/config
///
/// P2-2：与其余 API 一致使用 [`ApiResponse`] 信封。旧实现直接返回裸配置对象，
/// 前端 `request` 拦截器还得出「有没有 code 字段」来猜是否解包，外部脚本/网关
/// 联调也各写一套判断；统一信封后调用方只读 `data`。
///
/// D-01：原 `/api/config/export` 与本端点载荷逐字节等价，仅多一个 `include_secrets`
/// 分支，故合并为一个端点。默认**脱敏**（见 [`redact_config_secrets`]）；
/// `?include_secrets=true` 返回明文，仅主 token 可用（灾备迁移等场景），并落审计。
pub(crate) async fn get_config(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<ConfigQuery>,
    headers: axum::http::HeaderMap,
) -> Result<Json<ApiResponse<serde_json::Value>>, ApiError> {
    let cfg = state.config.load_full();
    let mut value = serde_json::to_value(&*cfg)
        .map_err(|e| ApiError::internal(format!("serialize config failed: {}", e)))?;
    if q.include_secrets {
        crate::web::api::config::handlers::ensure_primary_token(
            &state,
            &headers,
            "config.plaintext",
        )?;
    } else {
        redact_config_secrets(&mut value);
    }
    Ok(Json(ApiResponse::ok(value)))
}

pub(crate) async fn post_config(
    State(state): State<Arc<AppState>>,
    Json(new_config): Json<GatewayConfig>,
) -> Result<Json<ApiResponse<serde_json::Value>>, ApiError> {
    // 1.5 v2.0 license 配额校验（点位/协议/北向）—— 在变更前拦截超限保存
    // 统一走 config_apply::check_config_quota，与其余写端点（reload_then_save）共用同一实现。
    // 结构完整性校验（D1）已收敛到 config_apply::reload_then_save 内部统一执行。
    crate::web::config_apply::check_config_quota(&state, &new_config)?;

    // A5: 串行化整个热重载流程，防止并发配置更新交错产生半旧状态
    let _reload_guard = state.config.config_reload_lock.lock().await;
    let old = state.config.load_full();

    // 2-5. 展开设备模板 + 热重载 + 持久化（A5 顺序：reload → store → save）
    //      统一走 config_apply，消除与 rollback / restart_service 的重复实现。
    //      effective 为还原脱敏占位后的真实配置：版本快照与 web 变更判定都必须基于它，
    //      否则前端回传的 "********" 会被当作凭据落库、并误报 web 配置已变更。
    let (mut outcome, effective) =
        crate::web::config_apply::reload_then_save(&state, &old, &new_config).await?;

    // 记录配置热重载指标
    state.platform.metrics.inc_config_reload();

    match state
        .config
        .version_store
        .save_version(&effective, "user edit")
    {
        Ok(vid) => tracing::info!("config version v{} saved", vid),
        Err(e) => tracing::warn!("failed to save config version: {}", e),
    }

    // 6. Web 配置变更说明（基于真实生效配置比较，避免脱敏占位值造成误判）
    //    监听地址 / CORS / 限流 / 实时缓冲容量在启动期固化，其余鉴权字段由中间件动态读取。
    let web_changed = old.web != effective.web;
    if web_changed {
        if outcome.scope.restart_required.contains(&"web") {
            outcome.notes.push(
                "web config changed: requires gateway restart to take effect \
                 (listen_addr / cors_origins / rate_limit_per_sec / realtime_buffer are bound at startup)"
                    .to_string(),
            );
        } else {
            outcome.notes.push(
                "web config updated online (api_token / allow_anon_read / users take effect immediately)"
                    .to_string(),
            );
        }
    }

    let mut resp = serde_json::json!({"status": "ok"});
    outcome.apply_to(&mut resp);
    Ok(Json(ApiResponse::ok(resp)))
}

pub(crate) async fn get_version() -> impl IntoResponse {
    Json(VersionInfo {
        version: env!("CARGO_PKG_VERSION"),
        build_time: option_env!("BUILD_TIME").unwrap_or("unknown"),
        git_hash: option_env!("GIT_HASH").unwrap_or("unknown"),
    })
}

pub(crate) async fn list_config_versions(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ApiError> {
    let versions = state
        .config
        .version_store
        .list_versions()
        .map_err(ApiError::from)?;
    // 列表已按 id DESC 排序，首条即当前最新版本（与内存中最新保存的版本一致）
    let current_id = versions.first().map(|v| v.id);
    // 只返回元数据（不含完整 config_json，避免响应过大）
    // 前端 ConfigVersion 期望字段：id / created_at(秒) / message / size / is_current
    let summary: Vec<serde_json::Value> = versions
        .iter()
        .map(|v| {
            serde_json::json!({
                "id": v.id,
                "created_at": v.timestamp / 1000,
                "message": v.note,
                "size": v.config_json.len() as u64,
                "is_current": current_id == Some(v.id),
            })
        })
        .collect();
    Ok(Json(serde_json::json!({"versions": summary})))
}

/// 获取指定版本的配置快照（完整 JSON）
pub(crate) async fn get_config_version(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    let v = state
        .config
        .version_store
        .get_version(id)
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found(format!("version {} not found", id)))?;
    // config_json 是合法 JSON 字符串，直接解析为 serde_json::Value 透传
    let mut cfg: serde_json::Value = serde_json::from_str(&v.config_json)
        .map_err(|e| ApiError::internal(format!("parse config json failed: {}", e)))?;
    // P0-3：快照里可能带真实凭据（如来自 `GET /api/config?include_secrets=true` 的内容），
    // 返回前与 GET /api/config 做同样的脱敏。
    redact_config_secrets(&mut cfg);
    Ok(Json(cfg))
}

/// 两个版本之间的字段级差异对比
/// 查询参数：?compare_to=<id>，比较 base(id) 与 compare_to 两个版本
pub(crate) async fn compare_config_versions(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    axum::extract::Query(params): axum::extract::Query<CompareParams>,
) -> Result<impl IntoResponse, ApiError> {
    let other_id = params.compare_to;
    if id == other_id {
        return Ok(Json(serde_json::json!({
            "changed_fields": Vec::<String>::new(),
            "has_changes": false,
            "from": id,
            "to": other_id,
        })));
    }
    let from = state
        .config
        .version_store
        .get_version(id)
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found(format!("version {} not found", id)))?;
    let to = state
        .config
        .version_store
        .get_version(other_id)
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found(format!("version {} not found", other_id)))?;

    let from_cfg: GatewayConfig = serde_json::from_str(&from.config_json)
        .map_err(|e| ApiError::internal(format!("parse from config failed: {}", e)))?;
    let to_cfg: GatewayConfig = serde_json::from_str(&to.config_json)
        .map_err(|e| ApiError::internal(format!("parse to config failed: {}", e)))?;

    let diffs = config_field_diffs(&from_cfg, &to_cfg);
    Ok(Json(serde_json::json!({
        "changed_fields": diffs,
        "has_changes": !diffs.is_empty(),
        "from": serde_json::json!({ "id": from.id, "created_at": from.timestamp / 1000, "message": from.note }),
        "to":   serde_json::json!({ "id": to.id,   "created_at": to.timestamp / 1000,   "message": to.note   }),
    })))
}

pub(crate) async fn rollback_config_version(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    // A5: 串行化整个回滚流程
    let _reload_guard = state.config.config_reload_lock.lock().await;

    let old = state.config.load_full();
    let new_config = state
        .config
        .version_store
        .rollback(id)
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found(format!("version {} not found", id)))?;

    // C22: 回滚是「整份配置替换」，与 POST /api/config 同级，须同样做 license 配额前置校验；
    // 否则可先在高配额 license 下保存超配额版本、再降级 license 后回滚来绕过配额。
    // （自愈/重启路径 reload_services 刻意豁免此校验：license 降级后仍须能恢复运行，
    // 否则会因配额卡死重启，违背「稳定性第一」。）
    crate::web::config_apply::check_config_quota(&state, &new_config)?;

    // 展开设备模板 + 热重载 + 持久化（A5 顺序：reload → store → save）
    // 结构完整性校验（D1）已收敛到 config_apply::reload_then_save 内部统一执行。
    // effective 为还原脱敏占位后的真实配置：回滚后的版本快照必须基于它落库。
    let (outcome, effective) =
        crate::web::config_apply::reload_then_save(&state, &old, &new_config).await?;

    // 记录配置热重载指标
    state.platform.metrics.inc_config_reload();

    // D3: 回滚是敏感操作，须落审计（action=config.rollback，供自愈事件/审计查询消费）
    state.platform.audit.log(
        "config.rollback",
        "gateway",
        &format!("rollback from v{}", id),
        "api",
    );

    let note = format!("rollback from v{}", id);
    if let Ok(vid) = state.config.version_store.save_version(&effective, &note) {
        tracing::info!("rollback to v{} completed, new version v{}", id, vid);
    }

    let mut resp = serde_json::json!({"status": "ok", "rolled_back_to": id});
    outcome.apply_to(&mut resp);
    Ok(Json(resp))
}

/// 计算两份配置的字段级差异（与 config_diff / compare_config_versions 共用逻辑）
pub(crate) fn config_field_diffs(old: &GatewayConfig, new: &GatewayConfig) -> Vec<String> {
    let mut diffs = Vec::new();
    if old.buses != new.buses {
        diffs.push("buses".to_string());
    }
    if old.points != new.points {
        diffs.push("points".to_string());
    }
    if old.mqtt != new.mqtt {
        diffs.push("mqtt".to_string());
    }
    if old.web != new.web {
        diffs.push("web".to_string());
    }
    if old.ntp != new.ntp {
        diffs.push("ntp".to_string());
    }
    if old.devices != new.devices {
        diffs.push("devices".to_string());
    }
    if old.webhook != new.webhook {
        diffs.push("webhook".to_string());
    }
    if old.cellular != new.cellular {
        diffs.push("cellular".to_string());
    }
    if old.opcua != new.opcua {
        diffs.push("opcua".to_string());
    }
    if old.modbus_server != new.modbus_server {
        diffs.push("modbus_server".to_string());
    }
    if old.modbus_passthrough != new.modbus_passthrough {
        diffs.push("modbus_passthrough".to_string());
    }
    if old.telemetry != new.telemetry {
        diffs.push("telemetry".to_string());
    }
    if old.device_groups != new.device_groups {
        diffs.push("device_groups".to_string());
    }
    if old.device_assets != new.device_assets {
        diffs.push("device_assets".to_string());
    }
    if old.virtual_points != new.virtual_points {
        diffs.push("virtual_points".to_string());
    }
    diffs
}

/// 配置差异对比：比较提交的配置与当前内存配置，返回差异字段
pub(crate) async fn config_diff(
    State(state): State<Arc<AppState>>,
    Json(new_config): Json<GatewayConfig>,
) -> Result<impl IntoResponse, ApiError> {
    let old = state.config.load_full();
    let diffs = config_field_diffs(&old, &new_config);
    Ok(Json(serde_json::json!({
        "changed_fields": diffs,
        "has_changes": !diffs.is_empty(),
    })))
}
