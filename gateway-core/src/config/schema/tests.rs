use super::*;
use serde_json::json;

#[test]
fn valid_default_config_passes() {
    let cfg = serde_json::to_value(GatewayConfig::default()).unwrap();
    assert!(
        validate_value(&cfg).is_ok(),
        "default config should pass schema"
    );
}

#[test]
fn wrong_type_caught() {
    // baud_rate 应为数字，传字符串
    let bad = json!({
        "buses": [{ "port_name": "x", "baud_rate": "9600" }],
        "points": [],
        "mqtt": { "enabled": false }
    });
    let res = validate_value(&bad);
    assert!(res.is_err(), "string baud_rate must be rejected");
    let errs = res.unwrap_err().join("; ");
    assert!(errs.contains("baud_rate"), "errs={errs}");
}

#[test]
fn missing_required_caught() {
    // 缺少 buses（serde 必填，schema 亦必填）
    let bad = json!({ "points": [], "mqtt": { "enabled": false } });
    let res = validate_value(&bad);
    assert!(res.is_err(), "missing buses must be rejected");
}
