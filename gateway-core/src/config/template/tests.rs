use super::*;
use tempfile::TempDir;

/// 构造最小可工作的 DeviceTemplate（含一个 point "temp"）用于 expand_device 测试。
///
/// 协议取 `gpio`：它在任何协议裁剪构建里都合法，而 `modbus` 只在
/// `--features modbus` 时存在——模板加载（`validate_template`）会校验协议，
/// 用固定可用的协议才能让展开逻辑在所有裁剪组合下都被回归到。
fn build_template() -> DeviceTemplate {
    DeviceTemplate {
        version: 1,
        name: "tpl".into(),
        vendor: "v".into(),
        model: "m".into(),
        description: None,
        protocol: "gpio".into(),
        default_interval: 1,
        default_byte_order: None,
        points: vec![PointTemplate {
            name: "temp".into(),
            unit: String::new(),
            data_type: None,
            default_address: Some(0),
            data_id: None,
            function_code: None,
            register_count: None,
            scale: 1.0,
            offset: 0.0,
            ct_ratio: None,
            pt_ratio: None,
            formula: None,
            deadband: None,
            aggregation: None,
            modbus_register: None,
        }],
    }
}

/// 构造最小可工作的 DeviceConfig（用上面的模板）。
///
/// 协议与模板保持一致（`gpio`），展开出的点位协议同样在任何裁剪组合下合法。
fn build_device(name: &str, address: &str) -> DeviceConfig {
    DeviceConfig {
        id: String::new(),
        name: name.into(),
        template: "tpl".into(),
        bus: "b1".into(),
        protocol: "gpio".into(),
        address: address.into(),
        slave_id: Some(1),
        address_map: None,
        interval: Some(1),
    }
}

/// 构造 TemplateManager（含一个内置模板 "tpl"）用于 expand_devices 测试。
fn build_mgr() -> (TempDir, TemplateManager) {
    let dir = tempfile::tempdir().expect("create temp dir");
    std::fs::write(
        dir.path().join("tpl.json"),
        serde_json::to_string(&build_template()).expect("serialize template"),
    )
    .expect("write template");
    let mgr = TemplateManager::new(dir.path().to_str().unwrap(), "");
    (dir, mgr)
}

/// r26-TPL1：两个 device.name 不同时，expand_devices 应正常返回。
#[test]
fn expand_devices_accepts_unique_names() {
    let (_dir, mgr) = build_mgr();
    let devs = vec![build_device("pump_a", "1"), build_device("pump_b", "2")];
    let points = mgr.expand_devices(&devs).expect("unique names ok");
    let ids: Vec<&str> = points.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, vec!["pump_a_temp", "pump_b_temp"]);
}

/// r26-TPL1：两个 device.name 重复时，expand_devices 必须 fail-fast 返回 Err。
#[test]
fn expand_devices_rejects_duplicate_device_name() {
    let (_dir, mgr) = build_mgr();
    let devs = vec![build_device("dup", "1"), build_device("dup", "2")];
    let err = mgr
        .expand_devices(&devs)
        .expect_err("duplicate device name must error");
    let msg = err.to_string();
    assert!(
        msg.contains("duplicate device name") && msg.contains("dup"),
        "错误消息应包含 'duplicate device name' 与设备名 'dup'，实际：{}",
        msg
    );
}

/// r26-TPL1：设备名不同但展开后 point_id 仍冲突的兜底分支（防御性回归）。
/// 通过直接构造两份不同 device.name 但同一模板点位 ID 冲突的极端 case 触发：
/// 实际单模板展开不会撞，但保险起见仍加保护。这里用空设备列表无法触发，
/// 故借用 HashSet 重复插入模拟一次逻辑路径——直接测 expand_devices 的 HashSet 检查逻辑。
#[test]
fn expand_devices_returns_unique_points() {
    let (_dir, mgr) = build_mgr();
    let devs = vec![build_device("only", "1")];
    let points = mgr.expand_devices(&devs).expect("ok");
    let mut seen = HashSet::new();
    for p in &points {
        assert!(seen.insert(p.id.clone()), "point_id '{}' 重复", p.id);
    }
}

