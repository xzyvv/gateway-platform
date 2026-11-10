use crate::config::mqtt::MqttTlsConfig;
use crate::config::GatewayConfig;

fn cfg_with_mqtt(broker: &str, tls: Option<MqttTlsConfig>) -> GatewayConfig {
    let mut cfg = GatewayConfig::default();
    cfg.mqtt.enabled = true;
    cfg.mqtt.broker = broker.to_string();
    cfg.mqtt.tls = tls;
    cfg
}

/// P1 配置化：队列行数上限的范围校验。默认值必须与配置化前一致（100 万），
/// 越界值必须在配置期被拒（运行期淘汰线与水位判定都依赖它）。
#[test]
fn queue_max_rows_range_is_validated() {
    let cfg = GatewayConfig::default();
    assert_eq!(cfg.queue.max_rows, 1_000_000, "默认值必须保持不变");
    cfg.validate_all().expect("默认配置必须合法");

    for bad in [0, 199_999, 50_000_001] {
        let mut cfg = GatewayConfig::default();
        cfg.queue.max_rows = bad;
        let errs = cfg.validate_all().expect_err("越界上限必须被拒");
        assert!(
            errs.iter().any(|e| e.contains("queue.max_rows")),
            "错误信息应指明具体字段: {:?}",
            errs
        );
    }
}

/// P1 配置化：WS 并发连接上限的范围校验。默认值必须保持 64。
#[test]
fn realtime_max_subscribers_range_is_validated() {
    let mut cfg = GatewayConfig::default();
    let mut web: crate::config::WebConfig =
        serde_json::from_str("{}").expect("WebConfig 全字段都有 serde 默认");
    assert_eq!(web.realtime_max_subscribers, 64);

    web.realtime_max_subscribers = 0;
    cfg.web = Some(web);
    let errs = cfg.validate_all().expect_err("0 个连接必须被拒");
    assert!(
        errs.iter().any(|e| e.contains("realtime_max_subscribers")),
        "错误信息应指明具体字段: {:?}",
        errs
    );

    let mut web: crate::config::WebConfig = serde_json::from_str("{}").unwrap();
    web.realtime_max_subscribers = crate::config::REALTIME_MAX_SUBSCRIBERS_MAX + 1;
    cfg.web = Some(web);
    assert!(cfg.validate_all().is_err(), "超过上限必须被拒");
}

/// `insecure_skip_verify` 未实现，必须在配置期拒绝，而不是只打一行日志。
#[test]
fn mqtt_insecure_skip_verify_is_rejected_at_config_time() {
    let cfg = cfg_with_mqtt(
        "mqtts://broker.example:8883",
        Some(MqttTlsConfig {
            ca_cert_path: None,
            insecure_skip_verify: true,
        }),
    );
    let errs = cfg.validate_all().expect_err("应拒绝未实现的开关");
    assert!(
        errs.iter().any(|e| e.contains("insecure_skip_verify")),
        "错误信息应指明具体字段: {:?}",
        errs
    );
}

/// 明文链路下配置 TLS 证书材料会被静默忽略，同样属于撒谎，必须拒绝。
#[test]
fn mqtt_tls_material_on_plaintext_scheme_is_rejected() {
    let cfg = cfg_with_mqtt(
        "mqtt://broker.example:1883",
        Some(MqttTlsConfig {
            ca_cert_path: Some("/etc/gateway/ca.pem".to_string()),
            insecure_skip_verify: false,
        }),
    );
    let errs = cfg.validate_all().expect_err("明文链路配置 TLS 材料应拒绝");
    assert!(
        errs.iter().any(|e| e.contains("不会生效")),
        "应说明该配置不生效: {:?}",
        errs
    );
}

/// 正例：wss + 自定义 CA 是合法组合（此前该组合被静默丢弃）。
#[test]
fn mqtt_wss_with_custom_ca_is_accepted() {
    let cfg = cfg_with_mqtt(
        "wss://broker.example:8084/mqtt",
        Some(MqttTlsConfig {
            ca_cert_path: Some("/etc/gateway/ca.pem".to_string()),
            insecure_skip_verify: false,
        }),
    );
    cfg.validate_all().expect("wss + 自定义 CA 应通过校验");
}

