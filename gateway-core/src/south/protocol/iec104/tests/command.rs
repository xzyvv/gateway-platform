//! 决策表 #18：控制方向命令（45/46/50）
use super::*;

/// 功能码 → 命令类型映射：46=双点 / 50=调节步 / 其余（含 45 与未指定）=单点
#[test]
fn command_type_mapping_by_function_code() {
    assert_eq!(command_type_of(None), TYPE_C_SC_NA_1, "未指定 → 单点命令");
    assert_eq!(command_type_of(Some(45)), TYPE_C_SC_NA_1, "45 → 单点命令");
    assert_eq!(command_type_of(Some(46)), TYPE_C_DC_NA_1, "46 → 双点命令");
    assert_eq!(command_type_of(Some(50)), TYPE_C_RC_NA_1, "50 → 调节步命令");
    // 其他功能码（如 Modbus 3/4）不应被误判为双点/调节步，保守回落单点
    assert_eq!(command_type_of(Some(3)), TYPE_C_SC_NA_1);
}

/// 单点命令对象：bit0 = 状态（合=1/分=0），bit7 = S/E 执行位恒置 1
#[test]
fn single_command_object_encoding() {
    assert_eq!(encode_command_object(TYPE_C_SC_NA_1, 1.0), 0x81, "合");
    assert_eq!(encode_command_object(TYPE_C_SC_NA_1, 0.0), 0x80, "分");
    assert_eq!(
        encode_command_object(TYPE_C_SC_NA_1, -1.0),
        0x81,
        "非 0 视为合"
    );
}

/// 双点/调节步命令对象：DCS/RCS = 1（分/降）或 2（合/升），S/E 执行位恒置 1
#[test]
fn double_and_regulating_command_object_encoding() {
    assert_eq!(encode_command_object(TYPE_C_DC_NA_1, 0.0), 0x81, "双点-分");
    assert_eq!(encode_command_object(TYPE_C_DC_NA_1, 1.0), 0x82, "双点-合");
    assert_eq!(encode_command_object(TYPE_C_RC_NA_1, 0.0), 0x81, "调节-降");
    assert_eq!(encode_command_object(TYPE_C_RC_NA_1, 2.0), 0x82, "调节-升");
}

/// 命令 ASDU 结构：类型 + 1 个信息体 + COT 激活 + 公共地址 + IOA(3) + 命令值(1) = 10 字节
#[test]
fn command_asdu_structure() {
    let asdu = build_command_asdu(TYPE_C_SC_NA_1, 1, 2, 100, 0x81);
    assert_eq!(asdu.len(), 10);
    assert_eq!(asdu[0], TYPE_C_SC_NA_1);
    assert_eq!(asdu[1], 0x01, "VSQ：1 个信息体");
    assert_eq!(u16::from_le_bytes([asdu[2], asdu[3]]) & 0x3F, 6, "COT 激活");
    assert_eq!(asdu[3], 0x00, "源发地址 OA");
    assert_eq!(u16::from_le_bytes([asdu[4], asdu[5]]), 1, "公共地址");
    assert_eq!(&asdu[6..9], &[100, 0, 0], "IOA=100 低 3 字节");
    assert_eq!(asdu[9], 0x81, "命令对象字节");

    // 大 IOA（> 255）须正确编码低 3 字节
    let big = build_command_asdu(TYPE_C_DC_NA_1, 1, 2, 0x0102_0304, 0x82);
    assert_eq!(&big[6..9], &[0x04, 0x03, 0x02]);
}

/// 命令确认解析：仅识别 45/46/50 且长度足够；返回 (类型, IOA, COT)
#[test]
fn parse_command_confirmation_roundtrip() {
    let asdu = build_command_asdu(TYPE_C_DC_NA_1, 1, 2, 300, 0x82);
    // 构造确认：COT 由激活(6) 改为激活确认(7)
    let mut confirm = asdu.clone();
    confirm[2] = COT_ACTIVATION_CONFIRMATION as u8;
    confirm[3] = 0;
    assert_eq!(
        parse_command_confirmation(&confirm, 2),
        Some((TYPE_C_DC_NA_1, 300, COT_ACTIVATION_CONFIRMATION))
    );

    // 非命令类型（测量值 14）不识别
    let mut meas = vec![14u8, 0x01, 0x03, 0x00, 0x01, 0x00, 0, 0, 0, 0];
    meas[6] = 0;
    assert_eq!(parse_command_confirmation(&meas, 2), None);
    // 长度不足不识别
    assert_eq!(parse_command_confirmation(&[TYPE_C_SC_NA_1, 0x01], 2), None);
}

/// 肯定/否定确认 COT 必须互不相同且语义正确（7/10 成功；44-47 拒绝）
// 契约守门：断言对象就是常量取值范围本身，常量断言属预期，不简化
#[allow(clippy::assertions_on_constants)]
#[test]
fn command_confirmation_cot_are_distinct() {
    assert_eq!(COT_ACTIVATION_CONFIRMATION, 7, "激活确认");
    assert_eq!(COT_ACTIVATION_TERMINATION, 10, "激活终止");
    assert_eq!(COT_UNKNOWN_TYPE_ID, 44, "未知类型标识");
    assert_eq!(COT_UNKNOWN_COT, 45, "未知传送原因");
    assert_eq!(COT_UNKNOWN_CA, 46, "未知公共地址");
    assert_eq!(COT_UNKNOWN_IOA, 47, "未知信息对象地址");
    assert_ne!(COT_ACTIVATION_CONFIRMATION, COT_ACTIVATION_TERMINATION);
    assert!(
        COT_UNKNOWN_TYPE_ID > COT_ACTIVATION_TERMINATION,
        "否定确认 COT 独立区间"
    );
}
