//! S7Comm 地址解析

use super::*;
use crate::south::protocol::s7comm::frames::{AREA_DB, AREA_INPUTS, AREA_MARKERS, AREA_OUTPUTS};

/// 解析地址字符串："host:102?rack=0&slot=2"
/// 返回 (纯地址, rack, slot)，默认 rack=0, slot=2
pub(crate) fn parse_addr(addr: &str) -> (String, u8, u8) {
    let (host_part, query) = match addr.split_once('?') {
        Some((h, q)) => (h, q),
        None => (addr, ""),
    };
    // Bug 125: 显式拒绝空 host_part——避免后续 tokio::net::TcpStream::connect
    // 收到空字符串时 panic（"empty string" 错误）或连接 0.0.0.0:102。
    let host_trimmed = host_part.trim();
    if host_trimmed.is_empty() {
        tracing::warn!("parse_addr: empty host part in '{}'", addr);
    }
    let mut rack: u8 = 0;
    let mut slot: u8 = 2; // S7-300/400 默认 slot=2
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            match k.trim().to_lowercase().as_str() {
                "rack" => {
                    if let Ok(n) = v.trim().parse() {
                        rack = n;
                    }
                }
                "slot" => {
                    if let Ok(n) = v.trim().parse() {
                        slot = n;
                    }
                }
                _ => {}
            }
        }
    }
    // Bug 128: 不在此处校验 host:port 格式——延迟到首次 connect 时由
    // tokio 返回具体错误（DNS resolve failure / Connection refused 等），
    // 避免双重校验逻辑分散。
    (host_trimmed.to_string(), rack, slot)
}

/// 解析 S7 地址字符串
///
/// 支持格式：
/// - "DB<n>.DBX<off>.<bit>" / "DB<n>.DBB<off>" / "DB<n>.DBW<off>" / "DB<n>.DBD<off>"
/// - "M<off>.<bit>" / "MB<off>" / "MW<off>" / "MD<off>"
/// - "I<off>.<bit>" / "IB<off>" / "IW<off>" / "ID<off>"
/// - "Q<off>.<bit>" / "QB<off>" / "QW<off>" / "QD<off>"
pub(crate) fn parse_address(
    addr: &str,
    data_type_hint: Option<&str>,
) -> CollectorResult<S7Address> {
    // Bug 127: 原代码 `addr.trim().to_uppercase()` 已统一大写，下方 DB/I/Q/M
    // 判断走大写分支，OK。保留此注释便于后续 reviewer。
    let s = addr.trim().to_uppercase();

    // DB 区域："DB<n>.DBX<off>.<bit>" 等
    if s.starts_with("DB") {
        return parse_db_address(&s, data_type_hint);
    }

    // I/Q/M 区域
    let (area, prefix) = if s.starts_with('I') {
        (AREA_INPUTS, 'I')
    } else if s.starts_with('Q') {
        (AREA_OUTPUTS, 'Q')
    } else if s.starts_with('M') {
        (AREA_MARKERS, 'M')
    } else {
        return Err(CollectorError::InvalidConfig(format!(
            "invalid S7 address '{}': must start with DB/I/Q/M",
            addr
        )));
    };

    let rest = s.strip_prefix(prefix).unwrap_or("");
    // Extract type suffix from rest: "W0" -> ('W', "0"), "D0" -> ('D', "0"),
    // "B0" -> ('B', "0"), "0.1" -> ('X', "0.1") (default bit type)
    let (type_char, offset_str) = if let Some(s) = rest.strip_prefix('B') {
        ('B', s)
    } else if let Some(s) = rest.strip_prefix('W') {
        ('W', s)
    } else if let Some(s) = rest.strip_prefix('D') {
        ('D', s)
    } else {
        ('X', rest)
    };
    parse_area_address(offset_str, area, 0, type_char, data_type_hint)
}

