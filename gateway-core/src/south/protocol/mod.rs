//! Collector trait 与采集错误（§3.8 + §3.10）
//!
//! 所有协议采集器（Modbus/DLT645/CJT188/GPIO）均实现此 trait。
//!
//! Q13: DataPoint 扩展 device_id / quality_reason / source_ts / stale_count，
//! good/failed 已统一为新签名（含 device_id 等参数）；不保留旧签名 alias，
//! 各协议采集器（modbus/dlt645/cjt188）均直接调用新签名。

// 协议模块：feature gate 控制，关闭时编译期完全剥离
// common / gpio / registry 始终编译（基座/点位级/注册表）
// skeleton 仅为「复制即用」的协议模板，只在测试构建下编译（D3：移出生产二进制）
#[cfg(feature = "bacnet")]
pub mod bacnet;
#[cfg(feature = "bacnet-mstp")]
pub mod bacnet_mstp;
#[cfg(feature = "cjt188")]
pub mod cjt188;
pub mod common;
#[cfg(feature = "dlt645")]
pub mod dlt645;
#[cfg(feature = "fins")]
pub mod fins;
/// Golden 报文回归库（决策表 #10）：仅在测试构建下编译。
///
/// 见 golden 模块文档：样本落在 `tests/fixtures/golden/<protocol>/*.hex`，
/// 由各协议 `tests.rs` 调用比对，锁定线上字节不被静默改动。
///
/// 仅当至少一个「有 golden 样本」的协议被编译进来时才挂载本模块：
/// 协议裁剪构建（零协议 / 仅 opcua 等）下没有任何调用方，
/// 挂载只会触发 `dead_code` 告警（在 `RUSTFLAGS=-D warnings` 下即构建失败）。
#[cfg(test)]
#[cfg(any(
    feature = "bacnet",
    feature = "bacnet-mstp",
    feature = "cjt188",
    feature = "dlt645",
    feature = "fins",
    feature = "iec101",
    feature = "iec104",
    feature = "mc",
    feature = "modbus",
    feature = "s7comm"
))]
pub(crate) mod golden;
pub mod gpio;
#[cfg(feature = "iec101")]
pub mod iec101;
#[cfg(feature = "iec104")]
pub mod iec104;
#[cfg(feature = "mc")]
pub mod mc;
#[cfg(feature = "modbus")]
pub mod modbus;
#[cfg(feature = "opcua-client")]
pub mod opcua;
pub mod registry;
#[cfg(feature = "s7comm")]
pub mod s7comm;
/// 协议骨架模板（参考用，不注册到 ProtocolRegistry）
///
/// D3：本模块只服务于「复制新协议时的示例」以及几个复用它的单元测试，
/// 生产构建不包含它（减少二进制体积与无用代码面）。新增真实协议请复制本目录
/// 源码后自行实现，不受此 cfg 影响。
#[cfg(test)]
pub mod skeleton;

#[cfg(test)]
mod tests;

pub use common::decode::{convert_value, decode_binary_value, parse_bcd_address, reorder_bytes};
pub use common::{decode, util};
#[cfg(feature = "modbus")]
pub use modbus::ModbusExceptionCode;
pub use registry::{ProtocolRegistry, VALID_PROTOCOLS};

use crate::config::PointConfig;
use crate::engine::metrics::CollectErrorCode;
use crate::engine::model::{DataPoint, Quality};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use thiserror::Error;

/// 采集结果类型
pub type CollectorResult<T> = Result<T, CollectorError>;

/// 采集错误枚举（§3.8）
#[derive(Debug, Error)]
pub enum CollectorError {
    #[error("bus closed")]
    BusClosed,
    #[error("no reply from device")]
    NoReply,
    #[error("invalid frame: {0}")]
    InvalidFrame(String),
    #[error("invalid BCD: {0}")]
    InvalidBcd(String),
    #[error("checksum error")]
    ChecksumError,
    #[cfg(feature = "modbus")]
    #[error("modbus exception 0x{0:02X}: {}", ModbusExceptionCode::from_code(*_0).description())]
    ModbusException(u8),
    #[error("io error: {0}")]
    IoError(String),
    /// 采集超时：scheduler 对 `read_batch` 加超时保护，超时后所有点位返回此错误。
    /// 与 `IoError` 区分，便于监控按"链路 IO 故障"与"协议栈卡死/无响应"分类告警。
    #[error("collector read timeout after {0:?}")]
    Timeout(std::time::Duration),
    #[error("invalid config: {0}")]
    InvalidConfig(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
}

impl CollectorError {
    /// 映射到数据质量码（§7.1）
    ///
    /// Modbus 异常码按可恢复性细分：设备忙/故障 → Uncertain，配置错误 → Bad。
    pub fn to_quality(&self) -> Quality {
        match self {
            CollectorError::InvalidBcd(_) | CollectorError::ChecksumError => Quality::Uncertain,
            #[cfg(feature = "modbus")]
            CollectorError::ModbusException(code) => {
                let exc = ModbusExceptionCode::from_code(*code);
                if exc.is_recoverable() {
                    Quality::Uncertain
                } else {
                    Quality::Bad
                }
            }
            _ => Quality::Bad,
        }
    }

