//! DLT645 帧处理（构建 / 解析 / 解码）
//!
//! 帧格式：68 ADDR(6) 68 CTRL LEN DATA CS 16
//! - 地址：6 字节 BCD
//!   - 2007 版 / 1997-LE 变体：传输时低字节在前
//!   - 1997-BE 变体：传输时高字节在前
//! - 数据区：cipher 变体每字节 +0x33（接收后 -0x33），plain 变体明文
//! - 校验和：从第一个 68 到 CS 前所有字节之和 mod 256
//!
//! 版本差异由 [`Dlt645Version`] 的 `addr_be_on_wire` / `data_cipher`
//! 两个维度统一分派，采集器通过 `Dlt645Collector::with_version` 传入。

use crate::config::PointConfig;
use crate::engine::model::Quality;
use crate::south::bus::FrameBuffer;
use crate::south::protocol::common::codec::{encode_bcd_value, hex_digit};
use crate::south::protocol::common::decode::{decode_value_by_type, encode_value_by_type};
use crate::south::protocol::common::frame_extract::{
    build_68_frame, extract_dlt645_cjt188_frame, parse_68_response,
};
use crate::south::protocol::{CollectorError, CollectorResult};

/// DLT645 协议变体（配置项 `bus.dlt645_version` / `bus.variant`）
///
/// 国标 2007 与 1997 在「地址字节序 × 数据是否 +0x33」两维上形成 5 个变体；
/// 帧构建/解析统一由 [`Dlt645Version::addr_be_on_wire`] 与
/// [`Dlt645Version::data_cipher`] 两个方法分派，现场按厂家表配置即可，
/// 无需改代码。
///
/// 配置字符串约定（`bus.variant` 优先于遗留 `dlt645_version`）：
/// - `2007` / `2007-addr-le-cipher`（默认）→ [`Dlt645Version::V2007`]
/// - `1997` → [`Dlt645Version::V1997BePlain`]（国标：地址高字节在前、数据不 +0x33）
/// - `1997-addr-le-plain` → [`Dlt645Version::V1997LePlain`]
/// - `1997-addr-be-cipher` → [`Dlt645Version::V1997BeCipher`]
/// - `1997-addr-le-cipher` → [`Dlt645Version::V1997LeCipher`]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dlt645Version {
    /// 2007 国标：地址低字节在前、数据 +0x33
    V2007,
    /// 1997 国标：地址高字节在前、数据不 +0x33
    V1997BePlain,
    /// 1997 变体：地址低字节在前、数据不 +0x33
    V1997LePlain,
    /// 1997 变体：地址高字节在前、数据 +0x33
    V1997BeCipher,
    /// 1997 变体：地址低字节在前、数据 +0x33
    V1997LeCipher,
}

impl Dlt645Version {
    /// 从配置字符串解析；不识别值返回 V2007（默认）。
    pub fn from_config(s: &str) -> Self {
        match s.trim() {
            "1997" | "1997-addr-be-plain" => Self::V1997BePlain,
            "1997-addr-le-plain" => Self::V1997LePlain,
            "1997-addr-be-cipher" => Self::V1997BeCipher,
            "1997-addr-le-cipher" => Self::V1997LeCipher,
            // 2007（含遗留默认）与不识别值一律走 2007
            _ => Self::V2007,
        }
    }

    /// 1997 任一子变种（用于遗留调用点判断是否 1997 家族）
    pub const fn is_v1997_family(self) -> bool {
        !matches!(self, Dlt645Version::V2007)
    }

    /// 地址在报文中的字节序：`true` = 高字节在前（BE），`false` = 低字节在前（LE）。
    ///
    /// `parse_bcd_address` / `encode_bcd_value` 内部统一输出低字节在前，
    /// 构建帧时据此决定是否反转；解析响应时同一依据决定比较前是否反转。
    pub(crate) const fn addr_be_on_wire(self) -> bool {
        match self {
            Self::V2007 | Self::V1997LePlain | Self::V1997LeCipher => false,
            Self::V1997BePlain | Self::V1997BeCipher => true,
        }
    }

    /// 数据区是否逐字节 +0x33（`true` = 需要加密，解析时相应 -0x33）。
    pub(crate) const fn data_cipher(self) -> bool {
        match self {
            Self::V2007 | Self::V1997BeCipher | Self::V1997LeCipher => true,
            Self::V1997BePlain | Self::V1997LePlain => false,
        }
    }
}

