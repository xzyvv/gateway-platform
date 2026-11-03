//! CJT188 帧处理（构建 / 解析 / 解码）
//!
//! 帧格式：68 ADDR(7) CTRL LEN DATA CS 16
//! - 地址：7 字节，传输时低字节在前
//! - 数据区每个字节传输前 +0x33，接收后 -0x33
//! - 校验和：从第一个 68 到 CS 前所有字节之和 mod 256

use crate::config::PointConfig;
use crate::engine::model::Quality;
use crate::south::bus::FrameBuffer;
use crate::south::protocol::common::decode::{decode_value_by_type, encode_value_by_type};
use crate::south::protocol::common::frame_extract::{
    build_68_frame, extract_dlt645_cjt188_frame, parse_68_response,
};
use crate::south::protocol::common::util::hex_digit;
use crate::south::protocol::{CollectorError, CollectorResult};

/// CJT188 方言（配置项 `bus.variant`，决策表 #24/#6）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cjt188Version {
    /// 内建方言（默认）：7 字节 DI、无序列号、数据区 +0x33 偏移。
    Vendor7,
    /// CJ/T 188-2004 标准方言：2 字节 DI（DI1 DI0）、1 字节序列号 SER、数据区无偏移。
    V2004,
}

impl Cjt188Version {
    /// 从配置字符串解析；不识别值回退 Vendor7（保持既有行为）。
    ///
    /// CJ/T 188-2018 的明文帧数据区布局与 2004 一致（2 字节 DI + SER，无 +0x33 偏移），
    /// 本实现未提供 2018 新增的密文传输能力，故将 `2018` 与 `2004` 归为同一方言。
    pub fn from_config(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "2004" | "2018" => Self::V2004,
            _ => Self::Vendor7,
        }
    }

    /// 数据标识（DI）字节数：Vendor7=7，V2004=2。
    pub fn data_id_len(self) -> usize {
        match self {
            Self::Vendor7 => 7,
            Self::V2004 => 2,
        }
    }

    /// 响应数据域中「DI(+SER) 前缀」长度：Vendor7=7，V2004=2+1(SER)=3。
    pub fn response_prefix_len(self) -> usize {
        match self {
            Self::Vendor7 => 7,
            Self::V2004 => 3,
        }
    }

    /// 数据区是否使用 +0x33 偏移（Vendor7 使用，V2004 标准不使用）。
    pub fn uses_offset(self) -> bool {
        matches!(self, Self::Vendor7)
    }
}

/// CJ/T 188 请求/响应默认序列号 SER（单机场景固定 0）。
pub(crate) const DEFAULT_SER: u8 = 0x00;

/// 构建 CJT188 请求帧
///
/// Bug D1: CJT188 LEN 字段为 1 字节（u8），data.len() > 255 时 as u8 静默截断，
/// 导致设备按错误长度解析整帧失败（批量读每个 DI 占 7 字节，约 37 个点位即触发）。
/// 与 DLT645 侧对齐：超长直接丢弃并报错，不再静默接受。
pub(crate) fn build_cjt188_frame(addr: &[u8], ctrl: u8, data: &[u8]) -> Vec<u8> {
    // CJT188 无第二个 0x68 头、地址低字节在前（不反转）
    build_68_frame(addr, false, false, ctrl, data, "CJT188")
}

/// 解析 CJT188 响应帧（原始数据域，由 `remove_offset` 决定是否做 -0x33）
pub(crate) fn parse_cjt188_response_inner(
    response: &[u8],
    expected_addr: &[u8],
    metrics: &crate::engine::metrics::Metrics,
    remove_offset: bool,
) -> CollectorResult<Vec<u8>> {
    // CJT188：无第二个 0x68 头、地址 7 字节低字节在前、校验和失败计入指标
    parse_68_response(
        response,
        expected_addr,
        false,
        false,
        remove_offset,
        Some(metrics),
    )
}

/// 方言感知的响应解析（决策表 #6）：Vendor7 做 -0x33，V2004 标准方言无偏移。
pub(crate) fn parse_cjt188_response_v(
    response: &[u8],
    expected_addr: &[u8],
    metrics: &crate::engine::metrics::Metrics,
    version: Cjt188Version,
) -> CollectorResult<Vec<u8>> {
    parse_cjt188_response_inner(response, expected_addr, metrics, version.uses_offset())
}

/// 内建方言（Vendor7）响应解析封装——仅供既有单元测试直接断言；
/// 生产路径统一走方言感知的 [`parse_cjt188_response_v`]。
#[cfg(test)]
pub(crate) fn parse_cjt188_response(
    response: &[u8],
    expected_addr: &[u8],
    metrics: &crate::engine::metrics::Metrics,
) -> CollectorResult<Vec<u8>> {
    parse_cjt188_response_inner(response, expected_addr, metrics, true)
}

/// 解析 CJT188 数据标识（必须恰好 14 位 hex -> 7 字节）
pub(crate) fn parse_cjt188_data_id(id: &str) -> CollectorResult<Vec<u8>> {
    let digits: String = id.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if digits.len() != 14 {
        return Err(CollectorError::InvalidConfig(format!(
            "cjt188 data_id must be exactly 14 hex digits, got {}: {}",
            digits.len(),
            id
        )));
    }
    let padded_bytes = digits.as_bytes();
    let mut bytes = Vec::with_capacity(7);
    for i in 0..7 {
        // 已过滤为 ASCII 十六进制字符，直接字节索引 O(1)
        let hi = hex_digit(padded_bytes[i * 2]).ok_or_else(|| {
            CollectorError::InvalidFrame(format!("invalid hex digit: {:#x}", padded_bytes[i * 2]))
        })?;
        let lo = hex_digit(padded_bytes[i * 2 + 1]).ok_or_else(|| {
            CollectorError::InvalidFrame(format!(
                "invalid hex digit: {:#x}",
                padded_bytes[i * 2 + 1]
            ))
        })?;
        bytes.push((hi << 4) | lo);
    }
    Ok(bytes)
}

