use super::*;
use tempfile::tempdir;

#[test]
fn save_creates_new_file() {
    let dir = tempdir().expect("should succeed");
    let path = dir.path().join("config.json");
    let loader = ConfigLoader::new(&path);
    let cfg = GatewayConfig::default();

    loader.save(&cfg).expect("save should succeed");

    assert!(path.exists(), "config file should be created");
    let content = fs::read_to_string(&path).expect("should succeed");
    assert!(content.contains("schema_version"));
}

#[test]
fn save_overwrites_existing_file() {
    let dir = tempdir().expect("should succeed");
    let path = dir.path().join("config.json");
    let loader = ConfigLoader::new(&path);

    loader.save(&GatewayConfig::default()).expect("first save");
    let first_len = fs::metadata(&path).expect("should succeed").len();
    assert!(first_len > 0);

    loader.save(&GatewayConfig::default()).expect("second save");
    let second_len = fs::metadata(&path).expect("should succeed").len();
    assert!(second_len > 0);
    assert!(
        (first_len as i64 - second_len as i64).abs() < 50,
        "两次写入相同默认配置长度差异应很小"
    );
}

#[test]
fn save_creates_parent_dirs() {
    let dir = tempdir().expect("should succeed");
    let path = dir.path().join("nested/sub/dir/config.json");
    let loader = ConfigLoader::new(&path);

    loader
        .save(&GatewayConfig::default())
        .expect("save with nested parent should succeed");

    assert!(path.exists());
}

#[test]
fn load_after_save_returns_same_data() {
    let dir = tempdir().expect("should succeed");
    let path = dir.path().join("config.json");
    let loader = ConfigLoader::new(&path);

    let original = GatewayConfig::default();
    loader.save(&original).expect("save");
    let loaded = loader.load().expect("load");

    assert_eq!(loaded.schema_version, original.schema_version);
    assert_eq!(loaded.buses.len(), original.buses.len());
    assert_eq!(loaded.points.len(), original.points.len());
}

#[test]
fn no_tmp_file_left_on_success() {
    let dir = tempdir().expect("should succeed");
    let path = dir.path().join("config.json");
    let loader = ConfigLoader::new(&path);

    loader.save(&GatewayConfig::default()).expect("save");

    let tmp_files: Vec<_> = fs::read_dir(dir.path())
        .expect("should succeed")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
        .collect();
    assert!(
        tmp_files.is_empty(),
        "成功保存后不应残留 tmp 文件，但发现: {:?}",
        tmp_files.iter().map(|e| e.path()).collect::<Vec<_>>()
    );
}

#[test]
fn save_failure_does_not_corrupt_target() {
    let dir = tempdir().expect("should succeed");
    let real_path = dir.path().join("config.json");
    let loader_real = ConfigLoader::new(&real_path);
    loader_real
        .save(&GatewayConfig::default())
        .expect("initial save");
    let original_content = fs::read_to_string(&real_path).expect("read original");

    // 让 save 失败：把 target 的父目录建成一个普通文件，
    // 这样 create_dir_all 会报错（EEXIST 且不是目录）
    let blocking_file = dir.path().join("block");
    fs::write(&blocking_file, "not a directory").expect("should succeed");
    let blocked_path = blocking_file.join("config.json");
    let loader_bad = ConfigLoader::new(&blocked_path);

    let result = loader_bad.save(&GatewayConfig::default());
    assert!(result.is_err(), "写入被文件占用的路径应失败");

    // 原始文件完好
    assert!(real_path.exists());
    let current_content = fs::read_to_string(&real_path).expect("should succeed");
    assert_eq!(current_content, original_content, "原文件不应被损坏");
}

#[test]
fn concurrent_save_does_not_corrupt_target() {
    use std::sync::Arc;
    use std::thread;

    let dir = tempdir().expect("should succeed");
    let path = dir.path().join("config.json");
    let loader = Arc::new(ConfigLoader::new(&path));

    let mut handles = Vec::new();
    for _ in 0..10 {
        let l = loader.clone();
        handles.push(thread::spawn(move || {
            l.save(&GatewayConfig::default()).expect("concurrent save");
        }));
    }
    for h in handles {
        h.join().expect("thread should not panic");
    }

    let loaded = loader
        .load()
        .expect("load after concurrent saves should parse");
    assert_eq!(
        loaded.schema_version,
        GatewayConfig::default().schema_version
    );

    let tmp_files: Vec<_> = fs::read_dir(dir.path())
        .expect("should succeed")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
        .collect();
    assert!(
        tmp_files.is_empty(),
        "并发保存完成后不应残留 tmp 文件: {:?}",
        tmp_files.iter().map(|e| e.path()).collect::<Vec<_>>()
    );
}

