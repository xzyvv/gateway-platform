use super::*;
use crate::web::api::redact_config_secrets;
use gateway_core::config::{
    sensitive_field_paths, BusConfig, CellularConfig, OpcUaClientConfig, WebConfig, WebhookConfig,
    SENSITIVE_SECRET_HEADERS_PATH,
};
use std::collections::HashMap;

/// 一份所有敏感字段都已填真实值的配置（相当于脱敏前的 GET 响应来源）。
fn sample_config() -> GatewayConfig {
    let mut cfg = GatewayConfig::default();

    let web = WebConfig {
        api_token: "real-web-token".to_string(),
        password_hash: "real-password-hash".to_string(),
        ota_signing_key: Some("real-ota-signing-key".to_string()),
        ..Default::default()
    };
    cfg.web = Some(web);

    cfg.mqtt.username = Some("real-mqtt-username".to_string());
    cfg.mqtt.password = Some("real-mqtt-password".to_string());

    let cellular = CellularConfig {
        username: "real-cellular-username".to_string(),
        password: "real-cellular-password".to_string(),
        pin: Some("real-cellular-pin".to_string()),
        ..Default::default()
    };
    cfg.cellular = Some(cellular);

    let webhook = WebhookConfig {
        headers: HashMap::from([(
            "Authorization".to_string(),
            "real-webhook-authorization".to_string(),
        )]),
        ..Default::default()
    };
    cfg.webhook = Some(webhook);

    // P0-3：总线级 OPC UA 凭据（数组内字段，脱敏/还原单独处理）。
    cfg.buses = vec![BusConfig {
        port_name: "opcua_1".to_string(),
        protocol: "opcua".to_string(),
        opcua: Some(OpcUaClientConfig {
            auth: "username".to_string(),
            username: Some("real-opcua-username".to_string()),
            password: Some("real-opcua-password".to_string()),
            ..Default::default()
        }),
        ..Default::default()
    }];

    // OPC UA Server 用户名/密码用户（数组内字段，脱敏/还原单独处理）。
    cfg.opcua = Some(gateway_core::config::OpcuaConfig {
        enabled: true,
        endpoint: "opc.tcp://0.0.0.0:4840".to_string(),
        users: vec![gateway_core::config::OpcuaUserConfig {
            username: "real-opcua-server-user".to_string(),
            password: "real-opcua-server-password".to_string(),
        }],
        ..Default::default()
    });

    cfg
}

/// B-1 回归：北向通道计数必须覆盖全部 7 类 sink
/// （mqtt / webhook / cellular / opcua / modbus_server /
/// modbus_passthrough / telemetry）。
///
/// 旧实现只覆盖前 4 类，把启用了 opcua / modbus_server /
/// modbus_passthrough / telemetry 的配置错误地算成"少占"，
/// 配额形同虚设。v2.1 起北向通道数不再阻断保存，本测试直接断言计数函数，
/// 不再依赖「超限 → 4021」反推。
#[test]
fn count_northbound_channels_covers_all_sinks() {
    let mut cfg = GatewayConfig::default();
    // 默认全关
    assert_eq!(count_northbound_channels(&cfg), 0);

    cfg.mqtt.enabled = true;
    assert_eq!(count_northbound_channels(&cfg), 1);

    cfg.webhook = Some(WebhookConfig {
        enabled: true,
        ..Default::default()
    });
    cfg.cellular = Some(CellularConfig {
        enabled: true,
        ..Default::default()
    });
    cfg.opcua = Some(gateway_core::config::OpcuaConfig {
        enabled: true,
        ..Default::default()
    });
    cfg.modbus_server = Some(gateway_core::config::ModbusServerConfig {
        enabled: true,
        ..Default::default()
    });
    cfg.modbus_passthrough = Some(gateway_core::config::ModbusPassthroughConfig {
        enabled: true,
        ..Default::default()
    });
    cfg.telemetry = Some(gateway_core::config::TelemetryConfig {
        enabled: true,
        ..Default::default()
    });

    // 7 类全部启用
    assert_eq!(count_northbound_channels(&cfg), 7);
}

