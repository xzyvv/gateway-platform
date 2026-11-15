//! OPC UA Server 实现（F2）
//!
//! 使用 async-opcua 0.19（freeopcua fork，纯 Rust + tokio 原生异步）实现 OPC UA Server，
//! 把采集数据暴露为 OPC UA 地址空间，SCADA（UA Expert、WinCC、Ignition）可直连。
//!
//! 节点结构：
//!   Objects/Gateway/
//!     ├── <point_id_1>   Double, Readable, Subscribable
//!     ├── <point_id_2>   Double, Readable, Subscribable
//!     └── ...
//!
//! 安全策略：未配置证书时注册 None（匿名）端点便于调试/引导；配置证书后默认仅暴露
//! Basic256Sha256 + SignAndEncrypt，可由 opcua.allow_anonymous 显式放行匿名 None 端点。

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;

use super::{ActivePointSet, NorthServer};
use crate::engine::audit::AuditLog;
use crate::engine::error::{GatewayError, GatewayResult};
use crate::engine::model::DataPoint;
use crate::runtime::GatewayRuntime;

use opcua::crypto::SecurityPolicy;
use opcua::nodes::{AccessLevel, NodeType, ObjectBuilder, Variable};
use opcua::server::diagnostics::NamespaceMetadata;
use opcua::server::node_manager::memory::{simple_node_manager, SimpleNodeManager};
use opcua::server::{ServerBuilder, ServerHandle, ServerUserToken, ANONYMOUS_USER_TOKEN_ID};
use opcua::types::{
    DataValue, DateTime, MessageSecurityMode, NodeId, ObjectId, ReferenceTypeId, StatusCode,
    Variant,
};

/// Gateway 自定义命名空间 URI
const GATEWAY_NAMESPACE_URI: &str = "urn:gateway";

/// 写回调同步等待的下限。
///
/// 即使目标总线很快也至少等这么久，避免抖动把正常写判成超时；超时返回
/// `BadTimeout`（真实结果仍会落审计）。上限见 [`WRITE_TIMEOUT_MAX`]。
const WRITE_TIMEOUT_MIN: Duration = Duration::from_secs(3);

/// 写回调同步等待的上限。
///
/// async-opcua 在持有地址空间写锁期间同步调用写回调，等待期间该 Server 的
/// 所有读写/订阅都会被卡住，因此必须设上限，避免一个慢总线把整个 OPC UA
/// 服务拖死。总线 `timeout_ms` 允许配置到 600000ms，若直接按总线超时推导会
/// 长时间持锁；超过该上限一律截断（此时仍可能误报超时，但优先保证 Server 可读）。
const WRITE_TIMEOUT_MAX: Duration = Duration::from_secs(10);

/// 查不到目标总线时使用的 IO 超时兜底（与 `BusConfig::default().timeout_ms` 一致）。
const DEFAULT_IO_TIMEOUT: Duration = Duration::from_millis(500);

/// OPC UA 节点条目（内存缓存最新值；测试与 handle 未就绪时使用）
#[derive(Debug, Clone)]
pub struct OpcUaNode {
    /// 最新采集值
    pub value: f64,
}

/// OPC UA Server
/// OPC UA Server 配置（独立于 ModbusServerConfig）
#[derive(Debug, Clone)]
pub struct OpcUaServerConfig {
    pub enabled: bool,
    pub endpoint: String,
    /// 是否在已配置证书的情况下仍注册匿名/明文 None 端点（默认 false）
    ///
    /// 关闭时（默认），一旦配置了证书就只暴露 Basic256Sha256/SignAndEncrypt，
    /// 避免遗留一个无需凭据、无加密的读取通道。仅本地调试可显式开启。
    pub allow_anonymous: bool,
    /// 用户名/密码用户（用户名即 user token id）。
    ///
    /// 为空时端点不再自动放行匿名：匿名登录只由 [`Self::allow_anonymous`] 决定
    /// （无用户又未放行匿名时配置校验会拒绝启动）。
    pub users: Vec<(String, String)>,
    /// 是否自动信任未登记的客户端证书（对应 `opcua.trust_client_certs`）
    pub trust_client_certs: bool,
    /// 是否允许客户端写回点位（对应 `opcua.write_enabled`）
    pub write_enabled: bool,
}

impl Default for OpcUaServerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: "opc.tcp://0.0.0.0:4840".into(),
            allow_anonymous: false,
            users: Vec::new(),
            trust_client_certs: true,
            write_enabled: false,
        }
    }
}

/// OPC UA 写回依赖：GatewayRuntime 写链路（白名单 + collector 下发）与审计落盘。
///
/// 写回调是同步闭包（async-opcua 约束），真实下发是异步的：回调内用
/// `block_in_place` 同步等待写结果（上限见 [`write_wait_budget`]），返回给客户端的
/// StatusCode 即真实写结果（超时为 `BadTimeout`）。
pub struct OpcUaWriteHandler {
    pub gateway: Arc<GatewayRuntime>,
    pub audit: Arc<AuditLog>,
}

impl std::fmt::Debug for OpcUaWriteHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpcUaWriteHandler").finish_non_exhaustive()
    }
}

