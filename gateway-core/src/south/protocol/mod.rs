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

/// 协议帧性质测试用的确定性伪随机源（仅测试构建）。
///
/// 只有 modbus / dlt645 / cjt188 的测试会用到；协议裁剪构建下不挂载，
/// 避免 `-D dead_code` 因无调用方报错。
#[cfg(test)]
#[cfg(any(feature = "modbus", feature = "dlt645", feature = "cjt188"))]
pub(crate) mod test_support;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod write_semantics_tests;

pub use common::decode::{
    convert_value, decode_binary_value, parse_bcd_address, reorder_bytes, reverse_convert,
};
pub use common::{codec, decode};
#[cfg(feature = "modbus")]
pub use modbus::ModbusExceptionCode;
pub use registry::{ProtocolRegistry, VALID_PROTOCOLS};

use crate::config::PointConfig;
use crate::engine::metrics::CollectErrorCode;
use crate::engine::model::{AddressReadResult, DataPoint, Quality};
use crate::engine::write::WriteOutcome;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

/// 采集结果类型
pub type CollectorResult<T> = Result<T, CollectorError>;

/// 共享进度槽：批量读取每完成一个点位就把结果写入对应下标。
///
/// 调度层在 `tokio::time::timeout` 下使用：超时 drop 掉 future 时，已完成点位的
/// 结果仍保留在槽中，由调用方逐一取回，不会被整批判成 Timeout（缺陷 #7）。
pub(crate) type ResultSlots = Arc<std::sync::Mutex<Vec<Option<CollectorResult<DataPoint>>>>>;

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
    /// 设备明确拒绝：设备返回了错误码 / 状态字（不是通信失败）。
    ///
    /// 与 [`CollectorError::InvalidFrame`] 的区别：后者是「响应无法解析」
    /// （可能是噪声、错帧、截断），设备有没有执行**无法判断**；
    /// 本变体是「设备回话了，而且明确说不成功」——写**确定未生效**。
    ///
    /// 举例：S7Comm 的 Error class/code、FINS 的 MRES/SRES、
    /// 三菱 MC 的 end code、BACnet 的 BVLC-Result 错误码。
    ///
    /// 写语义价值：设备拒绝属于「明确失败」，调用方可以安全重试；
    /// 而响应解析失败属于「结果未知」，重试可能造成重复写。
    #[error("device rejected the request: {0}")]
    DeviceRejected(String),
    /// 采集超时：scheduler 对 `read_batch` 加超时保护，超时后所有点位返回此错误。
    /// 与 `IoError` 区分，便于监控按"链路 IO 故障"与"协议栈卡死/无响应"分类告警。
    #[error("collector read timeout after {0:?}")]
    Timeout(std::time::Duration),
    /// 总线队列拥堵：事务在超时时间内没能入队（总线 channel 已满）。
    ///
    /// 必须与 [`CollectorError::Timeout`] 区分——后者是「已下发但设备没回」，
    /// 前者是「根本没发出去」。混用会让现场无法判断是设备故障还是总线带宽不足，
    /// 也会把排队超时误算进设备超时率。
    #[error("bus busy: enqueue timeout after {0:?}")]
    BusBusy(std::time::Duration),
    #[error("invalid config: {0}")]
    InvalidConfig(String),
    /// 写结果未知：写帧已下发，但未收到可确认的应答。
    ///
    /// 与 [`CollectorError::NoReply`] 的区别：对读事务两者等价（设备没回）；
    /// 对写事务无法区分「没收到」与「收到并执行了、只是应答丢了」——
    /// 后者已经生效且不可回滚，自动重试会造成重复写。
    ///
    /// 语义：**不要自动重试**。调用方应先读回核实（写值类点位）或人工确认。
    /// 产生条件：写事务在传输层失败（无应答 / 超时 / IO 错 / 校验错），
    /// 且写路径已关闭自动重发（见 [`crate::south::bus::BusTransaction::retry_allowed`]）。
    #[error("write outcome uncertain: {0} (the write may already have been applied - do not retry blindly)")]
    WriteUncertain(String),
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
            // 设备明确拒绝：不是通信故障，而是设备回了错误码。
            // 与 Decode（响应解析失败）分开，便于现场区分「设备说不行」
            // 与「根本没听懂设备说的话」。
            CollectorError::DeviceRejected(_) => CollectErrorCode::Other,
            CollectorError::Timeout(_) => CollectErrorCode::Timeout,
            CollectorError::BusBusy(_) => CollectErrorCode::Busy,
            CollectorError::InvalidConfig(_) => CollectErrorCode::Config,
            // 写结果未知暂归 Other：by_reason 的分桶是固定长度数组，
            // 单独加一桶会牵动标签表与下标映射。写不确定另有专用计数器
            // `gateway_bus_write_uncertain_total`，不靠 by_reason 观察。
            CollectorError::WriteUncertain(_) => CollectErrorCode::Other,
            CollectorError::Unsupported(_) => CollectErrorCode::Other,
        }
    }

    /// 采集失败的质量原因字符串：`<前缀>:<稳定错误码>`（如 `collect_failed:TIMEOUT`）。
    ///
    /// 前缀由调用方给出（场景名 / 协议名），错误码取自 [`Self::error_code`] 的
    /// 稳定口径（`TIMEOUT` / `CRC` / `DECODE` / `EXC_02` ...）。北向报文与前端
    /// 按冒号后的段区分「怎么坏的」，可用 [`CollectErrorCode::from_code`] 还原。
    ///
    /// 为什么要带错误码：只写「采集失败」时，运维无法区分「设备不在线」与
    /// 「报文格式错」，排查方向容易整体跑偏。
    pub fn quality_reason(&self, prefix: &str) -> String {
        format!("{}:{}", prefix, self.error_code().code())
    }

    /// 映射为跨协议统一的写结果三态（见 [`crate::engine::write::WriteOutcome`]）。
    ///
    /// 判据只有一个：**能否证明写没有生效**。
    ///
    /// - `Uncertain`（结果未知，禁止自动重试）：写帧已下发但没有可确认的应答。
    ///   典型是超时、无应答、IO 错、校验错、响应无法解析——设备有没有执行
    ///   **无法判断**，重复下发可能让累加类点位翻倍、脉冲类点位多触发。
    /// - `Failed`（明确失败，可安全重试）：能证明写没有生效。设备回了错误码、
    ///   总线没排上队、参数非法、根本不支持写——写**确定未生效**。
    ///
    /// 注意：本方法只描述**写**语义。读路径上的 `InvalidFrame` / `ChecksumError`
    /// 依然表示「这一帧读坏了」，与写路径的「结果未知」互不干扰。
    pub fn write_outcome(&self) -> WriteOutcome {
        match self {
            // 结果未知：写帧已下发，无法确认是否生效。
            CollectorError::WriteUncertain(_)
            | CollectorError::NoReply
            | CollectorError::Timeout(_)
            | CollectorError::IoError(_)
            | CollectorError::ChecksumError
            | CollectorError::InvalidFrame(_) => WriteOutcome::Uncertain,
            // 明确失败：能证明写没有生效，调用方可安全重试。
            CollectorError::DeviceRejected(_)
            | CollectorError::BusClosed
            | CollectorError::InvalidBcd(_)
            | CollectorError::BusBusy(_)
            | CollectorError::InvalidConfig(_)
            | CollectorError::Unsupported(_) => WriteOutcome::Failed,
            #[cfg(feature = "modbus")]
            CollectorError::ModbusException(_) => WriteOutcome::Failed,
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
        CollectorError::DeviceRejected(s) => CollectorError::DeviceRejected(s.clone()),
        CollectorError::Timeout(d) => CollectorError::Timeout(*d),
        CollectorError::BusBusy(d) => CollectorError::BusBusy(*d),
        CollectorError::InvalidConfig(s) => CollectorError::InvalidConfig(s.clone()),
        CollectorError::WriteUncertain(s) => CollectorError::WriteUncertain(s.clone()),
        CollectorError::Unsupported(s) => CollectorError::Unsupported(s.clone()),
    }
}
/// 写失败定级（协议无关）：把「可能已下发」类错误转成 [`CollectorError::WriteUncertain`]。
///
/// 全网关统一的定级规则——**判据只有一个：能否证明写没有生效**。
///
/// **明确失败**（原样返回，调用方可安全重试）：
/// - [`CollectorError::DeviceRejected`]：设备回了错误码 / 状态字，明确说不成功。
/// - [`CollectorError::ModbusException`]：Modbus 异常码。
/// - [`CollectorError::BusBusy`]：事务根本没入队 / 根本没下发。
/// - [`CollectorError::Unsupported`] / [`CollectorError::InvalidConfig`]：请求本身不合法。
///
/// **结果未知**（转 [`CollectorError::WriteUncertain`]，禁止自动重试）：
/// - [`CollectorError::NoReply`] / [`CollectorError::Timeout`]：请求已发出，设备没回。
/// - [`CollectorError::IoError`]：链路故障，无法证明帧未送达。
/// - [`CollectorError::ChecksumError`]：响应校验错，设备多半已执行，只是响应损坏。
/// - [`CollectorError::InvalidFrame`]：响应无法解析——设备回过东西，
///   写很可能已执行（噪声、错帧、截断都无法排除）。
///
/// 刻意**不做**的事：不按「错误出现在哪个阶段」来定级，只看「能否证明」。
/// 能证明没生效的才是明确失败；证明不了的都算未知——宁可多报一次人工确认，
/// 也不要漏报一次重复写（累加 / 脉冲类点位一次重复写就是数据错）。
///
/// 仅供单元测试锁住纯映射语义（`write_semantics_tests`）；
/// 生产路径一律走带计数的 [`mark_write_uncertain_with`] / [`mark_write_uncertain_counted`]。
#[cfg(all(
    test,
    any(
        feature = "s7comm",
        feature = "fins",
        feature = "mc",
        feature = "iec104",
        feature = "bacnet"
    )
))]
pub(crate) fn mark_write_uncertain(e: CollectorError) -> CollectorError {
    classify_write_uncertain(None, e)
}

