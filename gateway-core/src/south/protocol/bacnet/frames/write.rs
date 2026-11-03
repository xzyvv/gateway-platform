//! BACnet 读写服务报文：ReadProperty / WriteProperty / ReadPropertyMultiple 的
//! APDU 构建与应答解析。
use super::decode::{parse_app_value_from, read_tag, read_tag_len};
use super::*;

/// 用 BVLC + NPDU 包裹 APDU，生成可直接发送的 BACnet/IP 数据报。
///
/// BVLC：type(0x81) + function(0x0a = Original-Unicast-NPDU) + 总长度；
/// NPDU：version(1) + control(0)，无 DNET/SNET（单播直连场景）。
/// 长度字段 = NPDU + APDU + 4（BVLC 头）。
pub(crate) fn wrap_bvlc_npdu(apdu: &[u8]) -> Vec<u8> {
    let npdu = BacnetCollector::build_npdu_header();
    let mut packet = vec![0x81, 0x0a];
    let total = npdu.len() + apdu.len() + 4;
    packet.extend_from_slice(&(total as u16).to_be_bytes());
    packet.extend_from_slice(&npdu);
    packet.extend_from_slice(apdu);
    packet
}

/// 构建 ReadProperty 请求数据报（BVLC + NPDU + APDU）。
pub(crate) fn build_read_property(obj_type: u16, instance: u32, invoke_id: u8) -> Vec<u8> {
    wrap_bvlc_npdu(&BacnetCollector::build_read_property_apdu(
        obj_type, instance, invoke_id,
    ))
}

/// 按目标对象类型编码写入值。
///
/// 二进制对象（BI/BO/BV）的 Present_Value 是 `BACnetBinaryPV`（Enumerated 0/1），
/// 不能用 REAL 写入；其余对象按 REAL 编码。
fn encode_write_value(obj_type: u16, value: f64) -> Vec<u8> {
    let binary = matches!(
        obj_type,
        OBJECT_BINARY_INPUT | OBJECT_BINARY_OUTPUT | OBJECT_BINARY_VALUE
    );
    if binary {
        let v = u8::from(value != 0.0);
        // Enumerated(9) 应用标签，长度 1
        vec![(TAG_ENUMERATED << 4) | 1, v]
    } else {
        let mut out = vec![(TAG_REAL << 4) | 4];
        out.extend_from_slice(&(value as f32).to_be_bytes());
        out
    }
}

/// 构建 WriteProperty 请求 APDU（写 Present_Value，优先级 8）。
///
/// 布局：Confirmed-Request(0x00) + MaxSeg/MaxAPDU(0x02) + invoke + service(15) +
/// objectIdentifier[0] + propertyIdentifier[1] + propertyValue[3](opening 3 +
/// 应用值 + closing 3) + priority[4]。
pub(crate) fn build_write_property_apdu(
    obj_type: u16,
    instance: u32,
    invoke_id: u8,
    value: f64,
) -> Vec<u8> {
    let mut apdu = vec![
        APDU_CONFIRMED_REQUEST,
        0x02,
        invoke_id,
        SERVICE_WRITE_PROPERTY,
    ];
    let oid = ((obj_type as u32) << 22) | (instance & 0x3FFFFF);
    apdu.extend(BacnetCollector::encode_context_tag(0, &oid.to_be_bytes()));
    apdu.extend(BacnetCollector::encode_context_tag(
        1,
        &BacnetCollector::encode_unsigned(PROP_PRESENT_VALUE),
    ));
    apdu.push(0x3E); // context tag 3 opening
    apdu.extend(encode_write_value(obj_type, value));
    apdu.push(0x3F); // context tag 3 closing
    apdu.extend(BacnetCollector::encode_context_tag(
        4,
        &[BACNET_WRITE_PRIORITY],
    ));
    apdu
}

/// 构建 ReadPropertyMultiple 请求 APDU（一次事务读多个对象的 Present_Value）。
///
/// 每个对象生成一条 ReadAccessSpecification：objectIdentifier[0] +
/// listOfPropertyReferences[1](opening 1 + propertyIdentifier[0] + closing 1)。
pub(crate) fn build_rpm_request_apdu(objects: &[(u16, u32)], invoke_id: u8) -> Vec<u8> {
    let mut apdu = vec![
        APDU_CONFIRMED_REQUEST,
        0x02,
        invoke_id,
        SERVICE_READ_PROPERTY_MULTIPLE,
    ];
    for &(obj_type, instance) in objects {
        let oid = ((obj_type as u32) << 22) | (instance & 0x3FFFFF);
        apdu.extend(BacnetCollector::encode_context_tag(0, &oid.to_be_bytes()));
        apdu.push(0x1E); // listOfPropertyReferences: opening tag 1
        apdu.extend(BacnetCollector::encode_context_tag(
            0,
            &BacnetCollector::encode_unsigned(PROP_PRESENT_VALUE),
        ));
        apdu.push(0x1F); // closing tag 1
    }
    apdu
}