/// 计算端点支持的 user token id 列表。
///
/// - 配置了用户名用户 → 各用户名（用户名即 token id）
/// - 匿名 id 只由 `allow_anonymous` 决定；未配置用户不再隐式放行匿名
///   （该组合已由 `OpcuaConfig::semantic_error` 在启动/保存时拦截）
fn user_token_ids(users: &[(String, String)], allow_anonymous: bool) -> Vec<String> {
    let mut ids: Vec<String> = users.iter().map(|(u, _)| u.clone()).collect();
    if allow_anonymous {
        ids.push(ANONYMOUS_USER_TOKEN_ID.to_string());
    }
    ids
}

pub struct OpcUaServer {
    config: OpcUaServerConfig,
    running: Arc<AtomicBool>,
    /// OPC UA Server handle（start 成功后设置，stop 时清除）
    handle: parking_lot::Mutex<Option<ServerHandle>>,
    /// 命名空间 index（start 时从 server 获取）
    namespace_index: Arc<parking_lot::Mutex<Option<u16>>>,
    /// 已注册点位 → OPC UA NodeId 映射
    point_to_node: Arc<RwLock<HashMap<String, NodeId>>>,
    /// 节点内存缓存（与地址空间中的节点一一对应；测试与未启动时使用）
    pub(crate) nodes: Arc<RwLock<HashMap<String, OpcUaNode>>>,
    /// 最近一次下发的活跃点位集（None = 尚未收到配置变更通知，按运行态点位配置判断）
    active_points: Arc<RwLock<Option<ActivePointSet>>>,
    /// 证书文件路径（cert.pem, key.pem），None=用临时自签名证书
    cert_paths: Option<(PathBuf, PathBuf)>,
    /// 写回依赖（None = 未接入写链路，节点一律只读）
    write_handler: Option<Arc<OpcUaWriteHandler>>,
}

impl OpcUaServer {
    pub fn new(config: OpcUaServerConfig, cert_paths: Option<(PathBuf, PathBuf)>) -> Self {
        Self {
            config,
            running: Arc::new(AtomicBool::new(false)),
            handle: parking_lot::Mutex::new(None),
            namespace_index: Arc::new(parking_lot::Mutex::new(None)),
            point_to_node: Arc::new(RwLock::new(HashMap::new())),
            nodes: Arc::new(RwLock::new(HashMap::new())),
            active_points: Arc::new(RwLock::new(None)),
            cert_paths,
            write_handler: None,
        }
    }

    /// 接入写回链路（仅在 `config.write_enabled` 为真时节点才会可写）
    pub fn with_write_handler(mut self, handler: Arc<OpcUaWriteHandler>) -> Self {
        self.write_handler = Some(handler);
        self
    }

    /// 注册一个点位节点（记录到内存缓存，实际 OPC UA 节点在 update_value 时懒创建）
    pub fn register_node(&self, source_point_id: &str) {
        self.nodes
            .write()
            .insert(source_point_id.to_string(), OpcUaNode { value: 0.0 });
    }

    /// 更新节点值（简化版，仅 value，无 quality/timestamp；测试场景用）
    /// - handle 就绪：更新 OPC UA 地址空间节点值并通知订阅
    /// - handle 未就绪：仅更新内存缓存（测试场景）
    pub fn update_value(&self, source_point_id: &str, value: f64) {
        // 始终更新内存缓存（测试兼容）：点位首次更新即建条目，
        // 避免「先 update 后 register_node」时首值被静默丢弃。
        self.nodes
            .write()
            .entry(source_point_id.to_string())
            .or_insert(OpcUaNode { value })
            .value = value;

        // 若 OPC UA server 已启动，同步到地址空间
        let handle_guard = self.handle.lock();
        let handle = match handle_guard.as_ref() {
            Some(h) => h,
            None => return,
        };
        let ns = match *self.namespace_index.lock() {
            Some(ns) => ns,
            None => return,
        };
        let nm = match handle.node_managers().get_of_type::<SimpleNodeManager>() {
            Some(nm) => nm,
            None => return,
        };

        // 确保节点存在于地址空间
        let node_id = self.ensure_node_in_address_space(&nm, source_point_id, ns);

        // 更新值并通知订阅
        let dv = DataValue::new_now(value);
        if let Err(e) = nm.set_value(handle.subscriptions(), &node_id, None, dv) {
            tracing::warn!("opcua set_value failed for {}: {}", source_point_id, e);
        }
    }

    /// 确保点位节点存在于 OPC UA 地址空间，返回其 NodeId
    fn ensure_node_in_address_space(
        &self,
        nm: &SimpleNodeManager,
        point_id: &str,
        ns: u16,
    ) -> NodeId {
        // 已注册则直接返回
        if let Some(id) = self.point_to_node.read().get(point_id) {
            return id.clone();
        }

        let node_id = NodeId::new(ns, point_id);
        let gateway_folder = NodeId::new(ns, "Gateway");

        // 写回能力：仅在 opcua.write_enabled 为真、已接入写链路、且点位当前
        // 可写（活跃点位集优先，启动阶段退回运行态配置）时，才把节点标记为可写。
        let writable = self.is_point_writable(point_id);

        let mut aspace = nm.address_space().write();
        if !aspace.node_exists(&node_id) {
            let mut var = Variable::new(&node_id, point_id, point_id, 0.0f64);
            if writable {
                // AccessLevel 决定服务端是否接受 Write；UserAccessLevel 决定客户端
                // 在 UAExpert 等工具里看到的可写提示，两者必须一致。
                var.set_writable(true);
                var.set_user_access_level(var.user_access_level() | AccessLevel::CURRENT_WRITE);
            }
            aspace.insert::<_, NodeId>(var, None);
            aspace.insert_reference(&gateway_folder, &node_id, ReferenceTypeId::HasComponent);
        }
        drop(aspace);

        self.point_to_node
            .write()
            .insert(point_id.to_string(), node_id.clone());
        // 地址空间节点真实建立后才计入缓存：`node_count` 必须反映实际节点数。
        // 旧实现只在测试专用的 `register_node` 里写缓存，生产路径恒为 0。
        self.nodes
            .write()
            .entry(point_id.to_string())
            .or_insert(OpcUaNode { value: 0.0 });

        if writable {
            self.register_write_callback(nm, &node_id, point_id);
        }

        node_id
    }

