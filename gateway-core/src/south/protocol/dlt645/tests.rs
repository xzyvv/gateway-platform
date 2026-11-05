//! DLT645 写功能（0x04/0x0C/0x0F）单元测试

use super::frames::*;
use super::*;
use crate::config::PointConfig;
use crate::engine::model::Quality;
use crate::south::protocol::common::util::{decode_bcd_value, encode_bcd_value};
use crate::south::protocol::Collector;

/// 组一个 DLT645 响应帧：68 ADDR(6) 68 CTRL LEN DATA CS 16
fn dlt_response(addr: &[u8; 6], ctrl: u8, data: &[u8]) -> Vec<u8> {
    let mut f = vec![0x68];
    f.extend_from_slice(addr);
    f.push(0x68);
    f.push(ctrl);
    f.push(data.len() as u8);
    f.extend_from_slice(data);
    let cs: u8 = f.iter().fold(0u8, |a, &b| a.wrapping_add(b));
    f.push(cs);
    f.push(0x16);
    f
}

/// P3: D5=0x20（后续帧标志）应显式报错，而不是把长数据截断成错误值静默上报。
#[test]
fn parse_rejects_multi_frame_flag() {
    let addr = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
    let payload = [0x33, 0x33, 0x33, 0x33, 0x34];

    // 单帧正常应答（ctrl=0x81）可解析
    let ok = dlt_response(&addr, 0x81, &payload);
    assert!(parse_dlt645_response(&ok, &addr, Dlt645Version::V2007).is_ok());

    // 多帧应答（D5=0x20）必须报错
    let multi = dlt_response(&addr, 0x81 | 0x20, &payload);
    match parse_dlt645_response(&multi, &addr, Dlt645Version::V2007) {
        Err(crate::south::protocol::CollectorError::InvalidFrame(m)) => {
            assert!(m.contains("multi-frame"), "错误信息应说明多帧: {}", m);
        }
        other => panic!("多帧应答应报 InvalidFrame，实际 {:?}", other),
    }
}

// ===== B3：读通信地址诊断（广播 0x13） =====

/// 2007：线上低字节在前 + 数据 +0x33，诊断应还原为 12 位表地址
#[test]
fn diag_read_address_v2007_formats_meter_address() {
    let wire_addr: [u8; 6] = [0x01, 0x00, 0x00, 0x00, 0x00, 0x00];
    let data: Vec<u8> = wire_addr.iter().map(|b| b.wrapping_add(0x33)).collect();
    let resp = dlt_response(&wire_addr, 0x93, &data);
    let msg = describe_read_address_response("busA", Dlt645Version::V2007, &resp);
    assert!(msg.contains("读通信地址成功"), "{msg}");
    assert!(msg.contains("表地址=000000000001"), "{msg}");
}

/// 1997：线上高字节在前 + 明文，诊断同样应得到 12 位表地址
#[test]
fn diag_read_address_v1997_formats_meter_address() {
    let wire_addr: [u8; 6] = [0x00, 0x00, 0x00, 0x00, 0x00, 0x01];
    let resp = dlt_response(&wire_addr, 0x93, &wire_addr);
    let msg = describe_read_address_response("busA", Dlt645Version::V1997BePlain, &resp);
    assert!(msg.contains("读通信地址成功"), "{msg}");
    assert!(msg.contains("表地址=000000000001"), "{msg}");
}

#[test]
fn diag_read_address_rejects_wrong_ctrl() {
    let addr: [u8; 6] = [0x01, 0x00, 0x00, 0x00, 0x00, 0x00];
    let resp = dlt_response(&addr, 0x91, &addr);
    let msg = describe_read_address_response("busA", Dlt645Version::V2007, &resp);
    assert!(msg.contains("非预期的 0x93"), "{msg}");
}

#[test]
fn diag_read_address_reports_short_frame() {
    let short = vec![0x68, 0x01, 0x02, 0x68, 0x93, 0x06, 0x00, 0x00];
    let msg = describe_read_address_response("busA", Dlt645Version::V2007, &short);
    assert!(msg.contains("回帧过短"), "{msg}");
}

// ===== 0x0C 写通信地址 =====

