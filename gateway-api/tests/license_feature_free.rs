//! v2.0 license 模型：调试/分析/批量功能全部免费测试
//!
//! 取代原 v1.2 license_feature_gates（已删除——v1.2 行为"调试被拦截"被 v2.0 推翻）。
//! v2.0 核心承诺：调试能力不被授权状态阻断，调试功能全 tier 全状态放行。
//!
//! 覆盖：所有原 feature gate 路由（packet-capture / step-debug / slave-sim / perf-monitor
//! / batch-cmd / protocol-scan）在 core 档 + degraded 态都返回 200 而非 403。
//!
//! 实际不真跑抓包/simulator 业务（mock 后端为 415 Unsupported Media Type 等），
//! 只断言"中间件不再拦截"——状态码不是 403。

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use common::mock_state;
use gateway_core::license::{LicenseState, LicenseStatus, Tier};
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

/// core 档 + 无失效（开源用户场景）：调试功能全放行
#[tokio::test]
async fn test_debug_routes_open_for_core_tier() {
    let state = mock_state::mock_app_state_with_license(LicenseState {
        present: false,
        tier: Tier::Core,
        quota: Default::default(),
        quota_source: gateway_core::license::QuotaSource::AbsentFallback,
        status: LicenseStatus::Ok,
        expires_at: None,
        license_id: None,
        customer: None,
    });
    let app = gateway_api::web::build_router(state);
    // 抽样原 §10 6 个 feature gate 的代表性路由（step-debug 后端已删除：
    // 前端单步调试直调 packet-editor/send，此处用 packet-editor 路由替代抽样）
    let paths = [
        "/api/point-tools/clone/sources",             // batch-cmd（§10 pro）
        "/api/diagnostics/bus-load",                  // perf-monitor（§10 pro）
        "/api/protocol-tools/register-scan",          // protocol-scan（§10 standard）
        "/api/protocol-tools/simulator/status",       // slave-sim（§10 ultra）
        "/api/protocol-tools/packet-editor/presets",  // packet-editor（原 step-debug 槽位）
        "/api/protocol-tools/packet-capture/packets", // packet-capture（§10 ultra）
    ];
    for p in paths {
        let resp = app.clone().oneshot(req_with_token("GET", p)).await.unwrap();
        assert_ne!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "path {} should NOT be 403 in v2.0 (debug features free)",
            p
        );
    }
}

/// 失效态（Tampered）：调试功能仍放行（不阻断调试工作）
#[tokio::test]
async fn test_debug_routes_open_in_degraded_state() {
    let state = mock_state::mock_app_state_with_license(LicenseState {
        present: false,
        tier: Tier::Core,
        quota: Default::default(),
        quota_source: gateway_core::license::QuotaSource::AbsentFallback,
        status: LicenseStatus::Tampered,
        expires_at: None,
        license_id: Some("LIC-DEGRADED".into()),
        customer: None,
    });
    let app = gateway_api::web::build_router(state);
    let paths = [
        "/api/diagnostics/bus-load",
        "/api/protocol-tools/simulator/status",
        "/api/protocol-tools/packet-capture/packets",
    ];
    for p in paths {
        let resp = app.clone().oneshot(req_with_token("GET", p)).await.unwrap();
        assert_ne!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "path {} should NOT be 403 in v2.0 (debug features free, even when degraded)",
            p
        );
    }
}

/// tier=Ultra：调试功能放行（v1.2 也放行，回归测试）
#[tokio::test]
async fn test_debug_routes_open_for_ultra_tier() {
    let state = mock_state::mock_app_state_with_license(LicenseState {
        present: true,
        tier: Tier::Ultra,
        quota: Default::default(),
        quota_source: gateway_core::license::QuotaSource::AbsentFallback,
        status: LicenseStatus::Ok,
        expires_at: None,
        license_id: Some("LIC-ULTRA".into()),
        customer: None,
    });
    let app = gateway_api::web::build_router(state);
    let resp = app
        .oneshot(req_with_token(
            "GET",
            "/api/protocol-tools/packet-capture/packets",
        ))
        .await
        .unwrap();
    assert_ne!(resp.status(), StatusCode::FORBIDDEN);
}
