//! D4: 帧提取分发单测——验证按协议名查注册表、未注册返回 None，
//! 以及完整/不完整帧的边界（提取器本身在各自协议模块内已有单测）。

use super::*;

/// 构造合法 Modbus RTU FC03 单寄存器响应帧。
#[cfg(feature = "modbus")]
fn modbus_fc03_response(value: u16) -> Vec<u8> {
    use crate::engine::util::crc::crc16;
    let mut f = vec![0x01, 0x03, 0x02];
    f.extend_from_slice(&value.to_be_bytes());
    let crc = crc16(&f);
    f.push((crc & 0xFF) as u8);
    f.push((crc >> 8) as u8);
    f
}

#[cfg(feature = "modbus")]
#[test]
fn registered_protocol_extracts_complete_frame() {
    let frame = modbus_fc03_response(0x002A);
    let mut fb = FrameBuffer::new();
    fb.push(&frame);
    let got = extract_frame(&mut fb, "modbus", Some(1));
    assert_eq!(got.as_deref(), Some(frame.as_slice()), "完整帧应被提取");
    assert!(fb.is_empty(), "提取成功后应消费已读字节");
}

#[cfg(feature = "modbus")]
#[test]
fn incomplete_frame_returns_none() {
    let frame = modbus_fc03_response(0x002A);
    let mut fb = FrameBuffer::new();
    fb.push(&frame[..frame.len() - 1]); // 少 1 字节 CRC
    assert!(
        extract_frame(&mut fb, "modbus", Some(1)).is_none(),
        "不完整帧不应被提取，须等待更多数据"
    );
}

/// TCP 协议（如 opcua）不注册帧提取器，分发应返回 None。
#[test]
fn protocol_without_extractor_returns_none() {
    let mut fb = FrameBuffer::new();
    fb.push(&[0x01, 0x02, 0x03, 0x04]);
    assert!(
        extract_frame(&mut fb, "opcua", None).is_none(),
        "未注册帧提取器的协议必须返回 None"
    );
}

#[test]
fn unknown_protocol_returns_none() {
    let mut fb = FrameBuffer::new();
    fb.push(&[0x68, 0x00]);
    assert!(extract_frame(&mut fb, "__no_such_protocol__", None).is_none());
}