#[test]
fn test_build_dlt645_write_address_data_basic() {
    // 新地址 12345678 (0x12 0x34 0x56 0x78 0x00 0x00 明文)
    let new_addr = [0x12, 0x34, 0x56, 0x78, 0x00, 0x00];
    let data = build_dlt645_write_address_data(&new_addr, 0, 0, true).unwrap();
    // 期望：6 字节明文新地址 + 4 字节旧密码(0)+0x33 + 4 字节新密码(0)+0x33
    assert_eq!(data.len(), 14);
    assert_eq!(&data[0..6], &new_addr[..], "新地址 6 字节明文");
    for b in &data[6..14] {
        assert_eq!(*b, 0x33, "密码字段为 0x00+0x33=0x33");
    }
}

#[test]
fn test_build_dlt645_write_address_data_with_passwords() {
    // 新地址 + 旧密码 1234 + 新密码 5678
    let new_addr = [0x00, 0x00, 0x00, 0x00, 0x00, 0x01];
    let data = build_dlt645_write_address_data(&new_addr, 1234, 5678, true).unwrap();
    // 旧密码 1234 → BCD 4 字节 0x34 0x12 0x00 0x00 +0x33 = 0x67 0x45 0x33 0x33
    assert_eq!(&data[6..10], &[0x67, 0x45, 0x33, 0x33], "旧密码 1234");
    // 新密码 5678 → BCD 4 字节 0x78 0x56 0x00 0x00 +0x33 = 0xAB 0x89 0x33 0x33
    assert_eq!(&data[10..14], &[0xAB, 0x89, 0x33, 0x33], "新密码 5678");
}

// ===== 0x0F 写密码 =====

#[test]
fn test_build_dlt645_write_password_data_basic() {
    // 旧密码 0，新密码 1234
    let data = build_dlt645_write_password_data(0, 1234, true).unwrap();
    assert_eq!(data.len(), 8);
    // 旧密码 0 → 4 字节 0+0x33
    assert_eq!(&data[0..4], &[0x33, 0x33, 0x33, 0x33], "旧密码 0");
    // 新密码 1234 → BCD 0x34 0x12 0x00 0x00 +0x33 = 0x67 0x45 0x33 0x33
    assert_eq!(&data[4..8], &[0x67, 0x45, 0x33, 0x33], "新密码 1234");
}

#[test]
fn test_build_dlt645_write_password_data_invalid() {
    // 密码越界（10^8=100000000 → 8 位 BCD 已满，100000000 不行）
    assert!(build_dlt645_write_password_data(0, 99_999_999, true).is_some());
    assert!(build_dlt645_write_password_data(0, 100_000_000, true).is_none());
}

// ===== 0x04 写数据 round-trip（与读路径对偶）=====

#[test]
fn test_bcd_write_value_roundtrip() {
    // 12345678 → encode → 4 字节 BCD → decode → 12345678
    let v = 12345678.0;
    let encoded = encode_bcd_value(v, 4).unwrap();
    let decoded = decode_bcd_value(&encoded).unwrap();
    assert_eq!(decoded, v);
}

// ===== Bug 222: 写值编码与读路径解码必须互逆（往返一致性） =====

/// 构造编解码测试点位（仅 data_type/byte_order 相关）
fn point_for_codec(dt: &str, order: &str) -> PointConfig {
    PointConfig {
        data_type: Some(dt.to_string()),
        byte_order: Some(order.to_string()),
        ..Default::default()
    }
}

/// 全类型 × 全字节序往返：decode(encode(v)) == v。
/// 原实现（to_le_bytes + reorder）对非回文字节值会字节序错乱
/// （如写 0x1234 读回 0x3412），本测试钉住修复后的互逆性。
#[test]
fn test_encode_decode_roundtrip_all_types_all_orders() {
    let cases: &[(&str, usize, f64)] = &[
        ("u16", 2, 0x1234_u16 as f64),
        ("i16", 2, -1234.0),
        ("u32", 4, 0x12345678_u32 as f64),
        ("i32", 4, -123456789.0),
        ("u64", 8, 0x123456789A_u64 as f64),
        ("i64", 8, -1234567890123.0),
        ("f32", 4, std::f64::consts::PI),
        ("f64", 8, -std::f64::consts::E),
    ];
    for order in ["ABCD", "DCBA", "CDAB", "BADC"] {
        for (dt, len, v) in cases {
            let point = point_for_codec(dt, order);
            let encoded = encode_dlt_value(&point, *v, *len)
                .unwrap_or_else(|| panic!("encode 失败: {dt}/{order}"));
            assert_eq!(encoded.len(), *len, "{dt}/{order} 编码长度");
            let (decoded, _t, q) = decode_dlt_value(&point, &encoded);
            assert_eq!(q, Quality::Good, "{dt}/{order} 质量");
            if *dt == "f32" {
                assert!(
                    (decoded - v).abs() < 1e-6,
                    "{dt}/{order}: 写 {v} 读回 {decoded}"
                );
            } else {
                assert_eq!(decoded, *v, "{dt}/{order}: 写 {v} 读回 {decoded}");
            }
        }
    }
}

