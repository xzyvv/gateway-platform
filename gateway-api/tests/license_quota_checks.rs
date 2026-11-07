//! license 配额校验集成测试
//!
//! 覆盖：POST /api/config 提交超限 GatewayConfig → 4021 quota_exceeded + 中文 message
//!
//! v2.1 口径：**只有采集点数是硬约束**（超限 → 4021）；
//! 协议种类数与北向通道数降级为**告警**（超限不阻断保存，见 `Quota::warned_kinds`）。
//! 因此本文件中 protocols / northbound 相关的集成断言验证的是「**不再** 4021」。
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

/// v2.1：协议种类数超限**不再**拒绝保存配置（降级为告警）
///
/// 显式配置了 `max_protocols` 上限时，超限只打 WARN 日志，不阻断交付。
#[tokio::test]
async fn test_post_config_protocols_over_quota_is_not_rejected() {
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
    if resp.status() == StatusCode::BAD_REQUEST {
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_ne!(
            v["code"],
            serde_json::json!(4021),
            "v2.1 起协议种类数只告警、不阻断保存：body = {v}"
        );
    }
}

/// v2.1：北向通道数超限**不再**拒绝保存配置（降级为告警）
#[tokio::test]
async fn test_post_config_northbound_over_quota_is_not_rejected() {
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
    if resp.status() == StatusCode::BAD_REQUEST {
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_ne!(
            v["code"],
            serde_json::json!(4021),
            "v2.1 起北向通道数只告警、不阻断保存：body = {v}"
        );
    }
}

// ===== B-1 回归：覆盖全部 7 类北向通道的配额计数 =====
//
// v2.1 起北向通道数不再阻断保存（只告警），因此这里不再用「超限 → 4021」
// 反推计数是否正确（该观测手段已随降级失效）。
// 计数正确性改由 gateway-api 单元测试 `count_northbound_channels_covers_all_sinks`
// 直接断言覆盖全部 7 类通道。
//
// 本文件保留一条端到端断言：显式配置了 northbound_channels 上限时，
// 超限**不得**拒绝保存（降级为告警）。
#[tokio::test]
async fn test_post_config_northbound_explicit_limit_does_not_block_save() {
    let mgr = manager_with_quota(
        gateway_core::license::Tier::Standard,
        Quota {
            northbound_channels: Some(2),
            ..Quota::default()
        },
    );
    let state = state_with_license(mgr);
    let app = gateway_api::web::build_router(state);
    // 启用 7 类北向通道（含 telemetry），远超显式约定的 2
    let cfg = config_with_n_northbound(7);
    let resp = app
        .oneshot(req_with_token(
            "POST",
            "/api/config",
            serde_json::to_value(&cfg).unwrap(),
        ))
        .await
        .unwrap();
    if resp.status() == StatusCode::BAD_REQUEST {
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_ne!(
            v["code"],
            serde_json::json!(4021),
            "v2.1 起北向通道数只告警、不阻断保存：body = {v}"
        );
    }
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
