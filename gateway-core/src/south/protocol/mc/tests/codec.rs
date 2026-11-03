//! 编解码单元测试：字/位解码、字编码回环与结束码校验。
use super::*;

#[test]
fn test_decode_words_u16() {
    // D100 = 0x1234，字内小端
    let data = [0x34, 0x12];
    assert_eq!(
        decode_words(&data, ModbusDataType::U16),
        Some(0x1234 as f64)
    );
}

#[test]
fn test_decode_words_f32() {
    // f32 1.5 = 0x3FC00000，小端全序为 00 00 C0 3F
    let data = [0x00, 0x00, 0xC0, 0x3F];
    let v = decode_words(&data, ModbusDataType::F32).expect("f32");
    assert!((v - 1.5).abs() < 1e-6, "got {v}");
}

#[test]
fn test_decode_words_u32_word_order() {
    // u32 = 0x00010002 → 低字(w0)=0x0002 在前，高字(w1)=0x0001
    // 字内小端：w0 → 02 00，w1 → 01 00
    let data = [0x02, 0x00, 0x01, 0x00];
    assert_eq!(
        decode_words(&data, ModbusDataType::U32),
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
        let bytes = encode_words(v, ModbusDataType::U16);
        assert_eq!(decode_words(&bytes, ModbusDataType::U16), Some(v));
    }
    let bytes = encode_words(1.5, ModbusDataType::F32);
    assert!((decode_words(&bytes, ModbusDataType::F32).unwrap() - 1.5).abs() < 1e-6);
}

#[test]
fn test_check_end_code() {
    assert!(check_end_code(0).is_ok());
    assert!(check_end_code(0xC059).is_err());
}