// ===== P1-1: DLT645-1997 地址字节序反向 =====

/// 1997 版设备回显地址是「高字节在前」，但 `expected_addr`（来自
/// `parse_bcd_address`）固定为「低字节在前」。V1997 解析必须先把
/// `response[1..7]` 反转再比对，否则恒 `address mismatch`，整条采集挂掉。
#[test]
fn parse_accepts_v1997_address_high_byte_first() {
    // 配置层 expected_addr（低字节在前）= 0x11 0x22 0x33 0x44 0x55 0x66
    let expected_addr = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
    // 1997 设备回显高字节在前
    let resp_addr_be = [0x66, 0x55, 0x44, 0x33, 0x22, 0x11];
    // DI(4 字节) + value(1 字节) = 5 字节 payload；0x01 响应 ctrl=0x81
    let payload = [0x33, 0x33, 0x33, 0x33, 0x34];
    let mut frame = vec![0x68];
    frame.extend_from_slice(&resp_addr_be);
    frame.push(0x68);
    frame.push(0x81);
    frame.push(payload.len() as u8);
    frame.extend_from_slice(&payload);
    let cs: u8 = frame.iter().fold(0u8, |a, &b| a.wrapping_add(b));
    frame.push(cs);
    frame.push(0x16);

    // V1997 路径必须成功（修复后）
    let res = parse_dlt645_response(&frame, &expected_addr, Dlt645Version::V1997BePlain);
    assert!(res.is_ok(), "V1997 高字节在前回显应被识别: {:?}", res);

    // V2007 路径用同一帧必须失败（保持原行为：高字节在前不是 V2007 线上序）
    let res2007 = parse_dlt645_response(&frame, &expected_addr, Dlt645Version::V2007);
    assert!(res2007.is_err(), "V2007 不应接受 V1997 反向地址");
}

/// 1997 帧构建应该把传入的"低字节在前"地址（与 expected_addr 同序）转成
/// "高字节在前"发出；并能被 V1997 解析路径正确还原。
#[test]
fn build_v1997_then_parse_roundtrip() {
    // 期望的 BCD 地址（与 parse_bcd_address 输出顺序一致：低字节在前）
    let addr_le = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
    // 请求帧数据区（4 字节 DI +0x33）+ 1 字节占位
    let data = [0x11, 0x22, 0x33, 0x44, 0x55];
    let req = build_dlt645_frame(&addr_le, 0x01, &data, Dlt645Version::V1997BePlain);

    // 解析请求帧：地址段必须是 addr_le 的反向（高字节在前）
    assert_eq!(&req[1..7], &[0x66, 0x55, 0x44, 0x33, 0x22, 0x11][..]);
    // 帧结构：68 ADDR(6) 68 C L DATA CS 16（第二个 0x68 与 2007 相同）
    assert_eq!(req[7], 0x68);
    assert_eq!(req.len(), 12 + data.len());

    // 设备回显（按帧格式，地址原样）→ 构造响应
    let mut resp = vec![0x68];
    resp.extend_from_slice(&req[1..7]); // 高字节在前
    resp.push(0x68);
    resp.push(0x81);
    let payload = [0x33, 0x33, 0x33, 0x33, 0x34];
    resp.push(payload.len() as u8);
    resp.extend_from_slice(&payload);
    let cs: u8 = resp.iter().fold(0u8, |a, &b| a.wrapping_add(b));
    resp.push(cs);
    resp.push(0x16);

    let parsed = parse_dlt645_response(&resp, &addr_le, Dlt645Version::V1997BePlain)
        .expect("V1997 端到端请求-响应 round-trip 必须成功");
    // 数据区 -0x33 后应等于原 payload
    assert_eq!(parsed, payload.to_vec());
}