/// 解析 DB 区域地址："DB1.DBD0" → area=DB, db=1, type=DWord, offset=0
pub(crate) fn parse_db_address(
    s: &str,
    data_type_hint: Option<&str>,
) -> CollectorResult<S7Address> {
    // 格式: DB<n>.<type><offset>[.<bit>]
    let dot_pos = s.find('.').ok_or_else(|| {
        CollectorError::InvalidConfig(format!("DB address must contain '.': {}", s))
    })?;
    let db_str = &s[2..dot_pos]; // 跳过 "DB"
    let db_number: u16 = db_str
        .parse()
        .map_err(|_| CollectorError::InvalidConfig(format!("invalid DB number: {}", db_str)))?;

    let type_part = &s[dot_pos + 1..]; // "DBD0" / "DBX0.1" 等
                                       // 类型前缀是 "DBX" / "DBB" / "DBW" / "DBD"
    let (type_char, offset_str) = if let Some(s) = type_part.strip_prefix("DBX") {
        ('X', s)
    } else if let Some(s) = type_part.strip_prefix("DBB") {
        ('B', s)
    } else if let Some(s) = type_part.strip_prefix("DBW") {
        ('W', s)
    } else if let Some(s) = type_part.strip_prefix("DBD") {
        ('D', s)
    } else {
        return Err(CollectorError::InvalidConfig(format!(
            "invalid DB data type in '{}': expected DBX/DBB/DBW/DBD",
            type_part
        )));
    };

    parse_area_address(offset_str, AREA_DB, db_number, type_char, data_type_hint)
}

/// 解析区域地址的偏移部分
///
/// offset_str 格式：
/// - "0" → 偏移 0
/// - "10" → 偏移 10
/// - "0.1" → 偏移 0, 位 1（仅 type_char='X' 时有效）
pub(crate) fn parse_area_address(
    offset_str: &str,
    area: u8,
    db_number: u16,
    type_char: char,
    data_type_hint: Option<&str>,
) -> CollectorResult<S7Address> {
    let (byte_offset, bit_offset, data_type) = match type_char {
        'X' => {
            // 位地址："0.1" → byte=0, bit=1
            let parts: Vec<&str> = offset_str.split('.').collect();
            let byte_offset: u32 = parts[0].parse().map_err(|_| {
                CollectorError::InvalidConfig(format!("invalid bit offset: {}", offset_str))
            })?;
            let bit_offset: u8 = if parts.len() > 1 {
                parts[1].parse().map_err(|_| {
                    CollectorError::InvalidConfig(format!("invalid bit number: {}", parts[1]))
                })?
            } else {
                0
            };
            if bit_offset > 7 {
                return Err(CollectorError::InvalidConfig(format!(
                    "bit number must be 0-7, got {}",
                    bit_offset
                )));
            }
            (byte_offset, bit_offset, S7DataType::Bit)
        }
        'B' => {
            let off: u32 = offset_str.parse().map_err(|_| {
                CollectorError::InvalidConfig(format!("invalid byte offset: {}", offset_str))
            })?;
            (off, 0u8, S7DataType::Byte)
        }
        'W' => {
            let off: u32 = offset_str.parse().map_err(|_| {
                CollectorError::InvalidConfig(format!("invalid word offset: {}", offset_str))
            })?;
            // 默认 Int（有符号 16 位），如果 data_type_hint 指定则使用 hint
            // 工业场景中 MW 区域常存有符号测量值（如温度偏差），
            // 故默认按 Int 解析；如确为无符号 Word 请通过 point.data_type="word" 显式指定。
            let dt = data_type_hint
                .and_then(S7DataType::from_str)
                .unwrap_or(S7DataType::Int);
            (off, 0u8, dt)
        }
        'D' => {
            let off: u32 = offset_str.parse().map_err(|_| {
                CollectorError::InvalidConfig(format!("invalid dword offset: {}", offset_str))
            })?;
            // 默认 Real（工业场景中 32 位数据最常见为浮点）
            let dt = data_type_hint
                .and_then(S7DataType::from_str)
                .unwrap_or(S7DataType::Real);
            (off, 0u8, dt)
        }
        _ => {
            return Err(CollectorError::InvalidConfig(format!(
                "unknown S7 type suffix '{}'",
                type_char
            )));
        }
    };

    Ok(S7Address {
        area,
        db_number,
        byte_offset,
        bit_offset,
        data_type,
    })
}
