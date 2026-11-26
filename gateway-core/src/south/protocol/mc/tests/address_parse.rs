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

/// MC-R1：软元件编号为 24 位，上限 0x00FF_FFFF；越界必须在建链前返回 `InvalidConfig`。
#[test]
fn parse_address_rejects_index_over_max_as_invalid_config() {
    // 上限本身可解析
    let max = parse_address("D16777215").expect("0x00FF_FFFF 应可解析");
    assert_eq!(max.head, 0x00FF_FFFF, "上限编号应原样保留");

    // 上限 +1 必须被拒，且锁定错误变体为 InvalidConfig（而非任意 is_err）
    match parse_address("D16777216") {
        Err(crate::south::protocol::CollectorError::InvalidConfig(msg)) => {
            assert!(msg.contains("16777216"), "原因需可辨识越界编号: {msg}");
        }
        other => panic!("越界编号应为 InvalidConfig，实际 {other:?}"),
    }
}
