use crate::config::{BusConfig, DeviceConfig, GatewayConfig, PointConfig};

fn bus(port: &str, proto: &str) -> BusConfig {
    serde_json::from_value(serde_json::json!({
        "port_name": port,
        "protocol": proto,
    }))
    .expect("bus json")
}

fn point(id: &str, bus: &str, addr: &str) -> PointConfig {
    PointConfig {
        id: id.to_string(),
        bus: bus.to_string(),
        protocol: "gpio".to_string(),
        address: addr.to_string(),
        slave_id: Some(1),
        interval: 10,
        ..Default::default()
    }
}

/// 协议取 `gpio`：它在任何协议裁剪构建里都合法（见 `domain::VALID_PROTOCOLS`），
/// 而 `modbus` 只在 `--features modbus` 时存在。点位协议会被 `validate_protocol`
/// 校验，报告规则（重复地址 / 悬空引用 / 重复端口）本身与协议无关，
/// 不应因裁剪构建而失去回归。
fn device(id: &str, name: &str, bus: &str) -> DeviceConfig {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "name": name,
        "template": "",
        "bus": bus,
        "protocol": "gpio",
        "address": "1",
    }))
    .expect("device json")
}

/// 干净配置：一条总线 + 一台挂在其上的设备 + 一个挂在该设备上的点位。
/// 只配总线不配设备会命中「总线无设备 / 缺少设备」规则，无法代表「无问题」。
fn base() -> GatewayConfig {
    let mut p = point("p1", "/dev/ttyS1", "40001");
    p.device_id = Some("d1".to_string());
    GatewayConfig {
        buses: vec![bus("/dev/ttyS1", "gpio")],
        devices: Some(vec![device("d1", "设备A", "/dev/ttyS1")]),
        points: vec![p],
        ..Default::default()
    }
}

#[test]
fn clean_config_has_no_issues() {
    assert!(base().validate_report().is_empty());
}

#[test]
fn duplicate_bus_port_is_error() {
    let mut cfg = base();
    cfg.buses.push(bus("/dev/ttyS1", "gpio"));
    let report = cfg.validate_report();
    assert!(
        report
            .iter()
            .any(|i| i.level == "error" && i.field == "buses[1].port_name"),
        "report: {:?}",
        report
    );
}

#[test]
fn duplicate_point_address_is_error() {
    let mut cfg = base();
    // 重复地址的 key 含 device_id：p2 必须挂同一台设备才会被判为重复
    let mut p2 = point("p2", "/dev/ttyS1", "40001");
    p2.device_id = Some("d1".to_string());
    cfg.points.push(p2);
    let report = cfg.validate_report();
    assert!(
        report
            .iter()
            .any(|i| i.level == "error" && i.field == "points[p2].address"),
        "report: {:?}",
        report
    );
}

#[test]
fn dangling_bus_reference_is_error() {
    let mut cfg = base();
    cfg.points[0].bus = "/dev/ttyS9".to_string();
    let report = cfg.validate_report();
    assert!(
        report
            .iter()
            .any(|i| i.level == "error" && i.field == "points[p1].bus"),
        "report: {:?}",
        report
    );
}

#[test]
fn telemetry_ratio_out_of_range_is_warning() {
    let mut cfg = base();
    cfg.telemetry = Some(
        serde_json::from_value(serde_json::json!({
            "enabled": true,
            "sample_ratio": 1.5,
        }))
        .expect("telemetry json"),
    );
    let report = cfg.validate_report();
    assert!(
        report
            .iter()
            .any(|i| i.level == "warning" && i.field == "telemetry.sample_ratio"),
        "report: {:?}",
        report
    );
}

/// 变比与单位不匹配只给 warning（不阻断保存）；匹配时不提示。
#[test]
fn ratio_not_applicable_is_warning() {
    let mut cfg = base();
    cfg.points[0].unit = Some("V".to_string());
    cfg.points[0].ct_ratio = Some(30.0);
    let report = cfg.validate_report();
    assert!(
        report.iter().any(|i| i.code == "ratio_not_applicable"
            && i.level == "warning"
            && i.field == "points[p1].ct_ratio"),
        "report: {:?}",
        report
    );
    assert!(report.iter().all(|i| i.level != "error"), "不应阻断");

    // 电压点位配 PT 才是有效用法
    cfg.points[0].ct_ratio = None;
    cfg.points[0].pt_ratio = Some(100.0);
    assert!(!cfg
        .validate_report()
        .iter()
        .any(|i| i.code == "ratio_not_applicable"));

    // 功率因数额位两种变比都不生效
    cfg.points[0].unit = Some("PF".to_string());
    cfg.points[0].ct_ratio = Some(30.0);
    let report = cfg.validate_report();
    assert!(
        report
            .iter()
            .any(|i| i.code == "ratio_not_applicable" && i.field == "points[p1].ct_ratio"),
        "report: {:?}",
        report
    );
    assert!(
        report
            .iter()
            .any(|i| i.code == "ratio_not_applicable" && i.field == "points[p1].pt_ratio"),
        "report: {:?}",
        report
    );
}

/// 模板存在性只在调用方给出模板清单时检查（清单为空代表模板库不可用）。
#[test]
fn missing_template_is_error_when_names_provided() {
    let mut cfg = base();
    cfg.devices.as_mut().unwrap()[0].template = "tpl-x".to_string();
    let names = vec!["tpl-a".to_string()];
    let report = cfg.validate_report_with(&names);
    assert!(
        report
            .iter()
            .any(|i| i.level == "error" && i.code == "tpl_missing"),
        "report: {:?}",
        report
    );
    // 不传模板清单时退化为跳过模板检查
    assert!(!cfg
        .validate_report()
        .iter()
        .any(|i| i.code == "tpl_missing"));
}

/// 超时越界与完整性都属于非阻断项：warning / info，不影响 valid。
#[test]
fn timeout_and_integrity_are_non_blocking() {
    let mut cfg = base();
    cfg.buses[0].timeout_ms = 5000;
    let report = cfg.validate_report();
    assert!(
        report
            .iter()
            .any(|i| i.level == "warning" && i.code == "bus_timeout"),
        "report: {:?}",
        report
    );

    let empty = GatewayConfig::default().validate_report();
    assert!(empty
        .iter()
        .any(|i| i.level == "info" && i.code == "cfg_no_bus"));
    assert!(empty
        .iter()
        .any(|i| i.level == "info" && i.code == "cfg_no_device"));
    assert!(empty.iter().all(|i| i.level != "error"));
}
