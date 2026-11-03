use super::*;

#[test]
fn defaults_are_applied_for_new_timeout_fields() {
    let c = CellularConfig::default();
    assert_eq!(c.at_cmd_timeout_secs, 2);
    assert_eq!(c.register_timeout_secs, 60);
    assert_eq!(c.connect_cmd_timeout_secs, 30);
    assert_eq!(c.ppp_bringup_timeout_secs, 45);
    assert_eq!(c.ppp_stop_timeout_secs, 2);
    assert_eq!(c.retry_backoff_secs, 10);
}

#[test]
fn legacy_json_without_new_fields_deserializes_to_defaults() {
    // 模拟旧配置：仅含历史字段，并额外带一个已下线字段。
    // 新增字段（追加在末尾）应取默认值；已下线字段属于未知键，serde 会忽略而不报错，
    // 保证「旧配置仍能加载」。
    let legacy = r#"{
        "enabled": true,
        "at_port": "/dev/ttyUSB0",
        "data_port": "/dev/ttyUSB2",
        "baud_rate": 115200,
        "apn": "cmnet",
        "username": "",
        "password": "",
        "pin": null,
        "dial_timeout_secs": 60,
        "heartbeat_secs": 30,
        "failover_enabled": true,
        "eth_iface": "eth0",
        "ppp_iface": "ppp0",
        "legacy_removed_field": "8.8.8.8"
    }"#;
    let c: CellularConfig =
        serde_json::from_str(legacy).expect("legacy cellular config should deserialize");
    assert_eq!(c.dial_timeout_secs, 60);
    assert_eq!(c.at_cmd_timeout_secs, 2);
    assert_eq!(c.register_timeout_secs, 60);
    assert_eq!(c.ppp_bringup_timeout_secs, 45);
    assert_eq!(c.retry_backoff_secs, 10);
}