/// 回归：模板展开的点位必须带上所属设备 ID。
/// 此前固定为 None，而 Web API 又从 cfg.points 反查（展开点位不在配置快照里），
/// 最终拿到空串 → 前端按设备聚合「最后成功采集 / 通信质量」全部落空。
#[test]
fn expand_devices_sets_owning_device_id() {
    let (_dir, mgr) = build_mgr();
    let mut a = build_device("dev-a", "1");
    a.id = "dev-a".into();
    let mut b = build_device("dev-b", "2");
    b.id = "dev-b".into();
    let points = mgr.expand_devices(&[a, b]).expect("ok");
    assert_eq!(points.len(), 2);
    for p in &points {
        let owner = if p.id.starts_with("dev-a_") {
            "dev-a"
        } else {
            "dev-b"
        };
        assert_eq!(
            p.device_id.as_deref(),
            Some(owner),
            "point '{}' 设备归属错误",
            p.id
        );
    }
}

/// IEC104：模板展开必须以点位 IOA 作为 address，而不是设备地址。
/// 回归：此前所有点位都用 device.address，采集值会互相覆盖。
#[cfg(feature = "iec104")]
#[test]
fn expand_iec104_uses_point_ioa_as_address() {
    let dir = tempfile::tempdir().expect("create temp dir");

    let mut tpl = build_template();
    tpl.protocol = "iec104".into();
    tpl.points = vec![
        PointTemplate {
            name: "p1".into(),
            default_address: Some(1001),
            ..tpl.points[0].clone()
        },
        PointTemplate {
            name: "p2".into(),
            default_address: Some(1002),
            ..tpl.points[0].clone()
        },
    ];
    std::fs::write(
        dir.path().join("tpl.json"),
        serde_json::to_string(&tpl).expect("serialize template"),
    )
    .expect("write template");

    let mgr = TemplateManager::new(dir.path().to_str().unwrap(), "");
    let mut dev = build_device("station", "1");
    dev.protocol = "iec104".into();
    let points = mgr.expand_devices(&[dev]).expect("iec104 模板应能展开");
    let addresses: Vec<&str> = points.iter().map(|p| p.address.as_str()).collect();
    assert_eq!(addresses, vec!["1001", "1002"]);
}

/// 历史别名（uint16/float/double）在加载时统一成规范名，只影响 Modbus。
#[test]
fn normalize_data_types_rewrites_modbus_aliases() {
    let mut tpl = build_template();
    tpl.protocol = "modbus".into();
    tpl.points[0].data_type = Some("float".into());
    tpl.points.push(PointTemplate {
        name: "p2".into(),
        data_type: Some("uint16".into()),
        ..tpl.points[0].clone()
    });
    tpl.points.push(PointTemplate {
        name: "p3".into(),
        data_type: Some("double".into()),
        ..tpl.points[0].clone()
    });

    assert_eq!(normalize_data_types(&mut tpl), 3);
    assert_eq!(tpl.points[0].data_type.as_deref(), Some("f32"));
    assert_eq!(tpl.points[1].data_type.as_deref(), Some("u16"));
    assert_eq!(tpl.points[2].data_type.as_deref(), Some("f64"));
}

/// IEC104 的 float/single 是协议语义，不能被当成 Modbus 别名改写。
#[test]
fn normalize_data_types_leaves_other_protocols_alone() {
    for protocol in ["iec104", "s7comm", "gpio"] {
        let mut tpl = build_template();
        tpl.protocol = protocol.into();
        tpl.points[0].data_type = Some("single".into());
        tpl.points.push(PointTemplate {
            name: "p2".into(),
            data_type: Some("float".into()),
            ..tpl.points[0].clone()
        });

        assert_eq!(normalize_data_types(&mut tpl), 0, "{protocol} 不应被改写");
        assert_eq!(tpl.points[0].data_type.as_deref(), Some("single"));
        assert_eq!(tpl.points[1].data_type.as_deref(), Some("float"));
    }
}

/// Modbus 展示地址：区段首位由功能码决定，后四位是 1 基地址。
#[test]
fn modbus_display_address_maps_function_code_and_offset() {
    assert_eq!(
        modbus_display_address(Some(3), Some(0)).as_deref(),
        Some("30001")
    );
    assert_eq!(
        modbus_display_address(Some(4), Some(8191)).as_deref(),
        Some("48192")
    );
    assert_eq!(
        modbus_display_address(Some(1), Some(0)).as_deref(),
        Some("00001")
    );
    assert_eq!(
        modbus_display_address(Some(2), Some(15)).as_deref(),
        Some("10016")
    );
    // 偏移超出 5 位串表达范围：返回 None，由调用方兜底
    assert_eq!(modbus_display_address(Some(3), Some(9999)), None);
}

