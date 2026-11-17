//! 配置变更统一辅助（C1：收敛 web 各 handler 重复的持久化/热重载机械步骤）
//!
//! web 有 10+ 个写端点都要做「保存 config → 更新内存 → 展开设备模板 → 热重载」，
//! 历史实现散落在各 handler 中且顺序不一。本模块集中这几步，消除重复。
//!
//! # 统一顺序（D-06 / E 批）
//! 所有配置写回只走 [`persist_then_apply`]：expand → **save（落盘）** → reload → store → notify。
//!
//! 落盘前置（E 批）：先写盘，成功后才热重载运行态、替换内存快照、通知北向。
//! 这样「保存」是原子的——写盘失败时内存/运行态/磁盘都还是旧配置，接口直接报错。
//! 旧顺序（reload → store → notify → save）在写盘失败时会出现「内存与运行态是新配置、
//! 磁盘是旧配置」，而接口只回 500；现场重启一次配置就悄悄回退，无从察觉。
//!
//! 落盘成功但热重载被拒时，回写旧配置把磁盘一起退回（保持「整体不生效」）。回写
//! 也失败才退化为「磁盘已新、运行态仍旧」，届时错误消息里明确告知需重启生效。
//!
//! 历史上并存 `save_then_reload`（先写盘后热更）与 `save_and_store`（纯元数据落盘）
//! 两套路径；前者与本路径的 `expand_points` 口径存在漂移温床（现场表现为
//! 「重启后正常、热更后点位错位」），现已统一收敛，只保留单一路径。
//!
//! 调用方须**已持有 `config_reload_lock`**（本模块不负责加锁）。
//!
//! D3：所有经本模块落盘的配置变更在成功后会写入一条**真实审计记录**
//! （`AuditLog`，非 tracing 桩），消除原先「仅 `POST /api/config` 有审计、
//! 增量编辑与元数据端点多无记录」的覆盖不一致。

use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::web::error::ApiError;
use crate::web::types::AppState;
use gateway_core::config::{reload_scope, GatewayConfig, PointConfig, ReloadScope};
use gateway_core::north::server::ActivePointSet;

/// 一次配置变更的热更新结果。
///
/// - `notes`：人类可读说明（保持既有 `notes` 字段语义，前端直接展示）；
/// - `applied` / `restart_required`：稳定的分节 key，供前端/外部客户端判断
///   「哪些已在线生效、哪些必须重启网关」。未覆盖的分节不再静默成功。
#[derive(Debug, Clone, Default)]
pub(crate) struct ReloadOutcome {
    pub notes: Vec<String>,
    pub scope: ReloadScope,
}

impl ReloadOutcome {
    fn new(notes: Vec<String>, scope: ReloadScope) -> Self {
        Self { notes, scope }
    }

    /// 追加到响应 JSON 的统一片段：`notes` / `applied` / `restart_required`。
    ///
    /// `notes` 为空时省略，保持与旧响应兼容；两个 key 数组始终输出，
    /// 便于调用方无脑读取（空数组 = 无对应分节）。
    pub(crate) fn apply_to(&self, resp: &mut serde_json::Value) {
        if !self.notes.is_empty() {
            resp["notes"] = serde_json::json!(self.notes);
        }
        resp["applied"] = serde_json::json!(self.scope.applied);
        resp["restart_required"] = serde_json::json!(self.scope.restart_required);
    }
}

/// F-04：配置热更指标写入（total / succeeded / rejected + 4 个细分原因）。
///
/// 历史上这 6 个 counter 的唯一写入点在
/// `gateway_core::config::loader::reload_atomic`，而该函数全仓零调用（死代码），
/// 运维看板上的 `gateway_config_reload_*` 因此恒为 0。现将写入点迁到真实热更路径
/// （本模块的 `persist_then_apply`）：每次尝试 +1 total，
/// 成功 +1 succeeded，失败 +1 rejected 并按错误语义分桶（前缀规则与旧实现一致）。
fn record_reload_attempt(state: &Arc<AppState>) {
    state
        .platform
        .metrics
        .lifecycle
        .config_reload_total
        .fetch_add(1, Ordering::Relaxed);
}

fn record_reload_success(state: &Arc<AppState>) {
    state
        .platform
        .metrics
        .lifecycle
        .config_reload_succeeded
        .fetch_add(1, Ordering::Relaxed);
}

