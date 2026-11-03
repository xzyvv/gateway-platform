//! 内置协议 Collector 工厂函数（由 `PROTOCOL_ENTRIES` 引用）

use std::sync::Arc;

// 协议 use 声明：feature gate 控制，关闭时编译期剥离
#[cfg(feature = "bacnet")]
use super::super::bacnet::BacnetCollector;
#[cfg(feature = "bacnet-mstp")]
use super::super::bacnet_mstp::BacnetMstpCollector;
#[cfg(feature = "cjt188")]
use super::super::cjt188::{Cjt188Collector, Cjt188Version};
#[cfg(feature = "dlt645")]
use super::super::dlt645::{Dlt645Collector, Dlt645Version};
#[cfg(feature = "fins")]
use super::super::fins::FinsCollector;
#[cfg(feature = "iec101")]
use super::super::iec101::Iec101Collector;
#[cfg(feature = "iec104")]
use super::super::iec104::Iec104Collector;
#[cfg(feature = "mc")]
use super::super::mc::McCollector;
#[cfg(feature = "modbus")]
use super::super::modbus::ModbusCollector;
#[cfg(feature = "opcua")]
use super::super::opcua::OpcUaCollector;
#[cfg(feature = "s7comm")]
use super::super::s7comm::S7CommCollector;
use super::super::Collector;
use crate::engine::metrics::Metrics;
use crate::south::bus::BusManager;

// ===== 内置协议工厂函数 =====

/// Modbus RTU/TCP 采集器（串行 / TCP 双模式，注册帧提取器）
#[cfg(feature = "modbus")]
pub(super) fn create_modbus(
    port_name: &str,
    bus: Arc<BusManager>,
    _metrics: Arc<Metrics>,
) -> Arc<dyn Collector> {
    Arc::new(ModbusCollector::new(port_name, bus))
}

/// DLT645 电力仪表采集器（RS485，注册帧提取器）
#[cfg(feature = "dlt645")]
pub(super) fn create_dlt645(
    port_name: &str,
    bus: Arc<BusManager>,
    _metrics: Arc<Metrics>,
) -> Arc<dyn Collector> {
    // 决策表 #24：统一变体（effective_variant）驱动版本分派。
    // B1：支持 1997 厂家子变体（addr-LE/BE × data-plain/cipher），
    // 配置字符串解析集中在 `Dlt645Version::from_config`，此处不再自行 if-else。
    let version = Dlt645Version::from_config(bus.variant().unwrap_or(""));
    Arc::new(Dlt645Collector::with_version(port_name, bus, version))
}

/// CJT188 水气热仪表采集器（RS485，注册帧提取器）
#[cfg(feature = "cjt188")]
pub(super) fn create_cjt188(
    port_name: &str,
    bus: Arc<BusManager>,
    metrics: Arc<Metrics>,
) -> Arc<dyn Collector> {
    // 决策表 #24/#6：统一变体驱动方言分派（默认 Vendor7，保持既有行为）。
    let version = Cjt188Version::from_config(bus.variant().unwrap_or(""));
    Arc::new(Cjt188Collector::with_version(
        port_name, bus, metrics, version,
    ))
}

/// OPC UA 客户端采集器（TCP，端点取自 bus.tcp_addr）
#[cfg(feature = "opcua")]
pub(super) fn create_opcua(
    port_name: &str,
    bus: Arc<BusManager>,
    _metrics: Arc<Metrics>,
) -> Arc<dyn Collector> {
    let endpoint = bus
        .tcp_addr
        .clone()
        .unwrap_or_else(|| "opc.tcp://localhost:4840".into());
    // P0-3: 认证/安全配置随总线下发；None 保持历史匿名 + None 明文行为
    Arc::new(OpcUaCollector::with_auth(
        endpoint,
        port_name.to_string(),
        bus.opcua_config().cloned(),
    ))
}

