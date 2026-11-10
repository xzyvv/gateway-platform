//! 测点 CRUD handler（从原 handlers.rs 拆分）
//!
//! 路由：
//! - GET    /api/devices/:id/points -> list_points
//! - POST   /api/devices/:id/points -> create_point
//! - PUT    /api/points/:id         -> update_point
//! - DELETE /api/points/:id         -> delete_point（软删除，进回收站）
//! - GET    /api/point-trash       -> list_point_trash
//! - DELETE /api/point-trash       -> purge_all_point_trash
//! - DELETE /api/point-trash/:id   -> purge_point
//! - POST   /api/points/:id/restore -> restore_point

use std::time::{SystemTime, UNIX_EPOCH};

use super::*;
use gateway_core::config::allowed_function_codes;
use gateway_core::engine::error::{GatewayError, GatewayResult};

/// 回收站容量上限：按 `deleted_at_ms` 只保留最新 200 条
pub(crate) const POINT_TRASH_MAX: usize = 200;

/// 回收站保留时长：30 天（毫秒），超期条目在写入时顺带清理
pub(crate) const POINT_TRASH_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// 当前 Unix 毫秒（软删除时间戳）
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 写入回收站，并顺带执行 TTL + 容量清理（所有软删除入口共用）。
///
/// 容量裁剪在写入时做（而非读取时），保证落盘配置文件本身不无限膨胀。
pub(crate) fn push_trash(cfg: &mut GatewayConfig, entry: PointTrashEntry) {
    let now = entry.deleted_at_ms;
    let trash = cfg.point_trash.get_or_insert_with(Vec::new);
    trash.push(entry);
    // TTL：早于 now-30 天的条目直接丢弃
    trash.retain(|e| now.saturating_sub(e.deleted_at_ms) <= POINT_TRASH_TTL_MS);
    // 容量：超限时按删除时间保留最新 200 条
    if trash.len() > POINT_TRASH_MAX {
        trash.sort_by_key(|a| std::cmp::Reverse(a.deleted_at_ms));
        trash.truncate(POINT_TRASH_MAX);
    }
}
// ===== 测点 CRUD =====

/// GET /api/devices/:id/points — 列出设备下测点
///
/// BUG-E：设备不存在时返回 404。原先无论设备是否存在都回 200 + 空数组，
/// 前端无法区分「设备没有点位」与「设备根本不存在」，会误判成配置没保存。
/// 点位来源改用 `expand_points`：模板化设备的点位由模板展开生成，不在 `cfg.points` 里，
/// 只查 `cfg.points` 会让模板设备的点位下拉恒为空。
pub async fn list_points(
    State(state): State<Arc<AppState>>,
    Path(device_id): Path<String>,
) -> Result<Json<ApiResponse<Vec<PointConfig>>>, ApiError> {
    let cfg = state.config.load_full();
    let device_exists = cfg
        .devices
        .as_ref()
        .is_some_and(|ds| ds.iter().any(|d| d.id == device_id));
    if !device_exists {
        return Err(ApiError::not_found(format!("设备 {device_id} 不存在")));
    }
    let points: Vec<PointConfig> = crate::web::config_apply::expand_points(&state, &cfg)
        .into_iter()
        .filter(|p| p.device_id.as_deref() == Some(&device_id))
        .collect();
    Ok(Json(ApiResponse::ok(points)))
}

