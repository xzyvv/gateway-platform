use super::*;
use crate::config::BusConfig;

fn serial_bus(port: &str) -> BusConfig {
    BusConfig {
        port_name: port.to_string(),
        protocol: "modbus".to_string(),
        tcp_addr: None,
        ..BusConfig::default()
    }
}

fn tcp_bus(port: &str) -> BusConfig {
    BusConfig {
        port_name: port.to_string(),
        protocol: "modbus".to_string(),
        tcp_addr: Some("10.0.0.2:502".to_string()),
        ..BusConfig::default()
    }
}

/// 强制重建判定：`force_only` 命中时才重建，且只对串口总线生效。
#[test]
fn should_force_rebuild_only_targets_named_serial_bus() {
    let s1 = serial_bus("/dev/ttyS1");
    let s2 = serial_bus("/dev/ttyS2");
    let tcp = tcp_bus("plc-a");

    // 指定目标：只有目标串口总线被强制重建
    assert!(should_force_rebuild(&s1, true, Some("/dev/ttyS1")));
    assert!(
        !should_force_rebuild(&s2, true, Some("/dev/ttyS1")),
        "非目标串口总线不得被强制重建"
    );
    assert!(
        !should_force_rebuild(&tcp, true, Some("plc-a")),
        "TCP 总线不得被强制重建"
    );

    // 未指定目标：重建全部串口总线（不含 TCP）
    assert!(should_force_rebuild(&s1, true, None));
    assert!(should_force_rebuild(&s2, true, None));
    assert!(!should_force_rebuild(&tcp, true, None));

    // 非强制模式（配置热重载）：一律不强制重建
    assert!(!should_force_rebuild(&s1, false, Some("/dev/ttyS1")));
    assert!(!should_force_rebuild(&s1, false, None));
}

/// `force_only` 传一个不存在的总线名时，不得退化成「重建全部串口总线」。
#[test]
fn unknown_force_only_target_rebuilds_nothing() {
    let s1 = serial_bus("/dev/ttyS1");
    assert!(!should_force_rebuild(&s1, true, Some("/dev/ttyS9")));
}

/// 目标名与总线名必须**完全一致**才命中（不得前缀/模糊匹配）。
#[test]
fn force_only_target_requires_exact_name_match() {
    let s1 = serial_bus("/dev/ttyS1");
    assert!(!should_force_rebuild(&s1, true, Some("/dev/ttyS")));
    assert!(!should_force_rebuild(&s1, true, Some("/dev/ttyS10")));
    assert!(should_force_rebuild(&s1, true, Some("/dev/ttyS1")));
}
