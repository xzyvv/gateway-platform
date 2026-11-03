use super::*;
use crate::web::services::{ExtraValue, TemplateVars};

#[test]
fn test_build_preview_vars_basic() {
    let mut raw = std::collections::HashMap::new();
    raw.insert("device_id".into(), serde_json::json!("meter1"));
    raw.insert("point_id".into(), serde_json::json!("voltage_a"));
    raw.insert("value".into(), serde_json::json!(220.5));
    raw.insert("quality_text".into(), serde_json::json!("good"));
    let vars = build_preview_vars(&raw);
    assert_eq!(vars.device_id, "meter1");
    assert_eq!(vars.point_id, "voltage_a");
    assert_eq!(vars.value, 220.5);
    assert_eq!(vars.quality_text, "good");
}

#[test]
fn test_build_preview_vars_unknown_field_goes_extra() {
    let mut raw = std::collections::HashMap::new();
    raw.insert("custom_field".into(), serde_json::json!("hello"));
    raw.insert("port".into(), serde_json::json!(502));
    let vars = build_preview_vars(&raw);
    assert!(vars.extra.contains_key("custom_field"));
    assert!(vars.extra.contains_key("port"));
}

#[test]
fn test_build_preview_vars_quality_code() {
    let mut raw = std::collections::HashMap::new();
    raw.insert("quality_code".into(), serde_json::json!(2));
    let vars = build_preview_vars(&raw);
    assert_eq!(vars.quality_code, 2);
}

#[test]
fn test_default_encoding_and_max_bytes() {
    assert_eq!(default_encoding(), "json_lines");
    assert_eq!(default_max_bytes(), 4096);
}

#[test]
fn test_default_history_limit() {
    assert_eq!(default_history_limit(), 50);
}

#[test]
fn test_apply_value_aggregation_fields() {
    let mut vars = TemplateVars::new();
    for (field, val) in [
        ("avg", serde_json::json!(10.0)),
        ("max", serde_json::json!(20.0)),
        ("min", serde_json::json!(5.0)),
        ("last", serde_json::json!(15.0)),
        ("sum", serde_json::json!(100.0)),
        ("count", serde_json::json!(6)),
    ] {
        apply_value(&mut vars, field, &val);
    }
    assert_eq!(vars.avg, Some(10.0));
    assert_eq!(vars.max, Some(20.0));
    assert_eq!(vars.min, Some(5.0));
    assert_eq!(vars.last, Some(15.0));
    assert_eq!(vars.sum, Some(100.0));
    assert_eq!(vars.count, Some(6));
}

#[test]
fn test_apply_value_bool_extra() {
    let mut vars = TemplateVars::new();
    apply_value(&mut vars, "enabled", &serde_json::json!(true));
    match vars.extra.get("enabled") {
        Some(ExtraValue::Bool(b)) => assert!(*b),
        _ => panic!("expected bool extra"),
    }
}

#[test]
fn test_import_request_to_save_preserves_fields() {
    let req = ImportTemplateRequest {
        template: "{\"ts\":${timestamp_ms}}".into(),
        enabled: true,
    };
    let save_req = import_request_to_save(req);
    assert_eq!(save_req.template, "{\"ts\":${timestamp_ms}}");
    assert!(save_req.enabled);
    assert_eq!(save_req.encoding, "json_lines");
    assert_eq!(save_req.max_bytes, 4096);
}

#[test]
fn test_import_request_to_save_disabled() {
    let req = ImportTemplateRequest {
        template: String::new(),
        enabled: false,
    };
    let save_req = import_request_to_save(req);
    assert!(!save_req.enabled);
    assert_eq!(save_req.encoding, "json_lines");
    assert_eq!(save_req.max_bytes, 4096);
}

#[test]
fn test_import_request_to_save_long_template() {
    let big = "x".repeat(5000);
    let req = ImportTemplateRequest {
        template: big.clone(),
        enabled: true,
    };
    let save_req = import_request_to_save(req);
    assert_eq!(save_req.template.len(), 5000);
}

#[test]
fn test_clamp_history_limit_below_cap() {
    assert_eq!(clamp_history_limit(0), 0);
    assert_eq!(clamp_history_limit(1), 1);
    assert_eq!(clamp_history_limit(100), 100);
}

#[test]
fn test_clamp_history_limit_at_cap() {
    assert_eq!(clamp_history_limit(200), 200);
}

#[test]
fn test_clamp_history_limit_above_cap() {
    assert_eq!(clamp_history_limit(201), 200);
    assert_eq!(clamp_history_limit(10000), 200);
    assert_eq!(clamp_history_limit(usize::MAX), 200);
}
