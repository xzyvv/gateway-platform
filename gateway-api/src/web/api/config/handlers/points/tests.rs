use super::*;

fn payload_with_scale(scale: Option<f64>) -> PointPayload {
    PointPayload {
        id: "p1".into(),
        device_id: None,
        name: None,
        bus: "bus0".into(),
        protocol: "modbus".into(),
        address: "1".into(),
        slave_id: Some(1),
        address_num: Some(0),
        data_id: None,
        function_code: Some(3),
        data_type: None,
        byte_order: None,
        register_count: None,
        interval: 10,
        scale,
        offset: None,
        formula: None,
        ct_ratio: None,
        pt_ratio: None,
        deadband: None,
        on_change: None,
        unit: None,
        alarm_high: None,
        alarm_low: None,
        aggregation: None,
        modbus_register: None,
        writable: None,
    }
}

/// Bug 228 回归：显式 scale=0 必须在入库前被拒（4002），不得静默归零。
#[test]
fn validate_point_rejects_explicit_zero_scale() {
    let cfg = GatewayConfig::default();
    let err =
        validate_point(&payload_with_scale(Some(0.0)), &cfg).expect_err("显式 scale=0 应被拒绝");
    assert!(
        err.message().contains("scale"),
        "错误信息应指向 scale 字段，实际: {}",
        err.message()
    );
}

/// 省略 scale 或使用非 0 倍率都必须通过校验。
#[test]
fn validate_point_accepts_absent_or_nonzero_scale() {
    let cfg = GatewayConfig::default();
    assert!(validate_point(&payload_with_scale(None), &cfg).is_ok());
    assert!(validate_point(&payload_with_scale(Some(1.0)), &cfg).is_ok());
    assert!(validate_point(&payload_with_scale(Some(0.1)), &cfg).is_ok());
}

/// r43-CFG2 回归：create_point 的校验须与 update_point 一致——
/// 采集周期 >= 北向上报周期必须被拒，否则可经新增路径落库「能建、不能改」的非法测点。
#[test]
fn validate_point_rejects_interval_not_below_north_period() {
    let mut cfg = GatewayConfig::default();
    cfg.mqtt.enabled = true;
    cfg.mqtt.publish_interval = 60;

    let mut p60 = payload_with_scale(None);
    p60.interval = 60;
    let err = validate_point(&p60, &cfg).expect_err("interval=60 应被拒绝");
    assert!(
        err.message().contains("60"),
        "错误信息应指向当前北向上报周期，实际: {}",
        err.message()
    );

    let mut p120 = payload_with_scale(None);
    p120.interval = 120;
    assert!(
        validate_point(&p120, &cfg).is_err(),
        "interval=120 应被拒绝"
    );
}

/// 北向上报周期可配：上界必须跟随配置，而不是硬编码 60。
#[test]
fn validate_point_interval_bound_follows_mqtt_publish_interval() {
    let mut cfg = GatewayConfig::default();
    cfg.mqtt.enabled = true;
    cfg.mqtt.publish_interval = 300;

    let mut p299 = payload_with_scale(None);
    p299.interval = 299;
    assert!(
        validate_point(&p299, &cfg).is_ok(),
        "上报周期 300s 时采集周期 299s 应通过"
    );

    let mut p300 = payload_with_scale(None);
    p300.interval = 300;
    assert!(
        validate_point(&p300, &cfg).is_err(),
        "采集周期等于上报周期仍应被拒绝"
    );
}

/// MQTT 关闭时无「上报周期」可比，长采集周期应放行（历史 60s 硬限属于误伤）。
#[test]
fn validate_point_allows_long_interval_when_mqtt_disabled() {
    let cfg = GatewayConfig::default();
    let mut p600 = payload_with_scale(None);
    p600.interval = 600;
    assert!(
        validate_point(&p600, &cfg).is_ok(),
        "MQTT 未启用时不应限制采集周期上界"
    );
}

/// interval=59（边界内侧）必须通过，避免把上界误设成保守值。
#[test]
fn validate_point_accepts_interval_just_below_north_period() {
    let mut cfg = GatewayConfig::default();
    cfg.mqtt.enabled = true;
    cfg.mqtt.publish_interval = 60;
    let mut p59 = payload_with_scale(None);
    p59.interval = 59;
    assert!(validate_point(&p59, &cfg).is_ok(), "interval=59 应通过");
}