// ===== B1: 厂家变体（地址字节序 × 数据 +0x33）全组合 =====

/// 5 个变体（2007 + 4 个 1997 子变体）的帧构建 / 解析必须按配置生效：
/// - 地址：BE 变体线上高字节在前，LE 变体线上低字节在前；
/// - 数据区：cipher 变体线上 +0x33，plain 变体线上原样。
#[test]
fn variants_build_and_parse_roundtrip() {
    let addr_le = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
    let plain_payload: [u8; 5] = [0x01, 0x02, 0x03, 0x04, 0x05];

    let cases = [
        (Dlt645Version::V2007, false, true),
        (Dlt645Version::V1997BePlain, true, false),
        (Dlt645Version::V1997LePlain, false, false),
        (Dlt645Version::V1997BeCipher, true, true),
        (Dlt645Version::V1997LeCipher, false, true),
    ];

    for (version, addr_be, cipher) in cases {
        assert_eq!(version.addr_be_on_wire(), addr_be, "{version:?} 地址序");
        assert_eq!(version.data_cipher(), cipher, "{version:?} 数据加密");

        let wire_addr: Vec<u8> = if addr_be {
            addr_le.iter().rev().copied().collect()
        } else {
            addr_le.to_vec()
        };
        let wire_data: Vec<u8> = if cipher {
            plain_payload
                .iter()
                .map(|b| (*b).wrapping_add(0x33))
                .collect()
        } else {
            plain_payload.to_vec()
        };

        // 请求帧：68 ADDR(6) 68 C L DATA CS 16
        let req = build_dlt645_frame(&addr_le, 0x01, &wire_data, version);
        assert_eq!(&req[1..7], &wire_addr[..], "{version:?} 地址字节序");
        assert_eq!(
            &req[10..10 + wire_data.len()],
            &wire_data[..],
            "{version:?} 数据区线上形态"
        );

        // 设备按同一变体回显 → 解析应还原为明文 payload
        let resp = build_dlt645_frame(&addr_le, 0x81, &wire_data, version);
        let parsed = parse_dlt645_response(&resp, &addr_le, version)
            .unwrap_or_else(|e| panic!("{version:?} round-trip 失败: {e:?}"));
        assert_eq!(
            parsed,
            plain_payload.to_vec(),
            "{version:?} 数据区应还原明文"
        );
    }

    // 地址字节序错误的变体必须拒绝该响应（防止配置选错时静默读错表）
    let resp_be = build_dlt645_frame(&addr_le, 0x81, &plain_payload, Dlt645Version::V1997BePlain);
    assert!(
        parse_dlt645_response(&resp_be, &addr_le, Dlt645Version::V1997LePlain).is_err(),
        "LE 变体不应接受高字节在前的回显"
    );
}

// =========================================================================
// C1.5：不可达端点/无响应时，诊断必须如实报错，不得伪造「成功」
// =========================================================================

/// 用不存在的串口起一条总线：模拟现场「表未接线 / 串口配错 / 转换器掉电」。
fn dead_serial_bus(port: &str) -> std::sync::Arc<crate::south::bus::BusManager> {
    use crate::config::BusConfig;
    use crate::engine::metrics::Metrics;
    use crate::south::bus::{BusDeps, BusManager};
    use std::sync::Arc;

    let cfg = BusConfig {
        port_name: port.to_string(),
        protocol: "dlt645".into(),
        timeout_ms: 300,
        retry_count: Some(0),
        retry_delay_ms: Some(0),
        ..Default::default()
    };
    Arc::new(BusManager::spawn_with(
        &cfg,
        None,
        Arc::new(Metrics::new()),
        BusDeps::production(),
    ))
}

#[tokio::test]
async fn diag_reports_unreachable_endpoint_instead_of_fake_success() {
    let c = Dlt645Collector::new("dlt645_diag_dead", dead_serial_bus("COM9999"));
    let msg = c.diag().await;
    assert!(msg.contains("无响应"), "应报无响应，实际: {msg}");
    assert!(
        msg.contains("COM9999") || msg.contains("dlt645_diag_dead"),
        "应点明总线，实际: {msg}"
    );
    assert!(!msg.contains("成功"), "不可达端点不得报成功: {msg}");
}