/// broker scheme 白名单必须与运行时 ``parse_broker`` 支持的一致。
#[test]
fn mqtt_broker_scheme_matches_runtime_parser() {
    for broker in [
        "tcp://localhost:1883", // 出厂 config.json / dummy 配置使用的写法
        "tls://broker:8883",    // MQTT over TLS
        "mqtt://broker:1883",
        "mqtts://broker:8883",
        "ws://broker:9001/mqtt",
        "wss://broker:8084/mqtt",
        "localhost:1883", // 裸 host:port，parse_broker 按 tcp://1883 处理
    ] {
        let cfg = cfg_with_mqtt(broker, None);
        cfg.validate_all()
            .unwrap_or_else(|e| panic!("{broker} 应通过配置校验: {e:?}"));
    }

    // 不支持的 scheme 仍要拒绝：否则 parse_broker 会把 "http://x" 当主机名
    for broker in ["http://broker:1883", ""] {
        let cfg = cfg_with_mqtt(broker, None);
        let errs = cfg.validate_all().expect_err("非法 broker 应被拒绝");
        assert!(
            errs.iter().any(|e| e.contains("mqtt.broker")),
            "{broker:?}: {errs:?}"
        );
    }
}

// ===== 配置负例：未知协议 / 非法枚举 / 缺失必填，必须在门禁期 fail-fast =====

fn point_with(protocol: &str) -> crate::config::PointConfig {
    crate::config::PointConfig {
        id: "p1".to_string(),
        bus: "bus1".to_string(),
        protocol: protocol.to_string(),
        address: "1".to_string(),
        interval: 10,
        ..Default::default()
    }
}

/// 拼写错误的协议名不能等到运行期建 Collector 时才暴露。
#[test]
fn unknown_point_protocol_is_rejected_by_gate() {
    let mut cfg = GatewayConfig::default();
    cfg.points.push(point_with("modbu"));
    let errs = cfg.validate_all().expect_err("未知协议名应被拒绝");
    assert!(
        errs.iter()
            .any(|e| e.contains("points[0].protocol") && e.contains("modbu")),
        "错误应定位到 points[0].protocol 并回显非法值: {:?}",
        errs
    );
}

/// 功能码必须按协议分流校验。
///
/// 旧实现用 Modbus 白名单（1~6/15/16）校验所有协议，导致 DLT645 的
/// 写通信地址(0x0C=12)/写密码(0x0F=15) 与 CJT188 的写通信地址(0x15=21)
/// 存不进配置；同时 Modbus 点位配上 DLT645 控制码又会被静默写错。
#[test]
fn function_code_is_validated_per_protocol() {
    // DLT645：0x0C 写通信地址必须能存进去
    let mut cfg = GatewayConfig::default();
    let mut p = point_with("dlt645");
    p.function_code = Some(0x0C);
    cfg.points.push(p);
    assert!(
        cfg.validate_all().is_ok(),
        "DLT645 写通信地址(0x0C) 应通过校验: {:?}",
        cfg.validate_all().err()
    );

    // DLT645：0x0F 写密码同样合法
    let mut cfg = GatewayConfig::default();
    let mut p = point_with("dlt645");
    p.function_code = Some(0x0F);
    cfg.points.push(p);
    assert!(cfg.validate_all().is_ok(), "DLT645 写密码(0x0F) 应通过校验");

    // CJT188：0x15 写通信地址
    let mut cfg = GatewayConfig::default();
    let mut p = point_with("cjt188");
    p.function_code = Some(0x15);
    cfg.points.push(p);
    assert!(
        cfg.validate_all().is_ok(),
        "CJT188 写通信地址(0x15) 应通过校验"
    );

    // Modbus：常规功能码必须照旧合法
    let mut cfg = GatewayConfig::default();
    let mut p = point_with("modbus");
    p.address_num = Some(0);
    p.function_code = Some(3);
    cfg.points.push(p);
    assert!(cfg.validate_all().is_ok(), "Modbus FC03 应通过校验");

    // Modbus：DLT645 控制码必须被拒（否则会静默写错设备）
    let mut cfg = GatewayConfig::default();
    let mut p = point_with("modbus");
    p.address_num = Some(0);
    p.function_code = Some(0x0C);
    cfg.points.push(p);
    let errs = cfg
        .validate_all()
        .expect_err("Modbus 配 DLT645 控制码应被拒绝");
    assert!(
        errs.iter()
            .any(|e| e.contains("function_code") && e.contains("12")),
        "错误应指出 function_code 非法: {:?}",
        errs
    );

    // DLT645：不认识的控制码不阻断加载（存量配置可能带 Modbus 遗留值），
    // 运行期会落到「写数据」分支，属于无害降级而非静默写错寄存器。
    let mut cfg = GatewayConfig::default();
    let mut p = point_with("dlt645");
    p.function_code = Some(3);
    cfg.points.push(p);
    assert!(
        cfg.validate_all().is_ok(),
        "DLT645 未知控制码不应阻断配置加载: {:?}",
        cfg.validate_all().err()
    );
}