fn record_reload_rejected(state: &Arc<AppState>, err: &ApiError) {
    let metrics = &state.platform.metrics.lifecycle;
    metrics
        .config_reload_rejected_total
        .fetch_add(1, Ordering::Relaxed);
    let msg = err.message.to_ascii_lowercase();
    let counter = if msg.contains("schema") {
        &metrics.config_reload_rejected_schema
    } else if msg.contains("decrypt") {
        &metrics.config_reload_rejected_decrypt
    } else if msg.contains("validate") {
        &metrics.config_reload_rejected_validate
    } else if msg.contains("include") {
        &metrics.config_reload_rejected_include
    } else {
        &metrics.config_reload_rejected_other
    };
    counter.fetch_add(1, Ordering::Relaxed);
}

/// v2.1 license 配额校验：**只有采集点数是硬约束**，超限即拒绝保存配置。
///
/// 口径变更（v2.0 → v2.1）：协议种类数与北向通道数从「拒绝保存」降级为「告警」。
/// 这两项反映的是部署形态而非产能规模，按它们拦保存会在交付现场反复咬人
/// （买的是高档位，配置里多挂一个协议就被拒），且四档默认表里本就是「不限」。
/// 保留告警是为了让显式配置了上限的部署仍能看见「已超出本项目约定」。
///
/// 设计依据 `docs/design/license-v2-quota.md` §5.2/§6：任何**新增/修改配置**操作
/// 超出**点位**上限时须拒绝。
/// 拒绝的错误码按**授权状态**翻译（见 [`ApiError::license_denied`]）：
/// 授权有效但档位配额不够 → 4021；从未导入 → 7001；授权已失效 → 7003。
/// storage_days 不属授权（v2.1）：历史保留固定 30 天，由队列 maintenance 按 FIFO
/// 截断执行，与配置保存无关，此处不查。
pub(crate) fn check_config_quota(
    state: &Arc<AppState>,
    cfg: &GatewayConfig,
) -> Result<(), ApiError> {
    let license = state.platform.license.as_ref();

    // 硬约束：仅采集点数
    license
        .check_quota(
            gateway_core::license::QuotaKind::Points,
            count_points(state, cfg),
        )
        .map_err(|e| {
            // 按**授权状态**翻译错误码：失效 → 7003 / 未导入 → 7001 / 有效但档位不够 → 4021。
            // 一律 4021 会把「授权过期」误导成「配额不足」，把「没导入」误导成「升级档位」。
            ApiError::license_denied(&license.current(), e.to_string())
        })?;

    // 软约束：协议种类数 / 北向通道数仅告警，不阻断保存
    let quota = license.current().quota.clone();
    let usage = gateway_core::license::QuotaUsage {
        points: count_points(state, cfg),
        protocols: count_protocols(state, cfg),
        storage_days: 0,
        northbound_channels: count_northbound_channels(cfg),
    };
    for kind in quota.warned_kinds(&usage) {
        let limit = kind.value_in(&quota).unwrap_or_default();
        let actual = match kind {
            gateway_core::license::QuotaKind::Protocols => usage.protocols,
            gateway_core::license::QuotaKind::NorthboundChannels => usage.northbound_channels,
            gateway_core::license::QuotaKind::Points => usage.points,
            gateway_core::license::QuotaKind::StorageDays => usage.storage_days,
        };
        tracing::warn!(
            "配置超出本项目约定的{}上限（约定 {}，实际 {}）——v2.1 起该项只告警、不阻断保存",
            kind.label_zh(),
            limit,
            actual
        );
    }

    Ok(())
}

/// 计算 GatewayConfig 中点位总数（与运行时口径一致：devices 模板展开后的真实点位）。
///
/// C21: 旧实现直接累加 `devices.len()`（每个 device 记 1 点），当模板每设备展开为
/// 多点时会严重**低估**点数 → 可保存超出 license 许可的点位，配额形同虚设。
/// 改为复用运行时同一套 [`expand_points`] 展开结果，保证「配额计数 = 实际运行点数」。
pub(crate) fn count_points(state: &Arc<AppState>, cfg: &GatewayConfig) -> u32 {
    expand_points(state, cfg).len() as u32
}

/// 计算已配置的协议数（去重：相同 protocol 字段算 1）
///
/// #12: 必须与点位计数同口径——按 devices 模板**展开后**的点位统计。
/// 旧实现只遍历 `cfg.points`，而模板化配置下 `points` 常为空 → 协议数恒为 0，
/// 「协议种类」配额形同虚设（可保存任意多协议）。改传 state 以复用 expand_points。
pub(crate) fn count_protocols(state: &Arc<AppState>, cfg: &GatewayConfig) -> u32 {
    use std::collections::HashSet;
    let mut seen = HashSet::new();
    for p in expand_points(state, cfg) {
        seen.insert(p.protocol.clone());
    }
    seen.len() as u32
}

