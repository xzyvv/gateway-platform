use super::*;
use serde_json::json;
use tempfile::tempdir;

/// 无 include 字段的配置 → 直接返回解析结果
#[test]
fn no_include_returns_parsed_value() {
    let dir = tempdir().expect("should succeed");
    let path = dir.path().join("config.json");
    fs::write(
        &path,
        r#"{"buses": [], "points": [], "mqtt": {"enabled": false}}"#,
    )
    .expect("should succeed");

    let value = expand_includes(&path).expect("should succeed");
    assert_eq!(value["mqtt"]["enabled"], json!(false));
    assert!(value.get("include").is_none());
}

/// include 一个文件 → 合并数组字段
#[test]
fn include_merges_arrays() {
    let dir = tempdir().expect("should succeed");
    let main = dir.path().join("config.json");
    let shard = dir.path().join("buses.json");

    fs::write(
        &shard,
        r#"{"buses": [{"port_name": "COM2", "baud_rate": 9600}]}"#,
    )
    .expect("should succeed");
    fs::write(
        &main,
        r#"{"include": ["buses.json"], "buses": [{"port_name": "COM1", "baud_rate": 9600}], "points": [], "mqtt": {"enabled": false}}"#,
    )
    .expect("should succeed");

    let value = expand_includes(&main).expect("should succeed");
    let buses = value["buses"].as_array().expect("buses should be array");
    assert_eq!(buses.len(), 2, "buses should be concatenated");
    assert_eq!(buses[0]["port_name"], "COM1");
    assert_eq!(buses[1]["port_name"], "COM2");
}

/// include 深合并对象字段
#[test]
fn include_deep_merges_objects() {
    let dir = tempdir().expect("should succeed");
    let main = dir.path().join("config.json");
    let shard = dir.path().join("mqtt.json");

    fs::write(
        &shard,
        r#"{"mqtt": {"broker": "mqtt://shard-broker:1883"}}"#,
    )
    .expect("should succeed");
    fs::write(
        &main,
        r#"{"include": ["mqtt.json"], "buses": [], "points": [], "mqtt": {"enabled": true, "qos": 1}}"#,
    )
    .expect("should succeed");

    let value = expand_includes(&main).expect("should succeed");
    assert_eq!(value["mqtt"]["broker"], "mqtt://shard-broker:1883");
    assert_eq!(value["mqtt"]["enabled"], true);
    assert_eq!(value["mqtt"]["qos"], 1);
}

