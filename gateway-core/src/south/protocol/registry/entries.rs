//! 协议注册项与单一来源表（`PROTOCOL_ENTRIES`）

use super::capabilities::ProtocolCapabilities;
use super::{CollectorFactory, FrameExtractor};

// 工厂与帧提取器：feature gate 控制，关闭时编译期剥离
#[cfg(feature = "bacnet-mstp")]
use super::super::bacnet_mstp::extract_mstp_frame;
#[cfg(feature = "cjt188")]
use super::super::cjt188::extract_cjt188_frame;
#[cfg(feature = "dlt645")]
use super::super::dlt645::extract_dlt645_frame;
#[cfg(feature = "iec101")]
use super::super::iec101::extract_iec101_frame;
#[cfg(feature = "modbus")]
use super::super::modbus::extract_modbus_frame;
#[cfg(feature = "bacnet")]
use super::factories::create_bacnet;
#[cfg(feature = "bacnet-mstp")]
use super::factories::create_bacnet_mstp;
#[cfg(feature = "cjt188")]
use super::factories::create_cjt188;
#[cfg(feature = "dlt645")]
use super::factories::create_dlt645;
#[cfg(feature = "fins")]
use super::factories::create_fins;
#[cfg(feature = "iec101")]
use super::factories::create_iec101;
#[cfg(feature = "iec104")]
use super::factories::create_iec104;
#[cfg(feature = "mc")]
use super::factories::create_mc;
#[cfg(feature = "modbus")]
use super::factories::create_modbus;
#[cfg(feature = "opcua")]
use super::factories::create_opcua;
#[cfg(feature = "s7comm")]
use super::factories::create_s7comm;

/// 协议注册项：协议名 + 采集器工厂 + 可选帧提取器
///
/// 单一来源（single source of truth）：[`super::ProtocolRegistry::new`] 遍历此表完成全部注册，
/// [`super::VALID_PROTOCOLS`] 与此表保持同步（由单元测试 `test_valid_protocols_match_entries` 校验）。
pub(crate) struct ProtocolEntry {
    /// 协议名（配置中 protocol 字段的合法值）
    pub(super) name: &'static str,
    /// Web UI 展示名（下拉框 / 标签），避免前端硬编码协议清单
    pub(super) label: &'static str,
    /// 物理链路形态：`serial`（串行总线）/ `tcp`（网络）/ `local`（本机 GPIO 等）
    pub(super) transport: &'static str,
    /// 新建总线/设备时的默认超时（毫秒），供 Web 表单预填
    pub(super) default_timeout_ms: u64,
    /// 地址输入提示（Web 表单 placeholder，指**设备/点位地址**而非链路端点）
    pub(super) address_hint: &'static str,
    /// 总线对端地址提示（Web 表单 `tcp_addr` 的 placeholder）；纯串行协议为空串
    pub(super) endpoint_hint: &'static str,
    /// 协议变体可选项（Web 表单 `variant` 的下拉项），元素为 (值, 展示名)；无变体概念时为空切片
    pub(super) variants: &'static [(&'static str, &'static str)],
    /// 协议标签色（Web `el-tag` 的 type），前端不再按协议名硬编码配色
    pub(super) tag_color: &'static str,
    /// Collector 工厂函数
    pub(super) factory: CollectorFactory,
    /// 帧提取器（仅串行总线协议注册；TCP/UDP 协议为 None）
    pub(super) frame_extractor: Option<FrameExtractor>,
    pub(super) capabilities: ProtocolCapabilities,
}

