//! 设备 CRUD handler（从原 handlers.rs 拆分）
//!
//! 路由：
//! - GET    /api/devices       -> list_devices
//! - POST   /api/devices       -> create_device
//! - PUT    /api/devices/:id   -> update_device
//! - DELETE /api/devices/:id   -> delete_device

use super::*;
use gateway_core::engine::error::{GatewayError, GatewayResult};
// ===== 设备 CRUD =====

/// GET /api/devices — 列出所有设备
pub async fn list_devices(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<DeviceConfig>>>, ApiError> {
    let cfg = state.config.load_full().as_ref().clone();
    let devices = cfg.devices.unwrap_or_default();
    Ok(Json(ApiResponse::ok(devices)))
}

// Bug 106: 错误码体系——
//   4001: 设备相关校验失败（payload 字段缺失/格式错误）
//   4002: 测点相关校验失败
//   4003: 配置导入校验失败
//   4004: 资源不存在（device not found / point not found）
// 客户端可根据错误码区分处理策略（重试/弹窗/重定向）。

/// POST /api/devices — 新增设备
pub async fn create_device(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<DevicePayload>,
) -> Result<ApiResponse<DeviceConfig>, ApiError> {
    // D2: 串行化「读→改→写」，防止并发配置变更丢更新
    let _guard = state.config.config_reload_lock.lock().await;
    let old = state.config.load_full().as_ref().clone();
    let mut cfg = old.clone();
    if let Err(e) = validate_device(&payload, &cfg) {
        return Ok(ApiResponse::err(4001, e.message()));
    }
    // S2: 设备 ID 唯一性校验。原实现直接 `devices.push(device)`，同 ID 可并存；
    // 而 delete_device 按 id 全量 retain，删一台会连带删掉同 ID 的另一台及其全部点位。
    if cfg
        .devices
        .as_ref()
        .is_some_and(|ds| ds.iter().any(|d| d.id == payload.id))
    {
        return Ok(ApiResponse::err(
            4001,
            format!("device id '{}' 已存在，请换一个设备名重试", payload.id),
        ));
    }
    // S1: 模板存在性校验。此前填任意字符串都能创建成功，直到展开阶段静默失效成 0 点位。
    if let Err(msg) = validate_template_exists(&state, &payload.template) {
        return Ok(ApiResponse::err(4001, msg));
    }
    // N1: 总线存在性校验。此前总线填错也能创建成功，采集器按 bus 建不起来
    // （仅日志 warn "no collector for bus"），设备永远采不到数据且界面无任何提示。
    if let Err(msg) = validate_bus_exists(&cfg, &payload.bus) {
        return Ok(ApiResponse::err(4001, msg));
    }
    // N2: 设备协议必须与模板协议一致。模板展开点位时用的是 device.protocol，
    // 选 DLT645 模板却填 modbus 会按 modbus 去读 DLT645 地址，必然全部失败。
    if let Err(msg) = validate_template_protocol(&state, &payload.template, &payload.protocol) {
        return Ok(ApiResponse::err(4001, msg));
    }
    let device = DeviceConfig {
        id: payload.id,
        name: payload.name,
        template: payload.template,
        bus: payload.bus,
        protocol: payload.protocol,
        address: payload.address,
        slave_id: payload.slave_id,
        address_map: None,
        interval: payload.interval,
    };
    with_devices_mut(&mut cfg, |devices| devices.push(device.clone()));
    apply_reload(&state, &old, cfg).await?;
    // 设备新增改变采集拓扑，补一条带设备名的专用审计（通用 config.update 不足以定位）
    state.platform.audit.log(
        "device.create",
        &format!("device:{}", device.id),
        &format!(
            "name={} bus={} protocol={}",
            device.name, device.bus, device.protocol
        ),
        "api",
    );
    Ok(ApiResponse::ok(device))
}

/// PUT /api/devices/:id — 修改设备
pub async fn update_device(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(payload): Json<DevicePayload>,
) -> Result<ApiResponse<()>, ApiError> {
    // D2: 串行化「读→改→写」
    let _guard = state.config.config_reload_lock.lock().await;
    let old = state.config.load_full().as_ref().clone();
    let mut cfg = old.clone();
    // Bug 38: validate payload fields (skip uniqueness check, ID unchanged)
    if payload.name.is_empty() {
        return Ok(ApiResponse::err(4001, "device name 不能为空"));
    }
    if payload.protocol.is_empty() {
        return Ok(ApiResponse::err(4001, "protocol 不能为空"));
    }
    if payload.bus.is_empty() {
        return Ok(ApiResponse::err(4001, "bus 不能为空"));
    }
    if payload.template.is_empty() {
        return Ok(ApiResponse::err(4001, "template 不能为空"));
    }
    // Bug 109: 复用 VALID_PROTOCOLS 常量，与 update_point 保持一致。
    if !VALID_PROTOCOLS.contains(&payload.protocol.as_str()) {
        return Ok(ApiResponse::err(
            4001,
            format!("不支持的协议: {}", payload.protocol),
        ));
    }
    // S1: 编辑时同样校验模板存在性（模板名可在弹窗里被改错）。
    if let Err(msg) = validate_template_exists(&state, &payload.template) {
        return Ok(ApiResponse::err(4001, msg));
    }
    // N1/N2: 编辑时同样校验总线存在性与模板协议一致性，否则改一次设备就把
    // 原本合法的配置改坏，且同样是静默失效。
    if let Err(msg) = validate_bus_exists(&cfg, &payload.bus) {
        return Ok(ApiResponse::err(4001, msg));
    }
    if let Err(msg) = validate_template_protocol(&state, &payload.template, &payload.protocol) {
        return Ok(ApiResponse::err(4001, msg));
    }
    let found = with_devices_mut(&mut cfg, |devices| {
        devices.iter_mut().find(|d| d.id == id).map(|d| {
            d.name = payload.name.clone();
            d.template = payload.template.clone();
            d.bus = payload.bus.clone();
            d.protocol = payload.protocol.clone();
            d.address = payload.address.clone();
            d.slave_id = payload.slave_id;
            d.interval = payload.interval;
        })
    });
    if found.is_none() {
        return Ok(ApiResponse::err(4004, "device not found"));
    }
    apply_reload(&state, &old, cfg).await?;
    state.platform.audit.log(
        "device.update",
        &format!("device:{}", id),
        &format!(
            "name={} bus={} protocol={}",
            payload.name, payload.bus, payload.protocol
        ),
        "api",
    );
    Ok(ApiResponse::ok(()))
}

/// DELETE /api/devices/:id — 删除设备（级联删除其下所有测点）
pub async fn delete_device(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<ApiResponse<()>, ApiError> {
    // D2: 串行化「读→改→写」
    let _guard = state.config.config_reload_lock.lock().await;
    let old = state.config.load_full().as_ref().clone();
    let mut cfg = old.clone();
    let mut removed = false;
    if let Some(devices) = cfg.devices.as_mut() {
        let before = devices.len();
        devices.retain(|d| d.id != id);
        removed = devices.len() != before;
    }
    if !removed {
        return Ok(ApiResponse::err(4004, "device not found"));
    }
    // 级联删除该设备下所有测点（依赖 PointConfig.device_id 字段）
    let removed_points = cfg.points.len();
    cfg.points.retain(|p| p.device_id.as_deref() != Some(&id));
    let cascade_count = removed_points - cfg.points.len();
    if cascade_count > 0 {
        tracing::warn!(
            "device '{}' deleted with cascade: {} point(s) removed",
            id,
            cascade_count
        );
    }
    apply_reload(&state, &old, cfg).await?;
    // E1: 级联删除测点是破坏性操作，须落审计——原实现只打 tracing::warn，
    // 审计日志中无任何删除记录。与 D3 的 config.rollback / ota.* 单独审计一致：
    // 在统一入口已落的通用 config.update 之外，再补一条带设备与级联数量的
    // 专用记录，便于事后追溯「删了哪台设备、连带删了多少测点」。
    // 放在 apply_reload 成功之后：保存/热重载失败时删除并未生效，不应留审计。
    state.platform.audit.log(
        "config.delete",
        &format!("device:{}", id),
        &format!(
            "deleted device '{}' with cascade: {} point(s) removed",
            id, cascade_count
        ),
        "api",
    );
    Ok(ApiResponse::ok(()))
}

/// S1: 模板存在性校验。
///
/// 返回 `Err(可读文案)` 时直接作为 4001 响应体；模板库为空时给出「模板目录可能未加载」
/// 的引导，而不是让用户对着「template not found」反复试错。
fn validate_template_exists(state: &Arc<AppState>, template: &str) -> Result<(), String> {
    if state.config.templates.get(template).is_some() {
        return Ok(());
    }
    let known: Vec<String> = state
        .config
        .templates
        .list()
        .into_iter()
        .map(|t| t.name)
        .collect();
    if known.is_empty() {
        Err(format!(
            "模板 '{}' 不存在：当前网关未加载到任何模板，请检查模板目录 \
             （GATEWAY_TEMPLATE_BUILTIN / GATEWAY_TEMPLATE_CUSTOM）",
            template
        ))
    } else {
        Err(format!(
            "模板 '{}' 不存在，可用模板：{}",
            template,
            known.join("、")
        ))
    }
}

/// 设备创建请求校验（create_device 用）
fn validate_device(payload: &DevicePayload, _cfg: &GatewayConfig) -> GatewayResult<()> {
    if payload.name.is_empty() {
        return Err(GatewayError::Config("device name 不能为空".into()));
    }
    if payload.protocol.is_empty() {
        return Err(GatewayError::Config("protocol 不能为空".into()));
    }
    if payload.bus.is_empty() {
        return Err(GatewayError::Config("bus 不能为空".into()));
    }
    if payload.template.is_empty() {
        return Err(GatewayError::Config("template 不能为空".into()));
    }
    // 协议合法性（含 gpio），白名单统一来源见 registry.rs
    if !VALID_PROTOCOLS.contains(&payload.protocol.as_str()) {
        return Err(GatewayError::Config(format!(
            "不支持的协议: {}",
            payload.protocol
        )));
    }
    Ok(())
}

/// N1: 总线存在性校验。
///
/// 设备挂在一条不存在的总线上时，采集器不会为该总线创建 collector，
/// 采集永远起不来且只留一行 warn 日志；界面侧看不出任何异常。
/// 一条总线都没有时给出「先去运行参数新增总线」的引导，避免用户反复改名试错。
fn validate_bus_exists(cfg: &GatewayConfig, bus: &str) -> Result<(), String> {
    if cfg.buses.iter().any(|b| b.port_name == bus) {
        return Ok(());
    }
    if cfg.buses.is_empty() {
        Err(format!(
            "总线 '{}' 不存在：当前网关还没有配置任何总线，请先到「系统设置 → 运行参数」新增一条总线，设备才能挂上去采集",
            bus
        ))
    } else {
        let known: Vec<&str> = cfg.buses.iter().map(|b| b.port_name.as_str()).collect();
        Err(format!(
            "总线 '{}' 不存在，可用总线：{}",
            bus,
            known.join("、")
        ))
    }
}

/// N2: 设备协议必须与模板协议一致。
///
/// 模板展开点位时用的是设备自身的 protocol，而不是模板里的 protocol；
/// 两者不一致会按错误协议去读寄存器（如把 DLT645 表当 Modbus 读），
/// 结果是整台设备全点位失败。模板不存在的情况由 [`validate_template_exists`]
/// 负责报错，这里只做一致性比对。
fn validate_template_protocol(
    state: &Arc<AppState>,
    template: &str,
    protocol: &str,
) -> Result<(), String> {
    let Some(tpl) = state.config.templates.get(template) else {
        return Ok(());
    };
    if tpl.protocol.is_empty() || tpl.protocol == protocol {
        return Ok(());
    }
    Err(format!(
        "模板 '{}' 的协议是 {}，与设备协议 {} 不一致：设备协议现在由模板决定，请重新选择模板",
        template, tpl.protocol, protocol
    ))
}