#[test]
fn atomic_write_basic() {
    let dir = tempdir().expect("should succeed");
    let path = dir.path().join("foo.txt");
    atomic_write(&path, b"hello world").expect("atomic_write should succeed");
    let content = fs::read_to_string(&path).expect("should succeed");
    assert_eq!(content, "hello world");
}

/// reload() 文件不存在时应返回 Err（不静默回退到默认配置）
#[test]
fn reload_returns_err_when_file_missing() {
    let dir = tempdir().expect("should succeed");
    let path = dir.path().join("never_saved.json");
    let loader = ConfigLoader::new(&path);
    let err = loader.reload().expect_err("reload 不存在的文件应返回 Err");
    assert!(
        err.to_string().contains("not found"),
        "应提示文件不存在，实际: {err}"
    );
}

/// reload() 解析失败时应返回 Err（不静默回退）
#[test]
fn reload_returns_err_on_parse_failure() {
    let dir = tempdir().expect("should succeed");
    let path = dir.path().join("bad.json");
    fs::write(&path, "{not valid json}").expect("should succeed");
    let loader = ConfigLoader::new(&path);
    let err = loader.reload().expect_err("reload 坏 JSON 应返回 Err");
    assert!(
        err.to_string().contains("parse"),
        "应提示解析失败，实际: {err}"
    );
}

/// atomic_write 父目录不存在时应返回 Err 且不残留 tmp 文件
#[test]
fn atomic_write_fails_when_parent_dir_missing() {
    let dir = tempdir().expect("should succeed");
    let missing_parent = dir.path().join("no_such_dir").join("file.txt");
    let err = atomic_write(&missing_parent, b"data").expect_err("父目录缺失应返回 Err");
    assert!(
        err.to_string().contains("write config tmp"),
        "应提示 tmp 写入失败，实际: {err}"
    );
    // 写入失败后不应在已存在的上级目录残留 tmp
    let tmp_left = std::fs::read_dir(dir.path())
        .expect("should succeed")
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
        .count();
    assert_eq!(tmp_left, 0, "写入失败后不应残留 tmp 文件");
}

// ===== C-3 修复配套：解密失败时 load() 必须阻断启动 =====
//
// 旧实现 `decrypt_config_secrets` 失败仅 warn 保留 `enc:xxx` 字面串，
// 下游模块拿到后用字面串当凭据，导致"启动看似正常但链路失败"。
// 新实现必须让 ConfigLoader::load() 返回 Err，把 fail-fast 一路传到顶层。

/// load() 在 config.json 含有非法 enc: 密文时必须返回 Err（C-3 fail-fast）
#[test]
fn load_returns_err_when_enc_ciphertext_invalid() {
    use crate::config::WebConfig;
    let dir = tempdir().expect("should succeed");
    let path = dir.path().join("config.json");
    let loader = ConfigLoader::new(&path);

    // 写一个含坏 enc: 密文的 config.json
    let mut cfg = GatewayConfig::default();
    let web = WebConfig {
        api_token: "enc:!@#not-valid-base64$%^".to_string(),
        ..Default::default()
    };
    cfg.web = Some(web);
    loader.save(&cfg).expect("save should succeed");

    let err = loader.load().expect_err("load 坏 enc 密文应返回 Err");
    let msg = err.to_string();
    assert!(
        msg.contains("decrypt config secrets failed"),
        "应提示解密失败，实际: {msg}"
    );
    assert!(msg.contains("web.api_token"), "应指明失败字段，实际: {msg}");
}

/// reload() 在 config.json 含有非法 enc: 密文时必须返回 Err（同 load）
#[test]
fn reload_returns_err_when_enc_ciphertext_invalid() {
    use crate::config::WebConfig;
    let dir = tempdir().expect("should succeed");
    let path = dir.path().join("config.json");
    let loader = ConfigLoader::new(&path);

    let mut cfg = GatewayConfig::default();
    let web = WebConfig {
        api_token: "enc:!@#not-valid-base64$%^".to_string(),
        ..Default::default()
    };
    cfg.web = Some(web);
    loader.save(&cfg).expect("save should succeed");

    let err = loader.reload().expect_err("reload 坏 enc 密文应返回 Err");
    let msg = err.to_string();
    assert!(
        msg.contains("reload decrypt config secrets failed"),
        "应提示 reload 解密失败，实际: {msg}"
    );
}

