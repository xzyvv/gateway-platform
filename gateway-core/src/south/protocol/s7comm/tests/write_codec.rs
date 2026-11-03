//! 单 item 写侧：Write Var 帧构建、值编码范围校验与写响应判定（决策表 #11）。
use super::*;

// ============================================================
// 决策表 #11：S7 Write Var（FUNC_WRITE 0x05）
// ============================================================

#[test]
fn test_build_write_var_real() {
    let frame = frames::build_write_var(AREA_DB, 1, 0, 0, S7DataType::Real, 1.0, 1).unwrap();
    // TPKT 长度自洽
    assert_eq!(frame[0], 0x03);
    assert_eq!(frame[1], 0x00);
    assert_eq!(
        u16::from_be_bytes([frame[2], frame[3]]) as usize,
        frame.len()
    );
    // COTP DT
    assert_eq!(&frame[4..7], &[0x02, 0xF0, 0x80]);
    // S7 header
    assert_eq!(frame[7], 0x32);
    assert_eq!(frame[8], PDU_JOB);
    assert_eq!(u16::from_be_bytes([frame[11], frame[12]]), 1); // PDU ref
    assert_eq!(u16::from_be_bytes([frame[13], frame[14]]), 14); // param len
    assert_eq!(u16::from_be_bytes([frame[15], frame[16]]), 8); // data len
                                                               // Parameter：单 Item 变量规格
    assert_eq!(frame[17], FUNC_WRITE);
    assert_eq!(frame[18], 0x01);
    assert_eq!(frame[19], 0x12);
    assert_eq!(frame[20], 0x0A);
    assert_eq!(frame[21], 0x10);
    assert_eq!(frame[22], TS_BYTE); // transport size
    assert_eq!(u16::from_be_bytes([frame[23], frame[24]]), 4); // 元素数 = 4 字节
    assert_eq!(u16::from_be_bytes([frame[25], frame[26]]), 1); // DB 号
    assert_eq!(frame[27], AREA_DB);
    assert_eq!(&frame[28..31], &[0x00, 0x00, 0x00]); // 位地址
                                                     // Data 段：request 占位 + TS + 长度 + 值
    assert_eq!(frame[31], 0x00);
    assert_eq!(frame[32], TS_BYTE);
    assert_eq!(u16::from_be_bytes([frame[33], frame[34]]), 4);
    assert_eq!(&frame[35..39], &1.0f32.to_be_bytes());
}

#[test]
fn test_build_write_var_bit_pads_to_even() {
    let frame = frames::build_write_var(AREA_MARKERS, 0, 10, 3, S7DataType::Bit, 1.0, 2).unwrap();
    assert_eq!(frame[22], TS_BIT);
    assert_eq!(u16::from_be_bytes([frame[23], frame[24]]), 1); // 位类型元素数恒为 1
    assert_eq!(frame[27], AREA_MARKERS);
    // 位地址 = 10*8 + 3 = 83 = 0x000053
    assert_eq!(&frame[28..31], &[0x00, 0x00, 0x53]);
    // Data：00 03 00 01 01 + 1 字节填充
    assert_eq!(frame[31], 0x00);
    assert_eq!(frame[32], TS_BIT);
    assert_eq!(u16::from_be_bytes([frame[33], frame[34]]), 1);
    assert_eq!(frame[35], 0x01);
    assert_eq!(frame[36], 0x00, "奇数数据长度须补 1 字节对齐");
    assert_eq!(frame.len(), 37);
}

#[test]
fn test_encode_value_ranges() {
    assert_eq!(
        frames::encode_value(1.0, S7DataType::Bit).unwrap(),
        vec![0x01]
    );
    assert_eq!(
        frames::encode_value(0.0, S7DataType::Bit).unwrap(),
        vec![0x00]
    );
    assert!(
        frames::encode_value(300.0, S7DataType::Byte).is_err(),
        "byte > 255 应拒绝"
    );
    assert!(
        frames::encode_value(-1.0, S7DataType::Word).is_err(),
        "word < 0 应拒绝"
    );
    assert!(
        frames::encode_value(f64::NAN, S7DataType::Int).is_err(),
        "非有限值应拒绝"
    );
    assert_eq!(
        frames::encode_value(-100.0, S7DataType::Int).unwrap(),
        (-100i16).to_be_bytes().to_vec()
    );
    assert_eq!(
        frames::encode_value(1.5, S7DataType::Real).unwrap(),
        1.5f32.to_be_bytes().to_vec()
    );
}

#[test]
fn test_build_write_var_rejects_out_of_range() {
    assert!(frames::build_write_var(AREA_DB, 1, 0, 0, S7DataType::Int, 99999.0, 1).is_err());
}

#[test]
fn test_parse_write_response_ok() {
    let mut frame = vec![0x02, 0xF0, 0x80];
    frame.push(0x32);
    frame.push(PDU_ACK);
    frame.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
    frame.extend_from_slice(&(2u16).to_be_bytes()); // param len
    frame.extend_from_slice(&(1u16).to_be_bytes()); // data len
    frame.extend_from_slice(&[0x00, 0x00]); // Error class + Error code
    frame.push(FUNC_WRITE);
    frame.push(0x01);
    frame.push(RET_OK);
    assert!(frames::parse_write_response(&frame).is_ok());
}

#[test]
fn test_parse_write_response_error_code() {
    let mut frame = vec![0x02, 0xF0, 0x80];
    frame.push(0x32);
    frame.push(PDU_ACK);
    frame.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
    frame.extend_from_slice(&(2u16).to_be_bytes());
    frame.extend_from_slice(&(1u16).to_be_bytes());
    frame.extend_from_slice(&[0x00, 0x00]); // Error class + Error code
    frame.push(FUNC_WRITE);
    frame.push(0x01);
    frame.push(0x0A); // 非 0xFF → 失败
    assert!(frames::parse_write_response(&frame).is_err());
}

#[test]
fn test_parse_write_response_pdu_error() {
    let mut frame = vec![0x02, 0xF0, 0x80];
    frame.push(0x32);
    frame.push(0x83); // Ack-Data with error
    frame.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
    frame.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
    frame.extend_from_slice(&[0x00, 0x00]);
    assert!(frames::parse_write_response(&frame).is_err());
}
