use super::*;
use tempfile::tempdir;

/// 存量配置的 ct_ratio=0 / 负数必须降级为 None，否则校验会阻断启动。
#[test]
fn legacy_non_positive_ratio_is_sanitized() {
    let mut cfg = GatewayConfig::default();
    let p = crate::config::PointConfig {
        id: "p1".to_string(),
        ct_ratio: Some(0.0),
        pt_ratio: Some(-1.0),
        ..Default::default()
    };
    cfg.points = vec![p];

    sanitize_legacy_ratios(&mut cfg);
    assert_eq!(cfg.points[0].ct_ratio, None, "0 应降级为未配置");
    assert_eq!(cfg.points[0].pt_ratio, None, "负数应降级为未配置");

    // 合法变比保持不变
    cfg.points[0].ct_ratio = Some(30.0);
    cfg.points[0].pt_ratio = Some(100.0);
    sanitize_legacy_ratios(&mut cfg);
    assert_eq!(cfg.points[0].ct_ratio, Some(30.0));
    assert_eq!(cfg.points[0].pt_ratio, Some(100.0));
}

/// snapshot 下聚合窗口小于上报周期：加载路径必须抬到上报周期，避免阻断启动。
#[test]
fn legacy_snapshot_small_aggregation_window_is_raised() {
    use crate::config::mqtt::ReportMode;
    use crate::config::AggregationConfig;
    use crate::engine::model::AggFn;

    let mut cfg = GatewayConfig::default();
    cfg.mqtt.enabled = true;
    cfg.mqtt.publish_interval = 5;
    cfg.mqtt.report_mode = ReportMode::Snapshot;
    cfg.points = vec![crate::config::PointConfig {
        id: "p1".to_string(),
        aggregation: Some(AggregationConfig {
            window: 1,
            function: AggFn::Avg,
        }),
        ..Default::default()
    }];

    assert!(
        cfg.validate_snapshot_aggregation().is_err(),
        "未兜底前该配置必须被判非法（Web 保存路径依赖它）"
    );

    sanitize_legacy_snapshot_aggregation(&mut cfg);
    assert_eq!(
        cfg.points[0].aggregation.as_ref().map(|a| a.window),
        Some(5),
        "窗口应抬到上报周期"
    );
    assert!(
        cfg.validate_snapshot_aggregation().is_ok(),
        "兜底后必须通过校验，否则手工改配置会导致升级后网关起不来"
    );

    // stream 模式不受影响：窗口保持原值
    cfg.mqtt.report_mode = ReportMode::Stream;
    cfg.points[0].aggregation = Some(AggregationConfig {
        window: 1,
        function: AggFn::Avg,
    });
    sanitize_legacy_snapshot_aggregation(&mut cfg);
    assert_eq!(
        cfg.points[0].aggregation.as_ref().map(|a| a.window),
        Some(1),
        "stream 下窗口应保持原值"
    );
}

/// 存量配置若同时打开 mqtt 与 sim_consumer（两个队列 ack 消费者），
/// 加载路径必须降级（关掉模拟消费者）而不是阻断启动。
#[test]
fn legacy_double_consumer_is_sanitized() {
    let mut cfg = GatewayConfig::default();
    cfg.mqtt.enabled = true;
    cfg.sim_consumer.enabled = true;

    assert!(
        cfg.validate_all().is_err(),
        "未兜底前该配置必须被判非法（Web 保存路径依赖它）"
    );

    assert!(
        sanitize_legacy_consumer_exclusivity(&mut cfg),
        "双开时应返回「已降级」，调用方据此告知用户配置被改过"
    );
    assert!(
        !cfg.sim_consumer.enabled,
        "兜底后模拟消费者必须被关闭（保留 MQTT 上行）"
    );
    assert!(cfg.mqtt.enabled, "MQTT 必须保留，不受影响");
    assert!(
        cfg.validate_all().is_ok(),
        "兜底后必须通过校验，否则手工改配置会导致升级后网关起不来"
    );

    // 只开一个消费者时不应被改动
    cfg.sim_consumer.enabled = true;
    cfg.mqtt.enabled = false;
    assert!(
        !sanitize_legacy_consumer_exclusivity(&mut cfg),
        "单开时不应降级"
    );
    assert!(cfg.sim_consumer.enabled, "仅开模拟消费者时应保持不变");
}

