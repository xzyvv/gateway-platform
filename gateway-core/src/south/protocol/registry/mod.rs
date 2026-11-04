//! 协议注册表（南向协议插件化）
//!
//! 将「协议名 → Collector 构造函数」和「协议名 → 帧提取器」的映射集中管理。
//! 新增协议只需在 [`PROTOCOL_ENTRIES`] 表追加一行，无需额外调用 register。
//!
//! # 快速开始：复制 `skeleton` 模板
//!
//! `src/south/protocol/skeleton/` 目录是最小可运行示例（RS485 串行协议骨架；
//! 注意它只在测试构建下编译，见 `south/protocol/mod.rs` 的 `#[cfg(test)]`），
//! 包含 `SkeletonCollector`、`extract_skeleton_frame` 和单元测试。
//! 新增协议推荐流程：
//!
//! 1. `cp -r src/south/protocol/skeleton src/south/protocol/<your_protocol>`
//! 2. 重命名 `SkeletonCollector` -> `YourProtocolCollector`，更新 `name()` 返回值
//! 3. 在 `gateway-core/Cargo.toml` 的 `[features]` 新增 `your_protocol = []`，并加入 `all-protocols`
//! 4. 在 [`super`]（`src/south/protocol/mod.rs`）追加
//!    `#[cfg(feature = "your_protocol")] pub mod your_protocol;`
//! 5. 在本文件 [`PROTOCOL_ENTRIES`] 追加注册项（见下方"最小示例"），同样用 `#[cfg(feature = "...")]` 门控
//! 6. 在 `src/domain.rs` 的 [`VALID_PROTOCOLS`] 数组追加协议名并加 `#[cfg(feature = "...")]`
//!    （本文件只是 re-export，该数组需手工维护，不会自动同步）
//! 7. 运行 `cargo test -p gateway-core protocol::registry` 验证
//!    `test_valid_protocols_match_entries` 与 `test_protocol_three_way_sync` 仍通过
//!
//! # 新增协议完整 checklist
//!
//! ## 代码
//! - [ ] 在 `src/south/protocol/<protocol>/` 下创建 `mod.rs`，实现 `Collector` trait
//! - [ ] 串行协议：在 `frames.rs` 实现 `extract_<protocol>_frame`（可参考 skeleton 模板中的 `extract_skeleton_frame`）
//! - [ ] TCP/UDP 协议：使用 `bus.tcp_addr` 作为连接端点，无需帧提取器
//! - [ ] 在 `gateway-core/Cargo.toml` 的 `[features]` 新增 `<protocol> = []`，并加入 `all-protocols`
//! - [ ] 在 `src/south/protocol/mod.rs` 追加 `#[cfg(feature = "<protocol>")] pub mod <protocol>;`
//!
//! ## 注册
//! - [ ] 在本文件 [`PROTOCOL_ENTRIES`] 追加 `ProtocolEntry`（见下方"最小示例"），并用 `#[cfg(feature = "<protocol>")]` 门控
//! - [ ] 填写 `label` / `transport` / `default_timeout_ms` / `address_hint` / `tag_color`
//!   （前端 UI 全部由注册表驱动，不再按协议名硬编码配色）
//! - [ ] 在 `src/domain.rs` 的 [`VALID_PROTOCOLS`] 追加协议名并加 `#[cfg(feature = "<protocol>")]`
//!   （该数组由 domain.rs 手工维护，本文件仅 re-export，**不会自动同步**；
//!   `PROTOCOL_ENTRIES` 有项而数组漏加时，`test_valid_protocols_match_entries` 会失败）
//! - [ ] 点位级协议（如 `gpio`）：不进 `PROTOCOL_ENTRIES`，仅在 [`VALID_PROTOCOLS`] 手动追加，
//!   且 `gpio` 始终合法、不加 feature 门控
//!
//! ## 测试
//! - [ ] 在 `<protocol>/tests.rs` 或 `mod.rs` 内 `#[cfg(test)] mod tests` 补充单元测试
//! - [ ] 至少覆盖：请求帧构造、响应解析、帧提取器（如有）
//! - [ ] 运行 `cargo test -p gateway-core protocol::<protocol>` 全部通过
//!
//! ## 命名约定
//! - 协议名（`ProtocolEntry.name` 与 `VALID_PROTOCOLS`）：全小写 + 下划线
//!   - 示例：`bacnet_mstp` / `iec104` / `dlt645`
//! - 工厂函数：`create_<protocol>(port_name, bus) -> Arc<dyn Collector>`
//! - 帧提取器：`extract_<protocol>_frame(buf, expected_len) -> Option<Vec<u8>>`
//! - 模块目录：`src/south/protocol/<protocol>/{mod.rs, frames.rs, tests.rs}`
//!
//! # 最小 ProtocolEntry 示例
//!
//! ```ignore
//! // 串行总线协议（RS485，需要帧提取器）
//! ProtocolEntry {
//!     name: "your_protocol",
//!     factory: create_your_protocol,
//!     frame_extractor: Some(extract_your_protocol_frame),
//! },
//!
//! // TCP/UDP 协议（无需帧提取器）
//! ProtocolEntry {
//!     name: "your_protocol",
//!     factory: create_your_protocol,
//!     frame_extractor: None,
//! },
//! ```
//!
//! # 协议分类
//!
//! - **串行总线**（RS485）：modbus / dlt645 / cjt188 / bacnet_mstp
//!   - 需要 `FrameExtractor` 从 `FrameBuffer` 提取完整帧
//! - **TCP/UDP**：opcua / iec104 / bacnet / s7comm
//!   - 用 `bus.tcp_addr` 作为连接端点，不注册帧提取器
//! - **GPIO 点位级**：gpio（不在 `PROTOCOL_ENTRIES` 中，单独处理）
//!   - 但 `gpio` 必须出现在 `VALID_PROTOCOLS` 中，以便配置校验通过
//!
//! # 新协议接入深度指南
//!
//! 上述 checklist 列出了"做什么"，本节说明"怎么做"的关键实现细节。
//!
//! ## Collector trait 实现要点
//!
//! [`super::Collector`] trait（定义于 `mod.rs`）必须实现：
//! - `name(&self) -> &'static str`：返回协议名，须与 `ProtocolEntry.name` 一致
//! - `bus_name(&self) -> &str`：所属总线名（串行协议返回端口名如 "COM1"，
//!   TCP 协议返回地址）
//! - `read(&self, point: &PointConfig) -> Future<CollectorResult<DataPoint>>`：
//!   读单个点位，返回异步 Future。错误用 `GatewayError` 的**具体变体**
//!   （Config/Protocol/NoReply/...），勿用 `Message` 兜底（code 9999，
//!   监控盲点，见 `crate::engine::error`）
//! - `read_batch`：有默认实现（逐点调 read），高性能协议可覆盖做批量请求
//!   （如 Modbus FC03 一次读多寄存器，减少往返）
//!
//! trait 约束 `Send + Sync`：采集器可能跨线程调度，内部共享状态用
//! `Arc`/`Mutex`/`parking_lot` 保护，**勿用 `Rc`/`RefCell`**（非线程安全）。
//!
//! ## 帧提取器实现（仅串行总线协议）
//!
//! `FrameExtractor` 签名：`fn(&mut FrameBuffer, Option<u16>) -> Option<Vec<u8>>`
//! - 参数 1 `&mut FrameBuffer`：串口原始字节缓冲，提取成功后消费已读字节
//! - 参数 2 `Option<u16>`：期望响应长度（部分协议可预知，如 Modbus 字节计数；
//!   未知时为 None，由提取器自行判断帧边界）
//! - 返回 `Some(Vec<u8>)`：一帧完整报文（含 CRC）
//! - 返回 `None`：缓冲数据不足以构成完整帧，框架等待更多数据后重试
//!
//! 实现要点：
//! - 用 CRC/校验和确认帧完整性（参考 `extract_modbus_frame`）
//! - 帧不完整返回 None，**不要阻塞等待**（超时由 `BusManager` 管理）
//! - 提取成功后从 `FrameBuffer` 移除已消费字节（drain）
//!
//! ## 串行 vs TCP 数据流
//!
//! **串行协议**（modbus/dlt645/cjt188/bacnet_mstp）：
//! 1. `BusManager` 从串口读字节到 `FrameBuffer`
//! 2. 帧提取器从 `FrameBuffer` 提取完整帧
//! 3. `Collector::read` 构造请求帧 → 通过 bus 发送 → 等待帧提取器返回响应
//!
//! **TCP/UDP 协议**（opcua/iec104/bacnet/s7comm）：
//! 1. `bus.tcp_addr` 提供连接端点
//! 2. `Collector::new(endpoint, ...)` 内部建立 TCP 连接
//! 3. **不注册** `frame_extractor`（TCP 流式协议自带帧边界，
//!    如 OPC UA 报头长度字段、IEC 104 起始字节 0x68）
//!
//! ## 配置集成
//!
//! 新协议注册后，在 `config.json` 的 `devices` 数组配置即可生效：
//! ```json
//! { "name": "电表1", "protocol": "your_protocol", "port": "COM3", "address": 1 }
//! ```
//! - `protocol` 字段值必须与 `ProtocolEntry.name` / `VALID_PROTOCOLS` 一致
//! - 配置校验由 `GatewayConfig::validate_all` 调用，非法 protocol 返回错误
//! - 点位级协议（gpio）在 `points` 数组配置，不走 `devices`
//!
//! ## 错误处理
//!
//! `Collector::read` 返回 `CollectorResult<DataPoint>`（即 `Result<_, GatewayError>`）。
//! 新协议应使用具体错误变体，便于监控告警分类（见 `crate::engine::error`）：
//! - 帧格式/CRC 错误 → `GatewayError::Protocol`（code 3000）
//! - 设备无响应/超时 → `GatewayError::NoReply`（code 0）
//! - 配置非法值 → `GatewayError::Config`（code 1000）
//! - 串口/网络 IO → `GatewayError::Io`（code 2000）
//! - **勿用** `GatewayError::Message` 兜底（code 9999，监控盲点）
//!
//! # 模块结构
//!
//! - `capabilities.rs`：协议能力位 `ProtocolCapabilities` 与命名构造器
//! - `entries.rs`：协议注册项 `ProtocolEntry` 与单一来源表 [`PROTOCOL_ENTRIES`]
//! - `protocol_registry.rs`：`ProtocolRegistry`（协议名 → 工厂 / 帧提取器映射）
//! - `descriptors.rs`：`ProtocolDescriptor` 与协议清单 / 能力查询辅助
//! - `factories.rs`：内置协议 Collector 工厂函数
//! - `tests.rs`：注册表一致性测试（三向同步 / 能力位回归钉子）
//!
//! 本模块对外只做重导出，`crate::south::protocol::registry::Xxx` 路径保持不变。

