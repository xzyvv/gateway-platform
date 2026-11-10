//! 调试总线访问层（P0-1：真实帧下发 / 报文回放）
//!
//! 位置说明：本模块放在 `web/` 而非 `web/api/`。`arch_deps` 只约束 `web/api`
//! 不得直接触摸 `gateway_core::south`；这里正是把 core 的总线能力收敛成
//! handler 可用的稳定视图（与 `traits.rs` 的 `OtaStateView` 同一手法），
//! handler 只依赖本模块的本地类型，core 侧重构不会外溢到业务代码。
//!
//! 分工：
//! - [`DebugBusTrait`]：一次「原样写帧 → 原样收字节」的事务（总线互斥/占用判定在 handler 侧）；
//! - [`verify_modbus_rtu`]：把收到的裸字节按 Modbus RTU 规则判成 正常 / CRC 错 / 异常帧，
//!   对应契约 §5 的 `error` 分类；
//! - HEX 编解码与超时上限等纯函数。

use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use gateway_core::engine::codec::crc::crc16;
use gateway_core::south::bus::BusManager;
use gateway_core::south::protocol::CollectorError;

/// 单帧最大长度（契约 §3）：防止误把整段内存/超长粘贴内容打到现场总线上
pub const MAX_FRAME_BYTES: usize = 512;
/// 单帧等待超时默认值（ms）
pub const DEFAULT_TIMEOUT_MS: u64 = 1500;
/// 单帧等待超时上限（ms）：超过则截断，避免一个调试请求把总线吊死
pub const MAX_TIMEOUT_MS: u64 = 10_000;
/// 相邻回放帧间隔默认值（ms）
pub const DEFAULT_INTERVAL_MS: u64 = 200;
/// 相邻回放帧间隔下限（ms）：保护 RTU 半双工总线，避免连发导致从站丢帧
pub const MIN_INTERVAL_MS: u64 = 20;
/// 单次回放执行总时长硬上限
pub const MAX_REPLAY_DURATION: Duration = Duration::from_secs(300);
/// 从站扫描单地址探测超时默认值（ms）：扫描是批量探测，默认比单帧下发短，
/// 避免 247 地址全超时的极端场次拖满时长上限（契约 p0-2 §2）
pub const DEFAULT_SCAN_TIMEOUT_MS: u64 = 500;
/// 从站扫描相邻探测间隔默认值（ms）
pub const DEFAULT_SCAN_INTERVAL_MS: u64 = 50;
/// 单次从站扫描总时长硬上限（与回放同值但语义独立，契约 p0-2 §6.2）
pub const MAX_SCAN_DURATION: Duration = Duration::from_secs(300);

// ===== 抓包帧日志门面（A 档：本网关收发帧） =====
//
// 帧采集点在 core 的 BusManager::transaction / raw_transaction（单点覆盖
// 全部协议与串口/TCP）。arch_deps 禁止 web/api 直引 gateway_core::south，
// 故在此收敛成 handler 可用的本地 DTO（与本模块既有定位一致）。

/// 单帧视图（handler 侧 DTO，不向外暴露 core 类型）
#[derive(Debug, Clone)]
pub struct FrameView {
    /// 记录时刻（Unix 毫秒）
    pub ts_ms: i64,
    /// 总线标识（BusConfig.port_name）
    pub bus: String,
    /// true = 网关下发；false = 设备响应
    pub is_tx: bool,
    /// 完整帧字节
    pub bytes: Vec<u8>,
    /// 仅响应帧：往返耗时（含排队）
    pub latency_ms: Option<i32>,
}

/// 帧快照：按 bus 过滤（None = 全部），新的在前，最多 limit 条。
pub fn frame_snapshot(bus: Option<&str>, limit: usize) -> Vec<FrameView> {
    gateway_core::south::frame_log::snapshot(bus, limit)
        .into_iter()
        .map(|f| FrameView {
            ts_ms: f.ts_ms,
            bus: f.bus,
            is_tx: f.is_tx,
            bytes: f.bytes,
            latency_ms: f.latency_ms,
        })
        .collect()
}

/// 清空全部帧日志（前端「清空」按钮语义）。
pub fn frame_clear() {
    gateway_core::south::frame_log::clear();
}

