use super::{
    map_data_point_to_value, should_register_anonymous_none_endpoint, user_token_ids,
    variant_to_f64, write_error_status, write_wait_budget, ActivePointSet, NorthServer,
    OpcUaServer, OpcUaServerConfig,
};

/// 契约：未配证书时必须注册 None 端点（否则服务没有任何可用端点）；
/// 已配证书且未显式 allow_anonymous 时必须不注册（关闭明文匿名读取通道）
#[test]
fn anonymous_none_endpoint_gating() {
    assert!(should_register_anonymous_none_endpoint(false, false));
    assert!(should_register_anonymous_none_endpoint(false, true));
    assert!(!should_register_anonymous_none_endpoint(true, false));
    assert!(should_register_anonymous_none_endpoint(true, true));
}

/// 契约：OpcUaServerConfig 默认不开放匿名 None 端点
#[test]
fn opcua_server_config_defaults_deny_anonymous_none() {
    assert!(!OpcUaServerConfig::default().allow_anonymous);
}

/// 契约：默认不开放写回、自动信任客户端证书、无用户
#[test]
fn opcua_server_config_defaults_close_write_and_users() {
    let cfg = OpcUaServerConfig::default();
    assert!(!cfg.write_enabled);
    assert!(cfg.trust_client_certs);
    assert!(cfg.users.is_empty());
}

/// 契约：配置了用户名时端点只列这些用户名；匿名仅在显式放行时追加
#[test]
fn user_token_ids_lists_configured_users() {
    let users = vec![
        ("alice".to_string(), "p1".to_string()),
        ("bob".to_string(), "p2".to_string()),
    ];
    assert_eq!(user_token_ids(&users, false), vec!["alice", "bob"]);
    let mut with_anonymous = user_token_ids(&users, true);
    assert_eq!(with_anonymous.pop().as_deref(), Some("ANONYMOUS"));
}

/// 契约：匿名 token 只由 `allow_anonymous` 决定——未配置用户不再隐式放行
/// 匿名（该组合由配置校验拦截，否则端点将没有任何可登录方式）
#[test]
fn user_token_ids_require_explicit_anonymous() {
    assert!(user_token_ids(&[], false).is_empty());
    assert_eq!(user_token_ids(&[], true), vec!["ANONYMOUS"]);
}

/// 契约：`source_ts` 是原始采集时刻，必须映射到 `source_timestamp`；
/// `source_ts == 0` 表示该样本**没有**原始时标（IEC 104 无时标类型，见 `asdu`），
/// 必须映射为 `None`。旧实现无条件 `from_timestamp_millis(0)`，得到 1970-01-01，
/// 客户端会按 source timestamp 判定数据陈旧，甚至直接丢弃该值。
#[test]
fn source_ts_maps_to_source_timestamp_not_epoch() {
    use crate::engine::model::DataPoint;

    let mut dp = DataPoint::good("p1", "d1", 1.0, 1.0, "u16");
    dp.source_ts = 1_700_000_000_000;
    dp.timestamp = 1_700_000_001_000;

    let dv = map_data_point_to_value(&dp);
    let expect: opcua::types::DateTime = chrono::DateTime::from_timestamp_millis(dp.source_ts)
        .expect("valid source_ts")
        .into();
    assert_eq!(
        dv.source_timestamp,
        Some(expect),
        "source_ts 必须映射为 source_timestamp"
    );
    assert_eq!(dv.source_picoseconds, Some(0));

    // 无原始时标：必须缺省，而不是 1970-01-01
    dp.source_ts = 0;
    let dv = map_data_point_to_value(&dp);
    assert!(
        dv.source_timestamp.is_none(),
        "source_ts=0 不得映射成 1970-01-01"
    );
    assert!(
        dv.source_picoseconds.is_none(),
        "无原始时标时 picoseconds 也应缺省"
    );
    // server_timestamp 不受影响，仍是上送时刻
    assert!(dv.server_timestamp.is_some());
}

/// 契约：写失败必须映射为可区分的 OPC UA 状态码，客户端据此判断原因
#[test]
fn write_error_maps_to_specific_status_codes() {
    use crate::engine::error::GatewayError;
    use opcua::types::StatusCode;

    assert_eq!(
        write_error_status(&GatewayError::Config("not writable".into())),
        StatusCode::BadNotWritable
    );
    assert_eq!(
        write_error_status(&GatewayError::NotFound("no point".into())),
        StatusCode::BadNodeIdUnknown
    );
    assert_eq!(
        write_error_status(&GatewayError::Protocol("crc".into())),
        StatusCode::BadDeviceFailure
    );
    assert_eq!(
        write_error_status(&GatewayError::Io("port closed".into())),
        StatusCode::BadDeviceFailure
    );
    assert_eq!(
        write_error_status(&GatewayError::NoReply),
        StatusCode::BadTimeout
    );
    assert_eq!(
        write_error_status(&GatewayError::Queue("db".into())),
        StatusCode::BadInternalError
    );
}