#[test]
fn unknown_bus_protocol_is_rejected_by_gate() {
    let mut cfg = GatewayConfig::default();
    cfg.buses.push(crate::config::BusConfig {
        port_name: "bus1".to_string(),
        protocol: "not-a-protocol".to_string(),
        ..Default::default()
    });
    let errs = cfg.validate_all().expect_err("未知总线协议应被拒绝");
    assert!(
        errs.iter()
            .any(|e| e.contains("buses[0].protocol") && e.contains("not-a-protocol")),
        "错误应定位到 buses[0].protocol: {:?}",
        errs
    );
}

/// 非法枚举字符串必须报错，而不是静默回退到默认值（ABCD / None）。
#[test]
fn illegal_enum_strings_are_rejected_not_silently_defaulted() {
    let mut cfg = GatewayConfig::default();
    let mut p = point_with("modbus");
    p.address_num = Some(0);
    p.data_type = Some("float128".to_string());
    p.byte_order = Some("ABCDE".to_string());
    cfg.points.push(p);

    let errs = cfg.validate_all().expect_err("非法枚举值应被拒绝");
    assert!(
        errs.iter().any(|e| e.contains("data_type")),
        "应指出非法 data_type: {:?}",
        errs
    );
    assert!(
        errs.iter().any(|e| e.contains("byte_order")),
        "应指出非法 byte_order: {:?}",
        errs
    );
}

/// `Option<SelfHealConfig>` 上的 `#[validate(nested)]` 必须真正生效。
///
/// 本文件上方对 `devices` / `points[].aggregation` / `buses[].opcua` 都做了
/// 手动展开，理由是「validator 未给 `Option<T>` 实现 `Validate`」。但实测
/// validator 0.21 + validator_derive 0.20 的 derive 对 `#[validate(nested)]`
/// 额外做了 Option 包裹（`if let Some(ref field) = ...`），嵌套校验照样跑。
/// 本用例把这一事实钉死：若将来升级 validator 导致 nested 失效，
/// `self_heal` 的 range 约束（周期/去抖/窗口等必须 >= 1）会静默失效，
/// 现场表现为自愈执行器按 0 周期疯狂重试。
#[test]
fn self_heal_option_nested_validation_is_enforced() {
    let cfg = GatewayConfig {
        self_heal: Some(crate::config::SelfHealConfig {
            enabled: true,
            check_interval_secs: 0, // 违反 range(min = 1)
            ..Default::default()
        }),
        ..Default::default()
    };
    let errs = cfg
        .validate_all()
        .expect_err("self_heal 嵌套校验必须生效，0 周期应被拒绝");
    assert!(
        errs.iter()
            .any(|e| e.contains("self_heal") && e.contains("check_interval_secs")),
        "错误应定位到 self_heal.check_interval_secs: {:?}",
        errs
    );
}

/// 缺失必填字段（周期为 0 / Modbus 缺寻址字段 / 总线协议为空）一次性全部报出。
#[test]
fn missing_required_fields_are_rejected_by_gate() {
    let mut cfg = GatewayConfig::default();
    cfg.buses.push(crate::config::BusConfig {
        port_name: "bus1".to_string(),
        protocol: String::new(), // 必填但为空
        ..Default::default()
    });
    let mut p = point_with("modbus");
    p.interval = 0; // 必填，且 0 会让采集永不触发
    p.address_num = None; // Modbus 寻址只认该字段
    cfg.points.push(p);

    let errs = cfg.validate_all().expect_err("缺失必填字段应被拒绝");
    assert!(
        errs.iter().any(|e| e.contains("buses[0].protocol")),
        "应指出空协议总线: {:?}",
        errs
    );
    assert!(
        errs.iter().any(|e| e.contains("interval")),
        "应指出 interval 非法: {:?}",
        errs
    );
    assert!(
        errs.iter().any(|e| e.contains("address_num")),
        "应指出 Modbus 缺 address_num: {:?}",
        errs
    );
}