    /// 点位当前是否允许北向写回。
    ///
    /// 优先取最近一次下发的活跃点位集（配置口径，热更后立即反映 `writable` 变更）；
    /// 尚未收到配置变更通知（启动阶段）时退回运行态点位配置。
    fn is_point_writable(&self, point_id: &str) -> bool {
        if !self.config.write_enabled || self.write_handler.is_none() {
            return false;
        }
        match self.active_points.read().as_ref() {
            Some(active) => active.writable_ids.contains(point_id),
            None => self
                .write_handler
                .as_ref()
                .and_then(|h| h.gateway.point_config(point_id))
                .is_some_and(|p| p.writable),
        }
    }

    /// 为点位节点注册写回调（同 NodeId 重复注册会覆盖旧回调，幂等）。
    ///
    /// 调用方负责同步节点 AccessLevel / UserAccessLevel；权限被拒时 async-opcua
    /// 在校验阶段即返回、不会走到回调，故撤销可写后残留的回调无害。
    fn register_write_callback(&self, nm: &SimpleNodeManager, node_id: &NodeId, point_id: &str) {
        if !self.is_point_writable(point_id) {
            return;
        }
        let Some(handler) = self.write_handler.as_ref() else {
            return;
        };
        // 运行态暂不可见（如总线暂停）时不注册：写回白名单同样查不到该点位。
        let Some(point) = handler.gateway.point_config(point_id) else {
            return;
        };
        // 审计字段提前在节点创建时算好，回调内不再查配置。
        let display_name = point.name.clone().unwrap_or_else(|| point_id.to_string());
        let audit_target = point.device_id.clone().unwrap_or_else(|| point.bus.clone());
        let handler = handler.clone();
        let pid = point_id.to_string();
        // 写等待上限按目标总线的单事务最坏耗时推导（见 write_wait_budget）。
        let point_bus = point.bus.clone();
        nm.inner()
            .add_write_callback(node_id.clone(), move |value, _range| {
                let Some(variant) = value.value else {
                    return StatusCode::BadNothingToDo;
                };
                let Some(write_value) = variant_to_f64(&variant) else {
                    return StatusCode::BadTypeMismatch;
                };
                // 回调是同步的，真实写命令（可能阻塞在总线 IO）必须在本线程内
                // 同步等待：返回给客户端的状态码即真实写结果，而不是「已受理」。
                let io_timeout = handler
                    .gateway
                    .bus_manager(&point_bus)
                    .map(|bus| bus.io_timeout())
                    .unwrap_or(DEFAULT_IO_TIMEOUT);
                let budget = write_wait_budget(io_timeout);
                let write = handler.gateway.write_point(&pid, write_value);
                let outcome = run_write_blocking(tokio::time::timeout(budget, write));
                let (status, detail) = match outcome {
                    Some(Ok(Ok(()))) => {
                        (StatusCode::Good, format!("{}={}", display_name, write_value))
                    }
                    Some(Ok(Err(e))) => (
                        write_error_status(&e),
                        format!("{}={} [失败: {}]", display_name, write_value, e),
                    ),
                    Some(Err(_)) => {
                        // 超时只取消了「等待」，总线任务多半已把帧发给设备：真实结果
                        // 无法得知。审计必须如实标注，否则运维会当成「没写进去」重试，
                        // 造成重复写（对累计量/寄存器等点位是实质错误）。
                        tracing::warn!(
                            "opcua 写超时: point={} value={} 等待 {}ms 未返回，写命令可能已下发（结果未知）",
                            pid,
                            write_value,
                            budget.as_millis()
                        );
                        (
                            StatusCode::BadTimeout,
                            format!(
                                "{}={} [超时: 等待写回 {}ms 未返回，结果未知（可能已写入，勿盲目重试）]",
                                display_name,
                                write_value,
                                budget.as_millis()
                            ),
                        )
                    }
                    None => {
                        // 单线程 runtime（如 #[tokio::test] 默认）不能安全阻塞等待：
                        // 退化为异步执行 + 乐观返回（生产 gatewayd 为 multi_thread）。
                        tracing::warn!(
                            "opcua: 当前 runtime 不支持阻塞等待，写回 {} 转异步（返回 Good 不代表写入成功）",
                            pid
                        );
                        let handler = handler.clone();
                        let pid = pid.clone();
                        let display_name = display_name.clone();
                        let audit_target = audit_target.clone();
                        tokio::spawn(async move {
                            let detail = match handler.gateway.write_point(&pid, write_value).await {
                                Ok(()) => format!("{}={}", display_name, write_value),
                                Err(e) => {
                                    format!("{}={} [失败: {}]", display_name, write_value, e)
                                }
                            };
                            handler
                                .audit
                                .log("point.write", &audit_target, &detail, "opcua");
                        });
                        return StatusCode::Good;
                    }
                };
                handler
                    .audit
                    .log("point.write", &audit_target, &detail, "opcua");
                status
            });
    }

