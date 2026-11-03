//! Golden 报文回归（决策表 #10）
//!
//! ASDU / APCI 逐字节与 `tests/fixtures/golden/iec104/*.hex` 比对；
//! 时间戳一律钉死，保证样本确定性。
use super::*;

/// 总召唤 ASDU（CA=2 字节，默认规约参数）
#[test]
fn golden_iec104_interrogation_ca2() {
    let asdu = build_interrogation_asdu(0x0001, 2);
    crate::south::protocol::golden::assert_golden("iec104/interrogation_ca2.hex", &asdu);
}

/// 总召唤 ASDU（CA=1 字节，决策表 #28 地址长度协商）
#[test]
fn golden_iec104_interrogation_ca1() {
    let asdu = build_interrogation_asdu(0x0001, 1);
    crate::south::protocol::golden::assert_golden("iec104/interrogation_ca1.hex", &asdu);
}

/// 总召唤 I 帧（APCI 4 字节控制域 + ASDU）
#[test]
fn golden_iec104_iframe_interrogation_ca2() {
    let asdu = build_interrogation_asdu(0x0001, 2);
    let frame = build_iframe(0, 0, &asdu).expect("总召唤 I-frame 构造失败");
    crate::south::protocol::golden::assert_golden("iec104/iframe_interrogation_ca2.hex", &frame);
}

/// 单点命令 C_SC_NA_1(45) 合闸（执行位=1），CA=2，IOA=100
#[test]
fn golden_iec104_command_c_sc_na_1_on() {
    let obj = encode_command_object(TYPE_C_SC_NA_1, 1.0);
    let asdu = build_command_asdu(TYPE_C_SC_NA_1, 0x0001, 2, 100, obj);
    crate::south::protocol::golden::assert_golden("iec104/command_c_sc_na_1_on.hex", &asdu);

    let loaded = crate::south::protocol::golden::load("iec104/command_c_sc_na_1_on.hex");
    let (t, ioa, cot) =
        parse_command_confirmation(&loaded, 2).expect("golden 命令 ASDU 必须可解析");
    assert_eq!(t, TYPE_C_SC_NA_1);
    assert_eq!(ioa, 100);
    assert_eq!(cot, 6, "COT 应为激活(6)");
}

/// 对时 ASDU（C_CS_NA_1=103）与 CP56Time2a 时标编解码往返
#[test]
fn golden_iec104_clock_sync_ca2() {
    use chrono::TimeZone;
    let dt = chrono::Utc
        .with_ymd_and_hms(2026, 10, 15, 12, 30, 45)
        .single()
        .expect("构造固定时间戳失败")
        + chrono::Duration::milliseconds(123);

    let cp = encode_cp56time2a(&dt);
    crate::south::protocol::golden::assert_golden("iec104/cp56time2a_sample.hex", &cp);
    let ms = decode_cp56time2a_millis(&cp).expect("CP56Time2a 必须可解码");
    assert_eq!(ms, dt.timestamp_millis() as u64, "时标编解码必须往返一致");

    let asdu = build_clock_sync_asdu(0x0001, 2, &dt);
    crate::south::protocol::golden::assert_golden("iec104/clock_sync_ca2.hex", &asdu);
}
