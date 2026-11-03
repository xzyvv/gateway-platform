//! 链路/读侧帧构建与值解码、单 item 读响应解析测试。
use super::*;

// ===== 帧构建测试 =====

#[test]
fn test_build_cotp_connect() {
    let frame = frames::build_cotp_connect(0, 2);
    assert_eq!(frame.len(), 22);
    // TPKT
    assert_eq!(&frame[0..4], &[0x03, 0x00, 0x00, 0x16]);
    // COTP CR
    assert_eq!(frame[4], 0x11); // PDU length
    assert_eq!(frame[5], 0xE0); // CR type
                                // dst TSAP = 0x0100 | (0<<5 | 2) = 0x0102
                                // Frame layout: TPKT(4) + PDUlen(1) + PDUtype(1) + DSTref(2) + SRCref(2) + Class(1) + TPDUsizeParam(3) + SrcTSAPheader(2) + SrcTSAP(2) + DstTSAPheader(2) + DstTSAP(2)
                                // Dst TSAP value is at frame[20..22]
    assert_eq!(frame[20], 0x01);
    assert_eq!(frame[21], 0x02);
}

#[test]
fn test_build_cotp_connect_rack1_slot3() {
    let frame = frames::build_cotp_connect(1, 3);
    // dst TSAP = 0x0100 | (1<<5 | 3) = 0x0100 | 0x23 = 0x0123
    // Dst TSAP value is at frame[20..22]
    assert_eq!(frame[20], 0x01);
    assert_eq!(frame[21], 0x23);
}

#[test]
fn test_build_setup_comm() {
    let frame = frames::build_setup_comm();
    assert_eq!(frame.len(), 25);
    // TPKT length = 25 = 0x19
    assert_eq!(frame[2], 0x00);
    assert_eq!(frame[3], 0x19);
    // COTP DT
    assert_eq!(&frame[4..7], &[0x02, 0xF0, 0x80]);
    // S7 Protocol ID
    assert_eq!(frame[7], 0x32);
    // PDU type = Job
    assert_eq!(frame[8], 0x01);
    // Parameter length = 8
    assert_eq!(frame[13], 0x00);
    assert_eq!(frame[14], 0x08);
}

#[test]
fn test_build_read_var_db_real() {
    let frame = frames::build_read_var(AREA_DB, 1, 0, 0, S7DataType::Real, 1);
    // TPKT
    assert_eq!(frame[0], 0x03);
    assert_eq!(frame[1], 0x00);
    let total = u16::from_be_bytes([frame[2], frame[3]]);
    assert_eq!(total as usize, frame.len());
    // COTP DT
    assert_eq!(&frame[4..7], &[0x02, 0xF0, 0x80]);
    // S7 header
    assert_eq!(frame[7], 0x32); // Protocol ID
    assert_eq!(frame[8], PDU_JOB); // Job
                                   // PDU ref = 1 (at frame[11..13])
    assert_eq!(frame[11], 0x00);
    assert_eq!(frame[12], 0x01);
    // Function = Read
    assert_eq!(frame[17], FUNC_READ);
    // Item count = 1
    assert_eq!(frame[18], 0x01);
    // Item: variable spec
    assert_eq!(frame[19], 0x12); // Item header
    assert_eq!(frame[20], 0x0A); // Item length
    assert_eq!(frame[21], 0x10); // Syntax ID
    assert_eq!(frame[22], TS_BYTE); // Transport size
                                    // Number of elements = 4 (Real = 4 bytes)
    assert_eq!(u16::from_be_bytes([frame[23], frame[24]]), 4);
    // DB number = 1
    assert_eq!(u16::from_be_bytes([frame[25], frame[26]]), 1);
    // Area = DB
    assert_eq!(frame[27], AREA_DB);
}

#[test]
fn test_build_read_var_bit() {
    let frame = frames::build_read_var(AREA_MARKERS, 0, 10, 3, S7DataType::Bit, 5);
    // Transport size = BIT
    assert_eq!(frame[22], TS_BIT);
    // Number of elements = 1
    assert_eq!(u16::from_be_bytes([frame[23], frame[24]]), 1);
    // Area = M
    assert_eq!(frame[27], AREA_MARKERS);
    // Address = 10*8 + 3 = 83 = 0x000053
    assert_eq!(frame[28], 0x00);
    assert_eq!(frame[29], 0x00);
    assert_eq!(frame[30], 83);
}

// ===== 值解码测试 =====

#[test]
#[allow(clippy::approx_constant)] // 字面量 3.14 用于位级可重现测试
fn test_decode_real() {
    // 3.14 f32 big-endian
    let bytes = 3.14f32.to_be_bytes();
    let v = frames::decode_value(&bytes, S7DataType::Real).unwrap();
    assert!((v - 3.14).abs() < 0.01);
}

#[test]
fn test_decode_int() {
    let bytes = (-42i16).to_be_bytes();
    let v = frames::decode_value(&bytes, S7DataType::Int).unwrap();
    assert_eq!(v, -42.0);
}

#[test]
fn test_decode_word() {
    let bytes = 60000u16.to_be_bytes();
    let v = frames::decode_value(&bytes, S7DataType::Word).unwrap();
    assert_eq!(v, 60000.0);
}

#[test]
fn test_decode_dint() {
    let bytes = (-100000i32).to_be_bytes();
    let v = frames::decode_value(&bytes, S7DataType::DInt).unwrap();
    assert_eq!(v, -100000.0);
}