    /// 按活跃点位集裁剪地址空间（删除已下线节点）并刷新可写权限。
    fn sync_address_space(&self, nm: &SimpleNodeManager, active: &ActivePointSet) {
        // 先快照映射与可写判定再释放锁：随后要持地址空间写锁，
        // 避免在地址空间锁内再取 active_points 锁造成锁序交叉。
        let mapped: Vec<(String, NodeId, bool)> = self
            .point_to_node
            .read()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone(), self.is_point_writable(k)))
            .collect();

        let mut removed: Vec<String> = Vec::new();
        {
            let mut aspace = nm.address_space().write();
            for (point_id, node_id, writable) in &mapped {
                if !active.ids.contains(point_id) {
                    // 配置里已删除/改名的点位：删除节点并清掉 Gateway folder 指向它的
                    // HasComponent 引用（true），否则客户端仍能浏览/订阅到冻结旧值。
                    aspace.delete(node_id, true);
                    removed.push(point_id.clone());
                    continue;
                }
                // 点位仍在配置中：同步可写权限。AccessLevel 决定服务端是否接受 Write，
                // UserAccessLevel 决定客户端看到的可写提示，两者必须一致。
                if let Some(NodeType::Variable(var)) = aspace.find_mut(node_id) {
                    var.set_writable(*writable);
                    let mut user_level = var.user_access_level();
                    if *writable {
                        user_level.insert(AccessLevel::CURRENT_WRITE);
                    } else {
                        user_level.remove(AccessLevel::CURRENT_WRITE);
                    }
                    var.set_user_access_level(user_level);
                }
            }
        }

        if !removed.is_empty() {
            let mut point_to_node = self.point_to_node.write();
            let mut nodes = self.nodes.write();
            for point_id in &removed {
                point_to_node.remove(point_id);
                nodes.remove(point_id);
                tracing::info!("opcua: removed stale node for point {}", point_id);
            }
        }

        // 授予可写后必须（重新）注册回调；撤销时不移除——async-opcua 没有删除回调的
        // API，且权限校验先于回调查找，无 CURRENT_WRITE 时残留回调永远不会被执行。
        for (point_id, node_id, writable) in &mapped {
            if active.ids.contains(point_id) && *writable {
                self.register_write_callback(nm, node_id, point_id);
            }
        }
    }

    /// 更新单个 DataPoint（完整映射：value + quality + timestamp → OPC UA DataValue）
    /// - handle 就绪：更新 OPC UA 地址空间节点值并通知订阅
    /// - handle 未就绪：仅更新内存缓存（测试场景）
    pub fn update_point(&self, p: &DataPoint) {
        // 始终更新内存缓存（测试兼容）：与 update_value 相同的 entry 语义，
        // 保证地址空间懒建节点时首值不会丢失。
        self.nodes
            .write()
            .entry(p.point_id.clone())
            .or_insert(OpcUaNode { value: p.value })
            .value = p.value;

        // 若 OPC UA server 已启动，同步到地址空间
        let handle_guard = self.handle.lock();
        let handle = match handle_guard.as_ref() {
            Some(h) => h,
            None => return,
        };
        let ns = match *self.namespace_index.lock() {
            Some(ns) => ns,
            None => return,
        };
        let nm = match handle.node_managers().get_of_type::<SimpleNodeManager>() {
            Some(nm) => nm,
            None => return,
        };

        // 确保节点存在于地址空间
        let node_id = self.ensure_node_in_address_space(&nm, &p.point_id, ns);

        // 完整映射 DataPoint → OPC UA DataValue
        let dv = map_data_point_to_value(p);
        if let Err(e) = nm.set_value(handle.subscriptions(), &node_id, None, dv) {
            tracing::warn!("opcua set_value failed for {}: {}", p.point_id, e);
        }
    }

    /// 批量推送 DataPoint（从 DataQueue 订阅）
    pub fn apply_points(&self, points: &[DataPoint]) {
        for p in points {
            self.update_point(p);
        }
    }

    /// 当前注册节点数
    pub fn node_count(&self) -> usize {
        self.nodes.read().len()
    }
}

/// 将网关 DataPoint 映射为 OPC UA DataValue
///
/// 映射规则：
/// - value → Variant::Double
/// - quality → StatusCode（Good/Uncertain/Bad/BadTimeout/BadCommunicationError）
/// - source_ts → source_timestamp（原始采集时刻）
/// - timestamp → server_timestamp（网关上送时刻）
fn map_data_point_to_value(p: &DataPoint) -> DataValue {
    // 质量码 → OPC UA StatusCode
    let status = match p.quality {
        crate::engine::model::Quality::Good => StatusCode::Good,
        crate::engine::model::Quality::Uncertain => StatusCode::Uncertain,
        // Bad 根据原因细分：超时 → BadTimeout，通信错误 → BadCommunicationError，其他 → Bad
        crate::engine::model::Quality::Bad => {
            if p.quality_reason.contains("timeout") {
                StatusCode::BadTimeout
            } else if p.quality_reason.contains("communication")
                || p.quality_reason.contains("disconnect")
                || p.quality_reason.contains("connection")
            {
                StatusCode::BadCommunicationError
            } else {
                StatusCode::Bad
            }
        }
        // Stale（数据过时，复用 last_good 值）→ Bad（值不可信）
        crate::engine::model::Quality::Stale => StatusCode::Bad,
    };

    // 时间戳映射：source_ts → source_timestamp，timestamp → server_timestamp
    // DataPoint 时间戳为 UTC 毫秒，转换为 chrono DateTime
    let source_dt = chrono::DateTime::from_timestamp_millis(p.source_ts);
    let server_dt = chrono::DateTime::from_timestamp_millis(p.timestamp);

    DataValue {
        value: Some(Variant::Double(p.value)),
        status: Some(status),
        source_timestamp: source_dt.map(DateTime::from),
        source_picoseconds: source_dt.and(Some(0)),
        server_timestamp: server_dt.map(DateTime::from),
        server_picoseconds: server_dt.and(Some(0)),
    }
}