/// 解析 WriteProperty 应答：仅 SimpleACK/ComplexACK + 匹配 invoke id + service 15 判成功。
///
/// Error / Reject / Abort 一律判写入失败——写操作绝不能因收到「某种应答」就成功。
pub(crate) fn parse_write_property_ack(apdu: &[u8], expected_invoke_id: u8) -> CollectorResult<()> {
    let first = *apdu
        .first()
        .ok_or_else(|| CollectorError::InvalidFrame("empty BACnet APDU".into()))?;
    match first {
        APDU_SIMPLE_ACK | APDU_COMPLEX_ACK => {
            if apdu.len() < 3 {
                return Err(CollectorError::InvalidFrame(
                    "BACnet WriteProperty ACK too short".into(),
                ));
            }
            if apdu[1] != expected_invoke_id {
                return Err(CollectorError::InvalidFrame(format!(
                    "BACnet invoke id mismatch: expected {}, got {}",
                    expected_invoke_id, apdu[1]
                )));
            }
            if apdu[2] != SERVICE_WRITE_PROPERTY {
                return Err(CollectorError::InvalidFrame(format!(
                    "BACnet unexpected service choice {} (expected WriteProperty {})",
                    apdu[2], SERVICE_WRITE_PROPERTY
                )));
            }
            Ok(())
        }
        APDU_ERROR | APDU_REJECT | APDU_ABORT => {
            let kind = match first {
                APDU_ERROR => "Error",
                APDU_REJECT => "Reject",
                _ => "Abort",
            };
            Err(CollectorError::InvalidFrame(format!(
                "BACnet WriteProperty rejected: APDU type 0x{:02X} ({})",
                first, kind
            )))
        }
        other => Err(CollectorError::InvalidFrame(format!(
            "BACnet WriteProperty unexpected response type 0x{:02X}",
            other
        ))),
    }
}

/// 跳过一段构造标签（opening ... closing）的内容，返回 closing 之后的偏移。
fn skip_constructed(apdu: &[u8], mut pos: usize, tag_no: u8) -> CollectorResult<usize> {
    while pos < apdu.len() {
        let (ctx, tag, lvt, p) = read_tag(apdu, pos)?;
        if ctx && tag == tag_no && lvt == 7 {
            return Ok(p);
        }
        if lvt == 6 || lvt == 7 {
            pos = p;
        } else {
            let (len, ds) = read_tag_len(apdu, p, lvt)?;
            if ds + len > apdu.len() {
                return Err(CollectorError::InvalidFrame(
                    "BACnet constructed tag truncated".into(),
                ));
            }
            pos = ds + len;
        }
    }
    Err(CollectorError::InvalidFrame(
        "unterminated BACnet constructed tag".into(),
    ))
}

/// 解析 ReadPropertyMultiple-ACK，返回每个对象的 `(对象 ID, Present_Value, 质量)`。
///
/// 对象 ID 编码为 `(object_type << 22) | instance`，与调用方入参映射一致。
/// `propertyAccessError`（context tag 5）表示该属性读取失败，整段跳过——该对象
/// 不进入结果集，由调用方按对象 ID 缺失回填「无应答」错误。
pub(crate) fn parse_rpm_ack(
    apdu: &[u8],
    expected_invoke_id: u8,
) -> CollectorResult<Vec<(u32, f64, Quality)>> {
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
    if apdu[2] != SERVICE_READ_PROPERTY_MULTIPLE {
        return Err(CollectorError::InvalidFrame(format!(
            "BACnet unexpected service choice {} (expected ReadPropertyMultiple {})",
            apdu[2], SERVICE_READ_PROPERTY_MULTIPLE
        )));
    }

    let mut out: Vec<(u32, f64, Quality)> = Vec::new();
    let mut current_oid: Option<u32> = None;
    let mut pos = 3usize;
    while pos < apdu.len() {
        let (ctx, tag, lvt, p) = read_tag(apdu, pos)?;
        if ctx {
            match tag {
                0 if lvt <= 4 => {
                    // 结果项起始：objectIdentifier[0]（长度 4）
                    if lvt != 4 || p + 4 > apdu.len() {
                        return Err(CollectorError::InvalidFrame(
                            "RPM ACK: objectIdentifier must be 4 bytes".into(),
                        ));
                    }
                    current_oid = Some(u32::from_be_bytes([
                        apdu[p],
                        apdu[p + 1],
                        apdu[p + 2],
                        apdu[p + 3],
                    ]));
                    pos = p + 4;
                }
                5 if lvt == 6 => {
                    // propertyAccessError：该属性读取失败，整段跳过
                    pos = skip_constructed(apdu, p, 5)?;
                }
                6 if lvt == 6 => {
                    // 服务级错误容器：同样跳过
                    pos = skip_constructed(apdu, p, 6)?;
                }
                _ if lvt == 6 || lvt == 7 => {
                    // listOfResults / propertyValue 的 opening / closing
                    pos = p;
                }
                _ => {
                    let (len, ds) = read_tag_len(apdu, p, lvt)?;
                    if ds + len > apdu.len() {
                        return Err(CollectorError::InvalidFrame(
                            "RPM ACK: context tag truncated".into(),
                        ));
                    }
                    pos = ds + len;
                }
            }
        } else {
            let (value, quality, next) = parse_app_value_from(apdu, pos)?;
            if let Some(oid) = current_oid {
                out.push((oid, value, quality));
            }
            pos = next;
        }
    }
    Ok(out)
}