/// 周期冲突校验不随上报模式放宽：`snapshot` 只是折叠上行，不改变「采样 < 上报」的约束。
#[test]
fn cycle_conflict_is_strict_in_both_report_modes() {
    use crate::config::mqtt::ReportMode;

    let mut cfg = GatewayConfig::default();
    cfg.mqtt.enabled = true;
    cfg.mqtt.publish_interval = 5;
    cfg.points = vec![serde_json::from_str(
        r#"{"id":"p1","name":"p1","device_id":"d1","bus":"","protocol":"modbus","address":"1","data_type":"u16","interval":5}"#,
    )
    .expect("minimal point config")];

    for mode in [ReportMode::Stream, ReportMode::Snapshot] {
        cfg.mqtt.report_mode = mode;
        assert!(
            cfg.validate_cycle_conflict().is_err(),
            "{mode:?} 必须拒绝 interval == publish_interval"
        );
    }

    cfg.points[0].interval = 4;
    cfg.validate_cycle_conflict()
        .expect("4s 采样 / 5s 上报在两种模式下都应通过");
}

/// snapshot 折叠会把同周期的多个聚合窗口压成 1 条，窗口必须 >= 上报周期。
#[test]
fn snapshot_aggregation_window_must_cover_publish_interval() {
    use crate::config::mqtt::ReportMode;
    use crate::config::AggregationConfig;
    use crate::engine::model::AggFn;

    let mut cfg = GatewayConfig::default();
    cfg.mqtt.enabled = true;
    cfg.mqtt.publish_interval = 5;
    cfg.mqtt.report_mode = ReportMode::Snapshot;
    cfg.points = vec![serde_json::from_str(
        r#"{"id":"p1","name":"p1","device_id":"d1","bus":"","protocol":"modbus","address":"1","data_type":"u16","interval":1}"#,
    )
    .expect("minimal point config")];

    cfg.points[0].aggregation = Some(AggregationConfig {
        window: 1,
        function: AggFn::Avg,
    });
    assert!(
        cfg.validate_snapshot_aggregation().is_err(),
        "window(1s) < 上报周期(5s) 必须拒绝"
    );

    cfg.points[0].aggregation = Some(AggregationConfig {
        window: 5,
        function: AggFn::Avg,
    });
    cfg.validate_snapshot_aggregation()
        .expect("window == 上报周期 应通过");

    // stream 模式逐条上行，不受该约束
    cfg.mqtt.report_mode = ReportMode::Stream;
    cfg.points[0].aggregation = Some(AggregationConfig {
        window: 1,
        function: AggFn::Avg,
    });
    cfg.validate_snapshot_aggregation()
        .expect("stream 模式无需限制聚合窗口");
}

/// MQTT 与模拟消费者都是队列的 ack 消费者，同时启用会分流数据导致静默丢数，
/// 必须在配置期拒绝（此前只有 bootstrap 的 warn 与 API 的 409 两道运行期防线）。
#[test]
fn consumer_exclusivity_is_rejected_at_config_time() {
    let mut cfg = GatewayConfig::default();
    cfg.mqtt.enabled = true;
    cfg.sim_consumer.enabled = true;

    let errs = cfg
        .validate_all()
        .expect_err("mqtt 与 sim_consumer 同时启用必须被拒");
    assert!(
        errs.iter().any(|e| e.contains("sim_consumer.enabled")),
        "错误信息应指明具体字段: {:?}",
        errs
    );

    // 两者只开一个都必须放行
    cfg.sim_consumer.enabled = false;
    cfg.validate_all().expect("只开 mqtt 应通过校验");

    cfg.mqtt.enabled = false;
    cfg.sim_consumer.enabled = true;
    cfg.validate_all().expect("只开 sim_consumer 应通过校验");
}
