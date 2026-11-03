//! 决策表 #19：对时（C_CS_NA_1，103）
use super::*;

/// 对时 ASDU 结构：类型 103 + COT 激活 + 公共地址 + IOA=0 + CP56Time2a（共 16 字节）
#[test]
fn clock_sync_asdu_structure() {
    let dt = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let asdu = build_clock_sync_asdu(1, 2, &dt);
    assert_eq!(asdu.len(), 16);
    assert_eq!(asdu[0], TYPE_C_CS_NA_1);
    assert_eq!(asdu[1], 0x01); // VSQ：1 个信息体
    assert_eq!(u16::from_le_bytes([asdu[2], asdu[3]]) & 0x3F, 6, "COT 激活");
    assert_eq!(asdu[3], 0x00, "源发地址 OA");
    assert_eq!(u16::from_le_bytes([asdu[4], asdu[5]]), 1, "公共地址");
    assert_eq!(&asdu[6..9], &[0x00, 0x00, 0x00], "时钟同步 IOA = 0");
    assert_eq!(&asdu[9..16], &encode_cp56time2a(&dt));
}

/// 对时等待窗口必须短于 keepalive 周期，且不阻断建链（best-effort 由实现保证）
#[test]
fn clock_sync_timeout_is_bounded() {
    assert!(
        CLOCK_SYNC_TIMEOUT <= Duration::from_secs(5),
        "对时等待必须短暂，避免拖慢建链，实际 = {:?}",
        CLOCK_SYNC_TIMEOUT
    );
}