/// 解析 CJ/T 188-2004 标准方言数据标识（必须恰好 4 位 hex -> 2 字节）。
///
/// 线序与配置书写顺序一致（DI1 DI0），不做反转、不加偏移。
pub(crate) fn parse_cjt188_data_id_v2004(id: &str) -> CollectorResult<Vec<u8>> {
    let digits: String = id.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if digits.len() != 4 {
        return Err(CollectorError::InvalidConfig(format!(
            "cjt188-2004 data_id must be exactly 4 hex digits, got {}: {}",
            digits.len(),
            id
        )));
    }
    let b = digits.as_bytes();
    let d0 = hex_digit(b[0])
        .ok_or_else(|| CollectorError::InvalidFrame(format!("invalid hex digit: {:#x}", b[0])))?;
    let d1 = hex_digit(b[1])
        .ok_or_else(|| CollectorError::InvalidFrame(format!("invalid hex digit: {:#x}", b[1])))?;
    let d2 = hex_digit(b[2])
        .ok_or_else(|| CollectorError::InvalidFrame(format!("invalid hex digit: {:#x}", b[2])))?;
    let d3 = hex_digit(b[3])
        .ok_or_else(|| CollectorError::InvalidFrame(format!("invalid hex digit: {:#x}", b[3])))?;
    Ok(vec![(d0 << 4) | d1, (d2 << 4) | d3])
}

/// 解码 CJT188 日期时间（BCD 编码）
///
/// CJT188 时间格式：YY MM DD hh mm ss（6 字节 BCD），部分设备附加 1 字节 weekday。
/// 返回 UTC 毫秒时间戳。年份统一按 2000+YY 处理（CJ/T 188 标准自 2000 年后才有
/// 时间戳语义，YY 0..99 覆盖 2000..2099）。
pub(crate) fn decode_cjt_datetime(bytes: &[u8]) -> Option<i64> {
    if bytes.len() < 6 {
        return None;
    }
    fn bcd2(b: u8) -> Option<u32> {
        let hi = (b >> 4) & 0x0F;
        let lo = b & 0x0F;
        if hi > 9 || lo > 9 {
            return None;
        }
        Some((hi * 10 + lo) as u32)
    }
    let yy = bcd2(bytes[0])?;
    let mo = bcd2(bytes[1])?;
    let dd = bcd2(bytes[2])?;
    let hh = bcd2(bytes[3])?;
    let mi = bcd2(bytes[4])?;
    let ss = bcd2(bytes[5])?;
    // A6 修复：旧实现 `if yy < 80 { 2000+yy } else { 1900+yy }` 把 YY=80..99 错误解读为
    // 1980..1999；CJ/T 188 标准自 2000 年后才有时间戳语义，YY 应统一映射为 20xx
    // （如 YY=80 → 2080），否则长期运行的表时标全部错误。
    let year = 2000 + yy;
    let dt = chrono::NaiveDate::from_ymd_opt(year as i32, mo, dd)?.and_hms_opt(hh, mi, ss)?;
    let utc = chrono::NaiveDateTime::new(dt.date(), dt.time());
    Some(utc.and_utc().timestamp_millis())
}

/// 按 point.data_type 分发解码；返回 (raw_value, raw_type_str, quality)
///
/// `data_type = "datetime"` 为 CJT188 独有分支（BCD 日期时间），
/// 其余类型与 DLT645 共用 `common::decode::decode_value_by_type`。
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all, level = "trace"))]
pub(crate) fn decode_cjt_value(point: &PointConfig, bytes: &[u8]) -> (f64, String, Quality) {
    if point.data_type.as_deref() == Some("datetime") {
        match decode_cjt_datetime(bytes) {
            Some(ts_ms) => return (ts_ms as f64, "datetime".to_string(), Quality::Good),
            None => {
                tracing::warn!("point {} datetime decode failed for {:?}", point.id, bytes);
                return (f64::NAN, "datetime".to_string(), Quality::Bad);
            }
        }
    }
    decode_value_by_type(point, bytes)
}

/// 编码写值（决策表 #6）：按 `point.data_type` 分派（与读路径同源），
/// 未配置二进制类型时回退 BCD。
///
/// 实现见 `common::decode::encode_value_by_type`（与 DLT645 共用）。
pub(crate) fn encode_cjt_value(point: &PointConfig, value: f64, out_len: usize) -> Option<Vec<u8>> {
    encode_value_by_type(point, value, out_len)
}

/// CJT188 帧提取（无第二个 0x68 头）
///
/// 共用逻辑见 `common::frame_extract::extract_dlt645_cjt188_frame`，
/// 此处指定 check_second_header = false。
pub(crate) fn extract_cjt188_frame(
    fb: &mut FrameBuffer,
    _expected_count: Option<u16>,
) -> Option<Vec<u8>> {
    extract_dlt645_cjt188_frame(fb, false)
}
