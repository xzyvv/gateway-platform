//! Server trait 与 Modbus 从站实现（T3.1）
//!
//! 网关核心语义：不仅采集，也要暴露。
//! - Passthrough 模式：采集点位 → 寄存器映射（详见 `mqtt_packet_passthrough`）
//! - Mirrored 模式：采集数据 + 内部状态
//! - Static 模式：从配置文件加载静态寄存器
//!
//! 与 Collector trait 对偶：Collector 读外部；Server 响应外部读。

use std::future::Future;
use std::pin::Pin;

use tokio::sync::Notify;

use crate::config::PointConfig;
use crate::engine::error::GatewayResult;

pub(crate) mod modbus_gateway_bridge;
pub(crate) mod modbus_server;
pub(crate) mod mqtt_packet_passthrough;
pub(crate) mod opcua_server;
pub(crate) mod registry;

// 启动编排位于 gatewayd（独立 crate），故这两个装配入口对外公开；
// 具体后端实现模块（modbus_server / opcua_server 等）仍为 crate 内私有。
pub use mqtt_packet_passthrough::MqttPacketPassthrough;
pub use registry::{ServerContext, ServerRegistry};

// D-05：Modbus Server 配置统一由 `config/modbus_server.rs` 提供（含 `base_register` /
// `mode`），server 侧不再自行定义同名类型，避免字段集分叉。
pub use crate::config::ModbusServerConfig;

/// Server trait：协议从站抽象
pub trait Server: Send + Sync {
    /// Server 名称（用于 API 展示与日志标识）
    fn name(&self) -> &'static str;

    /// 启动从站
    fn start<'a>(&'a self) -> Pin<Box<dyn Future<Output = GatewayResult<()>> + Send + 'a>>;

    /// 停止从站（由 bootstrap graceful_shutdown 调用）
    fn stop<'a>(&'a self) -> Pin<Box<dyn Future<Output = GatewayResult<()>> + Send + 'a>>;

    /// 是否正在运行
    fn is_running(&self) -> bool;
}

/// r60-SD2：与 r48-MQTT1 同型的「Notify 丢失唤醒」修复助手。
///
/// 旧模式 `notify_waiters()` + `select! { _ = n.notified() => ... }`：
/// 若 `notify_waiters()` 在 `notified()` 还没 `enable()` 的瞬间被调用，
/// 通知丢失——accept loop 永远等不到停机信号，只能等 `running` 标志轮询退出。
///
/// r48 在 `north/mqtt/mod.rs` 通过 `wait_shutdown` + `stopping: AtomicBool` 标志
/// 修复（先 enable 再 notify + 标志复查）；本助手把同一模式导出给 server 模块
/// 复用——`stop()` 先 `stopping.store(true)` 再 `notify_waiters()`，waiter 端
/// `wait_shutdown_notify` 立即检查标志，覆盖「置位与注册之间」的时间窗口。
///
/// 用法：
/// ```ignore
/// // server struct 内：stop_notify: Arc<Notify>, stopping: Arc<AtomicBool>
/// // stop():
/// fn stop() async {
///     self.stopping.store(true, Ordering::SeqCst);
///     self.stop_notify.notify_waiters();
///     Ok(())
/// }
/// // accept loop 内:
/// _ = wait_shutdown_notify(&self.stop_notify, &self.stopping) => break,
/// ```
pub(crate) async fn wait_shutdown_notify(
    shutdown: &Notify,
    stopping: &std::sync::atomic::AtomicBool,
) {
    use std::sync::atomic::Ordering;
    if stopping.load(Ordering::SeqCst) {
        return;
    }
    let notified = shutdown.notified();
    tokio::pin!(notified);
    notified.as_mut().enable();
    if stopping.load(Ordering::SeqCst) {
        return;
    }
    let _ = notified.await;
}

/// 寄存器映射条目
#[derive(Debug, Clone)]
pub struct RegisterMapping {
    pub source_point_id: String,
    pub register_addr: u16,
    pub scale: f64,
    pub offset: f64,
}

impl RegisterMapping {
    /// 从 PointConfig 生成默认映射：地址映射到 source_address_num
    pub fn from_point(p: &PointConfig, base_addr: u16) -> Self {
        Self {
            source_point_id: p.id.clone(),
            register_addr: base_addr,
            scale: p.scale,
            offset: p.offset,
        }
    }
}

#[cfg(test)]
mod tests;