/// 按协议变体构建帧。
///
/// 地址字节序由 [`Dlt645Version::addr_be_on_wire`] 决定；数据区的 +0x33
/// 由调用方按 [`Dlt645Version::data_cipher`] 预处理，本函数不重复加密
/// （写通信地址命令的新地址字段必须保持明文，不能在此统一加密）。
/// LEN 字段、CS 校验、起止 0x68/0x16 各变体一致。
pub(crate) fn build_dlt645_frame(
    addr: &[u8],
    ctrl: u8,
    data: &[u8],
    version: Dlt645Version,
) -> Vec<u8> {
    build_68_frame(addr, true, version.addr_be_on_wire(), ctrl, data, "DLT645")
}

/// 解析 DLT645 响应，返回数据区（按变体决定是否已 -0x33）
///
/// 地址比对方向与数据区偏移处理均由变体决定：
/// - [`Dlt645Version::addr_be_on_wire`]：wire 地址为高字节在前时，先反转
///   `response[1..7]` 再与 `expected_addr`（低字节在前）比对；
/// - [`Dlt645Version::data_cipher`]：数据区 +0x33 的变体在解析时 -0x33。
pub(crate) fn parse_dlt645_response(
    response: &[u8],
    expected_addr: &[u8],
    version: Dlt645Version,
) -> CollectorResult<Vec<u8>> {
    parse_68_response(
        response,
        expected_addr,
        true,
        version.addr_be_on_wire(),
        version.data_cipher(),
        None,
    )
}

/// 解析数据标识（8 位 hex）为 4 字节
pub(crate) fn parse_data_id(id: &str) -> CollectorResult<Vec<u8>> {
    let digits: String = id.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if digits.len() != 8 {
        return Err(CollectorError::InvalidConfig(format!(
            "data_id must be 8 hex digits: {}",
            id
        )));
    }
    let digit_bytes = digits.as_bytes();
    let mut bytes = Vec::with_capacity(4);
    for i in 0..4 {
        let hi = hex_digit(digit_bytes[i * 2]).ok_or_else(|| {
            CollectorError::InvalidFrame(format!("invalid hex digit: {:#x}", digit_bytes[i * 2]))
        })?;
        let lo = hex_digit(digit_bytes[i * 2 + 1]).ok_or_else(|| {
            CollectorError::InvalidFrame(format!(
                "invalid hex digit: {:#x}",
                digit_bytes[i * 2 + 1]
            ))
        })?;
        bytes.push((hi << 4) | lo);
    }
    Ok(bytes)
}

/// 按 point.data_type 分发解码；返回 (raw_value, raw_type_str, quality)
///
/// - data_type 配置为 u16/i16/u32/i32/u64/i64/f32/f64 时：按字节序重排后二进制解码
/// - 否则（默认 / "bcd" / 未识别）：回退到 BCD 路径，向后兼容
///
/// 实现见 `common::decode::decode_value_by_type`（与 CJT188 共用）。
pub(crate) fn decode_dlt_value(point: &PointConfig, bytes: &[u8]) -> (f64, String, Quality) {
    decode_value_by_type(point, bytes)
}

/// 编码 f64 为指定类型的大端字节序列（写数据用）
///
/// 类型由 `point.data_type` 决定（与读路径同源配置）：
/// - u16/i16/u32/i32/u64/i64/f32/f64：大端编码后按字节序重排
/// - BCD：回退到 `encode_bcd_value`
///
/// 返回字节长度 = `register_count * 2`。
/// 实现见 `common::decode::encode_value_by_type`（与 CJT188 共用）。
pub(crate) fn encode_dlt_value(point: &PointConfig, value: f64, out_len: usize) -> Option<Vec<u8>> {
    encode_value_by_type(point, value, out_len)
}