    /// 映射到统计层的稳定错误码（供「最近错误码 / 超时率」展示）。
    ///
    /// `engine` 不反向依赖协议实现：映射只在采集层（south）完成，统计层只接收
    /// [`CollectErrorCode`] 这一稳定口径。
    pub fn error_code(&self) -> CollectErrorCode {
        match self {
            CollectorError::BusClosed => CollectErrorCode::BusClosed,
            CollectorError::NoReply => CollectErrorCode::NoReply,
            CollectorError::InvalidFrame(_) | CollectorError::InvalidBcd(_) => {
                CollectErrorCode::Decode
            }
            CollectorError::ChecksumError => CollectErrorCode::Checksum,
            #[cfg(feature = "modbus")]
            CollectorError::ModbusException(code) => CollectErrorCode::ModbusException(*code),
            CollectorError::IoError(_) => CollectErrorCode::Io,
            CollectorError::Timeout(_) => CollectErrorCode::Timeout,
            CollectorError::InvalidConfig(_) => CollectErrorCode::Config,
            CollectorError::Unsupported(_) => CollectErrorCode::Other,
        }
    }
}

/// 复制一个采集错误：`CollectorError` 未实现 `Clone`，此处按变体重建。
///
/// 供批量操作（批量读片段失败、批量写片段失败）给同一片段内每个点位回填**同因错误**时使用，
/// 同时保留原始变体（NoReply / Timeout / IoError 等），避免统一压平为 `IoError`
/// 导致监控无法按「链路 IO 故障」与「协议栈卡死/无响应」正确分类。
///
/// 仅 modbus / mc / s7comm / bacnet / iec101 采集器的批量回填路径调用；这些协议
/// 全部关闭时本函数无人使用，在 `-D dead_code` 下会报错（F1 协议裁剪矩阵）。
#[cfg(any(
    feature = "modbus",
    feature = "mc",
    feature = "s7comm",
    feature = "bacnet",
    feature = "iec101"
))]
pub(crate) fn clone_collector_error(e: &CollectorError) -> CollectorError {
    match e {
        CollectorError::BusClosed => CollectorError::BusClosed,
        CollectorError::NoReply => CollectorError::NoReply,
        CollectorError::InvalidFrame(s) => CollectorError::InvalidFrame(s.clone()),
        CollectorError::InvalidBcd(s) => CollectorError::InvalidBcd(s.clone()),
        CollectorError::ChecksumError => CollectorError::ChecksumError,
        #[cfg(feature = "modbus")]
        CollectorError::ModbusException(c) => CollectorError::ModbusException(*c),
        CollectorError::IoError(s) => CollectorError::IoError(s.clone()),
        CollectorError::Timeout(d) => CollectorError::Timeout(*d),
        CollectorError::InvalidConfig(s) => CollectorError::InvalidConfig(s.clone()),
        CollectorError::Unsupported(s) => CollectorError::Unsupported(s.clone()),
    }
}
/// Collector trait 接口（§3.10 核心契约）
///
/// A-2: 使用原生 async fn（手写 Pin<Box<dyn Future>>），
/// 替代 async_trait 宏，保持对象安全（支持 Arc<dyn Collector>）。
pub trait Collector: Send + Sync {
    /// 协议名："modbus" / "dlt645" / "cjt188"
    fn name(&self) -> &'static str;

    /// 所属总线名
    fn bus_name(&self) -> &str;

