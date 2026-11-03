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
}

#[tokio::test]
async fn test_get_license_returns_active_state() {
    let state = state_with_license(LicenseState {
        present: true,
        tier: Tier::Pro,
        quota: gateway_core::license::Quota::default(),
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
    let tmp = std::env::temp_dir().join(format!("lic_import_bad_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let state = state_with_test_key_issuer(&tmp);
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
        !tmp.join("license.lic").exists(),
        "无效文件不应写入授权路径"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn test_import_accepts_signed_license() {
    use gateway_core::license::issuer_support;
    let tmp = std::env::temp_dir().join(format!("lic_import_ok_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let state = state_with_test_key_issuer(&tmp);
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
    assert!(tmp.join("license.lic").exists(), "授权文件应写入配置路径");
    let _ = std::fs::remove_dir_all(&tmp);
}
