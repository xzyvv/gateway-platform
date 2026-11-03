//! 虚拟点位 API 集成测试（v9 方案 P1）
//!
//! 覆盖虚拟点位的真实求值语义（不再「只存公式不求值」）：
//! - 创建：落盘 + 热重载，响应如实反映「尚未求值」
//! - 列表：合并 core 引擎的求值状态（`last_value` / `error`）
//! - 重算：返回引擎求值结果；点不存在 → 404
//! - 写路径触发**在线热重载**（`reload_scope.applied` 含 `virtual_points`）
//! - 更新 / 删除：幂等与 404 语义

mod common;

use std::collections::HashMap;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use common::mock_state::{mock_app_state_with_gateway, MockGateway};
use gateway_api::web::traits::VirtualPointStateView;
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789abcdef";

// ===== 请求工具 =====

fn req(method: &str, uri: &str, body: Option<serde_json::Value>) -> Request<Body> {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {}", TOKEN));
    let payload = match body {
        Some(v) => Body::from(v.to_string()),
        None => Body::empty(),
    };
    builder.body(payload).expect("build request")
}

async fn call(app: &Router, request: Request<Body>) -> (StatusCode, serde_json::Value) {
    let resp = app.clone().oneshot(request).await.expect("router call");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .expect("read body");
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

fn vp_payload(formula: &str) -> serde_json::Value {
    serde_json::json!({
        "id": "vp_power",
        "name": "总功率",
        "formula": formula,
        "unit": "W",
        "decimals": 2,
        "enabled": true,
    })
}

// ===== 用例 =====

#[tokio::test]
async fn create_persists_and_reports_not_yet_evaluated() {
    let (state, _gw) = mock_app_state_with_gateway();
    let app = gateway_api::web::build_router(state);

    let (status, body) = call(
        &app,
        req(
            "POST",
            "/api/points/virtual",
            Some(vp_payload("{voltage} * {current}")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "创建应成功: {}", body);

    let vp = &body["data"];
    assert_eq!(vp["id"], "vp_power");
    assert_eq!(vp["formula"], "{voltage} * {current}");
    assert_eq!(vp["unit"], "W");
    assert_eq!(vp["decimals"], 2);
    // 尚未有采集数据 → 无值，且如实给出原因（不再是恒为 None 的空壳字段）
    assert_eq!(vp["last_value"], serde_json::Value::Null);
    assert_eq!(vp["error"], "尚未求值（等待采集数据）");

    // 已落盘到内存配置（下次 list 能读到）
    let (status, body) = call(&app, req("GET", "/api/points/virtual", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().expect("array").len(), 1);
}

#[tokio::test]
async fn list_merges_engine_state() {
    let (state, gw) = mock_app_state_with_gateway();
    let app = gateway_api::web::build_router(state);

    let (status, _body) = call(
        &app,
        req(
            "POST",
            "/api/points/virtual",
            Some(vp_payload("{voltage} * {current}")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // 注入引擎求值结果
    let mut states = HashMap::new();
    states.insert(
        "vp_power".to_string(),
        VirtualPointStateView {
            value: Some(3230.5),
            error: None,
            updated_at_ms: 1_700_000_000_000,
        },
    );
    gw.set_virtual_point_states(states);

    let (status, body) = call(&app, req("GET", "/api/points/virtual", None)).await;
    assert_eq!(status, StatusCode::OK);
    let vp = &body["data"][0];
    assert_eq!(vp["last_value"], 3230.5);
    assert_eq!(vp["error"], serde_json::Value::Null);
}

#[tokio::test]
async fn list_reports_engine_error_for_broken_formula() {
    let (state, gw) = mock_app_state_with_gateway();
    let app = gateway_api::web::build_router(state);

    let (status, _body) = call(
        &app,
        req(
            "POST",
            "/api/points/virtual",
            Some(vp_payload("{voltage} * {current}")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let mut states = HashMap::new();
    states.insert(
        "vp_power".to_string(),
        VirtualPointStateView {
            value: None,
            error: Some("引用点位无值: current".into()),
            updated_at_ms: 1_700_000_000_000,
        },
    );
    gw.set_virtual_point_states(states);

    let (status, body) = call(&app, req("GET", "/api/points/virtual", None)).await;
    assert_eq!(status, StatusCode::OK);
    let vp = &body["data"][0];
    assert_eq!(vp["last_value"], serde_json::Value::Null);
    assert_eq!(vp["error"], "引用点位无值: current");
}

#[tokio::test]
async fn recalc_returns_engine_evaluation() {
    let (state, gw) = mock_app_state_with_gateway();
    let app = gateway_api::web::build_router(state);

    let (status, _body) = call(
        &app,
        req(
            "POST",
            "/api/points/virtual",
            Some(vp_payload("{voltage} * {current}")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let mut states = HashMap::new();
    states.insert(
        "vp_power".to_string(),
        VirtualPointStateView {
            value: Some(1980.0),
            error: None,
            updated_at_ms: 1_700_000_000_000,
        },
    );
    gw.set_virtual_point_states(states);

    let (status, body) = call(
        &app,
        req("POST", "/api/points/virtual/vp_power/recalc", None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "重算应成功: {}", body);
    assert_eq!(body["data"]["last_value"], 1980.0);
    assert_eq!(body["data"]["error"], serde_json::Value::Null);
}

#[tokio::test]
async fn recalc_unknown_id_returns_404() {
    let (state, _gw) = mock_app_state_with_gateway();
    let app = gateway_api::web::build_router(state);

    let (status, body) = call(
        &app,
        req("POST", "/api/points/virtual/vp_missing/recalc", None),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "未知点应 404: {}", body);
    assert_eq!(body["code"], 4004);
}

#[tokio::test]
async fn write_path_triggers_online_reload() {
    let (state, gw): (
        std::sync::Arc<gateway_api::web::AppState>,
        std::sync::Arc<MockGateway>,
    ) = mock_app_state_with_gateway();
    let app = gateway_api::web::build_router(state);

    let (status, _body) = call(
        &app,
        req(
            "POST",
            "/api/points/virtual",
            Some(vp_payload("{voltage} * {current}")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // 写路径必须走「落盘 + 热重载」，且 `virtual_points` 归入 `applied`
    // （公式改动在线生效，不需要重启网关）。
    let scopes = gw.reload_scopes();
    assert_eq!(scopes.len(), 1, "写路径应触发一次热重载: {:?}", scopes);
    assert!(
        scopes[0].applied.contains(&"virtual_points"),
        "virtual_points 应属在线生效分节: {:?}",
        scopes[0]
    );
    assert!(
        scopes[0].restart_required.is_empty(),
        "虚拟点位改动不应要求重启: {:?}",
        scopes[0]
    );
}

#[tokio::test]
async fn update_changes_formula_and_reloads() {
    let (state, gw) = mock_app_state_with_gateway();
    let app = gateway_api::web::build_router(state);

    let (status, _body) = call(
        &app,
        req(
            "POST",
            "/api/points/virtual",
            Some(vp_payload("{voltage} * {current}")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = call(
        &app,
        req(
            "PUT",
            "/api/points/virtual/vp_power",
            Some(vp_payload("{voltage} * {current} * 0.85")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "更新应成功: {}", body);
    assert_eq!(body["data"]["formula"], "{voltage} * {current} * 0.85");

    let scopes = gw.reload_scopes();
    assert_eq!(scopes.len(), 2, "创建 + 更新各触发一次热重载");
    assert!(scopes[1].applied.contains(&"virtual_points"));
}

#[tokio::test]
async fn update_unknown_id_returns_404() {
    let (state, _gw) = mock_app_state_with_gateway();
    let app = gateway_api::web::build_router(state);

    let (status, body) = call(
        &app,
        req(
            "PUT",
            "/api/points/virtual/vp_missing",
            Some(vp_payload("{a} + {b}")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "未知点应 404: {}", body);
    assert_eq!(body["code"], 4004);
}

#[tokio::test]
async fn delete_removes_point_and_reloads() {
    let (state, gw) = mock_app_state_with_gateway();
    let app = gateway_api::web::build_router(state);

    let (status, _body) = call(
        &app,
        req(
            "POST",
            "/api/points/virtual",
            Some(vp_payload("{voltage} * {current}")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = call(&app, req("DELETE", "/api/points/virtual/vp_power", None)).await;
    assert_eq!(status, StatusCode::OK, "删除应成功: {}", body);

    let (status, body) = call(&app, req("GET", "/api/points/virtual", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body["data"].as_array().expect("array").is_empty(),
        "删除后列表应为空: {}",
        body
    );

    let scopes = gw.reload_scopes();
    assert_eq!(scopes.len(), 2);
    assert!(scopes[1].applied.contains(&"virtual_points"));
}

#[tokio::test]
async fn delete_unknown_id_returns_404() {
    let (state, _gw) = mock_app_state_with_gateway();
    let app = gateway_api::web::build_router(state);

    let (status, body) = call(&app, req("DELETE", "/api/points/virtual/vp_missing", None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "未知点应 404: {}", body);
    assert_eq!(body["code"], 4004);
}

#[tokio::test]
async fn create_duplicate_id_returns_400() {
    let (state, _gw) = mock_app_state_with_gateway();
    let app = gateway_api::web::build_router(state);

    let (status, _body) = call(
        &app,
        req(
            "POST",
            "/api/points/virtual",
            Some(vp_payload("{voltage} * {current}")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = call(
        &app,
        req(
            "POST",
            "/api/points/virtual",
            Some(vp_payload("{voltage} * {current}")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "重复 id 应 400: {}", body);
    assert_eq!(body["code"], 4000);
}