/// 把客户端写入的 OPC UA 标量转成网关内部统一的 `f64`。
///
/// 只接受数值型（含 Boolean，按 0/1 处理）；其它类型返回 `None`，
/// 由调用方以 `BadTypeMismatch` 拒绝，避免把字符串/时间等静默强转为 0。
fn variant_to_f64(v: &Variant) -> Option<f64> {
    match v {
        Variant::Boolean(b) => Some(if *b { 1.0 } else { 0.0 }),
        Variant::SByte(x) => Some(*x as f64),
        Variant::Byte(x) => Some(*x as f64),
        Variant::Int16(x) => Some(*x as f64),
        Variant::UInt16(x) => Some(*x as f64),
        Variant::Int32(x) => Some(*x as f64),
        Variant::UInt32(x) => Some(*x as f64),
        Variant::Int64(x) => Some(*x as f64),
        Variant::UInt64(x) => Some(*x as f64),
        Variant::Float(x) => Some(*x as f64),
        Variant::Double(x) => Some(*x),
        _ => None,
    }
}

/// 把网关写回错误映射为 OPC UA 状态码，让客户端能区分失败原因。
///
/// 旧实现无论真实结果如何都返回 `Good`，客户端无法感知写失败；现在映射为
/// 「不可写 / 点位不存在 / 设备失败 / 超时」，其余归为 `BadInternalError`。
fn write_error_status(err: &GatewayError) -> StatusCode {
    match err {
        GatewayError::Config(_) => StatusCode::BadNotWritable,
        GatewayError::NotFound(_) => StatusCode::BadNodeIdUnknown,
        GatewayError::Protocol(_) | GatewayError::Io(_) => StatusCode::BadDeviceFailure,
        GatewayError::NoReply => StatusCode::BadTimeout,
        _ => StatusCode::BadInternalError,
    }
}

/// 在同步写回调里同步等待异步写结果。
///
/// 返回 `None` 表示当前 runtime 无法安全阻塞（没有 runtime，或单线程 runtime
/// ——`block_in_place` 在其上会 panic），调用方需退化为异步执行。
fn run_write_blocking<F: Future>(fut: F) -> Option<F::Output> {
    let handle = tokio::runtime::Handle::try_current().ok()?;
    if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::CurrentThread {
        return None;
    }
    // multi_thread 下用 block_in_place 让出当前 worker，避免阻塞同线程调度；
    // 等待期间仍持有 async-opcua 的地址空间锁，故调用方必须带超时。
    Some(tokio::task::block_in_place(|| handle.block_on(fut)))
}

/// 计算写回调的同步等待预算。
///
/// 总线单事务最坏耗时是「入队 `io_timeout` + 应答 `2×io_timeout+100ms`」
/// （见 `BusManager::transaction`），这里按 `3×io_timeout + 500ms` 留足余量。
/// 下限 [`WRITE_TIMEOUT_MIN`] 覆盖默认 500ms 总线（推导值 2s 会被抬到 3s）；
/// 上限 [`WRITE_TIMEOUT_MAX`] 保护地址空间锁。
///
/// 旧实现固定 3s：总线 `timeout_ms` 调到 1000ms 以上时，正常慢写会先于总线
/// 自己判定超时，客户端收到 `BadTimeout` 但设备其实已写入（重试会重复写）。
fn write_wait_budget(io_timeout: Duration) -> Duration {
    (io_timeout * 3 + Duration::from_millis(500)).clamp(WRITE_TIMEOUT_MIN, WRITE_TIMEOUT_MAX)
}

/// 解析 OPC UA endpoint，返回 (host, port)
/// 支持格式：opc.tcp://host:port, tcp://host:port, host:port
pub(crate) fn parse_endpoint(ep: &str) -> GatewayResult<(String, u16)> {
    let s = ep
        .strip_prefix("opc.tcp://")
        .or_else(|| ep.strip_prefix("tcp://"))
        .unwrap_or(ep);
    let (host, port_str) = s
        .rsplit_once(':')
        .ok_or_else(|| GatewayError::Config(format!("invalid opcua endpoint: {}", ep)))?;
    let port: u16 = port_str
        .parse()
        .map_err(|_| GatewayError::Config(format!("invalid opcua port: {}", port_str)))?;
    Ok((host.to_string(), port))
}

