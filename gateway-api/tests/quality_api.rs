//! 数据质量规则 API 集成测试
//!
//! 覆盖：读接口匿名、写接口要求 token、CRUD 往返、入参校验与 404 语义。

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use common::mock_state::mock_app_state;
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789abcdef";

fn req(
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
    token: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(t) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {}", t));
    }
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

async fn call_auth(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    call(app, req(method, uri, body, Some(TOKEN))).await
}

fn range_body(name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "condition": "range",
        "min": -100,
        "max": 100,
        "resultQuality": "bad",
        "enabled": true,
    })
}

#[tokio::test]
async fn read_allows_anonymous_and_starts_empty() {
    let state = mock_app_state();
    let app = gateway_api::web::build_router(state);

    let (status, body) = call(&app, req("GET", "/api/quality/rules", None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["code"], 0);
    assert!(body["data"].is_array());
    assert!(body["data"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn write_requires_token() {
    let state = mock_app_state();
    let app = gateway_api::web::build_router(state);

    let (status, body) = call(
        &app,
        req(
            "POST",
            "/api/quality/rules",
            Some(range_body("无 token")),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "写接口必须要求 token");
    assert_ne!(body["code"], 0);
}

#[tokio::test]
async fn crud_roundtrip() {
    let state = mock_app_state();
    let app = gateway_api::web::build_router(state);

    // 创建：返回 qr_ 前缀 id 与 createdAt/updatedAt
    let (status, body) = call_auth(
        &app,
        "POST",
        "/api/quality/rules",
        Some(range_body("超量程")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", body);
    let id = body["data"]["id"].as_str().expect("id").to_string();
    assert!(id.starts_with("qr_"));
    assert_eq!(body["data"]["condition"], "range");
    assert_eq!(body["data"]["resultQuality"], "bad");
    assert_eq!(body["data"]["min"], -100.0);
    assert!(body["data"]["createdAt"].as_i64().unwrap() > 0);

    // 列表
    let (status, body) = call_auth(&app, "GET", "/api/quality/rules", None).await;
    assert_eq!(status, StatusCode::OK);
    let rows = body["data"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], id);

    // 局部更新：仅改 enabled/name，其他字段保持
    let (status, body) = call_auth(
        &app,
        "PUT",
        &format!("/api/quality/rules/{}", id),
        Some(serde_json::json!({ "name": "停用后的规则", "enabled": false })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", body);
    assert_eq!(body["data"]["name"], "停用后的规则");
    assert_eq!(body["data"]["enabled"], false);
    assert_eq!(body["data"]["condition"], "range");
    assert_eq!(body["data"]["max"], 100.0);

    // 校验失败：min > max 返回 400
    let (status, body) = call_auth(
        &app,
        "PUT",
        &format!("/api/quality/rules/{}", id),
        Some(serde_json::json!({ "min": 200, "max": 100 })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_ne!(body["code"], 0);

    // 删除
    let (status, body) =
        call_auth(&app, "DELETE", &format!("/api/quality/rules/{}", id), None).await;
    assert_eq!(status, StatusCode::OK, "{}", body);
    assert_eq!(body["data"], true);

    let (status, _) = call_auth(&app, "DELETE", &format!("/api/quality/rules/{}", id), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "重复删除应为 404");

    let (status, body) = call_auth(&app, "GET", "/api/quality/rules", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn rejects_invalid_condition_and_result_quality() {
    let state = mock_app_state();
    let app = gateway_api::web::build_router(state);

    let mut bad_condition = range_body("未知条件");
    bad_condition["condition"] = serde_json::json!("unknown");
    let (status, _) = call_auth(&app, "POST", "/api/quality/rules", Some(bad_condition)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let mut bad_result = range_body("未知质量码");
    bad_result["resultQuality"] = serde_json::json!("perfect");
    let (status, _) = call_auth(&app, "POST", "/api/quality/rules", Some(bad_result)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // rate 缺少 maxRate
    let missing_rate = serde_json::json!({
        "name": "缺变化率",
        "condition": "rate",
        "resultQuality": "uncertain",
        "enabled": true,
    });
    let (status, _) = call_auth(&app, "POST", "/api/quality/rules", Some(missing_rate)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