/// 端到端：磁盘上的存量配置同时打开两个消费者时，`load()` 必须降级加载而不是返回 Err。
///
/// 与上面的直接调用用例互补——本用例覆盖 `try_load` 里的接线；若将来漏掉那次调用，
/// 这里会失败，而不会拖到现场表现为「升级后网关起不来」。
#[test]
fn legacy_double_consumer_config_loads_via_loader() {
    let dir = tempdir().expect("should succeed");
    let path = dir.path().join("config.json");
    let loader = ConfigLoader::new(&path);

    let mut cfg = GatewayConfig::default();
    cfg.mqtt.enabled = true;
    cfg.sim_consumer.enabled = true;
    fs::write(
        &path,
        serde_json::to_string_pretty(&cfg).expect("serialize"),
    )
    .expect("write legacy config");

    let loaded = loader.load().expect("双消费者存量配置必须能加载");
    assert!(!loaded.sim_consumer.enabled, "加载路径应关闭模拟消费者");
    assert!(loaded.mqtt.enabled, "MQTT 必须保留");
}

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
    // 坏配置用例的变形闭包。
    type Mutator = Box<dyn Fn(&mut serde_json::Value)>;
    // 一条坏配置用例：(场景说明, 配置变形, 报错应包含的关键词)。
    type BadCase<'a> = (&'a str, Mutator, &'a str);

    // 用 fn 而非闭包：闭包会被下面的用例捕获，导致 `BadCase` 无法满足 'static。
    fn addr_bus(bus: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "port_name": "COM3",
            "protocol": "modbus",
            "tcp_addr": bus,
        })
    }

    let cases: Vec<BadCase> = vec![
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

// ===== A3：加载失败现场取证 / 默认配置落盘 / 热重载正常路径 =====
//
// 这三条覆盖的是「配置加载器的边界行为」，此前只有失败路径有测试覆盖：
// 1. load() 遇到坏配置要留下现场（config.json.bak），且**不能覆盖历史现场**；
// 2. load() 遇到文件缺失要生成默认配置并落盘（下次启动不再走 fallback），
//    落盘失败必须返回 Err 而不是静默成功；
// 3. reload() 对有效文件的热重载正常路径必须返回最新内容。

/// load() 遇到坏配置时应留备份现场；连续失败**不得覆盖**首次现场的备份。
///
/// Bug 115 修复：`bak` 只在不存在时复制。覆盖会让连续失败时只能看到最后一次
/// 状态，现场无法回溯「第一次坏在哪」。
#[test]
fn load_failure_creates_backup_once_and_preserves_first_failure() {
    let dir = tempdir().expect("should succeed");
    let path = dir.path().join("config.json");
    let loader = loader_with_json(&dir, |v| {
        // 缺必填字段 port_name：schema 校验阶段就会失败
        v["buses"] = serde_json::json!([{"protocol": "modbus"}]);
    });
    let first_bad = fs::read_to_string(&path).expect("读原始坏配置");

    let err = loader.load().expect_err("坏配置必须加载失败");
    assert!(
        err.to_string().contains("port_name"),
        "报错应点名缺失字段，实际: {err}"
    );

    // 备份现场必须生成
    let bak = path.with_extension("json.bak");
    assert!(bak.exists(), "加载失败应生成备份现场 {}", bak.display());
    let first_bak = fs::read_to_string(&bak).expect("读备份现场");
    assert_eq!(first_bak, first_bad, "备份现场应与首次失败时的原始配置一致");

    // 第二次失败：配置内容变了，备份现场**不得**被覆盖
    let mut value = serde_json::to_value(GatewayConfig::default()).expect("默认配置应可序列化");
    value["buses"] = serde_json::json!([{"protocol": "modbus", "port_name": "COM9"}]);
    value["points"] = serde_json::json!([{"id": "p1"}]);
    fs::write(
        &path,
        serde_json::to_string_pretty(&value).expect("JSON 应可序列化"),
    )
    .expect("写入第二份坏配置");
    let second_bad = fs::read_to_string(&path).expect("读第二份坏配置");

    let err = loader.load().expect_err("第二份坏配置同样必须加载失败");
    assert!(
        err.to_string().contains("points"),
        "第二份坏配置应报点位缺失字段，实际: {err}"
    );

    let second_bak = fs::read_to_string(&bak).expect("读备份现场");
    assert_eq!(
        second_bak, first_bad,
        "连续失败时备份现场不得被覆盖（应保留第一次的现场）"
    );
    assert_ne!(
        second_bak, second_bad,
        "备份现场不应变成第二次失败时的配置内容"
    );

    // 失败路径不得篡改原始配置文件——现场取证的前提是原文件还在
    let current = fs::read_to_string(&path).expect("读当前配置");
    assert_eq!(current, second_bad, "加载失败时原始配置文件不得被改写");
}

/// load() 遇到配置文件缺失时应生成默认配置并**落盘**，下次启动不再走 fallback。
#[test]
fn load_missing_file_generates_and_persists_default_config() {
    let dir = tempdir().expect("should succeed");
    let path = dir.path().join("config.json");
    assert!(!path.exists(), "前置条件：配置文件不存在");
    let loader = ConfigLoader::new(&path);

    let cfg = loader
        .load()
        .expect("配置文件缺失时应生成默认配置而不是报错");
    assert_eq!(
        cfg.schema_version,
        crate::config::CURRENT_SCHEMA_VERSION,
        "生成的默认配置版本应为当前版本"
    );

    // 默认配置必须真的写到磁盘（否则下次启动仍走 fallback，故障现场反复出现）
    assert!(path.exists(), "默认配置应落盘到 {}", path.display());
    let content = fs::read_to_string(&path).expect("读落盘的默认配置");
    assert!(
        content.contains("schema_version"),
        "落盘的默认配置应包含 schema_version"
    );

    // 落盘后再次 load 走正常路径（try_load 成功），不再走 fallback 分支
    let again = loader.load().expect("已落盘的默认配置应可正常加载");
    assert_eq!(
        again.schema_version,
        crate::config::CURRENT_SCHEMA_VERSION,
        "二次加载应同样得到当前版本配置"
    );
}

/// load() 遇到配置文件缺失且默认配置**无法落盘**时必须返回 Err（不得静默成功）。
///
/// Bug 66 修复：原实现 `let _ = self.save(&cfg)` 静默吞掉保存错误，
/// 磁盘满 / 权限不足时运维完全无感，直到现场设备根本没配置却以为已配置。
#[test]
fn load_missing_file_returns_err_when_default_cannot_be_persisted() {
    let dir = tempdir().expect("should succeed");
    // 用普通文件占住父目录，使 create_dir_all 失败
    let blocking_file = dir.path().join("block");
    fs::write(&blocking_file, "not a directory").expect("should succeed");
    let path = blocking_file.join("config.json");
    assert!(!path.exists(), "前置条件：配置文件不存在");
    let loader = ConfigLoader::new(&path);

    let err = loader
        .load()
        .expect_err("默认配置无法落盘时必须返回 Err，不能静默成功");
    let msg = err.to_string();
    assert!(
        msg.contains("create config parent dir") || msg.contains("failed"),
        "报错应说明落盘失败原因，实际: {msg}"
    );
    assert!(!path.exists(), "落盘失败后不应留下半成品配置文件");
}

/// 出厂配置模板必须真的能被网关加载（P3-1b）。
///
/// `config.json.example` 是「配置片段示例」：缺 `buses`/`points`/`mqtt` 等必填段，
/// 直接当配置用会加载失败。交付现场需要的是一份**完整可加载**的初值，
/// 所以随包提供 `deploy/config.factory.json`，并由本用例把它钉死在 CI 里：
/// 模板一旦改坏（漏字段、JSON 语法错、配额档位丢失），这里立刻失败，
/// 而不是等交付方在设备上复制过去才发现起不来。
#[test]
fn factory_template_in_package_is_loadable() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("deploy")
        .join("config.factory.json");
    assert!(
        path.exists(),
        "出厂配置模板必须随包存在：{}",
        path.display()
    );

    // reload() 走完整严格链路：schema 校验 + serde 反序列化 + 迁移 + 业务校验，
    // 任何一环不过都会 Err —— 这正是「交付方复制过去就能起来」的判据。
    let cfg = ConfigLoader::new(&path)
        .reload()
        .expect("出厂配置模板必须通过 schema + serde + validate_all");

    assert!(
        !cfg.buses.is_empty(),
        "出厂模板必须带至少一条示例总线，否则设备首启无采集"
    );
    assert!(
        !cfg.points.is_empty(),
        "出厂模板必须带至少一个示例点位，否则开箱无法验证采集链路"
    );

    // web 段：模板必须给出听的地址，否则交付方还要自己猜
    let web = cfg.web.as_ref().expect("出厂模板必须包含 web 段");
    assert!(
        !web.listen_addr.is_empty(),
        "出厂模板的 web.listen_addr 不能为空"
    );

    // ★ 模板必须**能启动**，而不只是能被加载。
    //
    // 历史事故：出厂模板写死 `listen_addr=0.0.0.0:8080` + 空 `password_hash`，
    // 而 bootstrap 的 fail-closed 判定是「绑定所有网卡 + 默认凭据 = 硬性违规 →
    // 拒绝启动」。两者叠加的结果是：走 install.sh 兜底路径装机的网关**开箱起不来**，
    // 而模板注释还写着「启动会告警」。本用例只测「能加载」，完全测不到这一点；
    // CI 的 deploy-smoke 又自带一份 127.0.0.1 配置，绕开了出厂模板，CI 全绿也发现不了。
    //
    // 这里直接用生产代码的同一判定函数（WebConfig::security_violations），
    // 把「模板组合不得触发启动期拒绝」钉死在 CI 里。
    let violations = web.security_violations();
    assert!(
        violations.is_empty(),
        "出厂模板的 web 组合会触发启动期 fail-closed（网关直接拒绝启动）：{}",
        violations.join("; ")
    );
    // 兜底：模板必须显式允许默认凭据，否则 assemble 的另一条 bail
    // （password_hash 为空 && allow_default_token=false）同样会让网关起不来。
    assert!(
        web.allow_default_token,
        "出厂模板必须保留 allow_default_token=true：模板的 password_hash 留空，\
         若同时禁掉默认凭据，bootstrap 会因「无密码可用」拒绝启动"
    );

    // license 段：四档配额齐全（v2.1 口径：配额只管点位，四档仅 max_points 不同）
    let license = cfg.license.as_ref().expect("出厂模板必须包含 license 段");
    let quotas = license
        .tier_quotas
        .as_ref()
        .expect("出厂模板必须带 tier_quotas（否则 license 未带 quota 时查不到档位）");
    // 档位名用序列化后的字符串比对：配置里写的就是 "core"/"standard"/"pro"/"ultra"，
    // 只有序列化形态一致，模板里的 tier 字符串才能被正确识别成对应档位。
    let tiers: Vec<String> = quotas
        .iter()
        .map(|q| {
            serde_json::to_value(q.tier)
                .expect("tier 应可序列化")
                .as_str()
                .expect("tier 应序列化为字符串")
                .to_string()
        })
        .collect();
    assert_eq!(
        tiers,
        vec!["core", "standard", "pro", "ultra"],
        "tier_quotas 必须覆盖四个档位且按 core→ultra 排列，实际: {:?}",
        tiers
    );
    // 四档的 max_points 必须严格递增，否则「升级档位」在配额上没有任何意义
    let points: Vec<u32> = quotas
        .iter()
        .map(|q| q.quota.max_points.unwrap_or(u32::MAX))
        .collect();
    assert!(
        points.windows(2).all(|w| w[0] < w[1]),
        "四档 max_points 必须严格递增，实际: {:?}",
        points
    );

    // 模板里写死的四档数值必须与代码内置默认表一致。
    // config 里的值**优先于**内置表，模板抄错一个数字就等于悄悄改了档位容量，
    // 而现场只表现为「点数超限」——本仓库的 config.json.example 就与内置表
    // 漂移过（pro 档），所以这里把两者钉在一起。
    for entry in quotas {
        let builtin = crate::config::DEFAULT_TIER_QUOTAS
            .iter()
            .find(|(t, _)| *t == entry.tier)
            .map(|(_, q)| q)
            .expect("模板里的档位必须存在于代码内置默认表");
        assert_eq!(
            &entry.quota, builtin,
            "模板里 {:?} 档的配额与内置默认表不一致（模板值优先，等于悄悄改容量）",
            entry.tier
        );
    }

    // ★ 模板的「照注释操作」路径也必须合法：模板注释让用户接好 broker 后把
    // `mqtt.enabled` 改 true，而 `validate_cycle_conflict` 要求点位采集周期
    // **严格小于** `publish_interval`。两者相等（历史模板 5s / 5s）时，用户照做
    // 会立刻保存 400、重启也起不来。这里把「开启 MQTT 后模板仍合法」钉死。
    let mut mqtt_on = cfg.clone();
    mqtt_on.mqtt.enabled = true;
    mqtt_on.validate_all().expect(
        "出厂模板开启 mqtt.enabled 后必须仍通过校验：示例点位采集周期必须 < publish_interval",
    );

    // queue.max_rows 必须显式写进出厂模板。漏写时会静默退回代码默认的 100 万行，
    // 而 100 万行只够「500 点位 / 10 秒」撑约 5.6 小时（容量规划 §3.3）；现场断网超过
    // 这个时长就开始 FIFO 丢历史，且现场只表现为「数据少了」，不会报任何错。
    assert_eq!(
        cfg.queue.max_rows, 4_400_000,
        "出厂模板的 queue.max_rows 必须按「500 点位 / 10 秒 / 断网缓冲 24 小时」\
         显式给出：漏写会退回代码默认的 100 万行，断网数小时后即开始 FIFO 丢历史"
    );
}