    /// 读取单个点位
    fn read<'a>(
        &'a self,
        point: &'a PointConfig,
    ) -> Pin<Box<dyn Future<Output = CollectorResult<DataPoint>> + Send + 'a>>;

    /// 批量读取（默认实现：逐点调用 read）
    fn read_batch<'a>(
        &'a self,
        points: &'a [PointConfig],
    ) -> Pin<Box<dyn Future<Output = Vec<CollectorResult<DataPoint>>> + Send + 'a>> {
        Box::pin(async move {
            let mut results = Vec::with_capacity(points.len());
            for p in points {
                results.push(self.read(p).await);
            }
            results
        })
    }

    /// 修复(审查#B2)：单次 `read_batch` 的组级超时预算估计。
    ///
    /// 调度器据此为每个采集组设置超时。原实现硬编码 5s：DLT645/CJT188 逐点串行
    /// 读取、单点总线超时默认 500ms，10+ 点位的组每个采集周期必然「超时」而被
    /// 整组判失败——设备与链路其实完全正常。
    ///
    /// 默认按「点位数 × 单点预算」线性估算：单点预算取 1s（覆盖默认总线
    /// timeout_ms=500ms 的一次重试与往返余量），并钳制在 [5s, 60s]。
    /// **持总线、逐点串行读取的协议**（如 DLT645/CJT188）应覆写本方法，
    /// 用自身 `bus.io_timeout()` 估算，使现场配置的 `timeout_ms` 真正生效。
    fn batch_timeout_budget(&self, n_points: usize) -> Duration {
        const PER_POINT_MS: u64 = 1000;
        const MIN_MS: u64 = 5_000;
        const MAX_MS: u64 = 60_000;
        let ms = PER_POINT_MS
            .saturating_mul(n_points.max(1) as u64)
            .clamp(MIN_MS, MAX_MS);
        Duration::from_millis(ms)
    }

    /// 是否实现了真实协议诊断。
    ///
    /// 默认 `false`：协议未覆写 [`Collector::diag`] 时调用只能得到「未实现」提示。
    /// **覆写 `diag` 的协议必须同时覆写本方法返回 `true`**，能力元数据
    /// `ProtocolCapabilities::supports_diag` 与之一致，避免前端/运维把
    /// 「未实现」误读成「设备正常」。
    fn diag_supported(&self) -> bool {
        false
    }

    /// 诊断信息（默认实现：如实报告未实现，禁止返回假 "ok"）
    fn diag<'a>(&'a self) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
        Box::pin(async move {
            format!(
                "{}@{}: diag not implemented for this protocol",
                self.name(),
                self.bus_name()
            )
        })
    }

    /// 写入单个点位（默认不支持，返回错误）
    ///
    /// 仅 Modbus 等支持写操作的协议需要覆盖此方法。
    fn write<'a>(
        &'a self,
        _point: &'a PointConfig,
        _value: f64,
    ) -> Pin<Box<dyn Future<Output = CollectorResult<()>> + Send + 'a>> {
        Box::pin(async move {
            Err(CollectorError::Unsupported(
                "write not supported for this protocol".into(),
            ))
        })
    }

    /// 批量写入多个点位（决策表 #1：多点写入口）
    ///
    /// `points` 为（点位, 目标值）对，返回结果与入参**等长且逐位对应**。
    ///
    /// 默认实现逐点调用 [`Collector::write`]，不改变任何既有协议的行为。
    /// 支持"一次事务写多点"的协议（如 Modbus FC15/FC16）可覆盖此方法以减少事务数；
    /// 覆盖实现必须保证返回顺序与入参一致。
    fn write_multi<'a>(
        &'a self,
        points: &'a [(&'a PointConfig, f64)],
    ) -> Pin<Box<dyn Future<Output = Vec<CollectorResult<()>>> + Send + 'a>> {
        Box::pin(async move {
            let mut results = Vec::with_capacity(points.len());
            for (point, value) in points {
                results.push(self.write(point, *value).await);
            }
            results
        })
    }
    /// 建立连接（默认空实现：串口/RS485 无显式连接，Modbus TCP 等需覆盖）
    ///
    /// 对于长连接协议（Modbus TCP、未来 OPC UA），在采集前调用确保连接就绪。
    /// 串口协议（RTU/DLT645/CJT188）通常无需实现，总线由 BusManager 管理。
    fn connect<'a>(&'a self) -> Pin<Box<dyn Future<Output = CollectorResult<()>> + Send + 'a>> {
        Box::pin(async move { Ok(()) })
    }

    /// 断开连接（默认空实现）
    fn disconnect<'a>(&'a self) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {})
    }

    /// 是否已连接（默认 true：串口协议视为始终连接）
    fn is_connected(&self) -> bool {
        true
    }
}

// Bug 224: `group_indices_by_address` 已随 DLT645/CJT188 批量读改为逐点单 DI
// 事务而失去全部调用方（两协议均按点位独立发起请求，不再需要按表地址分段），
// 连同其单元测试一并删除，避免留下无调用方的实现。