/// 功能码必须按协议分流：Modbus 只认标准功能码。
///
/// 旧实现用 Modbus 白名单校验所有协议，导致 DLT645 的写通信地址(0x0C)、
/// 写密码(0x0F) 与 CJT188 的写通信地址(0x15) 根本存不进配置。
#[test]
fn validate_point_rejects_function_code_not_allowed_by_protocol() {
    let cfg = GatewayConfig::default();

    // Modbus 配 DLT645 控制码 → 拒绝
    let mut p = payload_with_scale(None);
    p.function_code = Some(0x0C);
    let err = validate_point(&p, &cfg).expect_err("Modbus 配 0x0C 应被拒绝");
    assert!(
        err.message().contains("功能码"),
        "错误信息应指向功能码，实际: {}",
        err.message()
    );

    // DLT645 写通信地址(0x0C) → 放行
    let mut p = payload_with_scale(None);
    p.protocol = "dlt645".into();
    p.function_code = Some(0x0C);
    assert!(
        validate_point(&p, &cfg).is_ok(),
        "DLT645 写通信地址(0x0C) 应通过"
    );

    // DLT645 写密码(0x0F) → 放行
    let mut p = payload_with_scale(None);
    p.protocol = "dlt645".into();
    p.function_code = Some(0x0F);
    assert!(
        validate_point(&p, &cfg).is_ok(),
        "DLT645 写密码(0x0F) 应通过"
    );

    // CJT188 写通信地址(0x15) → 放行
    let mut p = payload_with_scale(None);
    p.protocol = "cjt188".into();
    p.function_code = Some(0x15);
    assert!(
        validate_point(&p, &cfg).is_ok(),
        "CJT188 写通信地址(0x15) 应通过"
    );

    // Modbus 常规功能码 → 放行
    let mut p = payload_with_scale(None);
    p.function_code = Some(3);
    assert!(validate_point(&p, &cfg).is_ok(), "Modbus FC03 应通过");
}

fn trash_entry(id: &str, deleted_at_ms: i64) -> PointTrashEntry {
    PointTrashEntry {
        point: PointConfig {
            id: id.to_string(),
            interval: 10,
            ..Default::default()
        },
        deleted_at_ms,
    }
}

/// 容量上限：超过 200 条时只保留删除时间最新的 200 条。
#[test]
fn push_trash_keeps_newest_entries_within_capacity() {
    let mut cfg = GatewayConfig::default();
    let base = 1_700_000_000_000i64;
    for i in 0..(POINT_TRASH_MAX + 1) {
        // 全部落在 TTL 窗口内，只触发容量裁剪
        push_trash(&mut cfg, trash_entry(&format!("p{i}"), base + i as i64));
    }
    let trash = cfg.point_trash.as_ref().expect("回收站应已创建");
    assert_eq!(trash.len(), POINT_TRASH_MAX, "回收站不得超过容量上限");
    let ids: Vec<&str> = trash.iter().map(|e| e.point.id.as_str()).collect();
    assert!(
        !ids.contains(&"p0"),
        "最旧的条目应被容量裁剪丢弃, got {ids:?}"
    );
    assert!(
        ids.contains(&format!("p{}", POINT_TRASH_MAX).as_str()),
        "最新条目必须保留"
    );
}

/// TTL 清理：写入时顺带丢弃超过 30 天的历史条目。
#[test]
fn push_trash_drops_entries_older_than_ttl() {
    let mut cfg = GatewayConfig::default();
    let now = 1_700_000_000_000i64;
    cfg.point_trash = Some(vec![
        trash_entry("stale", now - POINT_TRASH_TTL_MS - 1),
        trash_entry("fresh", now - 1),
    ]);
    push_trash(&mut cfg, trash_entry("newest", now));
    let ids: Vec<&str> = cfg
        .point_trash
        .as_ref()
        .expect("回收站应已创建")
        .iter()
        .map(|e| e.point.id.as_str())
        .collect();
    assert!(!ids.contains(&"stale"), "超过 30 天的条目应被清理");
    assert!(ids.contains(&"fresh") && ids.contains(&"newest"));
}
