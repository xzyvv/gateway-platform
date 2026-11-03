use super::*;

#[test]
fn test_basic_substitution() {
    // 模板中用 \"${device_id}\" 包裹字符串场景
    let engine = TemplateEngine::new(
        "{\"ts\":${timestamp_ms},\"device\":\"${device_id}\",\"value\":${value}}",
    )
    .unwrap();

    let mut vars = TemplateVars::new();
    vars.timestamp_ms = 1727936400000;
    vars.device_id = "meter1".into();
    vars.value = 220.5;

    let out = engine.render(&vars);
    assert_eq!(
        out,
        "{\"ts\":1727936400000,\"device\":\"meter1\",\"value\":220.5}"
    );
}

#[test]
fn test_unknown_var_yields_empty() {
    let engine = TemplateEngine::new("hello ${nonexistent}!").unwrap();
    let vars = TemplateVars::new();
    let out = engine.render(&vars);
    assert_eq!(out, "hello !");
}

#[test]
fn test_string_literal_output() {
    // 字符串变量裸输出，不含 JSON 转义 —— 由模板中的 \" 负责包裹
    let engine = TemplateEngine::new("{\"x\":\"${device_id}\"}").unwrap();
    let mut vars = TemplateVars::new();
    vars.device_id = "a\"b\nc".into();
    let out = engine.render(&vars);
    // 警告：原始字符保留，JSON 不合法。需调用 render_str 单独包装。
    assert_eq!(out, "{\"x\":\"a\"b\nc\"}");
}

#[test]
fn test_render_str_helper() {
    assert_eq!(render_str("a\"b"), "\"a\\\"b\"");
    assert_eq!(render_str("hello\nworld"), "\"hello\\nworld\"");
    assert_eq!(render_str(""), "\"\"");
}

#[test]
fn test_nan_renders_null() {
    let engine = TemplateEngine::new("${value}").unwrap();
    let mut vars = TemplateVars::new();
    vars.value = f64::NAN;
    assert_eq!(engine.render(&vars), "null");
    vars.value = f64::INFINITY;
    assert_eq!(engine.render(&vars), "null");
}

#[test]
fn test_integer_value_no_decimal() {
    let engine = TemplateEngine::new("${value}").unwrap();
    let mut vars = TemplateVars::new();
    vars.value = 100.0;
    assert_eq!(engine.render(&vars), "100");
    vars.value = 100.5;
    assert_eq!(engine.render(&vars), "100.5");
}

#[test]
fn test_extra_field() {
    let engine = TemplateEngine::new("{\"line\":\"${line}\",\"v\":${value}}").unwrap();
    let vars = TemplateVars::new().with_extra("line", ExtraValue::Str("line-A".into()));
    let out = engine.render(&vars);
    assert_eq!(out, "{\"line\":\"line-A\",\"v\":0}");
}

#[test]
fn test_unclosed_template_error() {
    let err = TemplateEngine::new("hello ${abc").unwrap_err();
    assert!(matches!(err, TemplateError::UnclosedVariable { .. }));
}

#[test]
fn test_invalid_var_name_error() {
    let err = TemplateEngine::new("hello ${a-b}").unwrap_err();
    assert!(matches!(err, TemplateError::InvalidVariableName { .. }));
}

#[test]
fn test_empty_template() {
    let engine = TemplateEngine::new("").unwrap();
    let vars = TemplateVars::new();
    assert_eq!(engine.render(&vars), "");
}

#[test]
fn test_no_variables() {
    let engine = TemplateEngine::new("plain text only").unwrap();
    let vars = TemplateVars::new();
    assert_eq!(engine.render(&vars), "plain text only");
}

#[test]
fn test_dollar_literal() {
    let engine = TemplateEngine::new("price: $5").unwrap();
    let vars = TemplateVars::new();
    assert_eq!(engine.render(&vars), "price: $5");
}

