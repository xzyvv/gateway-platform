use super::*;

#[test]
fn test_ascii_roundtrip() {
    let rtu = vec![0x01, 0x03, 0x00, 0x00, 0x00, 0x0A, 0xC5, 0xCD];
    let ascii = rtu_to_ascii(&rtu);
    assert_eq!(ascii[0], b':');
    assert!(ascii.ends_with(b"\r\n"));
    let back = ascii_to_rtu(&ascii).unwrap();
    assert_eq!(back, rtu);
}

#[test]
fn test_lrc8_known_value() {
    // slave=01 fc=03 addr=0000 count=000A -> LRC = 0xF2
    let payload = [0x01, 0x03, 0x00, 0x00, 0x00, 0x0A];
    assert_eq!(lrc8(&payload), 0xF2);
}

#[test]
fn test_ascii_bad_lrc_rejected() {
    // 构造一帧 LRC 错误的 ASCII
    let rtu = vec![0x01, 0x03, 0x00, 0x00, 0x00, 0x0A, 0xC5, 0xCD];
    let mut ascii = rtu_to_ascii(&rtu);
    // 篡改 LRC 倒数第二个 hex 字符
    let lrc_pos = ascii.len() - 4;
    ascii[lrc_pos] = if ascii[lrc_pos] == b'0' { b'1' } else { b'0' };
    assert!(ascii_to_rtu(&ascii).is_none(), "bad LRC must be rejected");
}
