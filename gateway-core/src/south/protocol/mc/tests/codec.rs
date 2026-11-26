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
        let bytes = encode_words(v, PointDataType::U16).expect("范围内应可编码");
        assert_eq!(decode_words(&bytes, PointDataType::U16), Some(v));
    }
    let bytes = encode_words(1.5, PointDataType::F32).expect("范围内应可编码");
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

/// MC-E2：1E 帧的状态码与 3E/4E 结束码必须同口径映射。
///
/// 1E 只有 1 字节状态码，3E/4E 是 2 字节结束码；同一「设备拒绝」语义下，
/// 读/写路径都必须落到 `DeviceRejected`，且原因格式不得因帧格式而漂移。
#[test]
fn decode_1e_status_code_maps_like_3e_end_code() {
    use crate::south::protocol::CollectorError;

    // 1E 状态码 0x59：读响应头解析直接给出该状态码，且非零时不再要求后续数据
    let (status_1e, data_len) =
        parse_response_header_1e(&[0x81, 0x59], FC_BATCH_WORD_READ, true, false, 1)
            .expect("1E 状态码应可解析");
    assert_eq!(data_len, 0, "非零状态码不应要求后续数据");
    assert_eq!(
        status_1e, 0x0059,
        "1E 状态码按 u16 参与结束码校验（守护 MC-E2）"
    );

    // 3E/4E 结束码：帧体前两字节小端，同一错误码数值必须与 1E 状态码一致
    let end_code_3e = u16::from_le_bytes([0x59, 0x00]);
    assert_eq!(
        end_code_3e, status_1e,
        "同一错误码在 1E 与 3E/4E 帧下数值必须一致"
    );

    // 读路径：两种帧格式的错误映射完全一致（变体 + 原因文本）
    match (check_end_code(status_1e), check_end_code(end_code_3e)) {
        (Err(CollectorError::DeviceRejected(m1)), Err(CollectorError::DeviceRejected(m2))) => {
            assert_eq!(
                m1, m2,
                "1E 状态码与 3E/4E 结束码的原因必须同口径（守护 MC-E2）"
            );
            assert!(m1.contains("0x0059"), "原因需可辨识: {m1}");
        }
        other => panic!("1E 状态码与 3E/4E 结束码都应映射为 DeviceRejected，实际 {other:?}"),
    }

    // 真实 3E 结束码 0xC059（高字节为出错网络号）同样是设备拒绝，且低字节可辨识
    match check_end_code(0xC059) {
        Err(CollectorError::DeviceRejected(m)) => {
            assert!(m.contains("0xC059"), "真实结束码原因需可辨识: {m}");
        }
        other => panic!("3E 结束码 0xC059 应为 DeviceRejected，实际 {other:?}"),
    }

    // 写路径必须与读路径同口径，不得各自漂移
    match check_write_end_code(end_code_3e) {
        Err(CollectorError::DeviceRejected(m)) => {
            assert_eq!(m, "mc end code 0x0059", "写路径结束码映射不得与读路径漂移");
        }
        other => panic!("写路径非零结束码应为 DeviceRejected，实际 {other:?}"),
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

/// 构造一个只填「固定头 + 数据长度」的 3E 响应头（网络号/PC 号/IO 号/站号均为本站约定值）。
fn head_3e_with_body_len(body_len: u16) -> Vec<u8> {
    let mut head = vec![0xD0, 0x00, 0x00, 0xFF, 0xFF, 0x03, 0x00];
    head.extend_from_slice(&body_len.to_le_bytes());
    head
}

/// MC-R4：「结束码 + 数据」长度必须落在 2..=960 字节，越界帧在读取前即被拒，
/// 不得按对端声明的长度直接分配缓冲区。
#[test]
fn decode_rejects_body_len_out_of_2_to_960() {
    // 下界：装不下 2 字节结束码
    match parse_response_header_3e(&head_3e_with_body_len(1), 0) {
        Err(crate::south::protocol::CollectorError::InvalidFrame(msg)) => {
            assert!(msg.contains("too small"), "原因需可辨识: {msg}");
        }
        other => panic!("长度 1 应为 InvalidFrame，实际 {other:?}"),
    }
    // 上界：961 超过单帧最大 960 字节
    match parse_response_header_3e(&head_3e_with_body_len(961), 0) {
        Err(crate::south::protocol::CollectorError::InvalidFrame(msg)) => {
            assert!(msg.contains("too large"), "原因需可辨识: {msg}");
        }
        other => panic!("长度 961 应为 InvalidFrame，实际 {other:?}"),
    }
    // 边界内可解析
    assert_eq!(
        parse_response_header_3e(&head_3e_with_body_len(2), 0).expect("长度 2 应可解析"),
        2
    );
    assert_eq!(
        parse_response_header_3e(&head_3e_with_body_len(960), 0).expect("长度 960 应可解析"),
        960
    );
}

/// MC-R6：1E 响应功能码必须等于请求功能码 | 0x80，数据长度由点数推导且 ≤ 512 字节。
#[test]
fn decode_1e_rejects_function_code_mismatch_and_over_512() {
    // 正常：字读 1 点 → 功能码 0x81、状态 0、2 字节数据
    assert_eq!(
        parse_response_header_1e(&[0x81, 0x00], FC_BATCH_WORD_READ, true, false, 1)
            .expect("0x81 应匹配字读"),
        (0, 2)
    );

    // 功能码不匹配（0x80 对 0x01 字读）
    match parse_response_header_1e(&[0x80, 0x00], FC_BATCH_WORD_READ, true, false, 1) {
        Err(crate::south::protocol::CollectorError::InvalidFrame(msg)) => {
            assert!(msg.contains("func"), "原因需可辨识: {msg}");
        }
        other => panic!("功能码不匹配应为 InvalidFrame，实际 {other:?}"),
    }

    // 257 字 → 514 字节，超过 512 上限；256 字 = 512 恰好在界内
    match parse_response_header_1e(&[0x81, 0x00], FC_BATCH_WORD_READ, true, false, 257) {
        Err(crate::south::protocol::CollectorError::InvalidFrame(msg)) => {
            assert!(msg.contains("too large"), "原因需可辨识: {msg}");
        }
        other => panic!("514 字节应为 InvalidFrame，实际 {other:?}"),
    }
    assert_eq!(
        parse_response_header_1e(&[0x81, 0x00], FC_BATCH_WORD_READ, true, false, 256)
            .expect("512 字节应可解析"),
        (0, 512)
    );
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
