//! 软元件地址解析（parse_address）单元测试。
use super::*;

// ============================================================
// 纯函数单元测试
// ============================================================

#[test]
fn test_parse_address_word_devices() {
    let d = parse_address("D100").expect("D100");
    assert_eq!(d.code, 0xA8);
    assert_eq!(d.head, 100);
    assert!(!d.is_bit);

    let zr = parse_address("ZR1000").expect("ZR1000");
    assert_eq!(zr.code, 0xB0);
    assert_eq!(zr.head, 1000);

    let w = parse_address("W1F").ok();
    // 非数字字符会报错（W 后应跟十进制数字）
    assert!(w.is_none());
}

#[test]
fn test_parse_address_bit_devices() {
    let m = parse_address("M10").expect("M10");
    assert_eq!(m.code, 0x90);
    assert!(m.is_bit);

    let x = parse_address("X20").expect("X20");
    assert_eq!(x.code, 0x9C);
    assert!(x.is_bit);

    let y = parse_address("Y7").expect("Y7");
    assert_eq!(y.code, 0x9D);
}

#[test]
fn test_parse_address_errors() {
    assert!(parse_address("").is_err());
    assert!(parse_address("QQ10").is_err());
    assert!(parse_address("D").is_err());
    assert!(parse_address("D0x10").is_err());
}