/// 调试下发失败分类——即契约 §5 的 `error` 字段取值
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusErrorKind {
    /// 等待 `timeout_ms` 未收齐一帧
    Timeout,
    /// 收到帧但 CRC/校验和失败
    Crc,
    /// 协议异常帧（如 Modbus 功能码 | 0x80）
    Exception,
    /// 串口打开失败 / TCP 连接失败 / 驱动未运行
    BusOffline,
    /// 总线被采集引擎或另一调试会话占用
    BusBusy,
    /// 请求帧非法（hex 解析失败 / 超长 / 空帧）
    InvalidFrame,
    /// `bus` 不在当前配置中
    BusNotFound,
    /// 请求缺少必填参数 `bus`
    MissingBus,
    /// 目标总线协议不支持该调试操作（如对非 Modbus 总线发起从站扫描，契约 p0-2 §5）
    UnsupportedProtocol,
}

impl BusErrorKind {
    /// 稳定的机器可读标签（小写 snake_case），即 HTTP `error` 字段
    pub fn as_str(&self) -> &'static str {
        match self {
            BusErrorKind::Timeout => "timeout",
            BusErrorKind::Crc => "crc",
            BusErrorKind::Exception => "exception",
            BusErrorKind::BusOffline => "bus_offline",
            BusErrorKind::BusBusy => "bus_busy",
            BusErrorKind::InvalidFrame => "invalid_frame",
            BusErrorKind::BusNotFound => "bus_not_found",
            BusErrorKind::MissingBus => "missing_bus",
            BusErrorKind::UnsupportedProtocol => "unsupported_protocol",
        }
    }
}

/// 一次调试收发的结果（字节 + 真实耗时）
#[derive(Debug, Clone)]
pub struct ExchangeOutcome {
    pub bytes: Vec<u8>,
    pub elapsed_ms: u64,
}

/// 调试总线 trait（对象安全，风格与 `GatewayRuntimeTrait` 一致：手写 Pin<Box<dyn Future>>）
pub trait DebugBusTrait: Send + Sync {
    /// 原样写帧、原样收字节；失败按 [`BusErrorKind`] 分类。
    ///
    /// 实现方必须保证：不重试、不做协议分帧、不把调试流量计入采集指标。
    fn exchange<'a>(
        &'a self,
        request: &'a [u8],
        timeout_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<ExchangeOutcome, BusErrorKind>> + Send + 'a>>;
}

impl DebugBusTrait for BusManager {
    fn exchange<'a>(
        &'a self,
        request: &'a [u8],
        timeout_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<ExchangeOutcome, BusErrorKind>> + Send + 'a>> {
        Box::pin(async move {
            let started = Instant::now();
            let res = BusManager::raw_transaction(self, request.to_vec(), Some(timeout_ms)).await;
            let elapsed_ms = started.elapsed().as_millis() as u64;
            res.map(|bytes| ExchangeOutcome { bytes, elapsed_ms })
                .map_err(classify_bus_error)
        })
    }
}

/// 把 core 的总线错误映射为调试契约的分类。
///
/// `NoReply` / `Timeout` 都是「设备没回」——现场语义一致，统一报 `timeout`；
/// `ChecksumError` 由分帧器触发（调试直发不经过分帧器，此处保留以防底层调整）；
/// `BusClosed` / `IoError` 属于链路/端口不可用，报 `bus_offline`，
/// 让前端提示「检查串口是否存在 / 驱动是否加载」而不是让用户反复调超时。
pub(crate) fn classify_bus_error(e: CollectorError) -> BusErrorKind {
    match e {
        CollectorError::NoReply | CollectorError::Timeout(_) => BusErrorKind::Timeout,
        CollectorError::ChecksumError => BusErrorKind::Crc,
        // 协议未实现该能力（如某协议不支持按地址直读）
        CollectorError::Unsupported(_) => BusErrorKind::UnsupportedProtocol,
        // 协议裁剪构建（无 modbus feature）下 core 不含该错误变体
        #[cfg(feature = "modbus")]
        CollectorError::ModbusException(_) => BusErrorKind::Exception,
        CollectorError::InvalidFrame(_) => BusErrorKind::InvalidFrame,
        _ => BusErrorKind::BusOffline,
    }
}

/// Modbus RTU 响应帧判定结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameCheck {
    Ok,
    /// CRC16 不匹配，或帧短于最短合法长度（3 字节异常帧 / 5 字节正常帧）
    Crc,
    /// 异常响应（功能码最高位置位），`text` 为异常码的中文释义
    Exception {
        code: u8,
        text: String,
    },
}