/// POST /api/devices/:id/points — 新增测点
pub async fn create_point(
    State(state): State<Arc<AppState>>,
    Path(device_id): Path<String>,
    Json(payload): Json<PointPayload>,
) -> Result<ApiResponse<PointConfig>, ApiError> {
    // D2: 串行化「读→改→写」，防止并发配置变更丢更新（与 users/groups 等一致）
    let _guard = state.config.config_reload_lock.lock().await;
    let old = state.config.load_full();
    let mut cfg = old.as_ref().clone();
    // Q17: 校验 device_id 存在性
    let device_exists = cfg
        .devices
        .as_ref()
        .map(|ds| ds.iter().any(|d| d.id == device_id))
        .unwrap_or(false);
    if !device_exists {
        return Ok(ApiResponse::err(4004, "device not found"));
    }
    // Bug 108: 校验 ID 唯一性——避免覆盖已有测点（用户常常没意识到 id 已存在）。
    // BUG-H 同源：模板展开出来的点位虽然不在 cfg.points 里，但同样占用 id 空间，
    // 只查 cfg.points 会让用户建出「与模板点位同 id」的手动点位，展开合并后重复采集。
    if cfg.points.iter().any(|p| p.id == payload.id)
        || crate::web::config_apply::expand_points(&state, &cfg)
            .iter()
            .any(|p| p.id == payload.id)
    {
        return Ok(ApiResponse::err(
            4002,
            format!("point id '{}' 已存在", payload.id),
        ));
    }
    if let Err(e) = validate_point(&payload, &cfg) {
        return Ok(ApiResponse::err(4002, e.message()));
    }
    // N3: 总线/协议必须继承所属设备
    if let Err(msg) =
        validate_point_inherits_device(&cfg, &device_id, &payload.bus, &payload.protocol)
    {
        return Ok(ApiResponse::err(4002, msg));
    }
    let point = PointConfig {
        id: payload.id,
        device_id: Some(device_id),
        name: payload.name,
        bus: payload.bus,
        protocol: payload.protocol,
        address: payload.address,
        slave_id: payload.slave_id,
        address_num: payload.address_num,
        data_id: payload.data_id,
        function_code: payload.function_code,
        data_type: payload.data_type,
        byte_order: payload.byte_order,
        register_count: payload.register_count,
        interval: payload.interval,
        scale: payload.scale.unwrap_or(1.0),
        offset: payload.offset.unwrap_or(0.0),
        formula: payload.formula,
        ct_ratio: payload.ct_ratio,
        pt_ratio: payload.pt_ratio,
        deadband: payload.deadband.unwrap_or(0.0),
        on_change: payload.on_change.unwrap_or(false),
        unit: payload.unit,
        aggregation: payload.aggregation.map(|a| a.into_config()),
        alarm_high: payload.alarm_high,
        alarm_low: payload.alarm_low,
        modbus_register: payload.modbus_register,
        // S11: 写入白名单默认拒绝，需显式开启
        writable: payload.writable.unwrap_or(false),
    };
    cfg.points.push(point.clone());
    apply_reload(&state, &old, cfg).await?;
    state.platform.audit.log(
        "point.create",
        &point.id,
        &format!(
            "device={} @ {}",
            point.device_id.as_deref().unwrap_or("-"),
            point.address
        ),
        "api",
    );
    Ok(ApiResponse::ok(point))
}