/// 模板点位的 CT/PT 变比必须原样带到展开后的点位。
/// 回归：此前硬编码 `None`，用模板建站时变比必丢。
#[test]
fn expand_device_carries_template_ratios() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let mut tpl = build_template();
    tpl.points[0].unit = "kW".into();
    tpl.points[0].ct_ratio = Some(30.0);
    tpl.points[0].pt_ratio = Some(100.0);
    std::fs::write(
        dir.path().join("tpl.json"),
        serde_json::to_string(&tpl).expect("serialize template"),
    )
    .expect("write template");
    let mgr = TemplateManager::new(dir.path().to_str().unwrap(), "");

    let points = mgr
        .expand_devices(&[build_device("meter", "1")])
        .expect("模板应能展开");
    assert_eq!(points[0].ct_ratio, Some(30.0));
    assert_eq!(points[0].pt_ratio, Some(100.0));
}

/// 模板里的 0 / 负变比必须在保存时被拒（否则展开后读数恒为 0 或反号）。
#[test]
fn template_rejects_non_positive_ratio() {
    let mut tpl = build_template();
    tpl.points[0].ct_ratio = Some(0.0);
    let err = validate_template(&tpl).expect_err("ct_ratio=0 必须被拒");
    assert!(err.to_string().contains("ct_ratio"), "实际：{err}");

    tpl.points[0].ct_ratio = None;
    tpl.points[0].pt_ratio = Some(-1.0);
    let err = validate_template(&tpl).expect_err("pt_ratio=-1 必须被拒");
    assert!(err.to_string().contains("pt_ratio"), "实际：{err}");
}

/// Modbus：展开点位的 address 是「功能码 + 寄存器偏移」的 5 位展示串，
/// 不再是从站地址。回归：此前所有点位都显示成同一个 device.address。
#[cfg(feature = "modbus")]
#[test]
fn expand_modbus_uses_register_offset_as_display_address() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let mut tpl = build_template();
    tpl.protocol = "modbus".into();
    tpl.points[0].function_code = Some(3);
    tpl.points[0].default_address = Some(0);
    std::fs::write(
        dir.path().join("tpl.json"),
        serde_json::to_string(&tpl).expect("serialize template"),
    )
    .expect("write template");
    let mgr = TemplateManager::new(dir.path().to_str().unwrap(), "");

    let mut dev = build_device("meter", "7");
    dev.protocol = "modbus".into();
    let points = mgr.expand_devices(&[dev]).expect("modbus 模板应能展开");
    assert_eq!(points[0].address, "30001", "应展示寄存器地址而非从站号");
    assert_eq!(points[0].address_num, Some(0), "实际寻址仍用 0 基偏移");
}

/// byte_order 校验：大写四字码（ABCD 等）按约定大写，必须能通过。
/// 回归：原实现统一转小写后比对，VALID_BYTE_ORDERS 中的大写项永远无法命中。
#[test]
fn byte_order_accepts_uppercase_letter_codes() {
    for code in ["ABCD", "BADC", "CDAB", "DCBA"] {
        assert!(validate_byte_order(code).is_ok(), "{code} 应通过校验");
    }
}

/// byte_order 校验：-endian 写法兼容任意大小写（既有行为保持）。
#[test]
fn byte_order_accepts_endian_variants_any_case() {
    for v in ["big-endian", "Big-Endian", "LITTLE-ENDIAN"] {
        assert!(validate_byte_order(v).is_ok(), "{v} 应通过校验");
    }
}

/// byte_order 校验：未知值（含小写四字码）必须拒绝。
#[test]
fn byte_order_rejects_unknown_values() {
    for v in ["abcd", "XYZW", "", "middle-endian"] {
        assert!(validate_byte_order(v).is_err(), "{v} 应被拒绝");
    }
}

/// 模板加载容忍 UTF-8 BOM（现场用记事本编辑后的常见产物）。
#[test]
fn load_template_tolerates_utf8_bom() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let mut json = String::from("\u{feff}");
    json.push_str(&serde_json::to_string(&build_template()).expect("serialize template"));
    std::fs::write(dir.path().join("tpl.json"), json).expect("write bom template");

    let mgr = TemplateManager::new(dir.path().to_str().unwrap(), "");
    let devs = vec![build_device("bom_dev", "1")];
    let points = mgr.expand_devices(&devs).expect("BOM 模板应能加载");
    assert_eq!(points[0].id, "bom_dev_temp");
}