/// 按 Modbus RTU 规则校验收到的裸字节。
///
/// 判定顺序很关键：**先查 CRC 再看异常码**。设备返回的异常帧若在传输中被破坏，
/// 直接报异常码会把现场引向「设备不支持该功能」的错误方向；先报 CRC 才是真相。
pub fn verify_modbus_rtu(resp: &[u8]) -> FrameCheck {
    // 最短合法帧：异常帧 5 字节（addr+fc+code+crc16）；正常帧至少 5 字节同理
    if resp.len() < 5 {
        return FrameCheck::Crc;
    }
    let (body, tail) = resp.split_at(resp.len() - 2);
    let expect = crc16(body);
    let got = u16::from_le_bytes([tail[0], tail[1]]);
    if expect != got {
        return FrameCheck::Crc;
    }
    if resp[1] & 0x80 != 0 {
        let code = resp[2];
        return FrameCheck::Exception {
            code,
            text: exception_text(code),
        };
    }
    FrameCheck::Ok
}

/// 异常码 → 中文释义。
#[cfg(feature = "modbus")]
fn exception_text(code: u8) -> String {
    gateway_core::south::protocol::ModbusExceptionCode::from_code(code)
        .description()
        .to_string()
}

/// 协议裁剪构建（无 modbus feature）下的降级释义。
///
/// 该构建里 core 的 `ModbusExceptionCode` 释义表整体被剥离，调试工具仍能判出
/// 「异常帧 + 异常码」，只是没有标准释义可查——这种构建本身不含 Modbus 协议实现。
#[cfg(not(feature = "modbus"))]
fn exception_text(_code: u8) -> String {
    "本构建未编译 Modbus 协议，无异常码释义".to_string()
}

/// HEX 字符串 → 字节。允许空格分隔、大小写混用；空串、奇数长度、非 hex 字符均报错。
pub fn parse_hex(input: &str) -> Result<Vec<u8>, String> {
    let compact: String = input.split_whitespace().collect();
    if compact.is_empty() {
        return Err("hex 不能为空".into());
    }
    if !compact.len().is_multiple_of(2) {
        return Err(format!(
            "hex 长度必须为偶数（当前 {} 个字符）",
            compact.len()
        ));
    }
    if let Some(c) = compact.chars().find(|c| !c.is_ascii_hexdigit()) {
        return Err(format!("hex 含非法字符 '{}'", c));
    }
    if compact.len() / 2 > MAX_FRAME_BYTES {
        return Err(format!(
            "帧长 {} 字节，超过上限 {} 字节",
            compact.len() / 2,
            MAX_FRAME_BYTES
        ));
    }
    Ok((0..compact.len() / 2)
        .map(|i| u8::from_str_radix(&compact[i * 2..i * 2 + 2], 16).unwrap_or(0))
        .collect())
}

/// 字节 → 归一化 HEX（大写、空格分隔），与契约 `request_hex` / `response_hex` 一致
pub fn to_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join(" ")
}

/// HEX 归一化（去空白、统一大写），供回放逐帧对比使用（契约 §4 比较规则 2）
pub fn normalize_hex(hex: &str) -> String {
    hex.split_whitespace()
        .collect::<String>()
        .to_ascii_uppercase()
}

/// 把调用方传入的超时截断到 `[1, MAX_TIMEOUT_MS]`；缺省用 [`DEFAULT_TIMEOUT_MS`]
pub fn clamp_timeout_ms(v: Option<u64>) -> u64 {
    v.unwrap_or(DEFAULT_TIMEOUT_MS).clamp(1, MAX_TIMEOUT_MS)
}

/// 把调用方传入的回放间隔截断到 `[MIN_INTERVAL_MS, MAX_TIMEOUT_MS]`；缺省用默认值
pub fn clamp_interval_ms(v: Option<u64>) -> u64 {
    v.unwrap_or(DEFAULT_INTERVAL_MS).max(MIN_INTERVAL_MS)
}

/// 从站扫描单地址探测超时：缺省 [`DEFAULT_SCAN_TIMEOUT_MS`]，截断到 `[1, MAX_TIMEOUT_MS]`
pub fn clamp_scan_timeout_ms(v: Option<u64>) -> u64 {
    v.unwrap_or(DEFAULT_SCAN_TIMEOUT_MS)
        .clamp(1, MAX_TIMEOUT_MS)
}