/// 协议注册表（单一来源）
///
/// 新增协议只需在此追加一项，[`super::ProtocolRegistry::new`] 会自动注册 factory 和 frame_extractor。
/// [`super::VALID_PROTOCOLS`] 应与此表保持一致（gpio 除外，gpio 是点位级协议，不进注册表）。
pub(crate) const PROTOCOL_ENTRIES: &[ProtocolEntry] = &[
    #[cfg(feature = "modbus")]
    ProtocolEntry {
        name: "modbus",
        label: "Modbus",
        transport: "serial",
        default_timeout_ms: 1000,
        address_hint: "如: 1（从站号 1-247）",
        endpoint_hint: "如: 192.168.1.10:502（填写后该总线走 Modbus TCP）",
        variants: &[("rtu", "RTU"), ("ascii", "ASCII")],
        tag_color: "primary",
        factory: create_modbus,
        frame_extractor: Some(extract_modbus_frame),
        capabilities: ProtocolCapabilities::modbus(),
    },
    #[cfg(feature = "dlt645")]
    ProtocolEntry {
        name: "dlt645",
        label: "DLT645",
        transport: "serial",
        default_timeout_ms: 1000,
        address_hint: "如: 1（表号）",
        endpoint_hint: "",
        variants: &[
            ("2007", "2007（国标默认）"),
            ("1997", "1997（老款表）"),
            ("1997-addr-be-plain", "1997 大端地址·不加密"),
            ("1997-addr-le-plain", "1997 小端地址·不加密"),
            ("1997-addr-be-cipher", "1997 大端地址·加密"),
            ("1997-addr-le-cipher", "1997 小端地址·加密"),
        ],
        tag_color: "success",
        factory: create_dlt645,
        frame_extractor: Some(extract_dlt645_frame),
        capabilities: ProtocolCapabilities::serial_bcd_meter_writable(),
    },
    #[cfg(feature = "cjt188")]
    ProtocolEntry {
        name: "cjt188",
        label: "CJT188",
        transport: "serial",
        default_timeout_ms: 1000,
        address_hint: "如: 1（表号）",
        endpoint_hint: "",
        variants: &[("2018", "2018（内建方言）"), ("2004", "2004 标准方言")],
        tag_color: "warning",
        factory: create_cjt188,
        frame_extractor: Some(extract_cjt188_frame),
        capabilities: ProtocolCapabilities::serial_bcd_meter_writable(),
    },
    #[cfg(feature = "opcua")]
    ProtocolEntry {
        name: "opcua",
        label: "OPC UA",
        transport: "tcp",
        default_timeout_ms: 3000,
        address_hint: "如: ns=2;s=Demo.Tag（或 opc.tcp://host:4840 下的 NodeId）",
        endpoint_hint: "如: opc.tcp://192.168.1.10:4840",
        variants: &[],
        tag_color: "success",
        factory: create_opcua,
        frame_extractor: None,
        capabilities: ProtocolCapabilities::tcp_batch_readonly(),
    },
    #[cfg(feature = "iec104")]
    ProtocolEntry {
        name: "iec104",
        label: "IEC 60870-5-104",
        transport: "tcp",
        default_timeout_ms: 3000,
        address_hint: "如: 1（信息体地址 IA）",
        endpoint_hint: "如: 192.168.1.10:2404",
        variants: &[],
        tag_color: "danger",
        factory: create_iec104,
        frame_extractor: None,
        capabilities: ProtocolCapabilities::telecontrol_tcp_write(),
    },
    #[cfg(feature = "iec101")]
    ProtocolEntry {
        name: "iec101",
        label: "IEC 60870-5-101",
        transport: "serial",
        default_timeout_ms: 3000,
        address_hint: "如: 1（链路地址 LA）",
        endpoint_hint: "",
        variants: &[],
        tag_color: "info",
        factory: create_iec101,
        frame_extractor: Some(extract_iec101_frame),
        capabilities: ProtocolCapabilities::serial_telecontrol(),
    },
    #[cfg(feature = "mc")]
    ProtocolEntry {
        name: "mc",
        label: "三菱 MC",
        transport: "tcp",
        default_timeout_ms: 3000,
        address_hint: "如: D100 / M10 / X1F",
        endpoint_hint: "如: 192.168.1.10:5000",
        variants: &[
            ("3e", "3E（默认）"),
            ("4e", "4E"),
            ("1e", "1E（FX/A 系列）"),
        ],
        tag_color: "info",
        factory: create_mc,
        frame_extractor: None,
        capabilities: ProtocolCapabilities::tcp_batch_write(),
    },
    // 退役观察：FINS 已移出默认发行集，仅保留 feature 供存量项目/回归测试显式启用。
    // 连续两个版本无项目需求后删除本注册项、协议模块、测试与文档。
    #[cfg(feature = "fins")]
    ProtocolEntry {
        name: "fins",
        label: "欧姆龙 FINS",
        transport: "tcp",
        default_timeout_ms: 3000,
        address_hint: "如: D100 / W200（存储区地址）",
        endpoint_hint: "如: 192.168.1.10:9600（UDP 可追加 ?node=NN）",
        variants: &[("tcp", "TCP（默认）"), ("udp", "UDP")],
        tag_color: "info",
        factory: create_fins,
        frame_extractor: None,
        capabilities: ProtocolCapabilities::tcp_batch_write(),
    },
    #[cfg(feature = "bacnet")]
    ProtocolEntry {
        name: "bacnet",
        label: "BACnet/IP",
        transport: "tcp",
        default_timeout_ms: 3000,
        address_hint: "如: analog-input:1（对象类型:实例号）",
        endpoint_hint: "如: 192.168.1.100:47808",
        variants: &[],
        tag_color: "warning",
        factory: create_bacnet,
        frame_extractor: None,
        capabilities: ProtocolCapabilities::bacnet_ip(),
    },
    #[cfg(feature = "s7comm")]
    ProtocolEntry {
        name: "s7comm",
        label: "西门子 S7",
        transport: "tcp",
        default_timeout_ms: 3000,
        address_hint: "如: DB1.DBD0 / MW10 / I0.0",
        endpoint_hint: "如: 192.168.1.10:102?rack=0&slot=2",
        variants: &[],
        tag_color: "success",
        factory: create_s7comm,
        frame_extractor: None,
        capabilities: ProtocolCapabilities::s7comm(),
    },
    // 退役观察：BACnet MS/TP 已移出默认发行集（楼宇版只交付 BACnet/IP + BBMD），
    // 仅保留 feature 供存量项目/回归测试显式启用。
    // 连续两个版本无项目需求后删除本注册项、协议模块、测试与文档。
    #[cfg(feature = "bacnet-mstp")]
    ProtocolEntry {
        name: "bacnet_mstp",
        label: "BACnet MS/TP",
        transport: "serial",
        default_timeout_ms: 1000,
        address_hint: "如: analog-input:1（对象类型:实例号）",
        endpoint_hint: "可选: master=0&max_master=127",
        variants: &[],
        tag_color: "warning",
        factory: create_bacnet_mstp,
        frame_extractor: Some(extract_mstp_frame),
        capabilities: ProtocolCapabilities::bacnet_mstp(),
    },
];