/// include 标量字段覆盖
#[test]
fn include_overrides_scalars() {
    let dir = tempdir().expect("should succeed");
    let main = dir.path().join("config.json");
    let shard = dir.path().join("override.json");

    fs::write(&shard, r#"{"schema_version": 99}"#).expect("should succeed");
    fs::write(
        &main,
        r#"{"include": ["override.json"], "schema_version": 1, "buses": [], "points": [], "mqtt": {"enabled": false}}"#,
    )
    .expect("should succeed");

    let value = expand_includes(&main).expect("should succeed");
    assert_eq!(value["schema_version"], 99);
}

/// include 字段从结果中移除
#[test]
fn include_field_removed() {
    let dir = tempdir().expect("should succeed");
    let main = dir.path().join("config.json");
    fs::write(
        &main,
        r#"{"include": [], "buses": [], "points": [], "mqtt": {"enabled": false}}"#,
    )
    .expect("should succeed");

    let value = expand_includes(&main).expect("should succeed");
    assert!(
        value.get("include").is_none(),
        "include field should be removed"
    );
}

/// 循环引用检测
#[test]
fn circular_include_detected() {
    let dir = tempdir().expect("should succeed");
    let main = dir.path().join("config.json");
    let shard = dir.path().join("shard.json");

    fs::write(
        &shard,
        format!(
            r#"{{"include": ["{}"], "buses": []}}"#,
            main.file_name().unwrap().to_string_lossy()
        ),
    )
    .expect("should succeed");
    fs::write(
        &main,
        r#"{"include": ["shard.json"], "buses": [], "points": [], "mqtt": {"enabled": false}}"#,
    )
    .expect("should succeed");

    let err = expand_includes(&main).expect_err("circular include should fail");
    assert!(
        err.to_string().contains("circular"),
        "should detect circular reference: {err}"
    );
}

/// 自引用检测（文件 include 自己）
#[test]
fn self_include_detected() {
    let dir = tempdir().expect("should succeed");
    let main = dir.path().join("config.json");
    fs::write(
        &main,
        r#"{"include": ["config.json"], "buses": [], "points": [], "mqtt": {"enabled": false}}"#,
    )
    .expect("should succeed");

    let err = expand_includes(&main).expect_err("self include should fail");
    assert!(
        err.to_string().contains("circular"),
        "should detect self reference: {err}"
    );
}

/// include 文件不存在（非 glob）→ 报错
#[test]
fn missing_include_file_errors() {
    let dir = tempdir().expect("should succeed");
    let main = dir.path().join("config.json");
    fs::write(
        &main,
        r#"{"include": ["nonexistent.json"], "buses": [], "points": [], "mqtt": {"enabled": false}}"#,
    )
    .expect("should succeed");

    let err = expand_includes(&main).expect_err("missing include should fail");
    assert!(
        err.to_string().contains("not found"),
        "should report file not found: {err}"
    );
}

/// glob 匹配多个文件
#[test]
fn glob_matches_multiple_files() {
    let dir = tempdir().expect("should succeed");
    let main = dir.path().join("config.json");
    let points_dir = dir.path().join("points");
    fs::create_dir(&points_dir).expect("should succeed");

    fs::write(
        points_dir.join("01.json"),
        r#"{"points": [{"id": "p1", "interval": 10}]}"#,
    )
    .expect("should succeed");
    fs::write(
        points_dir.join("02.json"),
        r#"{"points": [{"id": "p2", "interval": 10}]}"#,
    )
    .expect("should succeed");
    fs::write(points_dir.join("README.txt"), "not a config").expect("should succeed");

    fs::write(
        &main,
        r#"{"include": ["points/*.json"], "buses": [], "points": [], "mqtt": {"enabled": false}}"#,
    )
    .expect("should succeed");

    let value = expand_includes(&main).expect("should succeed");
    let points = value["points"].as_array().expect("points should be array");
    assert_eq!(points.len(), 2, "should match 2 json files, not txt");
    assert_eq!(points[0]["id"], "p1");
    assert_eq!(points[1]["id"], "p2");
}

/// glob 无匹配 → 不报错（允许空目录）
#[test]
fn glob_no_match_is_ok() {
    let dir = tempdir().expect("should succeed");
    let main = dir.path().join("config.json");
    fs::write(
        &main,
        r#"{"include": ["empty/*.json"], "buses": [], "points": [], "mqtt": {"enabled": false}}"#,
    )
    .expect("should succeed");

    let value = expand_includes(&main).expect("should succeed");
    assert_eq!(value["buses"].as_array().unwrap().len(), 0);
}

/// 嵌套 include：A includes B, B includes C
#[test]
fn nested_includes() {
    let dir = tempdir().expect("should succeed");
    let main = dir.path().join("config.json");
    let mid = dir.path().join("mid.json");
    let leaf = dir.path().join("leaf.json");

    fs::write(&leaf, r#"{"points": [{"id": "leaf", "interval": 10}]}"#).expect("should succeed");
    fs::write(
        &mid,
        r#"{"include": ["leaf.json"], "points": [{"id": "mid", "interval": 10}]}"#,
    )
    .expect("should succeed");
    fs::write(
        &main,
        r#"{"include": ["mid.json"], "buses": [], "points": [{"id": "main", "interval": 10}], "mqtt": {"enabled": false}}"#,
    )
    .expect("should succeed");

    let value = expand_includes(&main).expect("should succeed");
    let points = value["points"].as_array().expect("points should be array");
    assert_eq!(points.len(), 3, "main + mid + leaf points");
    assert_eq!(points[0]["id"], "main");
    assert_eq!(points[1]["id"], "mid");
    assert_eq!(points[2]["id"], "leaf");
}

/// include 路径相对于 include 所在文件解析
#[test]
fn include_paths_relative_to_file() {
    let dir = tempdir().expect("should succeed");
    let sub = dir.path().join("sub");
    fs::create_dir(&sub).expect("should succeed");

    let main = dir.path().join("config.json");
    let shard = sub.join("buses.json");

    fs::write(
        &shard,
        r#"{"buses": [{"port_name": "COM2", "baud_rate": 9600}]}"#,
    )
    .expect("should succeed");
    fs::write(
        &main,
        r#"{"include": ["sub/buses.json"], "buses": [{"port_name": "COM1", "baud_rate": 9600}], "points": [], "mqtt": {"enabled": false}}"#,
    )
    .expect("should succeed");

    let value = expand_includes(&main).expect("should succeed");
    let buses = value["buses"].as_array().expect("buses should be array");
    assert_eq!(buses.len(), 2);
    assert_eq!(buses[1]["port_name"], "COM2");
}

// ===== merge_values 单元测试 =====

#[test]
fn merge_objects_deep() {
    let base = json!({"mqtt": {"broker": "a", "qos": 1}});
    let overlay = json!({"mqtt": {"broker": "b", "enabled": true}});
    let result = merge_values(base, overlay);
    assert_eq!(result["mqtt"]["broker"], "b");
    assert_eq!(result["mqtt"]["qos"], 1);
    assert_eq!(result["mqtt"]["enabled"], true);
}

#[test]
fn merge_arrays_concatenate() {
    let base = json!([1, 2, 3]);
    let overlay = json!([4, 5]);
    let result = merge_values(base, overlay);
    assert_eq!(result, json!([1, 2, 3, 4, 5]));
}

#[test]
fn merge_scalar_overlay_wins() {
    let base = json!(42);
    let overlay = json!("hello");
    let result = merge_values(base, overlay);
    assert_eq!(result, "hello");
}

#[test]
fn merge_mixed_type_overlay_wins() {
    let base = json!([1, 2]);
    let overlay = json!({"key": "val"});
    let result = merge_values(base, overlay);
    assert_eq!(result["key"], "val");
}

// ===== glob 匹配单元测试 =====

#[test]
fn glob_match_star_json() {
    assert!(simple_glob_match("*.json", "buses.json"));
    assert!(simple_glob_match("*.json", "a.json"));
    assert!(!simple_glob_match("*.json", "buses.txt"));
    assert!(!simple_glob_match("*.json", "busesjson"));
}

#[test]
fn glob_match_prefix_wildcard() {
    assert!(simple_glob_match("buses_*.json", "buses_01.json"));
    assert!(simple_glob_match("buses_*.json", "buses_abc.json"));
    assert!(!simple_glob_match("buses_*.json", "points_01.json"));
}

#[test]
fn glob_match_exact() {
    assert!(simple_glob_match("config.json", "config.json"));
    assert!(!simple_glob_match("config.json", "other.json"));
}

#[test]
fn glob_match_star_only() {
    assert!(simple_glob_match("*", "anything.json"));
    assert!(simple_glob_match("*", "noext"));
    // * matches zero or more chars, so it matches empty string
    assert!(simple_glob_match("*", ""));
}

/// 多个 include 文件按顺序合并
#[test]
fn multiple_includes_in_order() {
    let dir = tempdir().expect("should succeed");
    let main = dir.path().join("config.json");
    let shard1 = dir.path().join("s1.json");
    let shard2 = dir.path().join("s2.json");

    fs::write(&shard1, r#"{"points": [{"id": "p1", "interval": 10}]}"#).expect("should succeed");
    fs::write(&shard2, r#"{"points": [{"id": "p2", "interval": 10}]}"#).expect("should succeed");
    fs::write(
        &main,
        r#"{"include": ["s1.json", "s2.json"], "buses": [], "points": [], "mqtt": {"enabled": false}}"#,
    )
    .expect("should succeed");

    let value = expand_includes(&main).expect("should succeed");
    let points = value["points"].as_array().expect("points should be array");
    assert_eq!(points.len(), 2);
    assert_eq!(points[0]["id"], "p1");
    assert_eq!(points[1]["id"], "p2");
}

/// include 字段不是数组 → 报错
#[test]
fn include_not_array_errors() {
    let dir = tempdir().expect("should succeed");
    let main = dir.path().join("config.json");
    fs::write(
        &main,
        r#"{"include": "buses.json", "buses": [], "points": [], "mqtt": {"enabled": false}}"#,
    )
    .expect("should succeed");

    let err = expand_includes(&main).expect_err("non-array include should fail");
    assert!(err.to_string().contains("must be an array"));
}

/// include 项不是字符串 → 报错
#[test]
fn include_item_not_string_errors() {
    let dir = tempdir().expect("should succeed");
    let main = dir.path().join("config.json");
    fs::write(
        &main,
        r#"{"include": [123], "buses": [], "points": [], "mqtt": {"enabled": false}}"#,
    )
    .expect("should succeed");

    let err = expand_includes(&main).expect_err("non-string include item should fail");
    assert!(err.to_string().contains("must be strings"));
}

/// Bug C16: include 禁止 `..` 逃逸（越界读取任意文件）
#[test]
fn include_rejects_parent_dir_escape() {
    let dir = tempdir().expect("should succeed");
    let main = dir.path().join("config.json");
    fs::write(
        &main,
        r#"{"include": ["../secret.json"], "buses": [], "points": [], "mqtt": {"enabled": false}}"#,
    )
    .expect("should succeed");

    let err = expand_includes(&main).expect_err("parent dir escape should fail");
    assert!(
        err.to_string().contains("without '..'"),
        "should reject '..' escape: {err}"
    );
}

/// Bug C16: include 禁止绝对路径
#[test]
fn include_rejects_absolute_path() {
    let dir = tempdir().expect("should succeed");
    let main = dir.path().join("config.json");
    let abs = dir
        .path()
        .join("other.json")
        .to_string_lossy()
        .replace('\\', "/");
    fs::write(
        &main,
        format!(
            r#"{{"include": ["{}"], "buses": [], "points": [], "mqtt": {{"enabled": false}}}}"#,
            abs
        ),
    )
    .expect("should succeed");

    let err = expand_includes(&main).expect_err("absolute include should fail");
    assert!(
        err.to_string().contains("relative path"),
        "should reject absolute path: {err}"
    );
}
