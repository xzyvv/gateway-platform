//! AT 指令响应解析单元测试（C2）
use super::*;

#[test]
fn parse_csq_reads_rssi() {
    assert_eq!(parse_csq("+CSQ: 20,0\r\nOK"), 20);
    assert_eq!(parse_csq("AT+CSQ\r\n+CSQ: 17,99\r\nOK"), 17);
}

#[test]
fn parse_csq_defaults_to_99_when_missing_or_invalid() {
    // 无 +CSQ 行
    assert_eq!(parse_csq("OK"), 99);
    // 非数字 RSSI → 回退 99
    assert_eq!(parse_csq("+CSQ: abc,0"), 99);
}

#[test]
fn parse_operator_extracts_quoted_name() {
    assert_eq!(
        parse_operator("+COPS: 0,0,\"China Mobile\",7"),
        "China Mobile"
    );
}

#[test]
fn parse_operator_empty_when_no_quotes() {
    assert_eq!(parse_operator("+COPS: 0"), "");
    assert_eq!(parse_operator("OK"), "");
}

#[test]
fn parse_network_type_maps_act_code() {
    assert_eq!(parse_network_type("+CGREG: 0,1,\"x\",7"), "E-UTRAN");
    assert_eq!(parse_network_type("+CGREG: 0,1,\"x\",0"), "GSM");
    // 未知 act（如 42）：r13-ATC2 fix 改为不映射为 "Unknown"，返回空字符串。
    // 语义：act 未识别时不强行猜测，让 fallback（CEREG）继续尝试。
    assert_eq!(parse_network_type("+CGREG: 0,1,\"x\",42"), "");
}

#[test]
fn parse_network_type_prefers_cereg_over_cgreg() {
    // r13-ATC2 fix: 4G 模块回 +CEREG 时正确识别 E-UTRAN（之前返回 ""）。
    assert_eq!(parse_network_type("+CEREG: 0,1,\"x\",7"), "E-UTRAN");
    // CEREG + CGREG 同时存在时，优先 CEREG。
    let both = "+CEREG: 0,1,\"x\",7\r\n+CGREG: 0,1,\"x\",0\r\nOK";
    assert_eq!(parse_network_type(both), "E-UTRAN");
    // CGREG 单独存在时仍 fallback。
    assert_eq!(parse_network_type("+CGREG: 0,1,\"x\",7"), "E-UTRAN");
}

#[test]
fn parse_network_type_empty_when_no_line() {
    assert_eq!(parse_network_type("OK"), "");
}
