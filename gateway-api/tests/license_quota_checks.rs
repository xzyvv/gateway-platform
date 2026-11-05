//! v2.0 Step 3 license 配额校验集成测试
//!
//! 覆盖：POST /api/config 提交超限 GatewayConfig → 4021 quota_exceeded + 中文 message
//! 4 类配额：points / protocols / northbound channels
//!
//! 测试方法：构造带 tier_quotas 的 LicenseConfig，注入 mock license manager，
//! 然后用 axum oneshot 发 POST /api/config，断言状态码 + body message。

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use gateway_core::config::{LicenseConfig, TierQuotaEntry};
use gateway_core::license::{LicenseManager, Quota};
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789abcdef";

fn req_with_token(method: &str, uri: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {}", TOKEN))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// 构造一个带 tier_quotas 的 LicenseManager（不在这里建 full AppState，只验 LicenseManager API）
///
/// 测试场景：注入 fake LicenseState（带 quota）→ check_quota 走 license 自带 quota 路径，
/// 不依赖 LicenseConfig 兜底。
fn manager_with_quota(tier: gateway_core::license::Tier, quota: Quota) -> Arc<LicenseManager> {
    use gateway_core::license::{LicenseState, LicenseStatus};
    let dir = tempfile::tempdir().unwrap();
    let mgr = Arc::new(LicenseManager::new(dir.path().join("clock.db").as_path()));
    // 注入带 quota 的 state（模拟已加载 license）
    mgr.replace_state_for_test(LicenseState {
        present: true,
        tier,
        quota: quota.clone(),
        quota_source: gateway_core::license::QuotaSource::License,
        status: LicenseStatus::Ok,
        expires_at: None,
        license_id: Some("LIC-TEST".into()),
        customer: None,
    });
    // 仍设 tier_quotas 配置作为 backup（防止某些测试同时依赖两条路径）
    let mut map = BTreeMap::new();
    map.insert(tier, quota);
    mgr.set_quota_config(LicenseConfig {
        tier_quotas: Some(
            map.into_iter()
                .map(|(t, q)| TierQuotaEntry { tier: t, quota: q })
                .collect(),
        ),
        ..LicenseConfig::default()
    });
    mgr
}

// ===== LicenseManager::check_quota 单元测试（4 类）=====

#[test]
fn test_check_quota_points_under_limit_passes() {
    let mgr = manager_with_quota(
        gateway_core::license::Tier::Standard,
        Quota {
            max_points: Some(1500),
            ..Quota::default()
        },
    );
    assert!(mgr
        .check_quota(gateway_core::license::QuotaKind::Points, 1000)
        .is_ok());
}

#[test]
fn test_check_quota_points_at_limit_passes() {
    let mgr = manager_with_quota(
        gateway_core::license::Tier::Standard,
        Quota {
            max_points: Some(1500),
            ..Quota::default()
        },
    );
    assert!(mgr
        .check_quota(gateway_core::license::QuotaKind::Points, 1500)
        .is_ok());
}

#[test]
fn test_check_quota_points_over_limit_rejected() {
    let mgr = manager_with_quota(
        gateway_core::license::Tier::Standard,
        Quota {
            max_points: Some(1500),
            ..Quota::default()
        },
    );
    let err = mgr
        .check_quota(gateway_core::license::QuotaKind::Points, 1501)
        .unwrap_err();
    assert_eq!(err.kind, gateway_core::license::QuotaKind::Points);
    assert_eq!(err.limit, 1500);
    assert_eq!(err.attempted, 1501);
    // 中文 message 含"1500"和"采集点数"
    let msg = err.to_string();
    assert!(msg.contains("1500"), "msg = {}", msg);
    assert!(msg.contains("采集点数"), "msg = {}", msg);
}

#[test]
fn test_check_quota_protocols_over_limit_rejected() {
    let mgr = manager_with_quota(
        gateway_core::license::Tier::Standard,
        Quota {
            max_protocols: Some(8),
            ..Quota::default()
        },
    );
    let err = mgr
        .check_quota(gateway_core::license::QuotaKind::Protocols, 9)
        .unwrap_err();
    assert_eq!(err.kind, gateway_core::license::QuotaKind::Protocols);
    assert_eq!(err.limit, 8);
    assert!(err.to_string().contains("并发协议数"));
}

#[test]
fn test_check_quota_northbound_over_limit_rejected() {
    let mgr = manager_with_quota(
        gateway_core::license::Tier::Standard,
        Quota {
            northbound_channels: Some(3),
            ..Quota::default()
        },
    );
    let err = mgr
        .check_quota(gateway_core::license::QuotaKind::NorthboundChannels, 4)
        .unwrap_err();
    assert_eq!(
        err.kind,
        gateway_core::license::QuotaKind::NorthboundChannels
    );
    assert_eq!(err.limit, 3);
    assert!(err.to_string().contains("北向通道数"));
}

#[test]
fn test_check_quota_unlimited_passes_for_any_value() {
    let mgr = manager_with_quota(
        gateway_core::license::Tier::Ultra,
        Quota {
            max_points: None, // 无限
            max_protocols: None,
            storage_days: None,
            northbound_channels: None,
        },
    );
    assert!(mgr
        .check_quota(gateway_core::license::QuotaKind::Points, 1_000_000)
        .is_ok());
    assert!(mgr
        .check_quota(
            gateway_core::license::QuotaKind::NorthboundChannels,
            1_000_000
        )
        .is_ok());
}

// ===== POST /api/config 配额集成测试 =====

/// 最小有效 GatewayConfig（保持全字段默认）
fn minimal_config() -> gateway_core::config::GatewayConfig {
    use gateway_core::config::GatewayConfig;
    GatewayConfig::default()
}

/// 构造 N 个点位（占用配额）
fn config_with_n_points(n: usize) -> gateway_core::config::GatewayConfig {
    use gateway_core::config::{BusConfig, PointConfig};
    let mut cfg = minimal_config();
    cfg.buses = vec![BusConfig::default()];
    cfg.points = (0..n)
        .map(|i| PointConfig {
            id: format!("p{i}"),
            name: Some(format!("point-{i}")),
            device_id: None,
            bus: "bus0".into(),
            protocol: "modbus".into(),
            address: format!("{i}"),
            slave_id: Some(1),
            address_num: Some(i as u16),
            data_id: None,
            function_code: Some(3),
            data_type: Some("uint16".into()),
            byte_order: None,
            interval: 1,
            ..Default::default()
        })
        .collect();
    cfg
}

/// 构造 N 个不同协议的点位（占用 N 个 protocol 配额）
fn config_with_n_protocols(n: usize) -> gateway_core::config::GatewayConfig {
    use gateway_core::config::{BusConfig, PointConfig};
    let mut cfg = minimal_config();
    cfg.buses = vec![BusConfig::default()];
    // 使用 VALID_PROTOCOLS 中的合法协议名（配置结构校验会拒绝未知协议）
    let protos = [
        "modbus",
        "dlt645",
        "cjt188",
        "opcua",
        "iec104",
        "bacnet",
        "s7comm",
        "bacnet_mstp",
    ];
    cfg.points = (0..n)
        .map(|i| PointConfig {
            id: format!("p{i}"),
            bus: "bus0".into(),
            protocol: protos[i % protos.len()].into(),
            address: format!("{i}"),
            address_num: Some(i as u16),
            function_code: Some(3),
            interval: 1,
            ..Default::default()
        })
        .collect();
    cfg
}

/// 构造启用 N 个北向通道的 config
///
/// 维护说明（B-1 修复后）：北向通道总计 7 类（mqtt / webhook /
/// cellular / opcua / modbus_server / modbus_passthrough / telemetry），
/// 配额计数应与之一一对应。
///
/// 本 helper 按"逐类开启"的方式构造，可直接断言 `count == n`：
/// - n=1  → mqtt
/// - n=2  → +webhook
/// - n=3  → +cellular
/// - n=4  → +opcua
/// - n=5  → +modbus_server
/// - n=6  → +modbus_passthrough
/// - n=7  → +telemetry
fn config_with_n_northbound(n: usize) -> gateway_core::config::GatewayConfig {
    use gateway_core::config::{
        CellularConfig, ModbusPassthroughConfig, ModbusServerConfig, MqttConfig, OpcuaConfig,
        TelemetryConfig, WebhookConfig,
    };
    let mut cfg = minimal_config();
    cfg.mqtt = MqttConfig {
        enabled: n >= 1,
        broker: "mqtt://localhost:1883".into(),
        topic: "factory/line1".into(),
        publish_interval: 5,
        qos: 1,
        ..Default::default()
    };
    if n >= 2 {
        cfg.webhook = Some(WebhookConfig {
            enabled: true,
            ..Default::default()
        });
    }
    if n >= 3 {
        cfg.cellular = Some(CellularConfig {
            enabled: true,
            ..Default::default()
        });
    }
    if n >= 4 {
        // 第 4 类：opcua（始终计入，不受 feature gate 影响）
        cfg.opcua = Some(OpcuaConfig {
            enabled: true,
            ..Default::default()
        });
    }
    if n >= 5 {
        cfg.modbus_server = Some(ModbusServerConfig {
            enabled: true,
            ..Default::default()
        });
    }
    if n >= 6 {
        cfg.modbus_passthrough = Some(ModbusPassthroughConfig {
            enabled: true,
            ..Default::default()
        });
    }
    if n >= 7 {
        // 第 7 类：telemetry。仅 telemetry 是 OTLP 上报，但同样占用北向配额。
        cfg.telemetry = Some(TelemetryConfig {
            enabled: true,
            ..Default::default()
        });
    }
    cfg
}

/// mock_state + 注入 license manager
fn state_with_license(mgr: Arc<LicenseManager>) -> Arc<gateway_api::web::types::AppState> {
    common::mock_state::mock_app_state_with_license_manager(mgr)
}

#[tokio::test]
async fn test_post_config_points_under_quota_succeeds() {
    let mgr = manager_with_quota(
        gateway_core::license::Tier::Standard,
        Quota {
            max_points: Some(1500),
            ..Quota::default()
        },
    );
    let state = state_with_license(mgr);
    let app = gateway_api::web::build_router(state);
    let cfg = config_with_n_points(500);
    let resp = app
        .oneshot(req_with_token(
            "POST",
            "/api/config",
            serde_json::to_value(&cfg).unwrap(),
        ))
        .await
        .unwrap();
    // 期望非 4021（实际可能是 200/201/500，取决于完整 init_collectors；至少不能是 quota_exceeded）
    if resp.status() == StatusCode::BAD_REQUEST {
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_ne!(v["code"], serde_json::json!(4021), "不应触发配额限制");
    }
}

#[tokio::test]
async fn test_post_config_points_over_quota_returns_4021() {
    let mgr = manager_with_quota(
        gateway_core::license::Tier::Standard,
        Quota {
            max_points: Some(1500),
            ..Quota::default()
        },
    );
    let state = state_with_license(mgr);
    let app = gateway_api::web::build_router(state);
    let cfg = config_with_n_points(2000);
    let resp = app
        .oneshot(req_with_token(
            "POST",
            "/api/config",
            serde_json::to_value(&cfg).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["code"], serde_json::json!(4021));
    let msg = v["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("1500"),
        "msg should contain limit 1500: {}",
        msg
    );
    assert!(
        msg.contains("采集点数"),
        "msg should contain '采集点数': {}",
        msg
    );
}

#[tokio::test]
async fn test_post_config_protocols_over_quota_returns_4021() {
    let mgr = manager_with_quota(
        gateway_core::license::Tier::Standard,
        Quota {
            max_protocols: Some(3),
            ..Quota::default()
        },
    );
    let state = state_with_license(mgr);
    let app = gateway_api::web::build_router(state);
    let cfg = config_with_n_protocols(5);
    let resp = app
        .oneshot(req_with_token(
            "POST",
            "/api/config",
            serde_json::to_value(&cfg).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["code"], serde_json::json!(4021));
    assert!(v["message"].as_str().unwrap().contains("并发协议数"));
}

#[tokio::test]
async fn test_post_config_northbound_over_quota_returns_4021() {
    let mgr = manager_with_quota(
        gateway_core::license::Tier::Standard,
        Quota {
            northbound_channels: Some(2),
            ..Quota::default()
        },
    );
    let state = state_with_license(mgr);
    let app = gateway_api::web::build_router(state);
    let cfg = config_with_n_northbound(3); // mqtt + webhook + cellular = 3 > 2
    let resp = app
        .oneshot(req_with_token(
            "POST",
            "/api/config",
            serde_json::to_value(&cfg).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["code"], serde_json::json!(4021));
    assert!(v["message"].as_str().unwrap().contains("北向通道数"));
}

// ===== B-1 回归：覆盖全部 7 类北向通道的配额计数 =====

/// B-1 修复：配额计数必须把全部 7 类通道都计入
/// （mqtt / webhook / cellular / opcua / modbus_server /
/// modbus_passthrough / telemetry）。
///
/// 回归证据：旧实现只覆盖前 4 类，把启用了 opcua / modbus_server /
/// modbus_passthrough / telemetry 的配置错误地算成"0 类占用"，配额形同
/// 虚设（绕过付费墙）。本测试用端到端 POST /api/config 验证：每次「多启
/// 一类」都让 next 加 1，从而能精确定位配额是否按预期递增。
///
/// 由于 count_northbound_channels 私有（pub(crate)），本测试用"边界对照"
/// 表征其正确性：在 limit=N 时启用 N+1 类必拒；启用 ≤N 类必通（同 status
/// 不是 4021）。
async fn post_northbound_helper(n_enabled: usize, limit: u32) -> StatusCode {
    let mgr = manager_with_quota(
        gateway_core::license::Tier::Standard,
        Quota {
            northbound_channels: Some(limit),
            ..Quota::default()
        },
    );
    let state = state_with_license(mgr);
    let app = gateway_api::web::build_router(state);
    let cfg = config_with_n_northbound(n_enabled);
    let resp = app
        .oneshot(req_with_token(
            "POST",
            "/api/config",
            serde_json::to_value(&cfg).unwrap(),
        ))
        .await
        .unwrap();
    let status = resp.status();
    if status == StatusCode::BAD_REQUEST {
        // 触发配额超限的 400，验证确实是 quota_exceeded
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            v["code"],
            serde_json::json!(4021),
            "n={n_enabled} limit={limit}: 400 必须是 quota_exceeded，不是其他业务错误；body = {v}"
        );
        assert!(
            v["message"].as_str().unwrap().contains("北向通道数"),
            "配额超限 message 必须含「北向通道数」；body = {v}"
        );
    }
    status
}

#[tokio::test]
async fn test_b1_northbound_quota_is_per_sink_not_per_legacy_4() {
    // 旧实现下：n_enabled=5 时 count 仍为 4（漏算 opcua），limit=4 应被拒。
    //     新实现下：count=5 > 4 → 同样应被拒。
    // 通过断言 n=5, limit=4 → 400/4021 验证 opcua 已被计入。
    let status = post_northbound_helper(5, 4).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "B-1 回归：启用 5 类（mqtt+webhook+cellular+opcua+modbus_server），limit=4，\
         必须 4021 拒。旧实现会把 opcua 漏掉算成 4 类通过 → 现已修复应拒。"
    );

    // 关键回归：n=5, limit=5 → 不应被拒（不是 4021）。
    let status = post_northbound_helper(5, 5).await;
    assert_ne!(
        status,
        StatusCode::BAD_REQUEST,
        "B-1 回归：limit=5 时启 5 类（含 opcua）应通过；status = {status}"
    );

    // 启用第 7 类（telemetry），limit=6 → 应被拒（关键：telemetry 也算 1 类）
    let status = post_northbound_helper(7, 6).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "B-1 回归：启用 7 类（含 telemetry），limit=6 必须 4021 拒；\
         旧实现下 telemetry 漏算 → 会通过 → 这是付费墙绕过的根因。"
    );

    // 反向：仅启用 1 类（mqtt），即使 limit=1 也应通过（不是 4021）
    let status = post_northbound_helper(1, 1).await;
    assert_ne!(
        status,
        StatusCode::BAD_REQUEST,
        "B-1 回归：limit=1 时仅启用 mqtt（1 类）应通过；status = {status}"
    );
}

#[tokio::test]
async fn test_post_config_telemetry_counts_against_northbound_quota() {
    // 直接复用上面的 helper：启用第 7 类（包含 telemetry）+ limit=6 → 必拒
    let status = post_northbound_helper(7, 6).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "启用 7 类北向通道（包含 telemetry）应超 limit=6 配额并返回 400/4021"
    );
}