/// PUT /api/points/:id — 修改测点
pub async fn update_point(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<PointPayload>,
) -> Result<ApiResponse<()>, ApiError> {
    // D2: 串行化「读→改→写」
    let _guard = state.config.config_reload_lock.lock().await;
    let old = state.config.load_full();
    let mut cfg = old.as_ref().clone();
    // Bug 39: validate payload fields (skip uniqueness check, ID unchanged)
    if payload.protocol.is_empty() {
        return Ok(ApiResponse::err(4002, "protocol 不能为空"));
    }
    if payload.interval == 0 {
        return Ok(ApiResponse::err(4002, "interval 必须大于 0"));
    }
    // Bug 228: 显式 scale=0 会让采集值恒等于 offset，拒绝而非静默归零。
    if payload.scale == Some(0.0) {
        return Ok(ApiResponse::err(
            4002,
            "scale 不能为 0（会使采集值恒等于 offset；如需 1:1 透传请省略或设为 1.0）",
        ));
    }
    // CT/PT 变比必须 > 0：0 会把一次侧值乘成 0，负数反号，NaN/Inf 污染上送。
    if let Some(msg) = invalid_ratio_message(payload.ct_ratio, payload.pt_ratio) {
        return Ok(ApiResponse::err(4002, msg));
    }
    // 功能码必须符合所属协议的语义（与 create_point 同源）。
    if let Some(msg) =
        invalid_function_code_message(payload.protocol.as_str(), payload.function_code)
    {
        return Ok(ApiResponse::err(4002, msg));
    }
    // 采集周期必须小于北向上报周期：与 GatewayConfig::validate_cycle_conflict 同源。
    // 不再硬编码 60s——北向周期本身可配，硬编码会导致「把上报周期改大后仍按 60 拦截」。
    if cfg.mqtt.enabled && payload.interval >= cfg.mqtt.publish_interval {
        return Ok(ApiResponse::err(
            4002,
            format!(
                "采集周期必须小于北向上报周期（当前 {}s）",
                cfg.mqtt.publish_interval
            ),
        ));
    }
    // Bug 109: 复用 VALID_PROTOCOLS 常量，避免与 update_device 不同步。
    if !VALID_PROTOCOLS.contains(&payload.protocol.as_str()) {
        return Ok(ApiResponse::err(
            4002,
            format!("不支持的协议: {}", payload.protocol),
        ));
    }
    // Bug 39: validate device_id existence if specified
    if let Some(did) = &payload.device_id {
        let exists = cfg
            .devices
            .as_ref()
            .map(|ds| ds.iter().any(|d| &d.id == did))
            .unwrap_or(false);
        if !exists {
            return Ok(ApiResponse::err(
                4004,
                format!("device id '{}' 不存在", did),
            ));
        }
    }
    // N3: 总线/协议必须继承所属设备。payload 带了 device_id 就用它，
    // 否则回退到该点位原属设备（编辑表单只改地址等字段时也要校验）。
    let owner_device_id = match payload.device_id.as_deref() {
        Some(did) => Some(did.to_string()),
        None => cfg
            .points
            .iter()
            .find(|p| p.id == id)
            .and_then(|p| p.device_id.clone()),
    };
    if let Some(did) = owner_device_id {
        if let Err(msg) =
            validate_point_inherits_device(&cfg, &did, &payload.bus, &payload.protocol)
        {
            return Ok(ApiResponse::err(4002, msg));
        }
    }
    let found = cfg.points.iter_mut().find(|p| p.id == id).map(|p| {
        // Bug 39: only update device_id if payload provides one; keep existing otherwise
        if let Some(did) = &payload.device_id {
            p.device_id = Some(did.clone());
        }
        if let Some(n) = payload.name.clone() {
            p.name = Some(n);
        }
        p.bus = payload.bus.clone();
        p.protocol = payload.protocol.clone();
        p.address = payload.address.clone();
        p.slave_id = payload.slave_id;
        p.address_num = payload.address_num;
        p.data_id = payload.data_id.clone();
        p.function_code = payload.function_code;
        p.data_type = payload.data_type.clone();
        p.byte_order = payload.byte_order.clone();
        p.register_count = payload.register_count;
        p.interval = payload.interval;
        if let Some(s) = payload.scale {
            p.scale = s;
        }
        if let Some(o) = payload.offset {
            p.offset = o;
        }
        // BUG-B 收尾：unit 必须是「整行覆盖」语义。前端清空单位时传 undefined（JSON 里
        // 直接省略 → None），若沿用「有值才写」就再也无法清除已存的单位。所有调用方
        // （点位表单 / 行内编辑 / 批量编辑）都回传整行 PointConfig，改成无条件赋值安全。
        p.unit = payload.unit.clone();
        // BUG-B：以下字段此前完全没有落盘路径（结构体本身缺字段），
        // 编辑表单改了不生效。现按「有值即覆盖」语义写入。
        p.formula = payload.formula.clone();
        p.ct_ratio = payload.ct_ratio;
        p.pt_ratio = payload.pt_ratio;
        if let Some(d) = payload.deadband {
            p.deadband = d;
        }
        if let Some(oc) = payload.on_change {
            p.on_change = oc;
        }
        p.aggregation = payload.aggregation.clone().map(|a| a.into_config());
        p.alarm_high = payload.alarm_high;
        p.alarm_low = payload.alarm_low;
        p.modbus_register = payload.modbus_register;
        // S11: 可显式开关写入白名单
        if let Some(w) = payload.writable {
            p.writable = w;
        }
    });
    if found.is_none() {
        return Ok(ApiResponse::err(4004, "point not found"));
    }
    apply_reload(&state, &old, cfg).await?;
    state
        .platform
        .audit
        .log("point.update", &id, "更新点位配置", "api");
    Ok(ApiResponse::ok(()))
}