#[test]
fn test_decode_bit() {
    let v = frames::decode_value(&[0x01], S7DataType::Bit).unwrap();
    assert_eq!(v, 1.0);
    let v = frames::decode_value(&[0x00], S7DataType::Bit).unwrap();
    assert_eq!(v, 0.0);
}

#[test]
fn test_decode_too_short() {
    assert!(frames::decode_value(&[0x01], S7DataType::Real).is_err());
}

// ===== 响应解析测试 =====

#[test]
fn test_parse_read_response_real() {
    // 构造一个完整的 S7 Read Var 响应
    let value: f32 = 42.5;
    let value_bytes = value.to_be_bytes();
    let data_section = {
        let mut d = Vec::new();
        d.push(RET_OK); // return code
        d.push(TS_BYTE); // transport size
        d.extend_from_slice(&(value_bytes.len() as u16).to_be_bytes()); // data length
        d.extend_from_slice(&value_bytes);
        d
    };
    let param = vec![FUNC_READ, 0x01]; // function + item count

    let mut frame = Vec::new();
    frame.extend_from_slice(&[0x02, 0xF0, 0x80]); // COTP DT
    frame.push(0x32); // Protocol ID
    frame.push(PDU_ACK); // PDU type: Ack-Data
    frame.extend_from_slice(&[0x00, 0x00]); // Reserved
    frame.extend_from_slice(&[0x00, 0x01]); // PDU Reference
    frame.extend_from_slice(&(param.len() as u16).to_be_bytes()); // Parameter length
    frame.extend_from_slice(&(data_section.len() as u16).to_be_bytes()); // Data length
    frame.extend_from_slice(&[0x00, 0x00]); // Error class + Error code（Ack-Data 头共 12 字节）
    frame.extend_from_slice(&param);
    frame.extend_from_slice(&data_section);

    let v = frames::parse_read_response(&frame, S7DataType::Real).unwrap();
    assert!((v - 42.5).abs() < 0.01);
}

#[test]
fn test_parse_read_response_error() {
    let mut frame = Vec::new();
    frame.extend_from_slice(&[0x02, 0xF0, 0x80]); // COTP DT
    frame.push(0x32);
    frame.push(0x83); // Ack-Data with error (0x03 | 0x80)
    frame.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
    frame.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
    frame.extend_from_slice(&[0x00, 0x00]);

    assert!(frames::parse_read_response(&frame, S7DataType::Real).is_err());
}

#[test]
fn test_parse_read_response_not_found() {
    let data_section = vec![RET_NOT_FOUND, 0x00, 0x00, 0x00];
    let param = vec![FUNC_READ, 0x01];

    let mut frame = Vec::new();
    frame.extend_from_slice(&[0x02, 0xF0, 0x80]);
    frame.push(0x32);
    frame.push(PDU_ACK);
    frame.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
    frame.extend_from_slice(&(param.len() as u16).to_be_bytes());
    frame.extend_from_slice(&(data_section.len() as u16).to_be_bytes());
    frame.extend_from_slice(&[0x00, 0x00]); // Error class + Error code（Ack-Data 头共 12 字节）
    frame.extend_from_slice(&param);
    frame.extend_from_slice(&data_section);

    assert!(frames::parse_read_response(&frame, S7DataType::Real).is_err());
}

#[test]
fn test_parse_read_response_int() {
    let value: i16 = -100;
    let value_bytes = value.to_be_bytes();
    let data_section = {
        let mut d = Vec::new();
        d.push(RET_OK);
        d.push(TS_BYTE);
        d.extend_from_slice(&(value_bytes.len() as u16).to_be_bytes());
        d.extend_from_slice(&value_bytes);
        d
    };
    let param = vec![FUNC_READ, 0x01];

    let mut frame = Vec::new();
    frame.extend_from_slice(&[0x02, 0xF0, 0x80]);
    frame.push(0x32);
    frame.push(PDU_ACK);
    frame.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
    frame.extend_from_slice(&(param.len() as u16).to_be_bytes());
    frame.extend_from_slice(&(data_section.len() as u16).to_be_bytes());
    frame.extend_from_slice(&[0x00, 0x00]); // Error class + Error code（Ack-Data 头共 12 字节）
    frame.extend_from_slice(&param);
    frame.extend_from_slice(&data_section);

    let v = frames::parse_read_response(&frame, S7DataType::Int).unwrap();
    assert_eq!(v, -100.0);
}

#[test]
fn test_data_type_byte_size() {
    assert_eq!(S7DataType::Bit.byte_size(), 1);
    assert_eq!(S7DataType::Byte.byte_size(), 1);
    assert_eq!(S7DataType::Word.byte_size(), 2);
    assert_eq!(S7DataType::Int.byte_size(), 2);
    assert_eq!(S7DataType::DWord.byte_size(), 4);
    assert_eq!(S7DataType::DInt.byte_size(), 4);
    assert_eq!(S7DataType::Real.byte_size(), 4);
}

#[test]
fn test_data_type_from_str() {
    assert_eq!(S7DataType::from_str("real"), Some(S7DataType::Real));
    assert_eq!(S7DataType::from_str("float"), Some(S7DataType::Real));
    assert_eq!(S7DataType::from_str("int"), Some(S7DataType::Int));
    assert_eq!(S7DataType::from_str("dword"), Some(S7DataType::DWord));
    assert_eq!(S7DataType::from_str("bit"), Some(S7DataType::Bit));
    assert_eq!(S7DataType::from_str("unknown"), None);
}