/// 从站扫描相邻探测间隔：缺省 [`DEFAULT_SCAN_INTERVAL_MS`]，不低于 [`MIN_INTERVAL_MS`]
pub fn clamp_scan_interval_ms(v: Option<u64>) -> u64 {
    v.unwrap_or(DEFAULT_SCAN_INTERVAL_MS).max(MIN_INTERVAL_MS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gateway_core::engine::codec::crc::crc16;

    #[test]
    fn parse_hex_接受空格与大小写混用() {
        assert_eq!(
            parse_hex("01 03 0a ff").unwrap(),
            vec![0x01, 0x03, 0x0A, 0xFF]
        );
        assert_eq!(parse_hex("0103").unwrap(), vec![0x01, 0x03]);
    }

    #[test]
    fn parse_hex_拒绝空串奇数长度非法字符() {
        assert!(parse_hex("   ").is_err());
        assert!(parse_hex("01 0").is_err());
        assert!(parse_hex("01 GG").is_err());
    }

    #[test]
    fn parse_hex_拒绝超长帧() {
        let too_long = "00".repeat(MAX_FRAME_BYTES + 1);
        assert!(parse_hex(&too_long).is_err());
    }

    #[test]
    fn normalize_hex_去空白并大写() {
        assert_eq!(normalize_hex(" 01 03  ab CD "), "0103ABCD");
    }

    #[test]
    fn clamp_timeout_截断到上限且保留缺省() {
        assert_eq!(clamp_timeout_ms(None), DEFAULT_TIMEOUT_MS);
        assert_eq!(clamp_timeout_ms(Some(0)), 1);
        assert_eq!(clamp_timeout_ms(Some(99_999)), MAX_TIMEOUT_MS);
    }

    #[test]
    fn clamp_interval_不低于下限() {
        assert_eq!(clamp_interval_ms(None), DEFAULT_INTERVAL_MS);
        assert_eq!(clamp_interval_ms(Some(0)), MIN_INTERVAL_MS);
    }

    #[test]
    fn clamp_scan_扫描参数用扫描专属缺省值() {
        assert_eq!(clamp_scan_timeout_ms(None), DEFAULT_SCAN_TIMEOUT_MS);
        assert_eq!(clamp_scan_timeout_ms(Some(0)), 1);
        assert_eq!(clamp_scan_timeout_ms(Some(99_999)), MAX_TIMEOUT_MS);
        assert_eq!(clamp_scan_interval_ms(None), DEFAULT_SCAN_INTERVAL_MS);
        assert_eq!(clamp_scan_interval_ms(Some(0)), MIN_INTERVAL_MS);
    }

    #[test]
    fn error_kind_新增变体机器码稳定() {
        assert_eq!(
            BusErrorKind::UnsupportedProtocol.as_str(),
            "unsupported_protocol"
        );
    }

    /// 用真实 CRC 拼帧，避免测试自身写死常量导致的误判
    fn with_crc(prefix: &[u8]) -> Vec<u8> {
        let mut v = prefix.to_vec();
        let c = crc16(prefix);
        v.extend_from_slice(&c.to_le_bytes());
        v
    }

    #[test]
    fn verify_正常响应帧判为ok() {
        let frame = with_crc(&[0x01, 0x03, 0x04, 0x00, 0x2A, 0x00, 0x00]);
        assert_eq!(verify_modbus_rtu(&frame), FrameCheck::Ok);
    }

    #[test]
    fn verify_crc错帧判为crc() {
        let mut frame = with_crc(&[0x01, 0x03, 0x04, 0x00, 0x2A, 0x00, 0x00]);
        let last = frame.len() - 1;
        frame[last] ^= 0xFF;
        assert_eq!(verify_modbus_rtu(&frame), FrameCheck::Crc);
    }

    #[test]
    fn verify_截断帧判为crc() {
        assert_eq!(verify_modbus_rtu(&[0x01, 0x03]), FrameCheck::Crc);
    }

    #[test]
    fn verify_异常帧给出中文释义() {
        // 01 83 02：非法数据地址
        let frame = with_crc(&[0x01, 0x83, 0x02]);
        match verify_modbus_rtu(&frame) {
            FrameCheck::Exception { code, text } => {
                assert_eq!(code, 2);
                assert!(
                    text.contains("非法"),
                    "异常码 02 的释义应含「非法」，实际：{}",
                    text
                );
            }
            other => panic!("期望异常帧，实际 {:?}", other),
        }
    }
}