#[tokio::test]
async fn test_post_config_unlimited_quota_accepts_any_size() {
    let mgr = manager_with_quota(
        gateway_core::license::Tier::Ultra,
        Quota {
            max_points: None,
            max_protocols: None,
            storage_days: None,
            northbound_channels: None,
        },
    );
    let state = state_with_license(mgr);
    let app = gateway_api::web::build_router(state);
    let cfg = config_with_n_points(10000);
    let resp = app
        .oneshot(req_with_token(
            "POST",
            "/api/config",
            serde_json::to_value(&cfg).unwrap(),
        ))
        .await
        .unwrap();
    // 无限配额 → 不应返回 4021
    if resp.status() == StatusCode::BAD_REQUEST {
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_ne!(v["code"], serde_json::json!(4021));
    }
}

// ===== C3：增量端点（批量导入）统一走 reload_then_save 配额前置校验 =====

/// 批量导入点位超限 → 4021（证明增量端点也受配额约束，不再只有 POST /api/config 校验）。
#[tokio::test]
async fn test_batch_import_points_over_quota_returns_4021() {
    let mgr = manager_with_quota(
        gateway_core::license::Tier::Standard,
        Quota {
            max_points: Some(1500),
            ..Quota::default()
        },
    );
    let state = state_with_license(mgr);
    let app = gateway_api::web::build_router(state);
    let points = config_with_n_points(2000).points;
    let resp = app
        .oneshot(req_with_token(
            "POST",
            "/api/points/batch",
            serde_json::to_value(&points).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["code"], serde_json::json!(4021));
    assert!(v["message"].as_str().unwrap().contains("采集点数"));
}

/// 批量导入点位未超限 → 不应返回 4021（配额校验不得误拒）。
#[tokio::test]
async fn test_batch_import_points_under_quota_not_rejected() {
    let mgr = manager_with_quota(
        gateway_core::license::Tier::Standard,
        Quota {
            max_points: Some(1500),
            ..Quota::default()
        },
    );
    let state = state_with_license(mgr);
    let app = gateway_api::web::build_router(state);
    let points = config_with_n_points(100).points;
    let resp = app
        .oneshot(req_with_token(
            "POST",
            "/api/points/batch",
            serde_json::to_value(&points).unwrap(),
        ))
        .await
        .unwrap();
    if resp.status() == StatusCode::BAD_REQUEST {
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_ne!(v["code"], serde_json::json!(4021), "未超限不应触发配额限制");
    }
}

// 避免 unused warning
#[allow(dead_code)]
fn _unused() {
    // 暂无需要 placehold 的内部引用
}