/// reload() 对有效文件的热重载正常路径必须返回磁盘上的最新内容。
///
/// 此前 reload 只有失败路径测试，「改文件 → reload 拿到新内容」这条主路径
/// 从未被断言过——热重载若恒返回旧内容/默认值，现场改配置后不生效且无告警。
#[test]
fn reload_returns_updated_content_for_valid_file() {
    let dir = tempdir().expect("should succeed");
    let path = dir.path().join("config.json");

    let mut value = serde_json::to_value(GatewayConfig::default()).expect("默认配置应可序列化");
    value["mqtt"]["broker"] = serde_json::json!("tcp://broker-a:1883");
    fs::write(
        &path,
        serde_json::to_string_pretty(&value).expect("JSON 应可序列化"),
    )
    .expect("写入初始配置");

    let loader = ConfigLoader::new(&path);
    let first = loader.reload().expect("有效配置应可 reload");
    assert_eq!(
        first.mqtt.broker, "tcp://broker-a:1883",
        "reload 应读到磁盘上的 broker"
    );

    // 现场改配置（模拟运维热更新），reload 必须反映最新内容
    let mut value = serde_json::to_value(GatewayConfig::default()).expect("默认配置应可序列化");
    value["mqtt"]["broker"] = serde_json::json!("tcp://broker-b:1883");
    value["mqtt"]["publish_interval"] = serde_json::json!(30);
    fs::write(
        &path,
        serde_json::to_string_pretty(&value).expect("JSON 应可序列化"),
    )
    .expect("写入更新后的配置");

    let second = loader.reload().expect("更新后的配置应可 reload");
    assert_eq!(
        second.mqtt.broker, "tcp://broker-b:1883",
        "reload 必须返回改后的 broker（热重载生效）"
    );
    assert_eq!(
        second.mqtt.publish_interval, 30,
        "reload 必须返回改后的 publish_interval"
    );
}

/// 启动加载要清扫上次异常退出留下的 tmp，且不得误删同目录其它文件。
#[test]
fn load_cleans_stale_tmp_files_only() {
    let dir = tempdir().expect("should succeed");
    let path = dir.path().join("config.json");
    let loader = ConfigLoader::new(&path);
    loader
        .save(&GatewayConfig::default())
        .expect("save should succeed");

    let stale = dir.path().join("config.json.tmp.999.1.1");
    std::fs::write(&stale, b"{}").expect("write stale tmp");
    let unrelated = dir.path().join("notes.txt");
    std::fs::write(&unrelated, b"keep").expect("write unrelated");

    loader.load().expect("load should succeed");

    assert!(!stale.exists(), "残留临时配置必须被清理");
    assert!(unrelated.exists(), "不得误删同目录其它文件");
}