/// `mark_write_uncertain` 的带计数版本（自研 TCP/UDP 协议用）。
///
/// G-08: 这些协议没有 `BusManager`，metrics 由采集器在构造时注入
/// （`with_metrics`，生产路径见 `protocol/registry/factories.rs`）。
/// `None` 表示未注入（单元测试 / 独立构造），只做映射不计数。
#[cfg(any(
    feature = "s7comm",
    feature = "fins",
    feature = "mc",
    feature = "iec104",
    feature = "bacnet"
))]
pub(crate) fn mark_write_uncertain_with(
    metrics: Option<&crate::engine::metrics::Metrics>,
    e: CollectorError,
) -> CollectorError {
    classify_write_uncertain(metrics, e)
}

/// 判据只有一条：写已发出但无法证明其未生效 → WriteUncertain。
fn classify_write_uncertain(
    metrics: Option<&crate::engine::metrics::Metrics>,
    e: CollectorError,
) -> CollectorError {
    if matches!(
        e,
        CollectorError::NoReply
            | CollectorError::ChecksumError
            | CollectorError::Timeout(_)
            | CollectorError::IoError(_)
            | CollectorError::InvalidFrame(_)
    ) {
        if let Some(m) = metrics {
            m.inc_write_uncertain();
        }
        return CollectorError::WriteUncertain(e.to_string());
    }
    e
}