/// 写通信地址（控制码 0x0C）数据区
///
/// 数据区构成（DLT645-2007）：
/// - 6 字节新地址（**明文，不 +0x33**）
/// - 4 字节旧密码（`data_cipher=true` 时 +0x33）
/// - 4 字节新密码（`data_cipher=true` 时 +0x33）
///
/// `data_cipher` 由 [`Dlt645Version::data_cipher`] 传入：1997 国标（plain）
/// 变体按标准不做 +0x33，2007 与 1997 厂家 cipher 变体做 +0x33。
/// 旧/新密码默认 0x00000000（无密码场景）。
/// 成功返回 Some(Vec<u8>)，参数非法返回 None。
pub(crate) fn build_dlt645_write_address_data(
    new_addr: &[u8; 6],
    old_password: u32,
    new_password: u32,
    data_cipher: bool,
) -> Option<Vec<u8>> {
    let old_pwd_bytes = encode_bcd_value(old_password as f64, 4)?;
    let new_pwd_bytes = encode_bcd_value(new_password as f64, 4)?;
    let mut data = Vec::with_capacity(14);
    data.extend_from_slice(new_addr);
    for b in old_pwd_bytes.iter() {
        data.push(with_cipher(*b, data_cipher));
    }
    for b in new_pwd_bytes.iter() {
        data.push(with_cipher(*b, data_cipher));
    }
    Some(data)
}

/// 写密码（控制码 0x0F）数据区
///
/// 数据区构成（`data_cipher` 同 [`build_dlt645_write_address_data`]）：
/// - 4 字节旧密码（cipher 变体 +0x33）
/// - 4 字节新密码（cipher 变体 +0x33）
pub(crate) fn build_dlt645_write_password_data(
    old_password: u32,
    new_password: u32,
    data_cipher: bool,
) -> Option<Vec<u8>> {
    let old_pwd_bytes = encode_bcd_value(old_password as f64, 4)?;
    let new_pwd_bytes = encode_bcd_value(new_password as f64, 4)?;
    let mut data = Vec::with_capacity(8);
    for b in old_pwd_bytes.iter() {
        data.push(with_cipher(*b, data_cipher));
    }
    for b in new_pwd_bytes.iter() {
        data.push(with_cipher(*b, data_cipher));
    }
    Some(data)
}

/// 按变体对数据字节做 +0x33（`cipher=false` 时原样透传）
fn with_cipher(b: u8, cipher: bool) -> u8 {
    if cipher {
        b.wrapping_add(0x33)
    } else {
        b
    }
}

/// DLT645 帧提取（校验第二个 0x68 头）
///
/// 共用逻辑见 `common::frame_extract::extract_dlt645_cjt188_frame`，
/// 此处指定 check_second_header = true。
pub(crate) fn extract_dlt645_frame(
    fb: &mut FrameBuffer,
    _expected_count: Option<u16>,
) -> Option<Vec<u8>> {
    extract_dlt645_cjt188_frame(fb, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 契约：4 个 1997 子变种字符串均能解析到正确枚举（地址字节序 × 数据 +0x33 笛卡尔积）。
    ///
    /// 帧构建/解析已按枚举全量分派（见 `addr_be_on_wire` / `data_cipher`），
    /// 现场只需按厂家表配置字符串即可。
    #[test]
    fn dlt645_version_from_config_parses_all_4_v1997_subvariants() {
        assert_eq!(
            Dlt645Version::from_config("1997"),
            Dlt645Version::V1997BePlain
        );
        assert_eq!(
            Dlt645Version::from_config("1997-addr-be-plain"),
            Dlt645Version::V1997BePlain
        );
        assert_eq!(
            Dlt645Version::from_config("1997-addr-le-plain"),
            Dlt645Version::V1997LePlain
        );
        assert_eq!(
            Dlt645Version::from_config("1997-addr-be-cipher"),
            Dlt645Version::V1997BeCipher
        );
        assert_eq!(
            Dlt645Version::from_config("1997-addr-le-cipher"),
            Dlt645Version::V1997LeCipher
        );
        // 兼容：2007 / 默认 / 空白 → V2007
        assert_eq!(Dlt645Version::from_config("2007"), Dlt645Version::V2007);
        assert_eq!(Dlt645Version::from_config(""), Dlt645Version::V2007);
        assert_eq!(Dlt645Version::from_config("unknown"), Dlt645Version::V2007);
    }

    /// 契约：4 个 1997 子变种都属于 V1997 family（用于遗留调用点判断）
    #[test]
    fn dlt645_version_is_v1997_family_for_all_1997_subvariants() {
        for v in [
            Dlt645Version::V1997BePlain,
            Dlt645Version::V1997LePlain,
            Dlt645Version::V1997BeCipher,
            Dlt645Version::V1997LeCipher,
        ] {
            assert!(v.is_v1997_family(), "{v:?} should be V1997 family");
        }
        assert!(!Dlt645Version::V2007.is_v1997_family());
    }
}
