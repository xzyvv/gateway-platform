//! 编解码单元测试：字/位解码、字编码回环与结束码校验。
use super::*;

#[test]
fn test_decode_words_u16() {
    // D100 = 0x1234，字内小端
    let data = [0x34, 0x12];
    assert_eq!(decode_words(&data, PointDataType::U16), Some(0x1234 as f64));
}

#[test]
fn test_decode_words_f32() {
    // f32 1.5 = 0x3FC00000，小端全序为 00 00 C0 3F
    let data = [0x00, 0x00, 0xC0, 0x3F];
    let v = decode_words(&data, PointDataType::F32).expect("f32");
    assert!((v - 1.5).abs() < 1e-6, "got {v}");
}

#[test]
fn test_decode_words_u32_word_order() {
    // u32 = 0x00010002 → 低字(w0)=0x0002 在前，高字(w1)=0x0001
    // 字内小端：w0 → 02 00，w1 → 01 00
    let data = [0x02, 0x00, 0x01, 0x00];
    assert_eq!(
        decode_words(&data, PointDataType::U32),
        Some(0x00010002 as f64)
    );
}

#[test]
fn test_decode_bit_nibbles() {
    // 首点高半字节 = 0x10(ON)，次点低半字节 = 0x00(OFF)
    let data = [0x10];
    assert_eq!(decode_bit(&data, 0), Some(1.0));
    assert_eq!(decode_bit(&data, 1), Some(0.0));
    assert_eq!(decode_bit(&data, 2), None);
}

#[test]
fn test_encode_words_roundtrip() {
    for v in [0u16 as f64, 4660.0, 65535.0] {
        let bytes = encode_words(v, PointDataType::U16);
        assert_eq!(decode_words(&bytes, PointDataType::U16), Some(v));
    }
    let bytes = encode_words(1.5, PointDataType::F32);
    assert!((decode_words(&bytes, PointDataType::F32).unwrap() - 1.5).abs() < 1e-6);
}

#[test]
fn test_check_end_code() {
    assert!(check_end_code(0).is_ok());
    assert!(check_end_code(0xC059).is_err());
}

/// ROI-25：结束码是 PLC 明确拒绝，必须映射 DeviceRejected 而非 InvalidFrame。
#[test]
fn test_check_end_code_is_device_rejected() {
    match check_end_code(0xC059) {
        Err(crate::south::protocol::CollectorError::DeviceRejected(msg)) => {
            assert!(msg.contains("0xC059"), "原因需可辨识: {msg}");
        }
        other => panic!("期望 DeviceRejected，实际 {other:?}"),
    }
}

/// ROI-26：3E 响应固定头必须回显请求的网络号 / PC 号 / 模块 I/O 号 / 站号。
#[test]
fn test_3e_response_header_echo_mismatch_rejected() {
    // [子头2][网络号][PC号][IO高][IO低][站号][长度2]
    let mut head = vec![0xD0, 0x00, 0x00, 0xFF, 0xFF, 0x03, 0x00, 0x02, 0x00];
    assert!(parse_response_header_3e(&head, 0).is_ok());

    head[6] = 1; // 站号回显不匹配
    assert!(parse_response_header_3e(&head, 0).is_err());
    head[6] = 0;

    head[2] = 1; // 网络号回显不匹配
    assert!(parse_response_header_3e(&head, 0).is_err());
    head[2] = 0;

    head[3] = 0x00; // PC 号回显不匹配
    assert!(parse_response_header_3e(&head, 0).is_err());
    head[3] = 0xFF;

    head[4] = 0x01; // 模块 I/O 号回显不匹配
    assert!(parse_response_header_3e(&head, 0).is_err());
}

/// ROI-26：4E 响应除序列号外，固定头回显同样必须比对。
#[test]
fn test_4e_response_header_echo_mismatch_rejected() {
    // [子头2][序列号2][网络号][PC号][IO高][IO低][站号][长度2]
    let mut head = vec![
        0xD4, 0x00, 0x01, 0x00, 0x00, 0xFF, 0xFF, 0x03, 0x00, 0x02, 0x00,
    ];
    assert!(parse_response_header_4e(&head, 1, 0).is_ok());

    head[8] = 2; // 站号回显不匹配
    assert!(parse_response_header_4e(&head, 1, 0).is_err());
    head[8] = 0;

    head[2] = 0x02; // 序列号仍优先校验
    assert!(parse_response_header_4e(&head, 1, 0).is_err());
}