// ===== 坏配置矩阵（决策清单 P1-5）=====
//
// 现场手改配置是「装机当天起不来」的主要来源。这里表驱动地跑一遍典型坏配置，
// 逐条断言：加载必须失败，且错误里点名了字段 / 给出了可读原因（而非笼统失败）。
//
// 加载顺序（loader.rs:86-105）：expand includes → JSON Schema → 反序列化 →
// migrate → 解密 → validate_all。不同坏配置会被不同层拦下，故每条用例断言各自的关键词。

/// 从默认配置派生 JSON 后写入临时目录，返回 loader 与路径。
fn loader_with_json(
    dir: &tempfile::TempDir,
    mutate: impl FnOnce(&mut serde_json::Value),
) -> ConfigLoader {
    let mut value = serde_json::to_value(GatewayConfig::default()).expect("默认配置应可序列化");
    mutate(&mut value);
    let path = dir.path().join("config.json");
    fs::write(
        &path,
        serde_json::to_string_pretty(&value).expect("JSON 应可序列化"),
    )
    .expect("写入坏配置失败");
    ConfigLoader::new(path)
}

/// 断言 load() 失败，且错误消息包含 `needle`（用于确认报错点名了具体字段/原因）。
fn assert_load_error(loader: &ConfigLoader, case: &str, needle: &str) {
    let err = loader
        .load()
        .expect_err(&format!("坏配置 {case} 必须被拒绝加载"));
    let msg = err.to_string();
    assert!(
        msg.contains(needle),
        "坏配置 {case} 的报错应包含 {needle:?}，实际: {msg}"
    );
}

#[test]
fn bad_config_matrix_is_rejected_with_readable_error() {
    let addr_bus = |bus: serde_json::Value| -> serde_json::Value {
        serde_json::json!({
            "port_name": "COM3",
            "protocol": "modbus",
            "tcp_addr": bus,
        })
    };

    let cases: Vec<(&str, Box<dyn Fn(&mut serde_json::Value)>, &str)> = vec![
        (
            "缺必填字段 port_name",
            Box::new(|v: &mut serde_json::Value| {
                v["buses"] = serde_json::json!([{"protocol": "modbus"}]);
            }),
            "port_name",
        ),
        (
            "类型错 baud_rate=字符串",
            Box::new(|v: &mut serde_json::Value| {
                v["buses"] = serde_json::json!([{
                    "port_name": "COM3",
                    "protocol": "modbus",
                    "baud_rate": "fast"
                }]);
            }),
            "baud_rate",
        ),
        (
            "非法波特率 12345",
            Box::new(|v: &mut serde_json::Value| {
                v["buses"] = serde_json::json!([{
                    "port_name": "COM3",
                    "protocol": "modbus",
                    "baud_rate": 12345
                }]);
            }),
            "invalid_baud_rate",
        ),
        (
            "非法校验位 mark",
            Box::new(|v: &mut serde_json::Value| {
                v["buses"] = serde_json::json!([{
                    "port_name": "COM3",
                    "protocol": "modbus",
                    "parity": "mark"
                }]);
            }),
            "invalid_parity",
        ),
        (
            "非法 tcp_addr（缺端口）",
            Box::new(|v: &mut serde_json::Value| {
                v["buses"] = serde_json::json!([addr_bus(serde_json::json!("127.0.0.1"))]);
            }),
            "缺少端口",
        ),
        (
            "schema_version 超前",
            Box::new(|v: &mut serde_json::Value| {
                v["schema_version"] = serde_json::json!(99);
            }),
            "高于当前支持",
        ),
    ];

    for (case, mutate, needle) in cases {
        let dir = tempdir().expect("should succeed");
        let loader = loader_with_json(&dir, mutate);
        assert_load_error(&loader, case, needle);
    }
}

/// 版本落后（v0，无 schema_version 的历史配置）必须能迁移加载，而不是被拒绝。
///
/// 这是「旧配置升级后无法加载」风险的反向用例：migrate 只拒绝超前版本，
/// 落后版本应原地迁移到 CURRENT。
#[test]
fn legacy_schema_version_is_migrated_not_rejected() {
    let dir = tempdir().expect("should succeed");
    let loader = loader_with_json(&dir, |v| {
        v["schema_version"] = serde_json::json!(0);
    });
    let cfg = loader.load().expect("v0 历史配置应可迁移加载");
    assert_eq!(
        cfg.schema_version,
        crate::config::CURRENT_SCHEMA_VERSION,
        "v0 配置加载后应升到当前版本"
    );
}