/// DELETE /api/points/:id — 删除测点（软删除：移入回收站）
pub async fn delete_point(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<ApiResponse<()>, ApiError> {
    // D2: 串行化「读→改→写」
    let _guard = state.config.config_reload_lock.lock().await;
    let old = state.config.load_full();
    let mut cfg = old.as_ref().clone();
    let Some(pos) = cfg.points.iter().position(|p| p.id == id) else {
        return Ok(ApiResponse::err(4004, "point not found"));
    };
    let point = cfg.points.remove(pos);
    push_trash(
        &mut cfg,
        PointTrashEntry {
            point,
            deleted_at_ms: now_ms(),
        },
    );
    apply_reload(&state, &old, cfg).await?;
    state
        .platform
        .audit
        .log("point.delete", &id, "软删除（移入回收站）", "api");
    Ok(ApiResponse::ok(()))
}

/// POST /api/points/:id/restore — 从回收站恢复测点
pub async fn restore_point(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<ApiResponse<PointConfig>, ApiError> {
    // D2: 串行化「读→改→写」
    let _guard = state.config.config_reload_lock.lock().await;
    let old = state.config.load_full();
    let mut cfg = old.as_ref().clone();
    let Some(pos) = cfg
        .point_trash
        .as_ref()
        .and_then(|t| t.iter().position(|e| e.point.id == id))
    else {
        return Ok(ApiResponse::err(4004, "point not found"));
    };
    if cfg.points.iter().any(|p| p.id == id) {
        return Ok(ApiResponse::err(4009, "点位 id 已存在，无法恢复"));
    }
    let entry = match cfg.point_trash.as_mut() {
        Some(t) if pos < t.len() => t.remove(pos),
        _ => return Ok(ApiResponse::err(4004, "point not found")),
    };
    if cfg.point_trash.as_ref().is_some_and(|t| t.is_empty()) {
        cfg.point_trash = None;
    }
    let point = entry.point;
    cfg.points.push(point.clone());
    apply_reload(&state, &old, cfg).await?;
    state
        .platform
        .audit
        .log("point.restore", &id, "从回收站恢复点位", "api");
    Ok(ApiResponse::ok(point))
}

/// GET /api/point-trash — 列出回收站（按删除时间倒序）
pub async fn list_point_trash(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<PointTrashEntry>>>, ApiError> {
    let cfg = state.config.load_full();
    let mut trash = cfg.point_trash.clone().unwrap_or_default();
    trash.sort_by_key(|a| std::cmp::Reverse(a.deleted_at_ms));
    Ok(Json(ApiResponse::ok(trash)))
}

/// DELETE /api/point-trash/:id — 彻底删除回收站中的单条
pub async fn purge_point(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<ApiResponse<()>, ApiError> {
    // D2: 串行化「读→改→写」
    let _guard = state.config.config_reload_lock.lock().await;
    let old = state.config.load_full();
    let mut cfg = old.as_ref().clone();
    let removed = match cfg.point_trash.as_mut() {
        Some(trash) => {
            let before = trash.len();
            trash.retain(|e| e.point.id != id);
            before - trash.len()
        }
        None => 0,
    };
    if removed == 0 {
        return Ok(ApiResponse::err(4004, "point not found"));
    }
    if cfg.point_trash.as_ref().is_some_and(|t| t.is_empty()) {
        cfg.point_trash = None;
    }
    apply_reload(&state, &old, cfg).await?;
    state
        .platform
        .audit
        .log("point.trash.purge", &id, "彻底删除回收站点位", "api");
    Ok(ApiResponse::ok(()))
}

/// DELETE /api/point-trash — 清空回收站（全部彻底删除）
pub async fn purge_all_point_trash(
    State(state): State<Arc<AppState>>,
) -> Result<ApiResponse<()>, ApiError> {
    // D2: 串行化「读→改→写」
    let _guard = state.config.config_reload_lock.lock().await;
    let old = state.config.load_full();
    let mut cfg = old.as_ref().clone();
    cfg.point_trash = None;
    apply_reload(&state, &old, cfg).await?;
    state
        .platform
        .audit
        .log("point.trash.clear", "all", "清空回收站", "api");
    Ok(ApiResponse::ok(()))
}

/// 测点创建请求校验（create_point 用）
fn validate_point(payload: &PointPayload, cfg: &GatewayConfig) -> GatewayResult<()> {
    if payload.protocol.is_empty() {
        return Err(GatewayError::Config("protocol 不能为空".into()));
    }
    // 协议合法性（含 gpio 点位级协议），白名单统一来源见 registry.rs
    if !VALID_PROTOCOLS.contains(&payload.protocol.as_str()) {
        return Err(GatewayError::Config(format!(
            "不支持的协议: {}",
            payload.protocol
        )));
    }
    if payload.interval == 0 {
        return Err(GatewayError::Config("interval 必须大于 0".into()));
    }
    // 采集周期必须小于北向上报周期：与 update_point / validate_cycle_conflict 三处同源。
    // 旧实现硬编码 60s，与可配置的 mqtt.publish_interval 脱节，现按实际配置判定。
    if cfg.mqtt.enabled && payload.interval >= cfg.mqtt.publish_interval {
        return Err(GatewayError::Config(format!(
            "采集周期必须小于北向上报周期（当前 {}s）",
            cfg.mqtt.publish_interval
        )));
    }
    // Bug 228: 显式 scale=0 会让采集值恒等于 offset（读路径 raw*scale+offset），
    // 且与写路径 reverse_convert（把 0 当作"不缩放"）语义相反，必须拒绝而非静默归零。
    if let Some(s) = payload.scale {
        if s == 0.0 {
            return Err(GatewayError::Config(
                "scale 不能为 0（会使采集值恒等于 offset；如需 1:1 透传请省略或设为 1.0）".into(),
            ));
        }
    }
    // CT/PT 变比必须 > 0：与 GatewayConfig 的 validate_ratio 同源，提前给出明确字段名。
    if let Some(msg) = invalid_ratio_message(payload.ct_ratio, payload.pt_ratio) {
        return Err(GatewayError::Config(msg));
    }
    // 功能码必须符合所属协议的语义：Modbus 取值写错会长期采不到值或写错寄存器。
    // 与 config/validate/gate.rs 的结构级校验同源，但这里提前给出可读提示。
    if let Some(msg) =
        invalid_function_code_message(payload.protocol.as_str(), payload.function_code)
    {
        return Err(GatewayError::Config(msg));
    }
    Ok(())
}

/// 功能码与协议不匹配时的提示（`None` = 合法）。
///
/// 允许集合的唯一来源是 `gateway_core::config::allowed_function_codes`，
/// 与结构级校验（`gate.rs`）同源，避免两处白名单漂移。
/// DLT645/CJT188 的功能码是「写控制码」（0x04 写数据 / 0x0C 写通信地址 /
/// 0x0F 写密码 / 0x15 写通信地址），语义与 Modbus 完全不同，不能共用白名单。
fn invalid_function_code_message(protocol: &str, fc: Option<u8>) -> Option<String> {
    let fc = fc?;
    let allowed = allowed_function_codes(protocol)?;
    if allowed.contains(&fc) {
        return None;
    }
    Some(format!(
        "功能码 {} 对协议 {} 非法（允许 {:?}）；Modbus 功能码决定读写哪类寄存器，写错会长期采不到值或写错寄存器",
        fc, protocol, allowed
    ))
}

/// CT/PT 变比合法性：必须是有限正数（None 表示不放大，合法）。
///
/// 0 会把一次侧值乘成 0（采集成功但上报恒 0），负数会反号，NaN/Inf 会污染
/// 聚合与上送报文；都在写入口拦下，避免坏配置落盘后才在下一次加载时暴雷。
fn invalid_ratio_message(ct_ratio: Option<f64>, pt_ratio: Option<f64>) -> Option<String> {
    for (name, ratio) in [("ct_ratio", ct_ratio), ("pt_ratio", pt_ratio)] {
        if let Some(r) = ratio {
            if !r.is_finite() || r <= 0.0 {
                return Some(format!(
                    "{name} 必须大于 0（如 150/5 填 30、10000/100 填 100；无需变比请留空，不要填 0）"
                ));
            }
        }
    }
    None
}

/// N3: 点位总线/协议必须继承所属设备。
///
/// 点位按 `bus` 路由到具体采集器、按 `protocol` 解析报文。任一项与设备不一致时，
/// 采集器按设备总线建不出来、或按错误协议读寄存器，该点位必然长期采不到值，
/// 而后端既不报错也不提示——属于「配置静默失效」，必须在写入时拦下。
fn validate_point_inherits_device(
    cfg: &GatewayConfig,
    device_id: &str,
    bus: &str,
    protocol: &str,
) -> Result<(), String> {
    let Some(dev) = cfg
        .devices
        .as_ref()
        .and_then(|ds| ds.iter().find(|d| d.id == device_id))
    else {
        return Ok(());
    };
    if dev.bus != bus {
        return Err(format!(
            "点位总线 '{}' 与设备 '{}' 的总线 '{}' 不一致：点位总线必须继承所属设备，如需更换请到设备列表修改",
            bus, dev.id, dev.bus
        ));
    }
    if dev.protocol != protocol {
        return Err(format!(
            "点位协议 '{}' 与设备 '{}' 的协议 '{}' 不一致：点位协议必须继承所属设备",
            protocol, dev.id, dev.protocol
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