/// 计算已启用的北向通道数（mqtt / webhook / cellular / opcua /
/// modbus_server / modbus_passthrough / telemetry 各自 enabled 才算）。
///
/// B-1 修复：旧实现只覆盖 mqtt / webhook / cellular 4 类，遗漏
/// `opcua` / `modbus_server` / `modbus_passthrough` / `telemetry` —— 后 4
/// 类的配置项全部 `enabled=true` 后仍判定为 0 条占用，配额形同虚设。
/// 现统一为「全部通道只要 enabled 即占 1 个配额」，与前端展示 / 概览
/// 口径完全一致（n+1 必须等于运行时实际启用的北向消费者数量）。
///
/// 维护说明：新增北向通道时**必须**同时：
/// 1. 在本函数追加一行 `is_some_and(|c| c.enabled)` 计数；
/// 2. 在 `tests/license_quota_checks.rs` 加 `test_northbound_quota_counts_all_sinks`
///    等覆盖，避免再次漏算。
pub(crate) fn count_northbound_channels(cfg: &GatewayConfig) -> u32 {
    let mut n = 0;
    if cfg.mqtt.enabled {
        n += 1;
    }
    if cfg.webhook.as_ref().is_some_and(|w| w.enabled) {
        n += 1;
    }
    if cfg.cellular.as_ref().is_some_and(|c| c.enabled) {
        n += 1;
    }
    // 北向 OPC UA Server 未编入当前发行版时，即使配置里 enabled=true 也不会启动
    // （见 north::server::registry 的告警分支），因此不能占授权配额：
    // 否则现场会看到「北向通道超限」，而实际运行的通道数是 0。
    #[cfg(feature = "opcua-server")]
    if cfg.opcua.as_ref().is_some_and(|o| o.enabled) {
        n += 1;
    }
    if cfg.modbus_server.as_ref().is_some_and(|m| m.enabled) {
        n += 1;
    }
    if cfg.modbus_passthrough.as_ref().is_some_and(|m| m.enabled) {
        n += 1;
    }
    if cfg.telemetry.as_ref().is_some_and(|t| t.enabled) {
        n += 1;
    }
    n
}

/// 展开设备模板为最终点位列表（无 devices 或为空时回退 `cfg.points`）。
///
/// 收敛原 post_config / rollback / apply_reload / reload_points / batch_* 中
/// 逐字重复的展开逻辑。
///
/// S1 修复：展开失败**不再静默**。原实现 `unwrap_or_else(|_| cfg.points.clone())`
/// 会把「模板不存在 / 设备重名」吞成一个空点位列表，表现为「配置保存成功、界面正常、
/// 但全站 0 点位」，现场极难定位。现在：
/// - 保存前由 [`validate_template_expansion`] fail-fast 拦住并返回可读错误；
/// - 这里作为兜底仍回退 `cfg.points`（热重载链路不能因一个坏设备整站不可用），
///   但必须落 `error!` 日志，把失败设备与原因写清楚。
///
/// BUG-H 修复：模板展开结果必须与**手动点位 `cfg.points` 合并**，不能只保留模板点位。
/// 原实现 `Ok(points) => points` 会把手动点位整体丢弃，导致「热重载后手动点位消失、
/// 重启后又回来」的诡异现象。本函数的口径必须与启动路径
/// `gatewayd/src/bootstrap/helpers/collection.rs::expand_config_points` **完全一致**
/// （`cfg.points.clone()` + extend 展开结果），两处任一处改动都要同步另一处。
pub(crate) fn expand_points(state: &Arc<AppState>, cfg: &GatewayConfig) -> Vec<PointConfig> {
    let mut all = cfg.points.clone();
    if let Some(devices) = cfg.devices.as_ref().filter(|d| !d.is_empty()) {
        match state.config.templates.expand_devices(devices) {
            Ok(points) => all.extend(points),
            Err(e) => {
                tracing::error!(
                    devices = devices.len(),
                    "expand device templates failed, keeping {} raw point(s) only: {}",
                    cfg.points.len(),
                    e
                );
            }
        }
    }
    all
}

