//! 决策表 #17：CP56Time2a 7 字节时标编解码
use super::*;

/// 编解码往返必须无损（毫秒精度）
#[test]
fn cp56time2a_roundtrip() {
    let dt = chrono::DateTime::from_timestamp(1_700_000_000, 123_000_000).unwrap();
    let enc = encode_cp56time2a(&dt);
    assert_eq!(enc.len(), 7);
    assert_eq!(
        decode_cp56time2a_millis(&enc).unwrap(),
        dt.timestamp_millis() as u64
    );
}

/// 分内毫秒 = 秒 × 1000 + 毫秒；分/时字段按位打包
#[test]
fn cp56time2a_field_layout() {
    // 1_700_000_000 → 2023-11-14T22:13:20Z
    let dt = chrono::DateTime::from_timestamp(1_700_000_000, 500_000_000).unwrap();
    let enc = encode_cp56time2a(&dt);
    let ms_in_min = u16::from_le_bytes([enc[0], enc[1]]) as u32;
    assert_eq!(ms_in_min, 20 * 1000 + 500, "分内毫秒 = 秒×1000 + 毫秒");
    assert_eq!(enc[2] & 0x3F, 13, "分字段");
    assert_eq!(enc[3] & 0x1F, 22, "时字段");
}

/// 非法时标（长度不足 / 分内毫秒越界）应返回 None，不得 panic
#[test]
fn cp56time2a_rejects_invalid() {
    assert!(
        decode_cp56time2a_millis(&[0, 0, 0, 0, 0, 0]).is_none(),
        "长度不足"
    );
    let mut b = [0u8; 7];
    let ms_60000 = 60000u16.to_le_bytes();
    b[0] = ms_60000[0];
    b[1] = ms_60000[1];
    assert!(
        decode_cp56time2a_millis(&b).is_none(),
        "分内毫秒 > 59999 非法"
    );
}

/// 带时标的短浮点类型（34，M_ME_TF_1）须解析出值与 source_ts
#[test]
fn time_tagged_float_parses_value_and_source_ts() {
    let dt = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let mut asdu = vec![TYPE_M_ME_TF_1, 0x01, 0x03, 0x00, 0x01, 0x00];
    asdu.extend_from_slice(&100u32.to_le_bytes()[..3]); // IOA = 100
    asdu.extend_from_slice(&12.5f32.to_le_bytes()); // 短浮点值
    asdu.push(0x00); // 品质
    asdu.extend_from_slice(&encode_cp56time2a(&dt)); // CP56Time2a

    let mut map = std::collections::HashMap::new();
    parse_asdu(&asdu, &mut map, 2);
    let (v, q, ts) = map.get(&100).copied().expect("IOA 100 应被解析");
    assert!((v - 12.5).abs() < 1e-4, "值解码错误: {v}");
    assert_eq!(q, crate::engine::model::Quality::Good);
    assert_eq!(ts, dt.timestamp_millis() as u64, "source_ts 应等于时标");
}

/// 无时标类型（14）解析后 source_ts 必须为 0（不误填）
#[test]
fn non_tagged_type_has_zero_source_ts() {
    let mut asdu = vec![14u8, 0x01, 0x03, 0x00, 0x01, 0x00];
    asdu.extend_from_slice(&7u32.to_le_bytes()[..3]); // IOA = 7
    asdu.extend_from_slice(&3.5f32.to_le_bytes());
    asdu.push(0x00);
    let mut map = std::collections::HashMap::new();
    parse_asdu(&asdu, &mut map, 2);
    let (v, _q, ts) = map.get(&7).copied().expect("IOA 7 应被解析");
    assert!((v - 3.5).abs() < 1e-4);
    assert_eq!(ts, 0, "无时标类型 source_ts 应为 0");
}