impl NorthServer for OpcUaServer {
    fn name(&self) -> &'static str {
        "opcua"
    }

    fn start<'a>(&'a self) -> Pin<Box<dyn Future<Output = GatewayResult<()>> + Send + 'a>> {
        Box::pin(async move {
            if !self.config.enabled {
                return Err(GatewayError::Config("opcua server not enabled".into()));
            }

            let (host, port) = parse_endpoint(&self.config.endpoint)?;

            // 提取证书路径到本地变量（避免 async block 持有 &self 导致 !Send）
            let cert_paths = self.cert_paths.clone();
            let users = self.config.users.clone();
            let trust_client_certs = self.config.trust_client_certs;

            // 显式配置的证书缺失时拒绝启动（避免静默退化为临时自签名证书，
            // 使客户端固定信任的证书指纹与真实服务端不一致）。
            if let Some((cert_path, key_path)) = &cert_paths {
                for p in [cert_path, key_path] {
                    if !p.exists() {
                        return Err(GatewayError::Config(format!(
                            "opcua 证书文件不存在: {}",
                            p.display()
                        )));
                    }
                }
            }

            // 端点支持的 user token id：用户名列表 + （按配置/旧行为）匿名
            let token_ids = user_token_ids(&users, self.config.allow_anonymous);
            let token_refs: Vec<&str> = token_ids.iter().map(String::as_str).collect();

            // 构建 OPC UA Server（放在内部块中，确保 builder 在 await 前 drop）
            let (server, handle) = {
                let mut builder = ServerBuilder::new()
                    .application_name("Gateway OPC UA Server")
                    .application_uri("urn:gateway:opcua")
                    .product_uri("urn:gateway:opcua")
                    .host(&host)
                    .port(port)
                    // discovery_urls 必须非空：async-opcua 的 ServerConfig::validate() 会对空值
                    // 直接报 “Discovery urls not set” 使 build() 失败（服务无法启动）。
                    // 用配置的 endpoint 拼出服务地址，与 e2e 测试桩保持一致。
                    .discovery_urls(vec![format!("opc.tcp://{}:{}/", host, port)]);

                // 匿名/明文 None 端点仅在「未配置证书」或「显式 allow_anonymous」时注册：
                // 一旦配置了证书，默认只暴露 Basic256Sha256/SignAndEncrypt，避免同时留下
                // 一个无需凭据、无加密的读取通道（任何可达该端口者都能浏览过程数据）。
                let has_cert = cert_paths.is_some();
                if !users.is_empty() && !has_cert {
                    tracing::warn!(
                        "opcua: 已配置用户名/密码但未配置证书，凭据会在 None 端点上以明文传输；生产环境请配置 cert_path/key_path"
                    );
                }
                if should_register_anonymous_none_endpoint(has_cert, self.config.allow_anonymous) {
                    builder = builder.add_endpoint(
                        "none",
                        (
                            "/",
                            SecurityPolicy::None,
                            MessageSecurityMode::None,
                            token_refs.as_slice(),
                        ),
                    );
                    if has_cert {
                        tracing::warn!(
                            "opcua: allow_anonymous=true，已配证书仍注册明文匿名 None 端点（生产环境不建议）"
                        );
                    } else {
                        tracing::warn!(
                            "opcua: 未配置证书，已注册明文匿名 None 端点（仅建议本地调试/引导使用）"
                        );
                    }
                }

                // 证书：有持久化证书则使用，否则用临时自签名证书
                if let Some((cert_path, key_path)) = cert_paths {
                    builder = builder
                        .certificate_path(cert_path)
                        .private_key_path(key_path);
                    // 启用签名加密 endpoint（需要证书）
                    builder = builder.add_endpoint(
                        "sign_encrypt",
                        (
                            "/",
                            SecurityPolicy::Basic256Sha256,
                            MessageSecurityMode::SignAndEncrypt,
                            token_refs.as_slice(),
                        ),
                    );
                    tracing::info!("opcua: using persistent cert, Basic256Sha256 endpoint enabled");
                } else {
                    builder = builder.create_sample_keypair(true);
                }

                // 注册用户名/密码 token（默认认证器按 endpoint.user_token_ids
                // 中的 token id 逐一比对用户名与密码）
                for (user, password) in &users {
                    builder = builder.add_user_token(
                        user.clone(),
                        ServerUserToken::user_pass(user.clone(), password.clone()),
                    );
                }
                if !users.is_empty() {
                    tracing::info!(
                        "opcua: {} username token(s) registered (anonymous={})",
                        users.len(),
                        token_ids.iter().any(|id| id == ANONYMOUS_USER_TOKEN_ID)
                    );
                }

                builder
                    .trust_client_certs(trust_client_certs)
                    .with_node_manager(simple_node_manager(
                        NamespaceMetadata {
                            namespace_uri: GATEWAY_NAMESPACE_URI.to_string(),
                            ..Default::default()
                        },
                        "GatewayNodeManager",
                    ))
                    .build()
                    .map_err(|e| {
                        GatewayError::Protocol(format!("opcua server build failed: {}", e))
                    })?
            };

            // 获取 namespace index
            let ns = handle
                .get_namespace_index(GATEWAY_NAMESPACE_URI)
                .ok_or_else(|| {
                    GatewayError::NotFound("opcua gateway namespace not found".to_string())
                })?;
            *self.namespace_index.lock() = Some(ns);

            // 保存 handle 供 update_value 使用
            *self.handle.lock() = Some(handle.clone());

            // f3 修复：每次 start() 都会重建 Server 与全新的地址空间，上一次运行留下的
            // NodeId 已失效。必须清空映射表，否则 ensure_node_in_address_space 命中陈旧
            // NodeId 后直接早退，后续 set_value 写到一个不存在的节点上（静默失败），
            // OPC UA 订阅者永远收不到重启后的数据更新。
            self.point_to_node.write().clear();
            // 地址空间同样是全新的：内存节点缓存一并清空，否则 node_count 会把上一次
            // 运行的节点算进来（节点要等下一次 update 时才会重建）。
            self.nodes.write().clear();

            // 创建 Gateway folder 节点（Objects/Gateway）
            let gateway_folder = NodeId::new(ns, "Gateway");
            if let Some(nm) = handle.node_managers().get_of_type::<SimpleNodeManager>() {
                let mut aspace = nm.address_space().write();
                ObjectBuilder::new(&gateway_folder, "Gateway", "Gateway")
                    .organized_by(ObjectId::ObjectsFolder)
                    .insert(&mut *aspace);
            }

            self.running.store(true, Ordering::Relaxed);
            tracing::info!(
                "opcua server started on opc.tcp://{}:{} (namespace index={})",
                host,
                port,
                ns
            );

            // 运行 server（阻塞直到 cancel 或出错）
            let result = server.run().await;

            self.running.store(false, Ordering::Relaxed);
            *self.handle.lock() = None;
            *self.namespace_index.lock() = None;

            result.map_err(|e| GatewayError::Protocol(format!("opcua server error: {}", e)))
        })
    }

    fn stop<'a>(&'a self) -> Pin<Box<dyn Future<Output = GatewayResult<()>> + Send + 'a>> {
        Box::pin(async move {
            if let Some(handle) = self.handle.lock().take() {
                handle.cancel();
                tracing::info!("opcua server stop signal sent");
            }
            self.running.store(false, Ordering::Relaxed);
            Ok(())
        })
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    fn on_points_changed(&self, active: &ActivePointSet) {
        // 先记录配置口径：此后懒建的节点按活跃集决定初始写权限。
        *self.active_points.write() = Some(active.clone());

        // 取 handle 快照后立即释放锁（临时 guard 在 let 语句结束时 drop）。
        let handle = match self.handle.lock().as_ref() {
            Some(h) => h.clone(),
            // 未启动：地址空间尚不存在，节点会在 start 后按活跃集懒建。
            None => return,
        };
        let Some(nm) = handle.node_managers().get_of_type::<SimpleNodeManager>() else {
            return;
        };
        self.sync_address_space(&nm, active);
    }
}

