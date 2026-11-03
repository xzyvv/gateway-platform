//! BACnet 基础解码：tag 读取、invoke id 提取、应用值解码、ReadProperty-ACK 取值。
use super::*;

/// 大端无符号整数解析（1-4 字节变长）
pub(super) fn read_uint_be(bytes: &[u8]) -> u32 {
    let mut v: u32 = 0;
    for &b in bytes {
        v = (v << 8) | b as u32;
    }
    v
}

/// 大端有符号整数解析（1-4 字节，最高位为符号位）
fn read_int_be(bytes: &[u8]) -> i64 {
    let mut v: i64 = 0;
    for &b in bytes {
        v = (v << 8) | b as i64;
    }
    let bits = bytes.len() as u32 * 8;
    if bytes.len() < 8 && bytes.first().is_some_and(|b| b & 0x80 != 0) {
        v -= 1i64 << bits;
    }
    v
}

/// 读取一个 BACnet tag 头字节。
///
/// 返回 `(是否上下文标签, tag 编号, 长度/标记字段 lvt, 数据或扩展长度字段的起始偏移)`。
/// lvt 取值：0-4 = 直接长度，5 = 扩展长度，6 = opening tag，7 = closing tag。
pub(super) fn read_tag(bytes: &[u8], pos: usize) -> CollectorResult<(bool, u8, u32, usize)> {
    let b = *bytes
        .get(pos)
        .ok_or_else(|| CollectorError::InvalidFrame("truncated BACnet tag".into()))?;
    Ok(((b & 0x08) != 0, b >> 4, (b & 0x07) as u32, pos + 1))
}

/// 解析 tag 的数据长度，返回 `(数据长度, 数据起始偏移)`。
pub(super) fn read_tag_len(bytes: &[u8], pos: usize, lvt: u32) -> CollectorResult<(usize, usize)> {
    match lvt {
        0..=4 => Ok((lvt as usize, pos)),
        5 => {
            let ext = *bytes.get(pos).ok_or_else(|| {
                CollectorError::InvalidFrame("truncated BACnet extended tag length".into())
            })?;
            if ext < 0xFE {
                Ok((ext as usize, pos + 1))
            } else if ext == 0xFE {
                if pos + 2 >= bytes.len() {
                    return Err(CollectorError::InvalidFrame(
                        "truncated BACnet extended tag length (2-byte)".into(),
                    ));
                }
                Ok((
                    u16::from_be_bytes([bytes[pos + 1], bytes[pos + 2]]) as usize,
                    pos + 3,
                ))
            } else {
                Err(CollectorError::InvalidFrame(
                    "reserved BACnet tag length marker 0xFF".into(),
                ))
            }
        }
        6 | 7 => Err(CollectorError::InvalidFrame(format!(
            "BACnet tag with opening/closing marker {} has no length",
            lvt
        ))),
        _ => unreachable!("lvt is 3 bits"),
    }
}

/// 从 APDU 中提取 invoke id（Confirmed-Request / ComplexACK / SimpleACK / Error 等
/// 均把 invoke id 放在第 2 字节）。
///
/// 只做位置提取，不校验 PDU 类型；调用方先按 invoke id 归位，再按类型解析。
pub(crate) fn apdu_invoke_id(apdu: &[u8]) -> Option<u8> {
    apdu.get(1).copied()
}

/// 解码应用标签值（从偏移 0 开始），返回 `(值, 消耗到的偏移)`。
///
/// 生产路径不直接调用，测试与 MS/TP 复用同一套取值逻辑。
#[cfg(test)]
pub(crate) fn parse_app_value(bytes: &[u8]) -> CollectorResult<(f64, usize)> {
    let (value, _quality, next) = parse_app_value_from(bytes, 0)?;
    Ok((value, next))
}