/// 走总线总线的协议（modbus / dlt645 / cjt188）：metrics 从 `BusManager` 取。
#[cfg(any(feature = "modbus", feature = "dlt645", feature = "cjt188"))]
pub(crate) fn mark_write_uncertain_counted(
    bus: &crate::south::bus::BusManager,
    e: CollectorError,
) -> CollectorError {
    classify_write_uncertain(Some(bus.metrics()), e)
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

    /// DD-11：当前自适应合并间隔阈值；不支持合并的协议返回 None。
    /// 仅用于 Web「调度优化」只读展示复用运行时真实参数，不参与采集决策。
    fn merge_gap(&self) -> Option<u16> {
        None
    }

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

    /// 与 [`Collector::read_batch`] 等价的批量读取，但每完成一个点位就立即把结果
    /// 写入 `slots` 对应下标（锁外 await、锁内立即写，锁不跨 await）。
    ///
    /// 调度层在 `tokio::time::timeout` 下使用本方法：超时 drop 掉 future 时，
    /// 已完成点位的结果仍保留在槽中，由调用方逐一取回，不会被整批判 Timeout
    /// （缺陷 #7）。覆写 `read_batch` 做批量合并的协议（如 modbus）必须同时覆写
    /// 本方法，在产生结果的每一处同步写槽，以保留其合并 / 串行语义。
    // ResultSlots 是 pub(crate) 别名（底层类型均 public），仅供 crate 内调度层调用。
    #[allow(private_interfaces)]
    fn read_batch_with_slots<'a>(
        &'a self,
        points: &'a [PointConfig],
        slots: ResultSlots,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            for (i, p) in points.iter().enumerate() {
                let r = self.read(p).await;
                slots.lock().unwrap_or_else(|e| e.into_inner())[i] = Some(r);
            }
        })
    }

    /// 按协议原始地址直读（默认不支持，返回错误）
    ///
    /// 与 [`Collector::read`] 的区别：`read` 依赖**已建点位**（`PointConfig`），
    /// 本方法只依赖协议地址本身（如 IEC104 的 IOA），用于「点位表里没有这个点，
    /// 但现场就是想看一眼它的实时值」的调试场景。
    ///
    /// 语义约束（实现方必须遵守）：
    /// - 返回结果中的 `entries` 与传入 `addresses` **等长且逐位对应**；
    /// - 未命中地址 `value` 必须为 `None`，**不得静默返回 0**；
    /// - `snapshot` 为本轮读取到的全量地址快照。
    ///
    /// 仅支持「按地址取值」的协议需要覆盖此方法；默认实现返回
    /// [`CollectorError::Unsupported`]，使 `Arc<dyn Collector>` 路径可安全调用。
    fn read_addresses<'a>(
        &'a self,
        _addresses: &'a [u32],
    ) -> Pin<Box<dyn Future<Output = CollectorResult<AddressReadResult>> + Send + 'a>> {
        Box::pin(async move {
            Err(CollectorError::Unsupported(
                "read_addresses not supported for this protocol".into(),
            ))
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

    /// ROI-11：按点位配置估算组级预算。
    ///
    /// 没有任何点位设置 `timeout_ms` 时与 [`Self::batch_timeout_budget`] 完全一致
    /// （零配置零行为变化）；有配置时按点位累加 `max(1000, timeout_ms × 3)`
    /// （覆盖默认 2 次重试），仍钳制在 [5s, 60s]。
    ///
    /// 这是**上界**：合并读取时多个点位共用一次事务，实际耗时会小于按点位累加；
    /// 宁可为慢设备多等一会儿，也不能提前判组失败丢弃仍在回传的应答。
    fn batch_timeout_budget_for(&self, points: &[PointConfig]) -> Duration {
        const FALLBACK_MS: u64 = 1000;
        const MIN_MS: u64 = 5_000;
        const MAX_MS: u64 = 60_000;
        if points.iter().all(|p| p.timeout_ms.is_none()) {
            return self.batch_timeout_budget(points.len());
        }
        let ms = points
            .iter()
            .map(|p| {
                p.timeout_ms
                    .map(|t| t.saturating_mul(3).max(FALLBACK_MS))
                    .unwrap_or(FALLBACK_MS)
            })
            .fold(0u64, |acc, v| acc.saturating_add(v))
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
