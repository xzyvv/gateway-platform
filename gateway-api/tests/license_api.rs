//! License API 集成测试（v2.0）
//!
//! 用 MockState 构造 AppState，通过 axum oneshot 发请求，验证：
//! - GET /api/license：200 + 正确 JSON 结构（state + quota + machine）
//! - POST /api/license/import：无效文件拒收不落盘、合法签名导入成功
//!
//! v1.2 的 require_tier_* / require_*_feature 中间件已在 v2.0 删除
//! （调试/分析功能全部免费，行为由 tests/license_feature_free.rs 覆盖）。

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use common::mock_state::{mock_app_state, mock_license_manager};
use gateway_core::license::{LicenseState, LicenseStatus, Tier};
use std::sync::Arc;
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789abcdef";

fn req_with_token(method: &str, uri: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {}", TOKEN))
        .body(Body::empty())
        .unwrap()
}

fn req_anon(method: &str, uri: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

/// 用 LicenseState 直接构造可注入状态的 AppState
fn state_with_license(s: LicenseState) -> Arc<gateway_api::web::AppState> {
    common::mock_state::mock_app_state_with_license(s)
}

// ===== GET /api/license =====

#[tokio::test]
async fn test_get_license_returns_absent_when_no_file() {
    let state = mock_app_state();
    let app = gateway_api::web::build_router(state);
    let resp = app
        .oneshot(req_with_token("GET", "/api/license"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // P1-1：GET /api/license 已纳入统一 ApiResponse 信封
    let v = &v["data"];
    assert_eq!(v["present"], serde_json::json!(false));
    assert_eq!(v["tier"], serde_json::json!("core"));
    assert_eq!(v["status"], serde_json::json!("ok"));
    // v2.0：quota_source 暴露配额来源——无 license 走 absent 兜底（预期行为）
    assert_eq!(v["quota_source"], serde_json::json!("absent_fallback"));
    // v2.0 Step 2：GET 响应包含 quota 字段。
    // 无 license 时 `LicenseManager::current()` 走安全兜底（Bug T7）：
    // license 自带 quota 全空 → 回退 `DEFAULT_ABSENT_QUOTA`（100 点 / 1 协议 / 7 天 / 1 通道），
    // 而非全 null——全 null 语义为"无限配额"，会让无授权版形同虚设。
    assert!(v["quota"].is_object());
    assert_eq!(v["quota"]["max_points"], serde_json::json!(100));
    assert_eq!(v["quota"]["max_protocols"], serde_json::json!(1));
    assert_eq!(v["quota"]["storage_days"], serde_json::json!(7));
    assert_eq!(v["quota"]["northbound_channels"], serde_json::json!(1));
}

/// v2.0 Step 2：LicenseState.quota 字段透出到 GET /api/license
#[tokio::test]
async fn test_get_license_returns_quota_from_state() {
    use gateway_core::license::Quota;
    let state = state_with_license(LicenseState {
        present: true,
        tier: Tier::Pro,
        quota: Quota {
            max_points: Some(5000),
            max_protocols: Some(16),
            storage_days: Some(365),
            northbound_channels: Some(8),
        },
        quota_source: gateway_core::license::QuotaSource::License,
        status: LicenseStatus::Ok,
        expires_at: None,
        license_id: Some("LIC-QUOTA-0001".into()),
        customer: None,
    });
    let app = gateway_api::web::build_router(state);
    let resp = app
        .oneshot(req_with_token("GET", "/api/license"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // P1-1：GET /api/license 已纳入统一 ApiResponse 信封
    let v = &v["data"];
    assert_eq!(v["tier"], serde_json::json!("pro"));
    assert_eq!(v["quota"]["max_points"], serde_json::json!(5000));
    assert_eq!(v["quota"]["max_protocols"], serde_json::json!(16));
    assert_eq!(v["quota"]["storage_days"], serde_json::json!(365));
    assert_eq!(v["quota"]["northbound_channels"], serde_json::json!(8));
    // v2.0：license 自带 quota → 来源 = license
    assert_eq!(v["quota_source"], serde_json::json!("license"));
}

#[tokio::test]
async fn test_get_license_returns_active_state() {
    let state = state_with_license(LicenseState {
        present: true,
        tier: Tier::Pro,
        quota: gateway_core::license::Quota::default(),
        quota_source: gateway_core::license::QuotaSource::AbsentFallback,
        status: LicenseStatus::Ok,
        expires_at: Some(1_790_000_000),
        license_id: Some("LIC-2026-0001".into()),
        customer: Some("某某自动化".into()),
    });
    let app = gateway_api::web::build_router(state);
    let resp = app
        .oneshot(req_with_token("GET", "/api/license"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // P1-1：GET /api/license 已纳入统一 ApiResponse 信封
    let v = &v["data"];
    assert_eq!(v["present"], serde_json::json!(true));
    assert_eq!(v["tier"], serde_json::json!("pro"));
    assert_eq!(v["license_id"], serde_json::json!("LIC-2026-0001"));
    assert_eq!(v["customer"], serde_json::json!("某某自动化"));
    assert_eq!(v["expires_at"], serde_json::json!(1_790_000_000));
}

#[tokio::test]
async fn test_get_license_via_anon_read_returns_200_or_401() {
    let state = mock_app_state();
    let app = gateway_api::web::build_router(state);
    let resp = app.oneshot(req_anon("GET", "/api/license")).await.unwrap();
    assert!(
        resp.status() == StatusCode::OK || resp.status() == StatusCode::UNAUTHORIZED,
        "got {:?}",
        resp.status()
    );
}

// ===== v2.0 配额用量可视化（usage / available）=====

/// GET /api/license 必须带 usage / available 两个对象（前端据此展示"已用 / 剩余"）
#[tokio::test]
async fn test_get_license_includes_usage_and_available() {
    let state = mock_app_state();
    let app = gateway_api::web::build_router(state);
    let resp = app
        .oneshot(req_with_token("GET", "/api/license"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let v = &v["data"];
    let usage = v["usage"].as_object().expect("usage 字段缺失");
    for key in ["points", "protocols", "storage_days", "northbound_channels"] {
        assert!(
            usage.get(key).map(|x| x.is_u64()).unwrap_or(false),
            "usage.{key} 必须是数字，实际: {:?}",
            usage.get(key)
        );
    }
    let avail = v["available"].as_object().expect("available 字段缺失");
    for key in ["points", "protocols", "storage_days", "northbound_channels"] {
        assert!(
            avail
                .get(key)
                .map(|x| x.is_null() || x.is_u64())
                .unwrap_or(false),
            "available.{key} 必须是数字或 null，实际: {:?}",
            avail.get(key)
        );
    }
}

/// available = 上限 - 已用（mock 态无设备/点位 → 已用为 0，剩余等于上限）
#[tokio::test]
async fn test_get_license_available_is_limit_minus_usage() {
    use gateway_core::license::Quota;
    let state = state_with_license(LicenseState {
        present: true,
        tier: Tier::Pro,
        quota: Quota {
            max_points: Some(5000),
            max_protocols: Some(16),
            storage_days: Some(365),
            northbound_channels: Some(8),
        },
        quota_source: gateway_core::license::QuotaSource::License,
        status: LicenseStatus::Ok,
        expires_at: None,
        license_id: Some("LIC-USAGE-0001".into()),
        customer: None,
    });
    let app = gateway_api::web::build_router(state);
    let resp = app
        .oneshot(req_with_token("GET", "/api/license"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let v = &v["data"];
    assert_eq!(v["usage"]["points"], serde_json::json!(0));
    assert_eq!(v["available"]["points"], serde_json::json!(5000));
    assert_eq!(v["available"]["protocols"], serde_json::json!(16));
    assert_eq!(v["available"]["northbound_channels"], serde_json::json!(8));
}

// ===== LicenseManager 状态写入测试 =====

#[tokio::test]
async fn test_replace_state_for_test_propagates_to_current() {
    let m = Arc::new(mock_license_manager(999));
    let initial = m.current();
    assert!(!initial.present);
    assert_eq!(initial.tier, Tier::Core);

    let owned: gateway_core::license::LicenseManager =
        std::sync::Arc::try_unwrap(m).expect("single-owner");
    owned.replace_state_for_test(LicenseState {
        present: true,
        tier: Tier::Ultra,
        quota: gateway_core::license::Quota::default(),
        quota_source: gateway_core::license::QuotaSource::AbsentFallback,
        status: LicenseStatus::Ok,
        expires_at: None,
        license_id: Some("LIC-DIRECT".into()),
        customer: Some("direct".into()),
    });
    let after = owned.current();
    assert!(after.present);
    assert_eq!(after.tier, Tier::Ultra);
    assert_eq!(after.license_id.as_deref(), Some("LIC-DIRECT"));
}

// ===== 授权管理：GET machine 字段 + POST /api/license/import =====

/// 用测试钥对构造签发自洽的 AppState，并建立临时授权路径（无 license 文件 = absent）
fn state_with_test_key_issuer(tmp: &std::path::Path) -> Arc<gateway_api::web::AppState> {
    use gateway_core::license::LicenseManager;
    let mgr = Arc::new(LicenseManager::with_public_key(
        &tmp.join("clock.db"),
        gateway_core::license::issuer_support::public_key(),
    ));
    mgr.load(&tmp.join("license.lic")).expect("load absent");
    common::mock_state::mock_app_state_with_license_manager(mgr)
}

fn post_import_request(content: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/license/import")
        .header(header::AUTHORIZATION, format!("Bearer {}", TOKEN))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({ "content": content }).to_string(),
        ))
        .unwrap()
}

#[tokio::test]
async fn test_get_license_includes_machine() {
    let state = mock_app_state();
    let app = gateway_api::web::build_router(state);
    let resp = app
        .oneshot(req_with_token("GET", "/api/license"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // machine 视图字段必须存在（值可为 null：无对应硬件项）
    // P1-1：GET /api/license 已纳入统一 ApiResponse 信封
    let machine = v["data"]
        .get("machine")
        .and_then(|m| m.as_object())
        .expect("machine 视图缺失");
    for key in [
        "machine_key",
        "onboard_mac",
        "storage_serial",
        "device_serial",
    ] {
        assert!(
            machine
                .get(key)
                .map(|x| x.is_null() || x.is_string())
                .unwrap_or(false),
            "machine.{key} 字段异常"
        );
    }
}

#[tokio::test]
async fn test_import_rejects_garbage_with_readable_message() {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let state = state_with_test_key_issuer(tmp.path());
    let app = gateway_api::web::build_router(state);

    let resp = app
        .oneshot(post_import_request("not-a-license"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let msg = v["message"].as_str().expect("message 字段缺失");
    assert!(
        msg.contains("签名无效") || msg.contains("格式错误"),
        "got: {msg}"
    );
    // 拒绝导入不得落盘
    assert!(
        !tmp.path().join("license.lic").exists(),
        "无效文件不应写入授权路径"
    );
}

#[tokio::test]
async fn test_import_accepts_signed_license() {
    use gateway_core::license::issuer_support;
    let tmp = tempfile::tempdir().expect("create temp dir");
    let state = state_with_test_key_issuer(tmp.path());
    // 用 manager 的真实本机指纹构造 payload（签名/指纹自洽）
    let fp = state.platform.license.machine();
    let payload = serde_json::json!({
        "v": 1,
        "license_id": "LIC-TEST-0001",
        "customer": "某某自动化",
        "tier": "pro",
        "features": [],
        "machine_key": fp.machine_key.clone().unwrap_or_default(),
        "machine": "",
        "machine_parts": [
            fp.onboard_mac.clone().unwrap_or_default(),
            fp.storage_serial.clone().unwrap_or_default(),
            fp.device_serial.clone().unwrap_or_default(),
        ],
        "issued_at": "2026-10-12T00:00:00Z",
        "expires_at": null,
    });
    let lic = issuer_support::sign_payload_json(&payload.to_string());
    let app = gateway_api::web::build_router(state);

    let resp = app.oneshot(post_import_request(&lic)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // P1-1：POST /api/license/import 成功响应已纳入统一 ApiResponse 信封
    let v = &v["data"];
    assert_eq!(v["present"], serde_json::json!(true));
    assert_eq!(v["tier"], serde_json::json!("pro"));
    assert_eq!(v["license_id"], serde_json::json!("LIC-TEST-0001"));
    // 文件已落盘到授权路径
    assert!(
        tmp.path().join("license.lic").exists(),
        "授权文件应写入配置路径"
    );
}

// ===== 导入低档位 license：存量配置超限告警 =====

/// 构造 N 个点位的存量配置（占用配额）
fn config_with_n_points(n: usize) -> gateway_core::config::GatewayConfig {
    use gateway_core::config::{BusConfig, GatewayConfig, PointConfig};
    GatewayConfig {
        buses: vec![BusConfig::default()],
        points: (0..n)
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
            .collect(),
        ..Default::default()
    }
}

/// 当前时间（UTC 毫秒 epoch），用于审计日志的时间窗过滤
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 构造一份带 quota 的 license 文本（`max_points` 由调用方给）
fn signed_license_with_quota(
    state: &Arc<gateway_api::web::AppState>,
    license_id: &str,
    max_points: u32,
) -> String {
    use gateway_core::license::issuer_support;
    let fp = state.platform.license.machine();
    let payload = serde_json::json!({
        "v": 1,
        "license_id": license_id,
        "customer": "某某自动化",
        "tier": "core",
        "quota": {
            "max_points": max_points,
            "max_protocols": null,
            "storage_days": null,
            "northbound_channels": null,
        },
        "machine_key": fp.machine_key.clone().unwrap_or_default(),
        "machine": "",
        "machine_parts": [
            fp.onboard_mac.clone().unwrap_or_default(),
            fp.storage_serial.clone().unwrap_or_default(),
            fp.device_serial.clone().unwrap_or_default(),
        ],
        "issued_at": "2026-10-12T00:00:00Z",
        "expires_at": null,
    });
    issuer_support::sign_payload_json(&payload.to_string())
}

/// 查审计日志里 `license.quota_exceeded` 的条目（按时间窗过滤，避开历史遗留文件内容）
fn quota_exceeded_entries(
    state: &Arc<gateway_api::web::AppState>,
    ts_from: i64,
) -> Vec<gateway_core::engine::audit::AuditEntry> {
    state
        .platform
        .audit
        .list(gateway_core::engine::audit::AuditFilter {
            action: Some("license.quota_exceeded".to_string()),
            target: None,
            source: None,
            ts_from: Some(ts_from),
            ts_to: None,
            limit: Some(50),
        })
        .unwrap()
}

/// 导入低档位 license 后，存量配置已超出新档位配额时：
/// ① 导入**不被阻断**（license 已生效，配额是既定事实）；
/// ② 响应里 usage 反映存量占用、available 饱和到 0（不是负数）；
/// ③ 审计日志留下 `license.quota_exceeded` 痕迹。
#[tokio::test]
async fn test_import_low_tier_license_flags_existing_config_over_quota() {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let state = state_with_test_key_issuer(tmp.path());

    // 存量配置：2000 个点位，远超低档 license 的 100 点上限
    state.config.store(Arc::new(config_with_n_points(2000)));

    let lic = signed_license_with_quota(&state, "LIC-LOW-0001", 100);
    let ts_from = now_ms();
    let app = gateway_api::web::build_router(state.clone());

    let resp = app.oneshot(post_import_request(&lic)).await.unwrap();
    // 导入不被阻断
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let v = &v["data"];
    assert_eq!(v["present"], serde_json::json!(true));
    // 新档位配额生效（license 自带 quota 优先于 tier_quotas 配置）
    assert_eq!(v["quota"]["max_points"], serde_json::json!(100));
    assert_eq!(v["quota_source"], serde_json::json!("license"));
    // usage 反映存量配置占用
    assert_eq!(v["usage"]["points"], serde_json::json!(2000));
    // available 饱和到 0（超限时不是负数）
    assert_eq!(v["available"]["points"], serde_json::json!(0));
    // 未设上限的项 → available 为 null（不限）
    assert_eq!(v["available"]["protocols"], serde_json::Value::Null);

    // 审计留痕：现场可查「导入低档 license 后存量配置超限」
    let entries = quota_exceeded_entries(&state, ts_from);
    assert!(
        !entries.is_empty(),
        "导入低档位 license 且存量配置超限时，应写入 license.quota_exceeded 审计"
    );
    assert!(
        entries[0].detail.contains("采集点数"),
        "审计 detail 应列出超限项，实际: {}",
        entries[0].detail
    );
}

/// 反例：存量配置未超配额时，导入**不得**产生超限审计（避免噪音告警）
#[tokio::test]
async fn test_import_license_within_quota_emits_no_exceeded_audit() {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let state = state_with_test_key_issuer(tmp.path());

    // 存量配置 2000 点位，新档位给 10000 点 → 不超限
    state.config.store(Arc::new(config_with_n_points(2000)));

    let lic = signed_license_with_quota(&state, "LIC-HIGH-0001", 10_000);
    let ts_from = now_ms();
    let app = gateway_api::web::build_router(state.clone());

    let resp = app.oneshot(post_import_request(&lic)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let v = &v["data"];
    assert_eq!(v["usage"]["points"], serde_json::json!(2000));
    assert_eq!(v["available"]["points"], serde_json::json!(8000));

    let entries = quota_exceeded_entries(&state, ts_from);
    assert!(
        entries.is_empty(),
        "未超配额时不应写 license.quota_exceeded 审计，实际: {:?}",
        entries
    );
}