/// IEC 60870-5-104 主站采集器（TCP，默认端口 2404）
#[cfg(feature = "iec104")]
pub(super) fn create_iec104(
    port_name: &str,
    bus: Arc<BusManager>,
    _metrics: Arc<Metrics>,
) -> Arc<dyn Collector> {
    let addr = bus
        .tcp_addr
        .clone()
        .unwrap_or_else(|| "localhost:2404".into());
    let io_timeout = bus.io_timeout();
    Arc::new(Iec104Collector::new(
        addr,
        port_name.to_string(),
        io_timeout,
        bus.asdu_addr_len(),
    ))
}

/// IEC 60870-5-101 主站采集器（串行 FT1.2，注册帧提取器）
#[cfg(feature = "iec101")]
pub(super) fn create_iec101(
    port_name: &str,
    bus: Arc<BusManager>,
    _metrics: Arc<Metrics>,
) -> Arc<dyn Collector> {
    Arc::new(Iec101Collector::new(port_name, bus))
}

/// 三菱 MC 采集器（TCP，QnA 3E 二进制帧，默认端口 6000）
#[cfg(feature = "mc")]
pub(super) fn create_mc(
    port_name: &str,
    bus: Arc<BusManager>,
    _metrics: Arc<Metrics>,
) -> Arc<dyn Collector> {
    let addr = bus
        .tcp_addr
        .clone()
        .unwrap_or_else(|| "localhost:6000".into());
    let io_timeout = bus.io_timeout();
    // 帧格式经统一变体解析点（决策表 #16 / #24）传入采集器
    Arc::new(McCollector::new(
        addr,
        port_name.to_string(),
        io_timeout,
        bus.variant().map(str::to_string),
    ))
}

/// 欧姆龙 FINS 采集器（FINS/TCP 默认；`variant=udp` 时走 FINS/UDP，默认端口 9600）
#[cfg(feature = "fins")]
pub(super) fn create_fins(
    port_name: &str,
    bus: Arc<BusManager>,
    _metrics: Arc<Metrics>,
) -> Arc<dyn Collector> {
    let addr = bus
        .tcp_addr
        .clone()
        .unwrap_or_else(|| "localhost:9600".into());
    let io_timeout = bus.io_timeout();
    // 传输模式经统一变体解析点（决策表 #24）传入采集器
    Arc::new(FinsCollector::new(
        addr,
        port_name.to_string(),
        io_timeout,
        bus.variant().map(str::to_string),
    ))
}

/// BACnet/IP 采集器（UDP，默认端口 47808）
#[cfg(feature = "bacnet")]
pub(super) fn create_bacnet(
    port_name: &str,
    bus: Arc<BusManager>,
    _metrics: Arc<Metrics>,
) -> Arc<dyn Collector> {
    let addr = bus
        .tcp_addr
        .clone()
        .unwrap_or_else(|| "localhost:47808".into());
    let io_timeout = bus.io_timeout();
    Arc::new(BacnetCollector::new(
        addr,
        port_name.to_string(),
        io_timeout,
    ))
}

/// S7comm 西门子 PLC 采集器（TCP，默认 rack=0 slot=2）
#[cfg(feature = "s7comm")]
pub(super) fn create_s7comm(
    port_name: &str,
    bus: Arc<BusManager>,
    _metrics: Arc<Metrics>,
) -> Arc<dyn Collector> {
    let addr = bus
        .tcp_addr
        .clone()
        .unwrap_or_else(|| "localhost:102?rack=0&slot=2".into());
    let io_timeout = bus.io_timeout();
    Arc::new(S7CommCollector::new(
        addr,
        port_name.to_string(),
        io_timeout,
    ))
}

/// BACnet MS/TP 主站采集器（RS485，注册帧提取器）
#[cfg(feature = "bacnet-mstp")]
pub(super) fn create_bacnet_mstp(
    port_name: &str,
    bus: Arc<BusManager>,
    metrics: Arc<Metrics>,
) -> Arc<dyn Collector> {
    let config = bus.tcp_addr.clone().unwrap_or_default();
    let master_addr = BacnetMstpCollector::parse_master_addr(&config);
    Arc::new(BacnetMstpCollector::new(
        port_name,
        bus,
        master_addr,
        metrics,
    ))
}