/// 样本 JSON 中所有真实凭据值（静态路径 + webhook.headers 逐键）。
fn secret_values(original: &serde_json::Value) -> Vec<String> {
    let mut values: Vec<String> = sensitive_field_paths()
        .iter()
        .filter_map(|p| {
            original
                .pointer(p)
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .collect();
    if let Some(headers) = original
        .pointer(SENSITIVE_SECRET_HEADERS_PATH)
        .and_then(|v| v.as_object())
    {
        values.extend(
            headers
                .values()
                .filter_map(|v| v.as_str().map(str::to_string)),
        );
    }
    // P0-3：buses[].opcua 用户名/密码（数组路径，非静态清单）。
    if let Some(buses) = original.get("buses").and_then(|v| v.as_array()) {
        for bus in buses {
            if let Some(opcua) = bus.get("opcua").and_then(|v| v.as_object()) {
                values.extend(
                    ["username", "password"]
                        .iter()
                        .filter_map(|k| opcua.get(*k).and_then(|v| v.as_str()).map(str::to_string)),
                );
            }
        }
    }
    // OPC UA Server 用户密码（数组路径，非静态清单）。
    if let Some(users) = original
        .get("opcua")
        .and_then(|v| v.get("users"))
        .and_then(|v| v.as_array())
    {
        values.extend(users.iter().filter_map(|u| {
            u.get("password")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        }));
    }
    values
}

/// 未编入 `opcua-server` 的发行版：配置启用北向 UA Server 必须在保存时拒绝。
///
/// 否则现场是「保存成功、UA 端口不开、只在启动日志一行 warn」，运维会去查网络。
/// 同时验证编入该能力的构建不得误拒 —— 否则楼宇/电力/PLC 版一保存就报错。
#[test]
fn unbuilt_opcua_server_is_rejected_and_built_one_is_allowed() {
    let cfg = sample_config();
    assert!(
        reject_unbuilt_with(&cfg, false).is_err(),
        "未编入 opcua-server 时必须拒绝启用北向 UA Server"
    );
    assert!(
        reject_unbuilt_with(&cfg, true).is_ok(),
        "已编入 opcua-server 时不得误拒"
    );
}

/// 关闭状态不拒绝：字段存在但 `enabled=false` 是合法配置。
#[test]
fn disabled_opcua_server_passes_in_trimmed_build() {
    let mut cfg = sample_config();
    if let Some(o) = cfg.opcua.as_mut() {
        o.enabled = false;
    }
    assert!(reject_unbuilt_with(&cfg, false).is_ok());
    assert!(reject_unbuilt_with(&cfg, true).is_ok());
}

/// 清单快照：这是脱敏与还原的唯一来源，任何增删都必须同步更新本测试与
/// round-trip 样本（样本缺字段会让 round-trip 断言直接失败）。
#[test]
fn sensitive_field_paths_are_single_source_snapshot() {
    let expected: &[&str] = &[
        "/web/api_token",
        "/web/password_hash",
        "/web/ota_signing_key",
        "/mqtt/username",
        "/mqtt/password",
        "/cellular/username",
        "/cellular/password",
        "/cellular/pin",
    ];
    assert_eq!(sensitive_field_paths(), expected);
}

/// 工程文件导出使用 `/northbound` 前缀脱敏北向通道；`buses` 位于根层级，
/// 必须同时被覆盖，否则默认导出会带出 OPC UA 明文凭据。
#[test]
fn project_prefix_redaction_covers_root_buses() {
    let old = sample_config();
    let mut value = serde_json::to_value(&old).expect("serialize sample config");
    crate::web::api::redact_secrets_at(&mut value, "/northbound");
    for key in ["username", "password"] {
        assert_eq!(
            value
                .pointer(&format!("/buses/0/opcua/{key}"))
                .and_then(|v| v.as_str()),
            Some(REDACTED_SECRET),
            "工程导出（/northbound 前缀）未脱敏 buses[].opcua.{key}"
        );
    }
    // 完整配置 JSON 中 opcua 位于根层级；工程文件里才挂在 `/northbound` 下。
    assert_eq!(
        value
            .pointer("/opcua/users/0/password")
            .and_then(|v| v.as_str()),
        Some(REDACTED_SECRET),
        "完整配置未脱敏 opcua.users[].password"
    );
    // 工程文件布局：opcua 挂在 `/northbound` 下，同样必须脱敏。
    let mut project = serde_json::json!({
        "northbound": {
            "opcua": {
                "users": [{ "username": "project-user", "password": "project-pass" }]
            }
        }
    });
    crate::web::api::redact_secrets_at(&mut project, "/northbound");
    assert_eq!(
        project
            .pointer("/northbound/opcua/users/0/password")
            .and_then(|v| v.as_str()),
        Some(REDACTED_SECRET),
        "工程文件未脱敏 northbound.opcua.users[].password"
    );
}

/// round-trip 不变性：脱敏 → 原样回传 → 还原后，所有敏感字段恢复原值，
/// 且脱敏结果不含任何原始凭据明文。
///
/// 该断言表驱动自 `sensitive_field_paths()`：
/// - 清单新增路径但还原侧未实现 → 第 3 步失败；
/// - 还原侧多处理了清单外字段 → 第 2 步「脱敏后不得含明文」失败。
#[test]
fn redact_then_restore_round_trip_keeps_all_sensitive_fields() {
    let old = sample_config();
    let original = serde_json::to_value(&old).expect("serialize sample config");

    // 1) 脱敏：清单内每个路径都变为占位符
    let mut redacted = original.clone();
    redact_config_secrets(&mut redacted);
    for path in sensitive_field_paths() {
        assert_eq!(
            redacted.pointer(path).and_then(|v| v.as_str()),
            Some(REDACTED_SECRET),
            "敏感路径 {path} 未被脱敏（清单与样本不一致？）"
        );
    }
    let headers = redacted
        .pointer(SENSITIVE_SECRET_HEADERS_PATH)
        .and_then(|v| v.as_object())
        .expect("样本应含 webhook.headers");
    for (name, v) in headers {
        assert_eq!(
            v.as_str(),
            Some(REDACTED_SECRET),
            "webhook.headers.{name} 未被脱敏"
        );
    }
    for key in ["username", "password"] {
        assert_eq!(
            redacted
                .pointer(&format!("/buses/0/opcua/{key}"))
                .and_then(|v| v.as_str()),
            Some(REDACTED_SECRET),
            "buses[].opcua.{key} 未被脱敏"
        );
    }
    assert_eq!(
        redacted
            .pointer("/opcua/users/0/password")
            .and_then(|v| v.as_str()),
        Some(REDACTED_SECRET),
        "opcua.users[].password 未被脱敏"
    );

    // 2) 脱敏后的 JSON 不得再出现任何原始凭据明文
    let redacted_text = redacted.to_string();
    for value in secret_values(&original) {
        assert!(
            !redacted_text.contains(&value),
            "脱敏后仍出现原始凭据 {value}"
        );
    }

    // 3) 模拟前端原样回传（占位符反序列化为 typed config），再走保存路径还原
    let mut submitted: GatewayConfig =
        serde_json::from_value(redacted).expect("deserialize redacted config");
    restore_redacted_secrets(&old, &mut submitted);
    let restored = serde_json::to_value(&submitted).expect("serialize restored config");

    for path in sensitive_field_paths() {
        assert_eq!(
            restored.pointer(path),
            original.pointer(path),
            "敏感路径 {path} 往返后未恢复原值"
        );
    }
    assert_eq!(
        restored.pointer(SENSITIVE_SECRET_HEADERS_PATH),
        original.pointer(SENSITIVE_SECRET_HEADERS_PATH),
        "webhook.headers 往返后未恢复原值"
    );
    for key in ["username", "password"] {
        assert_eq!(
            restored.pointer(&format!("/buses/0/opcua/{key}")),
            original.pointer(&format!("/buses/0/opcua/{key}")),
            "buses[].opcua.{key} 往返后未恢复原值"
        );
    }
    assert_eq!(
        restored.pointer("/opcua/users/0/password"),
        original.pointer("/opcua/users/0/password"),
        "opcua.users[].password 往返后未恢复原值"
    );
    // 非敏感字段不受脱敏/还原影响
    assert_eq!(
        restored.pointer("/web/username"),
        original.pointer("/web/username")
    );
}

/// 契约：活跃点位集口径 = 展开点位 ∪ **启用中**的虚拟点位；
/// 写权限只来自普通点位的 `writable`（虚拟点位由公式派生，不接受外部写入）。
#[test]
fn active_points_include_only_enabled_virtual_points() {
    use gateway_core::config::VirtualPoint;

    let writable = PointConfig {
        id: "p_w".into(),
        writable: true,
        ..Default::default()
    };
    let readonly = PointConfig {
        id: "p_r".into(),
        ..Default::default()
    };
    let cfg = GatewayConfig {
        virtual_points: Some(vec![
            VirtualPoint {
                id: "v_on".into(),
                name: "v_on".into(),
                device_id: None,
                formula: "1".into(),
                unit: String::new(),
                decimals: 2,
                enabled: true,
                created_at: 0,
            },
            VirtualPoint {
                id: "v_off".into(),
                name: "v_off".into(),
                device_id: None,
                formula: "1".into(),
                unit: String::new(),
                decimals: 2,
                enabled: false,
                created_at: 0,
            },
        ]),
        ..Default::default()
    };

    let active = build_active_points(&[writable, readonly], &cfg);
    assert!(active.ids.contains("p_w"));
    assert!(active.ids.contains("p_r"));
    assert!(
        active.ids.contains("v_on"),
        "启用中的虚拟点位必须纳入清理口径"
    );
    assert!(!active.ids.contains("v_off"), "停用的虚拟点位不应保留节点");
    assert!(active.writable_ids.contains("p_w"));
    assert!(!active.writable_ids.contains("p_r"));
    assert!(!active.writable_ids.contains("v_on"));
}

/// `validate_all` 看不到模板库，手工点位与 devices 展开出的点位可能撞名。
/// 队列去重索引不含 device_id，撞名会在同批次互相覆盖 → 必须在落盘前拒绝。
#[test]
fn expanded_point_ids_must_be_unique_across_sources() {
    let manual = PointConfig {
        id: "device_pt".into(),
        ..Default::default()
    };
    let from_template = PointConfig {
        id: "device_pt".into(),
        ..Default::default()
    };
    // ApiError 未 derive Debug，不能用 expect_err（其边界要求 E: Debug）
    match ensure_unique_expanded_point_ids(&[manual, from_template]) {
        Err(err) => assert!(
            err.message.contains("重复"),
            "错误应说明重复原因: {}",
            err.message
        ),
        Ok(()) => panic!("手工点位与模板展开点位撞名必须拒绝"),
    }

    let distinct = vec![
        PointConfig {
            id: "manual_pt".into(),
            ..Default::default()
        },
        PointConfig {
            id: "template_pt".into(),
            ..Default::default()
        },
    ];
    assert!(
        ensure_unique_expanded_point_ids(&distinct).is_ok(),
        "不同 id 应通过"
    );
}

/// 自愈 / 看门狗重启必须走跳过配额的入口：license 失效时恰恰最需要重建运行态，
/// 而它只是重新应用磁盘上已有的配置，不引入新的点位规模。
///
/// 用文本契约而非构造完整 AppState：本模块其余测试都是纯函数级，而这条约束的
/// 失效方式是「调用点被改回普通入口」，一次断言即可守住注释里的豁免承诺。
#[test]
fn self_heal_reload_uses_quota_exempt_entry() {
    let heal = include_str!("../heal.rs");
    assert!(
        heal.contains("config_apply::persist_then_apply_skip_quota(state, &old, &new)"),
        "自愈重载必须调用 persist_then_apply_skip_quota，否则 license 降级后无法恢复采集"
    );

    let start = heal
        .find("pub(crate) async fn reload_services")
        .expect("reload_services 必须存在");
    assert!(
        !heal[start..].contains("persist_then_apply(state, &old, &new)"),
        "自愈重载不得回到受配额校验的普通入口"
    );
}


// ===== P0 提权门禁：系统级凭据字段变更须主 token =====

/// 前端回传占位符（未触碰凭据）不得被判为变更，否则每次保存都要主 token。
#[test]
fn redacted_web_secrets_are_not_treated_as_change() {
    let old = sample_config();
    let mut new = old.clone();
    if let Some(w) = new.web.as_mut() {
        w.api_token = "********".to_string();
        w.password_hash = "********".to_string();
    }
    let mut effective = new.clone();
    restore_redacted_secrets(&old, &mut effective);
    assert!(
        !system_secrets_changed(&old, &effective),
        "回传占位符经还原后应与旧值一致"
    );
}

/// 真实改写 api_token / password_hash / allow_anon_read 必须被识别为变更。
#[test]
fn real_web_secret_changes_are_detected() {
    let old = sample_config();
    for mutate in [
        |w: &mut WebConfig| w.api_token = "attacker-token".to_string(),
        |w: &mut WebConfig| w.password_hash = "attacker-hash".to_string(),
        |w: &mut WebConfig| w.allow_anon_read = !w.allow_anon_read,
    ] {
        let mut new = old.clone();
        if let Some(w) = new.web.as_mut() {
            mutate(w);
        }
        assert!(
            system_secrets_changed(&old, &new),
            "系统级凭据变更必须触发主 token 门禁"
        );
    }
}

/// 非系统字段（如点位）变更不应触发门禁。
#[test]
fn non_secret_changes_do_not_trigger_primary_token_gate() {
    let old = sample_config();
    let mut new = old.clone();
    new.points.push(gateway_core::config::PointConfig::default());
    assert!(!system_secrets_changed(&old, &new));
}

/// 三个整份配置入口必须全部接入门禁（文本契约，防止新增入口漏接）。
#[test]
fn all_whole_config_entrypoints_call_primary_token_gate() {
    let import_src = include_str!("../api/config/handlers/config.rs");
    assert!(
        import_src.contains("ensure_system_secrets_allowed"),
        "PUT /api/config/import 必须校验主 token"
    );
    let system_src = include_str!("../api/system/handlers/config.rs");
    let gate_count = system_src.matches("ensure_system_secrets_allowed").count();
    assert!(
        gate_count >= 2,
        "POST /api/config 与版本回滚必须都校验主 token（实际 {gate_count} 处）"
    );
}
