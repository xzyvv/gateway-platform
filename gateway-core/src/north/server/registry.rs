//! 北向从站注册表（北向通道插件化）
//!
//! 与 [`crate::south::protocol::registry::ProtocolRegistry`] 对偶：
//! - ProtocolRegistry 管理「协议名 → Collector 工厂」
//! - NorthServerRegistry 管理「从站名 → 从站工厂」
//!
//! 新增北向通道操作清单：
//! 1. 在 `super::<server>` 模块下实现 `NorthServer` trait
//! 2. 在本文件写 `fn create_<server>(ctx: &NorthServerContext) -> Option<Arc<dyn NorthServer>>`
//! 3. 在 `NorthServerRegistry::new()` 中调用 `self.register("<server>", create_<server>)`
//! 4. 无需修改 bootstrap 代码

use std::collections::HashMap;
use std::sync::Arc;

use crate::config::{GatewayConfig, PointConfig};
use crate::crypto::cert::{CertManager, CertPair};
use crate::north::NorthDispatcher;
use crate::support::app::GatewayRuntime;

use super::NorthServer;

/// 北向 Server 启动上下文
///
/// 包含所有 Server 工厂可能需要的依赖。新增 Server 若需要新依赖，
/// 在此结构体追加字段即可，不影响已有工厂签名。
pub struct NorthServerContext<'a> {
    pub config: &'a GatewayConfig,
    pub north_dispatcher: &'a Arc<NorthDispatcher>,
    pub cert_pair: &'a Option<CertPair>,
    pub cert_manager: &'a CertManager,
    /// GatewayRuntime 组件管理器（供需要读写总线/Collector 的 Server 使用，如 ModbusPassthrough）
    pub gateway: &'a Arc<GatewayRuntime>,
    /// 审计日志（供支持写回的 Server 记录 `point.write`，与 API 写入口同一落盘实例）
    pub audit: &'a Arc<crate::engine::audit::AuditLog>,
    pub expanded_points: &'a [PointConfig],
}

/// Server 工厂函数：根据上下文创建并启动一个 Server，返回句柄供优雅停机
///
/// 返回 `None` 表示该 Server 被配置禁用或初始化失败（已在内部 log）。
pub type NorthServerFactory = fn(ctx: &NorthServerContext) -> Option<Arc<dyn NorthServer>>;

/// 北向 Server 注册表
///
/// 维护 Server 名到工厂函数的映射。`start_all` 遍历所有工厂并启动已启用的 Server。
pub struct NorthServerRegistry {
    factories: HashMap<String, NorthServerFactory>,
}

impl NorthServerRegistry {
    /// 创建注册表并注册内置 Server
    pub fn new() -> Self {
        let mut registry = Self {
            factories: HashMap::new(),
        };
        registry.register("opcua", create_opcua_server);
        registry.register("modbus", create_modbus_server);
        registry.register("modbus_passthrough", create_modbus_passthrough);
        registry
    }

    /// 注册一个 Server 工厂
    pub fn register(&mut self, name: &str, factory: NorthServerFactory) {
        self.factories.insert(name.to_string(), factory);
    }

    /// 启动所有已注册的 Server，返回句柄列表供优雅停机
    pub fn start_all(&self, ctx: &NorthServerContext) -> Vec<Arc<dyn NorthServer>> {
        let mut servers: Vec<Arc<dyn NorthServer>> = Vec::new();
        for (name, factory) in &self.factories {
            match factory(ctx) {
                Some(srv) => servers.push(srv),
                None => tracing::debug!("server '{}' disabled or init failed", name),
            }
        }
        servers
    }
}

impl Default for NorthServerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ===== 内置 Server 工厂函数 =====