/// 保存前的模板展开校验（fail-fast）。
///
/// S1：`validate_config` 只做结构校验，看不到模板库，无法发现「模板名拼错 / 设备重名」。
/// 这两类问题会让 `expand_devices` 失败并静默退化为 0 点位，因此必须在落盘前显式拦下。
/// 无 devices 时直接通过（纯 points 模式不涉及模板）。
pub(crate) fn validate_template_expansion(
    state: &Arc<AppState>,
    cfg: &GatewayConfig,
) -> Result<(), ApiError> {
    let Some(devices) = cfg.devices.as_ref().filter(|d| !d.is_empty()) else {
        return Ok(());
    };
    state
        .config
        .templates
        .expand_devices(devices)
        .map(|_| ())
        .map_err(|e| ApiError::bad_request(format!("设备模板展开失败: {}", e)))
}

/// 配置结构完整性校验（fail-fast）。
///
/// 统一入口 [`gateway_core::config::validate_collection_config`]（= `validate_all`：validator +
/// slave_id + 循环冲突 + 扩展字段）。D1：所有经 web 变更配置的保存路径共用此校验，
/// 避免「仅 `POST /api/config` 校验、增量端点放行非法配置」的覆盖缺口。
pub(crate) fn validate_config(cfg: &GatewayConfig) -> Result<(), ApiError> {
    gateway_core::config::validate_collection_config(cfg)
        .map_err(|e| ApiError::bad_request(format!("invalid config: {}", e.join("; "))))?;
    reject_unbuilt_capabilities(cfg)?;
    Ok(())
}

/// 拒绝启用「当前二进制未编入」的北向通道。
///
/// 南向协议由 `domain::VALID_PROTOCOLS` 按 feature 裁剪，配置里写未编译的协议
/// 会立刻报错；而北向通道的能力位只来自配置字段（如 `opcua.enabled`），
/// 裁剪后配置依然"合法"，表现为**保存成功、端口不开、只在启动日志里一行 warn**，
/// 现场会去查网络 / 防火墙。因此在保存路径上直接拒绝，并给出可操作原因。
fn reject_unbuilt_capabilities(cfg: &GatewayConfig) -> Result<(), ApiError> {
    // 能力位由构建期决定；`cfg!` 让本函数在两种构建下都有同一份可测实现。
    reject_unbuilt_with(cfg, cfg!(feature = "opcua-server"))
}

/// `reject_unbuilt_capabilities` 的纯函数内核：能力是否编入由参数传入。
///
/// 这样两条分支都能被单测覆盖 —— 若直接按 `cfg(feature)` 分叉实现，
/// 默认构建下永远 `Ok`、裁剪构建又跑不到单测，等于两条分支都没人守。
fn reject_unbuilt_with(cfg: &GatewayConfig, opcua_server_built: bool) -> Result<(), ApiError> {
    if opcua_server_built {
        return Ok(());
    }
    if cfg.opcua.as_ref().is_some_and(|o| o.enabled) {
        return Err(ApiError::bad_request(
            "config 启用了北向 OPC UA Server（opcua.enabled=true），但当前发行版未编入该能力\
             （feature 'opcua-server'）。请关闭该项，或改用包含 opcua-server 的发行包 / SKU。"
                .to_string(),
        ));
    }
    Ok(())
}

/// GET 响应中敏感配置字段的脱敏占位值（P0-2/P0-3 修复）。
///
/// 前端拿到占位值后原样回传、保存路径在此**还原为旧值**，实现
/// 「显示脱敏 + 不触碰即保留」；用户把字段**清空**（空串 / null）才是
/// 真正清除凭据（如 MQTT「留空=匿名连接」），不做还原。
pub(crate) const REDACTED_SECRET: &str = "********";

pub(crate) fn is_redacted_secret(s: &str) -> bool {
    s == REDACTED_SECRET
}

fn restore_secret_opt(old: &Option<String>, new: &mut Option<String>) {
    if new.as_deref().is_some_and(is_redacted_secret) {
        *new = old.clone();
    }
}