#[test]
fn test_aggregation_vars() {
    let engine = TemplateEngine::new("avg=${avg},max=${max},min=${min},cnt=${count}").unwrap();
    let mut vars = TemplateVars::new();
    vars.avg = Some(220.3);
    vars.max = Some(230.1);
    vars.min = Some(215.0);
    vars.count = Some(6);
    assert_eq!(engine.render(&vars), "avg=220.3,max=230.1,min=215,cnt=6");
}

#[test]
fn test_thingsboard_payload() {
    // ThingsBoard 风格：变量直接做 JSON key
    let engine =
        TemplateEngine::new("{\"ts\":${timestamp_ms},\"values\":{\"${point_id}\":${value}}}")
            .unwrap();

    let mut vars = TemplateVars::new();
    vars.timestamp_ms = 1727936400000;
    vars.point_id = "voltage_a".into();
    vars.value = 220.5;

    assert_eq!(
        engine.render(&vars),
        "{\"ts\":1727936400000,\"values\":{\"voltage_a\":220.5}}"
    );
}

#[test]
fn test_render_bytes_matches_render() {
    let engine = TemplateEngine::new("{\"device\":\"${device_id}\",\"value\":${value}}").unwrap();
    let mut vars = TemplateVars::new();
    vars.device_id = "m1".into();
    vars.value = 12.5;
    let s = engine.render(&vars);
    let b = engine.render_bytes(&vars);
    assert_eq!(s.as_bytes(), b.as_slice());
}

#[test]
fn test_aliyun_iot_payload() {
    let engine = TemplateEngine::new(
        "{\"method\":\"thing.event.property.post\",\"params\":{\"${point_id}\":${value}}}",
    )
    .unwrap();

    let mut vars = TemplateVars::new();
    vars.point_id = "temperature".into();
    vars.value = 25.7;

    let out = engine.render(&vars);
    assert_eq!(
        out,
        "{\"method\":\"thing.event.property.post\",\"params\":{\"temperature\":25.7}}"
    );
}

#[test]
fn test_influxdb_line_protocol() {
    // InfluxDB 不需要引号包裹 tag/field value（除非含特殊字符）
    let engine = TemplateEngine::new(
        "gateway_measurement,device=${device_id},point=${point_id} value=${value} ${timestamp_ms}",
    )
    .unwrap();

    let mut vars = TemplateVars::new();
    vars.device_id = "m1".into();
    vars.point_id = "voltage_a".into();
    vars.value = 220.5;
    vars.timestamp_ms = 1727936400000;

    let out = engine.render(&vars);
    assert_eq!(
        out,
        "gateway_measurement,device=m1,point=voltage_a value=220.5 1727936400000"
    );
}

#[test]
fn test_extra_num_field() {
    let engine = TemplateEngine::new("v=${port}").unwrap();
    let vars = TemplateVars::new().with_extra("port", ExtraValue::Num(502.0));
    assert_eq!(vars.render_with_engine(&engine), "v=502");
}

// ============================================================
// R3: 未解析变量诊断 API
// ============================================================

#[test]
fn test_unresolved_vars_reports_missing_and_dedups() {
    // device_id 已设置；avg 未设置（None）；typo_var 为未知变量（拼写错误）
    let engine = TemplateEngine::new("${device_id}|${avg}|${typo_var}|${avg}").unwrap();
    let mut vars = TemplateVars::new();
    vars.device_id = "m1".into();
    let missing = engine.unresolved_vars(&vars);
    assert_eq!(missing, vec!["avg".to_string(), "typo_var".to_string()]);
}

#[test]
fn test_unresolved_vars_empty_when_all_present() {
    let engine = TemplateEngine::new("${device_id}:${value}").unwrap();
    let mut vars = TemplateVars::new();
    vars.device_id = "m1".into();
    vars.value = 1.0;
    assert!(engine.unresolved_vars(&vars).is_empty());
}