use std::sync::Arc;

use super::Collector;
use crate::engine::metrics::Metrics;
use crate::south::bus::{BusManager, FrameBuffer};

mod capabilities;
mod descriptors;
mod entries;
// 工厂函数所在的子模块仅在编译至少一个协议时存在；零协议构建整块剥离
#[cfg(any(
    feature = "modbus",
    feature = "dlt645",
    feature = "cjt188",
    feature = "opcua-client",
    feature = "iec104",
    feature = "bacnet",
    feature = "s7comm",
    feature = "bacnet-mstp",
    feature = "mc",
    feature = "fins",
    feature = "iec101"
))]
mod factories;
mod protocol_registry;

#[cfg(test)]
mod tests;

// 子模块成员统一由本模块重导出，引用方路径 `registry::Xxx` 无需改动
pub use capabilities::ProtocolCapabilities;
pub use descriptors::{
    all_protocol_capabilities, all_protocol_descriptors, protocol_capabilities, ProtocolDescriptor,
};
pub use protocol_registry::ProtocolRegistry;

// 供 `tests`（`use super::*`）与本文档链接取用；生产代码经 `entries` 直接引用
#[allow(unused_imports)]
pub(crate) use entries::PROTOCOL_ENTRIES;

/// Collector 工厂函数：根据端口名和总线管理器创建采集器
pub type CollectorFactory =
    fn(port_name: &str, bus: Arc<BusManager>, metrics: Arc<Metrics>) -> Arc<dyn Collector>;

/// 帧提取函数类型：从 FrameBuffer 中按协议规则提取一帧完整报文
pub type FrameExtractor = fn(&mut FrameBuffer, Option<u16>) -> Option<Vec<u8>>;

/// 合法协议名白名单（配置校验与 API 校验的唯一来源）
///
/// 定义已下沉到中立模块 [`crate::domain`]（消除 `config` → `south` 反向依赖），
/// 此处保留 re-export，使 `south::protocol::VALID_PROTOCOLS` 对外路径保持不变。
/// 不变式：本数组必须与 [`PROTOCOL_ENTRIES`] 保持同步
/// （由单元测试 `test_valid_protocols_match_entries` 校验）。
pub use crate::domain::VALID_PROTOCOLS;
