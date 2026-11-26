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

// ============================================================================
// Bug 169：北向枚举标签口径与 pipeline 转换结果对齐
// ============================================================================

/// 回归（Bug 169 主症状）：报文必须优先使用**入队时落库**的标签。
///
/// 原实现每次发送都按当前规则集重算，于是 enum 映射热重载后（`1 → 运行` 改成
/// `1 → 运转`），队列里入队时的旧样本会被追溯改标签 —— 同一条采样的 MQTT 报文
/// 与历史查询（读 `data_points.value_text`）标签不一致；规则被删则标签直接丢失。
#[test]
fn test_default_payload_prefers_persisted_value_text() {
    let engine = make_enum_engine(); // 当前规则：1 → 运行
    let mut item = make_item("dev1", "p1", 1.0);
    item.value_text = Some("运转".into()); // 入队时按当时的规则落下的标签

    let s = String::from_utf8(build_payload_default(&[item], Some(&*engine))).unwrap();
    assert!(
        s.contains("\"value_text\":\"运转\""),
        "报文应沿用入队时落库的标签（运转），不得按当前规则重算成「运行」: {}",
        s
    );
    assert!(
        !s.contains("\"运行\""),
        "不得用当前规则覆盖已落库标签: {}",
        s
    );
}

/// 兜底路径：`value_text` 为空（批次 C 之前落库的历史行，列为 NULL）时，
/// 仍按当前规则集回查补标签，老数据不丢文本。
#[test]
fn test_default_payload_falls_back_to_live_lookup_when_value_text_absent() {
    let engine = make_enum_engine();
    let item = make_item("dev1", "p1", 1.0); // value_text = None
    let s = String::from_utf8(build_payload_default(&[item], Some(&*engine))).unwrap();
    assert!(
        s.contains("\"value_text\":\"运行\""),
        "value_text 缺失时应回查规则补标签: {}",
        s
    );

    // 空串按「缺失」处理，避免报文出现 "value_text":"" 的空键
    let mut blank = make_item("dev1", "p1", 1.0);
    blank.value_text = Some(String::new());
    let s = String::from_utf8(build_payload_default(&[blank], Some(&*engine))).unwrap();
    assert!(
        s.contains("\"value_text\":\"运行\""),
        "空串标签应回退到规则回查，而非输出空键: {}",
        s
    );
}

/// 模板路径与默认 JSON 路径必须同口径（都优先持久化标签）。
#[test]
fn test_template_value_text_prefers_persisted() {
    let engine = make_enum_engine();
    let mut item = make_item("dev1", "p1", 1.0);
    item.value_text = Some("运转".into());

    let vars = template_vars_from_item(&item, None, Some(&*engine));
    assert_eq!(
        vars.value_text.as_deref(),
        Some("运转"),
        "模板变量应与默认 JSON 路径一致，优先使用已落库标签"
    );
}

/// 回归（Bug 169 根因）：报文标签必须与 `ConvertEngine::apply` 的产物一致。
///
/// 「线性 + 枚举」串联时，apply 用**转换后**的 `dp.value` 查 enum_map（1.0 → 10.0
/// → key "10" 未命中 → 无标签）；原北向实现用 `raw_value`（1.0 → key "1" → 命中
/// 「运行」）。同一采样在实时值/历史表里没有标签，报文里却有 —— 口径分裂。
/// 修复后两路都取 `value`，结果一致（都无标签）。
#[test]
fn test_payload_label_matches_convert_apply_output() {
    use crate::config::{ConvertEnumItemConfig, ConvertRuleConfig};
    use crate::engine::model::DataPoint;
    use crate::queue::QueuedItem;

    let linear_then_enum = vec![
        ConvertRuleConfig {
            id: "cv-linear".into(),
            name: "cv-linear".into(),
            source_proto: "modbus-tcp".into(),
            source_point_id: "p1".into(),
            source_point_name: String::new(),
            target_proto: "mqtt".into(),
            target_tag: String::new(),
            transform: "linear".into(),
            scale: Some(10.0),
            offset: Some(0.0),
            enum_map: None,
            enabled: true,
            created_at_ms: 0,
        },
        ConvertRuleConfig {
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
            enum_map: Some(vec![ConvertEnumItemConfig {
                key: "1".into(),
                label: "运行".into(),
            }]),
            enabled: true,
            created_at_ms: 0,
        },
    ];
    let engine = crate::pipeline::ConvertEngine::new(linear_then_enum);

    // 管线侧：apply 后 value=10.0，enum 查 "10" 未命中 → 无标签
    let mut dp = DataPoint::good("p1", "dev1", 1.0, 1.0, "u16");
    engine.apply(std::slice::from_mut(&mut dp));
    assert_eq!(dp.value, 10.0, "线性规则应把 1.0 换算为 10.0");
    assert!(
        dp.value_text.is_none(),
        "apply 侧 key 是转换后的值，\"10\" 不在映射表内 → 不应产出标签"
    );

    // 北向侧：由同一 dp 构造 QueuedItem，报文标签必须与 apply 一致（同为 None）
    let item = QueuedItem::from_datapoint(&dp);
    let s = String::from_utf8(build_payload_default(&[item], Some(&*engine))).unwrap();
    assert!(
        !s.contains("value_text"),
        "报文不得因用 raw_value 回查而出现 apply 侧并不存在的标签: {}",
        s
    );
}

