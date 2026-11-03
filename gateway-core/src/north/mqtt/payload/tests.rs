use super::*;

fn make_item(device_id: &str, point_id: &str, value: f64) -> QueuedItem {
    QueuedItem {
        id: 1,
        point_id: point_id.into(),
        device_id: device_id.into(),
        value,
        raw_value: value,
        raw_type: "u16".into(),
        quality: "good".into(),
        quality_reason: "".into(),
        timestamp: 1727936400000,
        source_ts: 1727936400000,
        stale_count: 0,
        value_text: None,
    }
}

/// 测试 template_vars_from_item — 不构造 MqttClient，直接测核心数据流
#[test]
fn test_template_vars_basic() {
    let item = make_item("dev1", "p1", 220.5);
    let vars = template_vars_from_item(&item, None, None);
    assert_eq!(vars.device_id, "dev1");
    assert_eq!(vars.point_id, "p1");
    assert_eq!(vars.value, 220.5);
    assert_eq!(vars.quality_code, 0); // good
    assert_eq!(vars.timestamp_ms, 1727936400000);
    assert!(!vars.timestamp_iso.is_empty());
}

#[test]
fn test_template_vars_quality_codes() {
    for (q, code) in [("good", 0), ("uncertain", 1), ("bad", 2), ("stale", 3)] {
        let mut item = make_item("d", "p", 0.0);
        item.quality = q.into();
        let vars = template_vars_from_item(&item, None, None);
        assert_eq!(vars.quality_code, code, "quality={}", q);
    }
}

#[test]
fn test_template_vars_overrides_from_point_config() {
    let item = make_item("raw_dev_id", "p1", 1.0);
    let pc = PointConfig {
        id: "p1".into(),
        name: Some("A相电压".into()),
        device_id: Some("meter1".into()),
        bus: "com1".into(),
        protocol: "modbus".into(),
        address: "1".into(),
        interval: 1,
        unit: Some("V".into()),
        ..Default::default()
    };
    let vars = template_vars_from_item(&item, Some(&pc), None);
    // PointConfig 覆盖了 raw device_id
    assert_eq!(vars.device_id, "meter1");
    assert_eq!(vars.point_name, "A相电压");
    assert_eq!(vars.unit, "V");
}

/// 测试 build_payload_default（默认 JSON 模式）
#[test]
fn test_build_payload_default_groups_by_device() {
    let items = vec![make_item("dev1", "p1", 1.0), make_item("dev1", "p2", 2.0)];
    let bytes = build_payload_default(&items, None);
    let s = String::from_utf8(bytes).unwrap();
    // 顶层为数组
    assert!(s.starts_with('['), "got: {}", s);
    // 嵌套设备ID
    assert!(s.contains("\"device_id\":\"dev1\""));
    // 两个点位
    assert!(s.contains("\"point_id\":\"p1\""));
    assert!(s.contains("\"point_id\":\"p2\""));
}

/// 测试模板渲染路径：直接渲染（不通过 MqttClient）— 验证 JSON Lines 行为
#[test]
fn test_template_rendering_json_lines() {
    use crate::north::template::TemplateEngine;

    let template =
        r#"{"ts":${timestamp_ms},"device":"${device_id}","point":"${point_id}","value":${value}}"#;
    let engine = TemplateEngine::new(template).unwrap();

    let items = vec![
        make_item("m1", "voltage_a", 220.5),
        make_item("m1", "voltage_b", 221.3),
        make_item("m2", "current_a", 5.2),
    ];

    // 模拟 MqttClient::build_payload_with_template 的渲染循环
    let mut rendered = Vec::new();
    for item in &items {
        let vars = template_vars_from_item(item, None, None);
        let line = engine.render(&vars);
        rendered.push(line);
    }
    let joined = rendered.join("\n");

    let lines: Vec<&str> = joined.split('\n').collect();
    assert_eq!(
        lines.len(),
        3,
        "expected 3 lines, got {}: {}",
        lines.len(),
        joined
    );
    assert!(lines[0].contains("\"device\":\"m1\""));
    assert!(lines[0].contains("\"point\":\"voltage_a\""));
    assert!(lines[0].contains("\"value\":220.5"));
    assert!(lines[1].contains("\"point\":\"voltage_b\""));
    assert!(lines[2].contains("\"device\":\"m2\""));
    // 没有数组括号（与默认 JSON 不同）
    assert!(!joined.starts_with('['));
}