/// 是否注册匿名/明文 None 端点。
///
/// - 未配置证书：服务只能靠 None 端点对外，必须注册（调用方会打告警）
/// - 已配置证书：默认不注册，需显式 `allow_anonymous = true` 才注册
pub(crate) fn should_register_anonymous_none_endpoint(
    has_cert: bool,
    allow_anonymous: bool,
) -> bool {
    !has_cert || allow_anonymous
}

#[cfg(test)]
mod tests {
    use super::{
        should_register_anonymous_none_endpoint, user_token_ids, variant_to_f64,
        write_error_status, write_wait_budget, ActivePointSet, NorthServer, OpcUaServer,
        OpcUaServerConfig,
    };

    /// 契约：未配证书时必须注册 None 端点（否则服务没有任何可用端点）；
    /// 已配证书且未显式 allow_anonymous 时必须不注册（关闭明文匿名读取通道）
    #[test]
    fn anonymous_none_endpoint_gating() {
        assert!(should_register_anonymous_none_endpoint(false, false));
        assert!(should_register_anonymous_none_endpoint(false, true));
        assert!(!should_register_anonymous_none_endpoint(true, false));
        assert!(should_register_anonymous_none_endpoint(true, true));
    }

    /// 契约：OpcUaServerConfig 默认不开放匿名 None 端点
    #[test]
    fn opcua_server_config_defaults_deny_anonymous_none() {
        assert!(!OpcUaServerConfig::default().allow_anonymous);
    }

    /// 契约：默认不开放写回、自动信任客户端证书、无用户
    #[test]
    fn opcua_server_config_defaults_close_write_and_users() {
        let cfg = OpcUaServerConfig::default();
        assert!(!cfg.write_enabled);
        assert!(cfg.trust_client_certs);
        assert!(cfg.users.is_empty());
    }

    /// 契约：配置了用户名时端点只列这些用户名；匿名仅在显式放行时追加
    #[test]
    fn user_token_ids_lists_configured_users() {
        let users = vec![
            ("alice".to_string(), "p1".to_string()),
            ("bob".to_string(), "p2".to_string()),
        ];
        assert_eq!(user_token_ids(&users, false), vec!["alice", "bob"]);
        let mut with_anonymous = user_token_ids(&users, true);
        assert_eq!(with_anonymous.pop().as_deref(), Some("ANONYMOUS"));
    }

    /// 契约：匿名 token 只由 `allow_anonymous` 决定——未配置用户不再隐式放行
    /// 匿名（该组合由配置校验拦截，否则端点将没有任何可登录方式）
    #[test]
    fn user_token_ids_require_explicit_anonymous() {
        assert!(user_token_ids(&[], false).is_empty());
        assert_eq!(user_token_ids(&[], true), vec!["ANONYMOUS"]);
    }

    /// 契约：写失败必须映射为可区分的 OPC UA 状态码，客户端据此判断原因
    #[test]
    fn write_error_maps_to_specific_status_codes() {
        use crate::engine::error::GatewayError;
        use opcua::types::StatusCode;

        assert_eq!(
            write_error_status(&GatewayError::Config("not writable".into())),
            StatusCode::BadNotWritable
        );
        assert_eq!(
            write_error_status(&GatewayError::NotFound("no point".into())),
            StatusCode::BadNodeIdUnknown
        );
        assert_eq!(
            write_error_status(&GatewayError::Protocol("crc".into())),
            StatusCode::BadDeviceFailure
        );
        assert_eq!(
            write_error_status(&GatewayError::Io("port closed".into())),
            StatusCode::BadDeviceFailure
        );
        assert_eq!(
            write_error_status(&GatewayError::NoReply),
            StatusCode::BadTimeout
        );
        assert_eq!(
            write_error_status(&GatewayError::Queue("db".into())),
            StatusCode::BadInternalError
        );
    }