/// 保存前把 new 中被脱敏的敏感字段还原为 old 的实际值。
///
/// 字段集合与 `system::handlers::config::redact_config_secrets` 共用唯一来源
/// [`gateway_core::config::sensitive_field_paths`]（静态路径）；`webhook.headers`
/// 是动态键 Map，两侧均按 [`gateway_core::config::SENSITIVE_SECRET_HEADERS_PATH`]
/// 逐键处理。新增含密字段时只改清单，并由本模块的 round-trip 测试守护两侧覆盖一致。
pub(crate) fn restore_redacted_secrets(old: &GatewayConfig, new: &mut GatewayConfig) {
    if let (Some(o), Some(n)) = (&old.web, &mut new.web) {
        if is_redacted_secret(&n.api_token) {
            n.api_token = o.api_token.clone();
        }
        if is_redacted_secret(&n.password_hash) {
            n.password_hash = o.password_hash.clone();
        }
        restore_secret_opt(&o.ota_signing_key, &mut n.ota_signing_key);
    }
    restore_secret_opt(&old.mqtt.username, &mut new.mqtt.username);
    restore_secret_opt(&old.mqtt.password, &mut new.mqtt.password);
    if let (Some(o), Some(n)) = (&old.cellular, &mut new.cellular) {
        if is_redacted_secret(&n.username) {
            n.username = o.username.clone();
        }
        if is_redacted_secret(&n.password) {
            n.password = o.password.clone();
        }
        restore_secret_opt(&o.pin, &mut n.pin);
    }
    // Bug: webhook.headers 逐项脱敏（见 system::handlers::config::redact_config_secrets），
    // 保存时按键还原：键名保留，值为占位符即回填旧值；旧配置无此键则保持前端原样输入。
    if let (Some(o), Some(n)) = (&old.webhook, &mut new.webhook) {
        for (name, v) in n.headers.iter_mut() {
            if is_redacted_secret(v) {
                if let Some(orig) = o.headers.get(name) {
                    *v = orig.clone();
                }
            }
        }
    }
    // P0-3: buses[].opcua 凭据按 **port_name** 匹配还原（数组下标在前端增删总线后不稳定）。
    // 匹配不到同名旧总线时退回同下标旧项，避免前端仅调整顺序时凭据被占位符覆盖；
    // 两者都找不到（新增总线却回传占位符）则保持输入，交由校验/用户修正。
    for (i, bus) in new.buses.iter_mut().enumerate() {
        let Some(new_opcua) = bus.opcua.as_mut() else {
            continue;
        };
        let old_opcua = old
            .buses
            .iter()
            .find(|b| b.port_name == bus.port_name)
            .or_else(|| old.buses.get(i))
            .and_then(|b| b.opcua.as_ref());
        let Some(old_opcua) = old_opcua else {
            continue;
        };
        restore_secret_opt(&old_opcua.username, &mut new_opcua.username);
        restore_secret_opt(&old_opcua.password, &mut new_opcua.password);
    }
    // OPC UA Server 用户名/密码用户列表：按 **username** 匹配还原占位密码
    // （用户名即 user token id，唯一性由 `OpcuaConfig::semantic_error` 保证）。
    // 旧配置无同名用户（新增用户却回传占位符）时不还原，交由校验/用户修正。
    if let (Some(o), Some(n)) = (&old.opcua, &mut new.opcua) {
        for user in n.users.iter_mut() {
            if !is_redacted_secret(&user.password) {
                continue;
            }
            if let Some(orig) = o.users.iter().find(|u| u.username == user.username) {
                user.password = orig.password.clone();
            }
        }
    }
}