/// plain 变体的密码字段不做 +0x33（1997 国标）；cipher 变体做 +0x33。
#[test]
fn write_password_fields_respect_variant_cipher() {
    let plain = build_dlt645_write_password_data(0, 1234, false).unwrap();
    assert_eq!(&plain[0..4], &[0x00, 0x00, 0x00, 0x00], "plain 旧密码原样");
    assert_eq!(
        &plain[4..8],
        &[0x34, 0x12, 0x00, 0x00],
        "plain 新密码 BCD 明文"
    );

    let cipher = build_dlt645_write_password_data(0, 1234, true).unwrap();
    assert_eq!(
        &cipher[0..4],
        &[0x33, 0x33, 0x33, 0x33],
        "cipher 旧密码 +0x33"
    );
    assert_eq!(
        &cipher[4..8],
        &[0x67, 0x45, 0x33, 0x33],
        "cipher 新密码 BCD +0x33"
    );

    // 新地址字段始终明文，只有密码字段随变体处理
    let new_addr = [0x12, 0x34, 0x56, 0x78, 0x00, 0x00];
    let addr_plain = build_dlt645_write_address_data(&new_addr, 0, 0, false).unwrap();
    assert_eq!(&addr_plain[0..6], &new_addr[..], "新地址明文");
    assert_eq!(&addr_plain[6..14], &[0x00; 8][..], "plain 密码字段原样");
}

// =========================================================================
// Golden 报文回归（决策清单 P0-3）
//
// 样本在 `gateway-core/tests/fixtures/golden/dlt645/*.hex`，逐字节锁定：
// - 2007 读请求（地址低字节在前、DI 反转 +0x33）
// - 2007 读响应（DI/值 +0x33，解码出正确数值）
// - 1997 读响应（地址高字节在前、数据明文）
// - 2007 写通信地址请求（0x0C，新地址明文 + 密码 +0x33）
// - 异常应答（CTRL 带 0x40），解析必须报「device abnormal response」
//
// 首次生成 / 有意变更：$env:GOLDEN_UPDATE='1' 跑这些用例后提交 hex。
// =========================================================================

/// 2007 默认地址样本（低字节在前：000000000001）
fn golden_addr() -> [u8; 6] {
    [0x01, 0x00, 0x00, 0x00, 0x00, 0x00]
}

#[test]
fn golden_v2007_read_request() {
    let addr = golden_addr();
    // DI 00010000 → 线上 4 字节反转 + 0x33
    let mut data = parse_data_id("00010000").expect("DI 解析失败");
    data.reverse();
    for b in data.iter_mut() {
        *b = b.wrapping_add(0x33);
    }
    let f = build_dlt645_frame(&addr, 0x01, &data, Dlt645Version::V2007);
    crate::south::protocol::golden::assert_golden("dlt645/v2007_read_request.hex", &f);
}

#[test]
fn golden_v2007_read_response_decodes_value() {
    let addr = golden_addr();
    // 明文数据区 = DI(4) + 值(4)：DI=00000100，值=0x00001234
    let plain: Vec<u8> = vec![0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x12, 0x34];
    let wire: Vec<u8> = plain.iter().map(|b| b.wrapping_add(0x33)).collect();
    let f = build_dlt645_frame(&addr, 0x81, &wire, Dlt645Version::V2007);
    crate::south::protocol::golden::assert_golden("dlt645/v2007_read_response.hex", &f);

    let loaded = crate::south::protocol::golden::load("dlt645/v2007_read_response.hex");
    let parsed = parse_dlt645_response(&loaded, &addr, Dlt645Version::V2007)
        .expect("golden 2007 读响应必须可解析");
    assert_eq!(parsed, plain, "数据区 -0x33 后应还原明文");

    let point = point_for_codec("u32", "ABCD");
    let (raw, _t, quality) = decode_dlt_value(&point, &parsed[4..]);
    assert_eq!(raw, 0x1234_u32 as f64, "DI 后 4 字节应解码为 0x1234");
    assert_eq!(quality, Quality::Good);
}