/// 测试模板渲染空字符串场景
#[test]
fn test_template_empty_rendering_skipped() {
    use crate::north::template::TemplateEngine;

    // 模板只用变量，变量全空 → 渲染为空字符串 → 应跳过
    let engine = TemplateEngine::new("${nothing}").unwrap();
    let vars = template_vars_from_item(&make_item("d", "p", 0.0), None, None);
    let line = engine.render(&vars);
    assert!(line.is_empty(), "expected empty line, got: {:?}", line);
}

// ============================================================================
// 批次 B：enum 标签（value_text）在 payload 构建期回查补回
// ============================================================================

/// 构造含一条 enum 规则的转换引擎：p1 的 0→停机 / 1→运行
fn make_enum_engine() -> std::sync::Arc<crate::pipeline::ConvertEngine> {
    use crate::config::{ConvertEnumItemConfig, ConvertRuleConfig};
    let rule = ConvertRuleConfig {
        id: "cv-enum".into(),
        name: "cv-enum".into(),
        source_proto: "modbus-tcp".into(),
        source_point_id: "p1".into(),
        source_point_name: String::new(),
        target_proto: "mqtt".into(),
        target_tag: String::new(),
        transform: "enum".into(),
        scale: None,
        offset: None,
        enum_map: Some(vec![
            ConvertEnumItemConfig {
                key: "0".into(),
                label: "停机".into(),
            },
            ConvertEnumItemConfig {
                key: "1".into(),
                label: "运行".into(),
            },
        ]),
        enabled: true,
        created_at_ms: 0,
    };
    crate::pipeline::ConvertEngine::new(vec![rule])
}

/// 默认 JSON：raw_value 命中 enum 映射时 payload 必须带 value_text
#[test]
fn test_default_payload_includes_value_text_on_enum_hit() {
    let engine = make_enum_engine();
    let items = vec![make_item("dev1", "p1", 1.0)];
    let bytes = build_payload_default(&items, Some(&*engine));
    let s = String::from_utf8(bytes).unwrap();
    assert!(
        s.contains("\"value_text\":\"运行\""),
        "enum 命中应出现 value_text=运行, got: {}",
        s
    );
}

/// 默认 JSON：未命中映射 / 未注入引擎时 value_text 键不得出现（schema 不变）
#[test]
fn test_default_payload_omits_value_text_when_no_match_or_no_engine() {
    let engine = make_enum_engine();
    // raw_value=9.0 不在映射表
    let items = vec![make_item("dev1", "p1", 9.0)];
    let s = String::from_utf8(build_payload_default(&items, Some(&*engine))).unwrap();
    assert!(
        !s.contains("value_text"),
        "未命中不应出现 value_text: {}",
        s
    );

    // point_id 无规则
    let items = vec![make_item("dev1", "p_other", 1.0)];
    let s = String::from_utf8(build_payload_default(&items, Some(&*engine))).unwrap();
    assert!(
        !s.contains("value_text"),
        "无规则点位不应出现 value_text: {}",
        s
    );

    // 未注入引擎（None）
    let items = vec![make_item("dev1", "p1", 1.0)];
    let s = String::from_utf8(build_payload_default(&items, None)).unwrap();
    assert!(
        !s.contains("value_text"),
        "未注入引擎不应出现 value_text: {}",
        s
    );
}

/// 模板路径：value_text 变量渲染标签；未命中渲染为空串
#[test]
fn test_template_value_text_variable() {
    use crate::north::template::TemplateEngine;

    let engine = make_enum_engine();
    let tpl = TemplateEngine::new(r#"{"text":"${value_text}"}"#).unwrap();

    // 命中
    let item = make_item("dev1", "p1", 1.0);
    let vars = template_vars_from_item(&item, None, Some(&*engine));
    assert_eq!(vars.value_text.as_deref(), Some("运行"));
    let (line, missing) = tpl.render_checked(&vars);
    assert_eq!(line, r#"{"text":"运行"}"#);
    assert!(missing.is_empty());

    // 未命中：渲染为空串（与 avg/max 等 Option 字段同风格，计入未解析变量）
    let item = make_item("dev1", "p1", 9.0);
    let vars = template_vars_from_item(&item, None, Some(&*engine));
    assert!(vars.value_text.is_none());
    let (line, missing) = tpl.render_checked(&vars);
    assert_eq!(line, r#"{"text":""}"#);
    assert_eq!(missing, vec!["value_text".to_string()]);
}