    /// 契约：写等待预算必须覆盖总线「入队 + 应答」的最坏耗时，同时受上下限约束。
    /// 旧实现固定 3s，总线 `timeout_ms ≥ 1000ms` 时正常慢写会先被判超时，
    /// 客户端重试会重复写。
    #[test]
    fn write_wait_budget_follows_bus_timeout() {
        use std::time::Duration;

        // 默认 500ms 总线：推导 2s 低于下限，抬到 3s（保持旧行为）
        assert_eq!(
            write_wait_budget(Duration::from_millis(500)),
            Duration::from_secs(3)
        );
        // 总线调到 1500ms：必须覆盖 3×1500+100=4600ms 的最坏路径
        assert_eq!(
            write_wait_budget(Duration::from_millis(1500)),
            Duration::from_millis(5000)
        );
        // 极端配置：按上限截断，避免长时间持有地址空间写锁
        assert_eq!(
            write_wait_budget(Duration::from_millis(600_000)),
            Duration::from_secs(10)
        );
        // 任何取值都不得低于下限
        assert!(write_wait_budget(Duration::from_millis(1)) >= Duration::from_secs(3));
    }

    /// 契约：写回调只接受数值型（含 Boolean 按 0/1），其它类型拒绝而非静默转 0
    #[test]
    fn variant_to_f64_accepts_numeric_and_rejects_others() {
        use opcua::types::UAString;
        use opcua::types::Variant;

        assert_eq!(variant_to_f64(&Variant::Double(1.5)), Some(1.5));
        assert_eq!(variant_to_f64(&Variant::Float(2.5)), Some(2.5));
        assert_eq!(variant_to_f64(&Variant::Int32(-3)), Some(-3.0));
        assert_eq!(variant_to_f64(&Variant::Boolean(true)), Some(1.0));
        assert_eq!(variant_to_f64(&Variant::String(UAString::from("7"))), None);
    }

    /// 契约：`node_count` 必须反映真实节点——首次 update 即建缓存条目，不再恒为 0。
    #[test]
    fn update_value_seeds_node_cache() {
        let srv = OpcUaServer::new(OpcUaServerConfig::default(), None);
        assert_eq!(srv.node_count(), 0);

        srv.update_value("p1", 42.0);
        assert_eq!(srv.node_count(), 1);
        assert_eq!(srv.nodes.read().get("p1").map(|n| n.value), Some(42.0));

        srv.update_value("p1", 43.0);
        assert_eq!(srv.node_count(), 1, "同一节点重复更新不应重复计数");
        assert_eq!(srv.nodes.read().get("p1").map(|n| n.value), Some(43.0));
    }

    /// 契约：配置热更后按活跃点位集裁剪地址空间——已删除/改名的点位节点必须从
    /// 地址空间、`point_to_node` 与 `nodes` 三处一并消失，仍在配置中的节点保留。
    #[tokio::test]
    async fn points_changed_prunes_stale_nodes() {
        use std::sync::Arc;
        use std::time::Duration;

        use opcua::server::node_manager::memory::SimpleNodeManager;
        use opcua::types::NodeId;

        let probe = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind probe");
        let port = probe.local_addr().expect("local_addr").port();
        drop(probe);

        let cfg = OpcUaServerConfig {
            enabled: true,
            endpoint: format!("opc.tcp://127.0.0.1:{port}"),
            ..Default::default()
        };
        let srv = Arc::new(OpcUaServer::new(cfg, None));
        let run_srv = srv.clone();
        let run_task = tokio::spawn(async move {
            let _ = run_srv.start().await;
        });

        let mut running = false;
        for _ in 0..50 {
            if srv.is_running() {
                running = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(running, "opcua server 未进入 running");

        srv.update_value("keep", 1.0);
        srv.update_value("stale", 2.0);
        assert_eq!(srv.node_count(), 2);

        let mut active = ActivePointSet::default();
        active.ids.insert("keep".to_string());
        srv.on_points_changed(&active);

        assert_eq!(srv.node_count(), 1, "stale 节点应已从缓存移除");
        assert!(srv.point_to_node.read().contains_key("keep"));
        assert!(!srv.point_to_node.read().contains_key("stale"));

        // 地址空间里也必须真的删除，否则客户端仍能浏览/订阅到冻结旧值
        let ns = *srv
            .namespace_index
            .lock()
            .as_ref()
            .expect("namespace index not set");
        let handle = srv.handle.lock().as_ref().cloned().expect("handle");
        {
            let nm = handle
                .node_managers()
                .get_of_type::<SimpleNodeManager>()
                .expect("node manager");
            let aspace = nm.address_space().read();
            assert!(aspace.node_exists(&NodeId::new(ns, "keep")));
            assert!(
                !aspace.node_exists(&NodeId::new(ns, "stale")),
                "地址空间中的僵尸节点未被删除"
            );
        }
        drop(handle);

        srv.stop().await.expect("stop");
        let _ = tokio::time::timeout(Duration::from_secs(5), run_task).await;
    }
}