/// OPC UA Server 工厂（可选，由 config.opcua.enabled 控制）
///
/// 仅在 `opcua-server` feature 下编译；裁剪构建的占位实现见下方同名函数。
#[cfg(feature = "opcua-server")]
fn create_opcua_server(ctx: &NorthServerContext) -> Option<Arc<dyn NorthServer>> {
    use crate::engine::supervision::spawn_supervised;
    use crate::north::server::opcua_server::{OpcUaServer, OpcUaServerConfig, OpcUaWriteHandler};
    use crate::north::OpcUaServerAdapter;
    use tracing::{error, info};

    let opcua_cfg = ctx.config.opcua.clone()?;
    if !opcua_cfg.enabled {
        info!("opcua server disabled by config (enabled=false)");
        return None;
    }

    let opcua_config = OpcUaServerConfig {
        enabled: true,
        endpoint: opcua_cfg.endpoint.clone(),
        allow_anonymous: opcua_cfg.allow_anonymous,
        users: opcua_cfg
            .users
            .iter()
            .map(|u| (u.username.clone(), u.password.clone()))
            .collect(),
        trust_client_certs: opcua_cfg.trust_client_certs,
        write_enabled: opcua_cfg.write_enabled,
    };
    // 证书优先级：opcua.cert_path/key_path 显式配置 > 全局设备证书；
    // 都未配置时由 OpcUaServer 内部生成临时自签名证书。
    let opcua_cert_paths = match (&opcua_cfg.cert_path, &opcua_cfg.key_path) {
        (Some(cert), Some(key)) => Some((
            std::path::PathBuf::from(cert),
            std::path::PathBuf::from(key),
        )),
        _ => ctx.cert_pair.as_ref().map(|_| {
            (
                ctx.cert_manager.cert_path().to_path_buf(),
                ctx.cert_manager.key_path().to_path_buf(),
            )
        }),
    };
    let opcua_write_handler = opcua_cfg.write_enabled.then(|| {
        Arc::new(OpcUaWriteHandler {
            gateway: ctx.gateway.clone(),
            audit: ctx.audit.clone(),
        })
    });
    let mut opcua_server = OpcUaServer::new(opcua_config, opcua_cert_paths);
    if let Some(handler) = opcua_write_handler {
        opcua_server = opcua_server.with_write_handler(handler);
    }
    let opcua_server = Arc::new(opcua_server);
    let opcua_for_start = opcua_server.clone();

    spawn_supervised("opcua_server", move || {
        let srv = opcua_for_start.clone();
        async move {
            if let Err(e) = srv.start().await {
                error!("opcua server exited with error: {}", e);
            }
        }
    });

    // 复用 NorthDispatcher 的关闭信号：进程停机时统一触发，sink task 得以 drain 收尾队列后退出，
    // 而不是等到进程被杀时才被动丢弃在途数据。
    ctx.north_dispatcher
        .register_connector(OpcUaServerAdapter::with_shutdown(
            opcua_server.clone(),
            ctx.north_dispatcher.shutdown_token(),
            ctx.gateway.metrics.clone(),
        ) as Arc<dyn crate::north::NorthConnector>);
    info!(
        "opcua server task launched on {} (bridged via NorthDispatcher)",
        opcua_cfg.endpoint
    );

    Some(opcua_server)
}

/// 未编译 `opcua-server` 的构建：配置若启用北向 OPC UA Server，必须显式告警。
///
/// 静默不启动会让运维以为「配置生效了、SCADA 连不上是网络问题」，
/// 因此把「这个二进制没带该能力」写进日志，并给出可执行的修复方式。
#[cfg(not(feature = "opcua-server"))]
fn create_opcua_server(ctx: &NorthServerContext) -> Option<Arc<dyn NorthServer>> {
    if ctx.config.opcua.as_ref().is_some_and(|c| c.enabled) {
        tracing::warn!(
            "config enables north OPC UA server, but this binary was compiled without feature \
             `opcua-server`: the server will NOT start. Rebuild with --features opcua-server, \
             or remove the `opcua` section from config."
        );
    }
    None
}

/// Modbus Server 工厂（可选，由 config.modbus_server.enabled 控制）
fn create_modbus_server(ctx: &NorthServerContext) -> Option<Arc<dyn NorthServer>> {
    use crate::engine::supervision::spawn_supervised;
    use crate::north::server::modbus_server::ModbusServer;
    use crate::north::ModbusServerAdapter;
    use tracing::{error, info};

    let mb_cfg = ctx.config.modbus_server.clone()?;
    if !mb_cfg.enabled {
        info!("modbus server disabled by config (enabled=false)");
        return None;
    }

    let mb_config = mb_cfg.clone();
    let mb_server = Arc::new(ModbusServer::new(mb_config));
    let mb_for_start = mb_server.clone();

    spawn_supervised("modbus_server", move || {
        let srv = mb_for_start.clone();
        async move {
            if let Err(e) = srv.start().await {
                error!("modbus server exited with error: {}", e);
            }
        }
    });

    // 复用 NorthDispatcher 的关闭信号（同 opcua sink），保证 sink task 停机时优雅 drain。
    let mb_sink = ModbusServerAdapter::with_shutdown(
        mb_server.clone(),
        ctx.north_dispatcher.shutdown_token(),
        ctx.gateway.metrics.clone(),
    );
    mb_sink.build_mappings(ctx.expanded_points, mb_cfg.base_register);
    ctx.north_dispatcher
        .register_connector(mb_sink as Arc<dyn crate::north::NorthConnector>);
    info!(
        "modbus server started on {} ({} mode, {} register mappings)",
        mb_cfg.endpoint,
        mb_cfg.mode,
        ctx.expanded_points.len()
    );

    Some(mb_server)
}

/// Modbus TCP<->RTU 网关桥接服务工厂（可选，由 config.modbus_passthrough.enabled 控制）
fn create_modbus_passthrough(ctx: &NorthServerContext) -> Option<Arc<dyn NorthServer>> {
    use crate::engine::supervision::spawn_supervised;
    use crate::north::server::modbus_gateway_bridge::ModbusGatewayBridgeServer;
    use tracing::{error, info};

    let pt_cfg = ctx.config.modbus_passthrough.clone()?;
    if !pt_cfg.enabled {
        info!("modbus passthrough disabled by config (enabled=false)");
        return None;
    }

    let pt_server = ModbusGatewayBridgeServer::new(pt_cfg.clone(), ctx.gateway.clone());
    let pt_for_start = pt_server.clone();

    spawn_supervised("modbus_passthrough", move || {
        let srv = pt_for_start.clone();
        async move {
            if let Err(e) = srv.start().await {
                error!("modbus passthrough exited with error: {}", e);
            }
        }
    });

    info!(
        "modbus TCP<->RTU passthrough started on {} -> bus {}",
        pt_cfg.listen_addr, pt_cfg.target_bus
    );

    Some(pt_server)
}