#[test]
fn golden_v1997_read_response() {
    let addr = golden_addr();
    // 1997 国标：地址高字节在前、数据区明文（无 +0x33）
    let plain: Vec<u8> = vec![0x00, 0x00, 0x01, 0x00, 0x12, 0x34];
    let f = build_dlt645_frame(&addr, 0x81, &plain, Dlt645Version::V1997BePlain);
    crate::south::protocol::golden::assert_golden("dlt645/v1997_read_response.hex", &f);

    let loaded = crate::south::protocol::golden::load("dlt645/v1997_read_response.hex");
    let parsed = parse_dlt645_response(&loaded, &addr, Dlt645Version::V1997BePlain)
        .expect("golden 1997 读响应必须可解析");
    assert_eq!(parsed, plain);
}

#[test]
fn golden_v2007_write_address_request() {
    let addr = golden_addr();
    let new_addr = [0x12, 0x34, 0x56, 0x78, 0x00, 0x00];
    let data = build_dlt645_write_address_data(&new_addr, 0, 0, true).expect("写地址数据构建失败");
    let f = build_dlt645_frame(&addr, 0x0C, &data, Dlt645Version::V2007);
    crate::south::protocol::golden::assert_golden("dlt645/v2007_write_address_request.hex", &f);
}

#[test]
fn golden_v2007_exception_response_is_rejected() {
    let addr = golden_addr();
    // 异常应答：0x01 | 0x80 | 0x40 = 0xC1，数据区为错误码 0x01
    let f = build_dlt645_frame(&addr, 0xC1, &[0x01], Dlt645Version::V2007);
    crate::south::protocol::golden::assert_golden("dlt645/v2007_exception_response.hex", &f);

    let loaded = crate::south::protocol::golden::load("dlt645/v2007_exception_response.hex");
    match parse_dlt645_response(&loaded, &addr, Dlt645Version::V2007) {
        Err(crate::south::protocol::CollectorError::InvalidFrame(msg)) => {
            assert!(msg.contains("abnormal"), "应报设备异常应答，实际: {msg}");
        }
        other => panic!("异常应答必须解析失败，实际: {other:?}"),
    }
}

// =========================================================================
// 畸形帧负例：任意截断 / LEN 撒谎 / CS 错误 / 尾标错误都不得 panic
// =========================================================================

#[test]
fn malformed_dlt645_response_is_rejected_without_panic() {
    let addr = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
    let payload = [0x33u8, 0x34, 0x35];
    let full = dlt_response(&addr, 0x81, &payload);
    assert!(parse_dlt645_response(&full, &addr, Dlt645Version::V2007).is_ok());

    // 任意严格前缀都必须报错（0 字节也不能 panic）
    for cut in 0..full.len() {
        assert!(
            parse_dlt645_response(&full[..cut], &addr, Dlt645Version::V2007).is_err(),
            "截断到 {cut} 字节应被拒绝"
        );
    }

    // LEN 字段撒谎（远超实际数据）→ 截断错误，不得越界读
    let mut lying = full.clone();
    lying[9] = 0xFE;
    assert!(parse_dlt645_response(&lying, &addr, Dlt645Version::V2007).is_err());

    // CS 篡改
    let mut bad_cs = full.clone();
    let cs_idx = bad_cs.len() - 2;
    bad_cs[cs_idx] ^= 0xFF;
    assert!(matches!(
        parse_dlt645_response(&bad_cs, &addr, Dlt645Version::V2007),
        Err(crate::south::protocol::CollectorError::ChecksumError)
    ));

    // 尾标不是 0x16
    let mut bad_end = full.clone();
    *bad_end.last_mut().unwrap() = 0x00;
    assert!(parse_dlt645_response(&bad_end, &addr, Dlt645Version::V2007).is_err());

    // 请求帧（D7 未置位）不能被当成响应
    let request_like = dlt_response(&addr, 0x01, &payload);
    assert!(parse_dlt645_response(&request_like, &addr, Dlt645Version::V2007).is_err());

    // 地址不匹配
    let mut other_addr = addr;
    other_addr[0] ^= 0xFF;
    assert!(parse_dlt645_response(&full, &other_addr, Dlt645Version::V2007).is_err());
}