// ============================================================================
// I-47 / I-48：默认 JSON 报文的顺序与时间戳语义
// ============================================================================

/// I-47（第一部分）：同一 topic 分组内的点位顺序必须等于出队顺序。
///
/// 组间（按 device_id）用 HashMap 收集，组内用 Vec 顺次 push——组内顺序即
/// `dequeue` 的输出顺序。一旦有人把组内改成 HashSet / 排序，平台侧看到的
/// 点位序列就会与入队顺序不一致（纯展示层或许无感，但按序去重/对齐的上游
/// 会错位）。
#[test]
fn payload_groups_keep_dequeue_order() {
    let mut items = Vec::new();
    for i in 0..8 {
        let point_id = format!("p{}", i);
        items.push(make_item("dev1", &point_id, i as f64));
    }

    let bytes = build_payload_default(&items, None);
    let v: serde_json::Value =
        serde_json::from_slice(&bytes).expect("默认 JSON 模式报文必须是合法 JSON");
    let reports = v.as_array().expect("顶层必须是设备报文数组");
    assert_eq!(reports.len(), 1, "单一 device_id 应只产生 1 个设备分组");
    let points = reports[0]["points"]
        .as_array()
        .expect("设备分组必须含 points 数组");

    let got: Vec<&str> = points
        .iter()
        .map(|p| p["point_id"].as_str().expect("point_id 必须是字符串"))
        .collect();
    let want: Vec<String> = (0..8).map(|i| format!("p{}", i)).collect();
    let want: Vec<&str> = want.iter().map(String::as_str).collect();
    assert_eq!(
        got, want,
        "I-47：同一 topic 分组内的点位顺序必须严格等于出队顺序"
    );
}

/// I-48：报文顶层 `ts` 是**构建时刻**，采样时刻只保留在每点的 `source_ts`。
///
/// 若把顶层 ts 写成某个点位的采样时间，平台会把「补发的旧数据」当成
/// 刚刚采集的新数据（断网恢复后批量补传时尤其明显），时间序列与计费口径
/// 都会错位。这里用一个明显早于构建时刻的 source_ts 来守护该语义。
#[test]
fn payload_top_level_ts_is_build_time_not_sample_time() {
    use crate::engine::model::DataPoint;

    // 2020-09-13 的采样时刻，远早于今天的构建时刻
    const SAMPLE_TS: i64 = 1_600_000_000_000;
    let mut item = make_item("dev1", "p1", 1.0);
    item.source_ts = SAMPLE_TS;
    item.timestamp = SAMPLE_TS;

    let build_started_at = DataPoint::now();
    let bytes = build_payload_default(std::slice::from_ref(&item), None);
    let v: serde_json::Value =
        serde_json::from_slice(&bytes).expect("默认 JSON 模式报文必须是合法 JSON");

    let top_ts = v[0]["ts"].as_i64().expect("报文顶层 ts 必须是整数");
    assert!(
        top_ts >= build_started_at,
        "I-48：报文顶层 ts 必须是构建时刻（>= 构建前时刻 {}），实际 {}",
        build_started_at,
        top_ts
    );

    let point_ts = v[0]["points"][0]["source_ts"]
        .as_i64()
        .expect("点位 source_ts 必须是整数");
    assert_eq!(
        point_ts, SAMPLE_TS,
        "I-48：点的 source_ts 必须原样保留采样时刻"
    );
    assert_ne!(
        top_ts, point_ts,
        "I-48：顶层 ts 不得被写成采样时刻（两者语义不同）"
    );
    assert!(
        top_ts - point_ts > 60_000,
        "I-48：旧采样补发时顶层 ts 与 source_ts 的差值应体现真实采样间隔，实际差 {}ms",
        top_ts - point_ts
    );
}
