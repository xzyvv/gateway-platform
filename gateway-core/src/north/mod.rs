//! 北向（上报层）：MQTT / Webhook / Server（OPC UA + Modbus）
//!
//! 统一管理所有北向上报通道。NorthConnector trait + ConversionManager
//! 负责把 RealtimeBus 的采集数据分发到已注册的 connector。

pub mod conversion;
pub mod mqtt;
pub mod outbox_monitor;
pub mod server;
pub mod server_sinks;
pub mod sim_consumer;
pub mod template;
pub mod webhook;

pub use conversion::ConversionManager;
pub use server_sinks::ModbusSink;
#[cfg(feature = "opcua-server")]
pub use server_sinks::OpcUaSink;
pub use sim_consumer::{SimConsumer, SimConsumerStats};

use crate::engine::error::GatewayResult;
use crate::engine::model::DataPoint;

/// 北向连接器统一 SPI（1.2: 替代 NorthSink + NorthboundSink + PassthroughNode）
///
/// 所有北向通道（MQTT/Webhook/Server Sinks）实现此 trait，
/// 由 ConversionManager 统一持有，消除 3 套抽象。
#[async_trait::async_trait]
pub trait NorthConnector: Send + Sync {
    /// 连接器名称（如 "mqtt" / "webhook" / "modbus_sink"）
    fn name(&self) -> &str;
    /// 启动连接器（建立连接、启动 event loop 等）
    ///
    /// 默认 no-op：多数连接器在 `new()` / `with_shutdown()` 时已启动内部 task
    /// （或由 bootstrap 外部启动 run 循环），此处无需再做什么。
    async fn start(&self) -> GatewayResult<()> {
        Ok(())
    }
    /// 停止连接器（优雅关闭）
    ///
    /// 默认 no-op：优雅停机通常由 `ShutdownToken` 或 drop tx sender 触发；
    /// 需要显式清理的连接器（如 MQTT）覆盖此方法。
    async fn stop(&self) {}
    /// 发布一批数据点（非阻塞语义：立即返回 Ok，内部 spawn 异步处理）
    async fn publish_batch(&self, batch: &[DataPoint]) -> GatewayResult<()>;
    /// 健康状态
    fn health(&self) -> ConnectorHealth;
    /// 是否启用（依据配置）
    fn is_enabled(&self) -> bool;
}

/// 连接器健康状态
#[derive(Debug, Clone, Default)]
pub struct ConnectorHealth {
    pub connected: bool,
    pub last_error: Option<String>,
}

/// N-7：北向连接器状态快照（供 `/api/north/status` 展示真实连通性）。
///
/// 与 [`ConnectorHealth`] 的区别是带上 `name` / `enabled`，可直接序列化为
/// 通道状态；`connected` 一律来自 connector 自身的 `health()`，不再由配置推断。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorStatus {
    pub name: String,
    pub enabled: bool,
    pub connected: bool,
    pub last_error: Option<String>,
}

#[cfg(test)]
mod tests;