/// 契约：写等待预算必须覆盖总线「入队 + 应答」的最坏耗时，同时受上下限约束。
/// 旧实现固定 3s，总线 `timeout_ms ≥ 1000ms` 时正常慢写会先被判超时，
/// 客户端重试会重复写。
#[test]
fn write_wait_budget_follows_bus_timeout() {
    use std::time::Duration;

    // 默认 500ms 总线：推导 2s 低于下限，抬到 3s（保持旧行为）
    assert_eq!(
        write_wait_budget(Duration::from_millis(500)),
        Duration::from_secs(3)
    );
    // 总线调到 1500ms：恰好 3×1500+500=5000ms，顶到上限
    assert_eq!(
        write_wait_budget(Duration::from_millis(1500)),
        Duration::from_secs(5)
    );
    // 极端配置：按上限截断，避免长时间持有地址空间写锁阻塞读/订阅
    assert_eq!(
        write_wait_budget(Duration::from_millis(600_000)),
        Duration::from_secs(5)
    );
    assert_eq!(
        write_wait_budget(Duration::from_millis(3000)),
        Duration::from_secs(5)
    );
    // 任何取值都不得低于下限
    assert!(write_wait_budget(Duration::from_millis(1)) >= Duration::from_secs(3));
}

/// 契约：写回调只接受数值型（含 Boolean 按 0/1），其它类型拒绝而非静默转 0
#[test]
fn variant_to_f64_accepts_numeric_and_rejects_others() {
    use opcua::types::UAString;
    use opcua::types::Variant;

    assert_eq!(variant_to_f64(&Variant::Double(1.5)), Some(1.5));
    assert_eq!(variant_to_f64(&Variant::Float(2.5)), Some(2.5));
    assert_eq!(variant_to_f64(&Variant::Int32(-3)), Some(-3.0));
    assert_eq!(variant_to_f64(&Variant::Boolean(true)), Some(1.0));
    assert_eq!(variant_to_f64(&Variant::String(UAString::from("7"))), None);
}

/// 契约：`node_count` 必须反映真实节点——首次 update 即建缓存条目，不再恒为 0。
#[test]
fn update_value_seeds_node_cache() {
    let srv = OpcUaServer::new(OpcUaServerConfig::default(), None);
    assert_eq!(srv.node_count(), 0);

    srv.update_value("p1", 42.0);
    assert_eq!(srv.node_count(), 1);
    assert_eq!(srv.nodes.read().get("p1").map(|n| n.value), Some(42.0));

    srv.update_value("p1", 43.0);
    assert_eq!(srv.node_count(), 1, "同一节点重复更新不应重复计数");
    assert_eq!(srv.nodes.read().get("p1").map(|n| n.value), Some(43.0));
}

/// 契约：配置热更后按活跃点位集裁剪地址空间——已删除/改名的点位节点必须从
/// 地址空间、`point_to_node` 与 `nodes` 三处一并消失，仍在配置中的节点保留。
#[tokio::test]
async fn points_changed_prunes_stale_nodes() {
    use std::sync::Arc;
    use std::time::Duration;

    use opcua::server::node_manager::memory::SimpleNodeManager;
    use opcua::types::NodeId;

    let probe = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind probe");
    let port = probe.local_addr().expect("local_addr").port();
    drop(probe);

    let cfg = OpcUaServerConfig {
        enabled: true,
        endpoint: format!("opc.tcp://127.0.0.1:{port}"),
        ..Default::default()
    };
    let srv = Arc::new(OpcUaServer::new(cfg, None));
    let run_srv = srv.clone();
    let run_task = tokio::spawn(async move {
        let _ = run_srv.start().await;
    });

    let mut running = false;
    for _ in 0..50 {
        if srv.is_running() {
            running = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(running, "opcua server 未进入 running");

    srv.update_value("keep", 1.0);
    srv.update_value("stale", 2.0);
    assert_eq!(srv.node_count(), 2);

    let mut active = ActivePointSet::default();
    active.ids.insert("keep".to_string());
    srv.on_points_changed(&active);

    assert_eq!(srv.node_count(), 1, "stale 节点应已从缓存移除");
    assert!(srv.point_to_node.read().contains_key("keep"));
    assert!(!srv.point_to_node.read().contains_key("stale"));

    // 地址空间里也必须真的删除，否则客户端仍能浏览/订阅到冻结旧值
    let ns = *srv
        .namespace_index
        .lock()
        .as_ref()
        .expect("namespace index not set");
    let handle = srv.handle.lock().as_ref().cloned().expect("handle");
    {
        let nm = handle
            .node_managers()
            .get_of_type::<SimpleNodeManager>()
            .expect("node manager");
        let aspace = nm.address_space().read();
        assert!(aspace.node_exists(&NodeId::new(ns, "keep")));
        assert!(
            !aspace.node_exists(&NodeId::new(ns, "stale")),
            "地址空间中的僵尸节点未被删除"
        );
    }
    drop(handle);

    srv.stop().await.expect("stop");
    let _ = tokio::time::timeout(Duration::from_secs(5), run_task).await;
}