/// 整份替换路径（A5 / E 批）：expand → save → reload → store → notify，
/// 返回 `(热重载说明列表, 实际生效配置)`。
///
/// 落盘前置：写盘失败直接返回 500，内存/运行态/磁盘保持旧配置（整体不生效）。
/// 落盘成功而热重载被拒时，回写旧配置让磁盘一起退回；回写也失败才退化为
/// 「磁盘已新、运行态仍旧」，并在错误消息里说明需重启生效。
///
/// 返回的第二个元素是**还原脱敏占位后的真实配置**（即真正落盘/生效的那份）。
/// 调用方（版本快照 / 变更判定 / 审计）必须用它，绝不能再用入参 `new`：
/// 前端可能原样回传 `GET /api/config` 的 `"********"` 占位值，若拿入参做快照
/// 会把占位串当作凭据落库（回滚时读到错误凭据），也会因占位值 != 旧值而误报「已变更」。
///
/// D-06：本函数是**唯一**的配置写回路径（整份替换 / 增量编辑 / 元数据变更共用），
/// 在此集中做 license 配额前置校验（V-8），避免各端点各自校验导致覆盖不全。
pub(crate) async fn persist_then_apply(
    state: &Arc<AppState>,
    old: &GatewayConfig,
    new: &GatewayConfig,
) -> Result<(ReloadOutcome, GatewayConfig), ApiError> {
    // P0-2：先还原脱敏占位值，校验/热重载/落盘看到的都是真实值。
    let mut new = new.clone();
    restore_redacted_secrets(old, &mut new);
    validate_config(&new)?;
    validate_template_expansion(state, &new)?;
    check_config_quota(state, &new)?;
    // 契约层如实上报：本次变更哪些分节可在线生效、哪些需重启（基于还原后的真实配置比较）
    let scope = reload_scope(old, &new);
    let expanded = expand_points(state, &new);
    // 活跃点位集在 reload 前按配置口径算好（expanded 随后会被 move 进 reload_config）。
    let active_points = build_active_points(&expanded, &new);

    // ① 落盘前置：写盘失败则整体不生效，运行态一行都没动。
    state.config.config_loader.save(&new).map_err(|e| {
        tracing::error!("config save failed, runtime untouched: {}", e);
        ApiError::internal(format!("save config failed: {}", e))
    })?;

    // ② 热重载：被拒时把磁盘退回旧配置，保持「要么整体生效、要么整体不生效」。
    record_reload_attempt(state);
    let notes = match state.north.gateway.reload_config(old, &new, expanded).await {
        Ok(notes) => {
            record_reload_success(state);
            notes
        }
        Err(e) => {
            let err: ApiError = e.into();
            let err = rollback_saved_config(state, old, err);
            record_reload_rejected(state, &err);
            return Err(err);
        }
    };

    // ③ 落盘与热重载都已成功，才让内存快照与新运行态对外可见。
    state.config.store(Arc::new(new.clone()));
    // 北向节点集同步：按配置口径裁剪地址空间（清理删除/改名的点位节点）并刷新写权限。
    // 放在 store 之后：此刻运行态配置已是新值，通知逻辑与配置口径一致。
    notify_north_points_changed(state, &active_points);
    // D3: 所有配置变更（含用户/密码/分组/资产等元数据）统一落审计。
    state
        .platform
        .audit
        .log("config.update", "gateway", "config update via api", "api");
    Ok((ReloadOutcome::new(notes, scope), new))
}

/// 热重载被拒后把磁盘退回旧配置。
///
/// 成功时磁盘与运行态一起回到「本次未保存」，错误消息保持原样（前端按
/// 「配置未被修改」提示）。回写也失败时只能如实说明：磁盘上已是新配置，
/// 重启后会生效——这条消息前端专门识别（`disk holds the new config`）。
fn rollback_saved_config(state: &Arc<AppState>, old: &GatewayConfig, err: ApiError) -> ApiError {
    match state.config.config_loader.save(old) {
        Ok(()) => {
            tracing::warn!(
                "config reload rejected after disk save; disk rolled back to previous config: {}",
                err.message
            );
            err
        }
        Err(re) => {
            tracing::error!(
                "config reload rejected after disk save, and rollback failed too: {}; \
                 disk holds the new config, it will take effect after restart",
                re
            );
            ApiError {
                status: err.status,
                code: err.code,
                message: format!(
                    "{} (disk holds the new config; rollback of the previous config failed: {})",
                    err.message, re
                ),
            }
        }
    }
}

/// 组装配置口径的活跃点位集（纯函数，不依赖运行态）。
///
/// 口径：设备模板展开后的全部点位（`expanded`）∪ 启用中的虚拟点位——虚拟点位经
/// 处理管道派生后同样进入北向、建立节点，不纳入清理口径就会变成僵尸节点。
/// 写权限只取普通点位的 `writable`（虚拟点位由公式派生，不接受外部写入）。
fn build_active_points(expanded: &[PointConfig], cfg: &GatewayConfig) -> ActivePointSet {
    let mut active = ActivePointSet::default();
    for p in expanded {
        active.ids.insert(p.id.clone());
        if p.writable {
            active.writable_ids.insert(p.id.clone());
        }
    }
    if let Some(virtual_points) = &cfg.virtual_points {
        for vp in virtual_points.iter().filter(|v| v.enabled) {
            active.ids.insert(vp.id.clone());
        }
    }
    active
}

/// 把活跃点位集下发给各北向从站：裁剪已下线节点、刷新节点写权限。
fn notify_north_points_changed(state: &Arc<AppState>, active: &ActivePointSet) {
    for server in &state.north.north_servers {
        server.on_points_changed(active);
    }
}

#[cfg(test)]
mod tests;
