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

use parking_lot::RwLock;

use super::Server;
use crate::engine::audit::AuditLog;
use crate::engine::error::{GatewayError, GatewayResult};
use crate::engine::model::DataPoint;
use crate::support::app::Gateway;

use opcua::crypto::SecurityPolicy;
use opcua::nodes::{AccessLevel, ObjectBuilder, Variable};
use opcua::server::diagnostics::NamespaceMetadata;
use opcua::server::node_manager::memory::{simple_node_manager, SimpleNodeManager};
use opcua::server::{ServerBuilder, ServerHandle, ServerUserToken, ANONYMOUS_USER_TOKEN_ID};
use opcua::types::{
    DataValue, DateTime, MessageSecurityMode, NodeId, ObjectId, ReferenceTypeId, StatusCode,
    Variant,
};

/// Gateway 自定义命名空间 URI
const GATEWAY_NAMESPACE_URI: &str = "urn:gateway";

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
    /// 用户名/密码用户（用户名即 user token id）。为空时保持旧行为：匿名登录。
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

/// OPC UA 写回依赖：Gateway 写链路（白名单 + collector 下发）与审计落盘。
///
/// 写回调是同步闭包（async-opcua 约束），实际下发通过 `tokio::spawn` 转异步，
/// 回调本身只做「值解析 + 白名单预检 + 乐观返回 Good」。
pub struct OpcUaWriteHandler {
    pub gateway: Arc<Gateway>,
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
/// - `allow_anonymous` 为真，或未配置任何用户（旧配置，必须保留匿名通道）→ 追加匿名 id
fn user_token_ids(users: &[(String, String)], allow_anonymous: bool) -> Vec<String> {
    let mut ids: Vec<String> = users.iter().map(|(u, _)| u.clone()).collect();
    if allow_anonymous || users.is_empty() {
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
    /// 节点内存缓存（测试与未启动时使用）
    pub(crate) nodes: Arc<RwLock<HashMap<String, OpcUaNode>>>,
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
        // 始终更新内存缓存（测试兼容）
        if let Some(n) = self.nodes.write().get_mut(source_point_id) {
            n.value = value;
        }

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

        // 写回能力：仅在 opcua.write_enabled 为真、已接入写链路、且点位
        // `writable = true` 时，才把节点标记为可写并注册写回调。
        let point_cfg = self
            .write_handler
            .as_ref()
            .and_then(|h| h.gateway.point_config(point_id));
        let writable = self.config.write_enabled
            && self.write_handler.is_some()
            && point_cfg.as_ref().is_some_and(|p| p.writable);

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

        if writable {
            if let (Some(handler), Some(point)) = (self.write_handler.as_ref(), point_cfg) {
                // 审计字段提前在节点创建时算好，回调内不再查配置。
                let display_name = point.name.clone().unwrap_or_else(|| point_id.to_string());
                let audit_target = point.device_id.clone().unwrap_or_else(|| point.bus.clone());
                let handler = handler.clone();
                let pid = point_id.to_string();
                nm.inner()
                    .add_write_callback(node_id.clone(), move |value, _range| {
                        let Some(variant) = value.value else {
                            return StatusCode::BadNothingToDo;
                        };
                        let Some(write_value) = variant_to_f64(&variant) else {
                            return StatusCode::BadTypeMismatch;
                        };
                        // 回调是同步的，真实写命令（持有总线 IO）转异步执行；
                        // 同步路径只做类型校验并乐观返回 Good，失败经审计暴露。
                        let handler = handler.clone();
                        let pid = pid.clone();
                        let display_name = display_name.clone();
                        let audit_target = audit_target.clone();
                        tokio::spawn(async move {
                            let detail = match handler.gateway.write_point(&pid, write_value).await
                            {
                                Ok(()) => format!("{}={}", display_name, write_value),
                                Err(e) => {
                                    format!("{}={} [失败: {}]", display_name, write_value, e)
                                }
                            };
                            handler
                                .audit
                                .log("point.write", &audit_target, &detail, "opcua");
                        });
                        StatusCode::Good
                    });
            }
        }

        node_id
    }

    /// 更新单个 DataPoint（完整映射：value + quality + timestamp → OPC UA DataValue）
    /// - handle 就绪：更新 OPC UA 地址空间节点值并通知订阅
    /// - handle 未就绪：仅更新内存缓存（测试场景）
    pub fn update_point(&self, p: &DataPoint) {
        // 始终更新内存缓存（测试兼容）
        if let Some(n) = self.nodes.write().get_mut(&p.point_id) {
            n.value = p.value;
        }

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

impl Server for OpcUaServer {
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
        should_register_anonymous_none_endpoint, user_token_ids, variant_to_f64, OpcUaServerConfig,
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

    /// 契约：未配置用户时必须保留匿名通道（兼容存量配置/客户端）
    #[test]
    fn user_token_ids_keep_anonymous_without_users() {
        let ids = user_token_ids(&[], false);
        assert_eq!(ids, vec!["ANONYMOUS"]);
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
}