#[test]
fn test_render_checked_matches_render_and_reports_missing() {
    let engine = TemplateEngine::new("{\"d\":\"${device_id}\",\"a\":${avg}}").unwrap();
    let mut vars = TemplateVars::new();
    vars.device_id = "m1".into();
    let (out, missing) = engine.render_checked(&vars);
    // 输出与 render 完全一致（未解析变量仍渲染为空串）
    assert_eq!(out, engine.render(&vars));
    assert_eq!(out, "{\"d\":\"m1\",\"a\":}");
    assert_eq!(missing, vec!["avg".to_string()]);
}

// ============================================================
// R35: 解析/注册表资源上限
// ============================================================

#[test]
fn test_template_text_too_large_rejected() {
    // 模板文本超过 64KB 上限时拒绝加载（r35-TPL2）
    let s = "a".repeat(64 * 1024 + 1);
    let err = TemplateEngine::new(&s).unwrap_err();
    match err {
        TemplateError::TemplateTooLarge { size } => assert_eq!(size, 64 * 1024 + 1),
        other => panic!("expected TemplateTooLarge, got {:?}", other),
    }
}

#[test]
fn test_template_text_at_limit_accepted() {
    // 恰好等于上限（64KB 字面量）应正常解析
    let s = "a".repeat(64 * 1024);
    assert!(TemplateEngine::new(&s).is_ok());
}

#[test]
fn test_variable_name_too_long_rejected() {
    // 变量名超过 64 字符上限时拒绝（r35-TPL1）
    let long_name = "a".repeat(65);
    let tmpl = format!("${{{}}}", long_name);
    let err = TemplateEngine::new(&tmpl).unwrap_err();
    match err {
        TemplateError::VariableNameTooLong { name, .. } => assert_eq!(name.len(), 65),
        other => panic!("expected VariableNameTooLong, got {:?}", other),
    }
}

#[test]
fn test_variable_name_at_limit_accepted() {
    // 恰好 64 字符的变量名应可解析（并可正常渲染）
    let name = "a".repeat(64);
    let tmpl = format!("${{{}}}", name);
    let engine = TemplateEngine::new(&tmpl).expect("64-char var name should parse");
    let mut vars = TemplateVars::new();
    vars.extra.insert(name, ExtraValue::Str("ok".into()));
    assert_eq!(engine.render(&vars), "ok");
}

#[test]
fn test_extra_entries_over_limit_ignored() {
    // 扩展项达到上限后，新增键被拒绝但已有键可继续更新（r35-TPL3）
    let mut vars = TemplateVars::new();
    for i in 0..MAX_EXTRA_ENTRIES {
        vars = vars.with_extra(format!("k{}", i), ExtraValue::Num(i as f64));
    }
    assert_eq!(vars.extra.len(), MAX_EXTRA_ENTRIES);

    // 超限新增被拒绝
    vars = vars.with_extra("overflow", ExtraValue::Num(1.0));
    assert_eq!(vars.extra.len(), MAX_EXTRA_ENTRIES);
    assert!(!vars.extra.contains_key("overflow"));

    // 已有键仍可更新
    vars = vars.with_extra("k0", ExtraValue::Num(999.0));
    assert_eq!(vars.extra.len(), MAX_EXTRA_ENTRIES);
    assert!(matches!(vars.extra.get("k0"), Some(ExtraValue::Num(n)) if *n == 999.0));
}

#[test]
fn test_render_str_escapes_bidi_and_line_separators() {
    // r36-TPL4: bidi 控制符 / 行分隔符必须转义，防 Trojan Source 式视觉欺骗与
    // U+2028/2029 内嵌 JS 断行。普通可打印字符不受影响。
    assert_eq!(render_str("\u{202e}abc"), "\"\\u202eabc\"");
    assert_eq!(render_str("a\u{2066}b"), "\"a\\u2066b\"");
    assert_eq!(render_str("x\u{2028}y"), "\"x\\u2028y\"");
    assert_eq!(render_str("x\u{2029}y"), "\"x\\u2029y\"");
    assert_eq!(render_str("\u{200e}"), "\"\\u200e\"");
    // 回归：既有转义行为不变
    assert_eq!(render_str("a\"b"), "\"a\\\"b\"");
    assert_eq!(render_str("中文"), "\"中文\"");
}