/// 从指定偏移解码一个应用标签值，返回 `(值, 质量, 消耗到的偏移)`。
///
/// 支持 BOOLEAN / UNSIGNED / SIGNED / REAL / DOUBLE / ENUMERATED；其余应用标签
/// （字符串、时间等）不属于 Present_Value 取值范围，一律报错而不是静默当 0。
pub(crate) fn parse_app_value_from(
    bytes: &[u8],
    pos: usize,
) -> CollectorResult<(f64, Quality, usize)> {
    let (ctx, tag, lvt, p) = read_tag(bytes, pos)?;
    if ctx {
        return Err(CollectorError::InvalidFrame(format!(
            "expected application tag at offset {}, got context tag {}",
            pos, tag
        )));
    }
    let (len, ds) = read_tag_len(bytes, p, lvt)?;
    if ds + len > bytes.len() {
        return Err(CollectorError::InvalidFrame(
            "BACnet application value truncated".into(),
        ));
    }
    let raw = &bytes[ds..ds + len];
    let value = match tag {
        TAG_NULL => 0.0,
        TAG_BOOLEAN => {
            if raw.first().copied().unwrap_or(0) != 0 {
                1.0
            } else {
                0.0
            }
        }
        TAG_UNSIGNED | TAG_ENUMERATED => read_uint_be(raw) as f64,
        TAG_SIGNED => read_int_be(raw) as f64,
        TAG_REAL => {
            if len != 4 {
                return Err(CollectorError::InvalidFrame(format!(
                    "BACnet REAL must be 4 bytes, got {}",
                    len
                )));
            }
            f32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]) as f64
        }
        TAG_DOUBLE => {
            if len != 8 {
                return Err(CollectorError::InvalidFrame(format!(
                    "BACnet DOUBLE must be 8 bytes, got {}",
                    len
                )));
            }
            f64::from_be_bytes([
                raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
            ])
        }
        other => {
            return Err(CollectorError::InvalidFrame(format!(
                "unsupported BACnet application tag {}",
                other
            )))
        }
    };
    Ok((value, Quality::Good, ds + len))
}

/// 解析 ReadProperty-ACK（ComplexACK）取出 Present_Value。
///
/// 布局：ComplexACK(0x30) + invoke id + service(12) + objectIdentifier[0] +
/// propertyIdentifier[1] + propertyValue[3](opening tag 3 + 应用值 + closing tag 3)。
/// invoke id / 服务号不符一律报错，防止迟到或残留回包被当作本次结果。
pub(crate) fn parse_value_from_apdu(
    apdu: &[u8],
    expected_invoke_id: u8,
) -> CollectorResult<(f64, Quality)> {
    if apdu.len() < 3 {
        return Err(CollectorError::InvalidFrame("BACnet APDU too short".into()));
    }
    if apdu[0] != APDU_COMPLEX_ACK {
        return Err(CollectorError::InvalidFrame(format!(
            "expected ComplexACK 0x30, got 0x{:02X}",
            apdu[0]
        )));
    }
    if apdu[1] != expected_invoke_id {
        return Err(CollectorError::InvalidFrame(format!(
            "BACnet invoke id mismatch: expected {}, got {}",
            expected_invoke_id, apdu[1]
        )));
    }
    if apdu[2] != SERVICE_READ_PROPERTY {
        return Err(CollectorError::InvalidFrame(format!(
            "BACnet unexpected service choice {} (expected ReadProperty {})",
            apdu[2], SERVICE_READ_PROPERTY
        )));
    }

    // objectIdentifier [0] + propertyIdentifier [1]：跳过（取值不依赖它们）
    let mut pos = 3usize;
    for expected_tag in [0u8, 1u8] {
        let (ctx, tag, lvt, p) = read_tag(apdu, pos)?;
        if !ctx || tag != expected_tag {
            return Err(CollectorError::InvalidFrame(format!(
                "ReadProperty ACK: expected context tag {}, got {}",
                expected_tag, tag
            )));
        }
        let (len, ds) = read_tag_len(apdu, p, lvt)?;
        if ds + len > apdu.len() {
            return Err(CollectorError::InvalidFrame(
                "ReadProperty ACK: property header truncated".into(),
            ));
        }
        pos = ds + len;
    }

    // propertyValue [3]：opening tag 3 + 应用值 + closing tag 3
    let (ctx, tag, lvt, p) = read_tag(apdu, pos)?;
    if !ctx || tag != 3 || lvt != 6 {
        return Err(CollectorError::InvalidFrame(
            "ReadProperty ACK: expected opening tag 3 for propertyValue".into(),
        ));
    }
    let (value, quality, _next) = parse_app_value_from(apdu, p)?;
    Ok((value, quality))
}
