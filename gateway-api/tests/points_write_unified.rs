//! B2 点位写入统一入口（POST /api/point-tools/write）集成测试
//!
//! 契约：请求体 `{ "writes": [ { "id": "<point_id>", "value": <f64> } ] }`
//! （`id` / `point_id` 等价）。
//!
//! 须验证：
//! - 鉴权与既有写路由一致：无 token → 401，只读 scope 的 API 密钥 → 403；
//! - 1 条走**单写**业务：成功返回统一摘要；点位不存在 → 404（而非逐项失败）、
//!   写失败 → 业务码 5101（与 `/:id/write`、`/devices/control/send` 一致）；
//! - ≥2 条走**批写**业务：按 bus 分组 write_multi，失败项独立返回、不影响其它条；
//! - 成功项逐条落审计 `point.write`。

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::mock_state::{mock_app_state, mock_app_state_with_collectors, MockCollector};
use gateway_api::web::{build_router, AppState};
use gateway_core::config::PointConfig;
use gateway_core::south::protocol::Collector;
use tower::ServiceExt;

const TOKEN: &str = "Bearer test-token-0123456789abcdef";

fn point(id: &str, bus: &str) -> PointConfig {
    PointConfig {
        id: id.to_string(),
        name: Some(id.to_uppercase()),
        bus: bus.to_string(),
        protocol: "modbus".to_string(),
        address: "1".to_string(),
        interval: 10,
        // S11: 写入测试的点位需在白名单内
        writable: true,
        ..Default::default()
    }
}

fn store(state: &Arc<AppState>, p: PointConfig) {
    let mut cfg = state.config.load_full().as_ref().clone();
    cfg.points.push(p);
    state.config.store(Arc::new(cfg));
}

fn write_req(body: &str, token: Option<&str>) -> Request<Body> {
    let mut b = Request::builder()
        .method("POST")
        .uri("/api/point-tools/write")
        .header("content-type", "application/json");
    if let Some(t) = token {
        b = b.header("authorization", t);
    }
    b.body(Body::from(body.to_string())).unwrap()
}

async fn body_json(res: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(res.into_body(), 65536).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

fn collectors_bus0(ok: bool) -> HashMap<String, Arc<dyn Collector>> {
    let mut collectors: HashMap<String, Arc<dyn Collector>> = HashMap::new();
    collectors.insert(
        "bus0".to_string(),
        Arc::new(MockCollector::new("bus0").with_write_result(ok)),
    );
    collectors
}

/// 无 token → 401（与既有写路由同一 require_auth 保护层）
#[tokio::test]
async fn unified_write_without_token_returns_401() {
    let app = build_router(mock_app_state());
    let res = app
        .oneshot(write_req(r#"{"writes":[{"id":"p1","value":1.0}]}"#, None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

/// 只读 scope 的 API 密钥调用写路由 → 403（不得绕过域写权限）
#[tokio::test]
async fn unified_write_with_readonly_api_key_returns_403() {
    let state = mock_app_state();
    let (_, key) = state
        .platform
        .api_keys
        .create("readonly", &["points:read".to_string()], None)
        .expect("create api key");
    let app = build_router(state);
    let req = Request::builder()
        .method("POST")
        .uri("/api/point-tools/write")
        .header("content-type", "application/json")
        .header("x-api-token", key)
        .body(Body::from(r#"{"writes":[{"id":"p1","value":1.0}]}"#))
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

/// 空 writes → 400
#[tokio::test]
async fn unified_write_empty_returns_400() {
    let app = build_router(mock_app_state());
    let res = app
        .oneshot(write_req(r#"{"writes":[]}"#, Some(TOKEN)))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

/// 1 条成功 → 走单写业务，返回统一摘要（results 与入参逐位对应）
#[tokio::test]
async fn unified_write_single_entry_success() {
    let state = mock_app_state_with_collectors(collectors_bus0(true));
    store(&state, point("p1", "bus0"));
    let app = build_router(state);
    let res = app
        .oneshot(write_req(
            r#"{"writes":[{"id":"p1","value":42.5}]}"#,
            Some(TOKEN),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    let v = &v["data"];
    assert_eq!(v["status"], "ok");
    assert_eq!(v["total"], 1);
    assert_eq!(v["succeeded"], 1);
    assert_eq!(v["failed"], 0);
    assert_eq!(v["results"][0]["point_id"], "p1");
    assert_eq!(v["results"][0]["status"], "ok");
    assert_eq!(v["results"][0]["value"], 42.5);
}

/// 1 条时 `point_id` 与 `id` 等价
#[tokio::test]
async fn unified_write_single_entry_accepts_point_id_alias() {
    let state = mock_app_state_with_collectors(collectors_bus0(true));
    store(&state, point("p1", "bus0"));
    let app = build_router(state);
    let res = app
        .oneshot(write_req(
            r#"{"writes":[{"point_id":"p1","value":5.0}]}"#,
            Some(TOKEN),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert_eq!(v["data"]["results"][0]["status"], "ok");
}

/// 1 条但点位不存在 → 404（单写语义，与 send_command / 旧单写接口一致）
#[tokio::test]
async fn unified_write_single_entry_unknown_point_returns_404() {
    let app = build_router(mock_app_state());
    let res = app
        .oneshot(write_req(
            r#"{"writes":[{"id":"ghost","value":1.0}]}"#,
            Some(TOKEN),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    let body = body_json(res).await;
    assert!(
        body.to_string().contains("ghost"),
        "应提示点位不存在: {body}"
    );
}

/// 1 条但 collector 写失败 → 业务码 5101（与旧单写 / 远程控制同一错误码）
#[tokio::test]
async fn unified_write_single_entry_write_failure_returns_5101() {
    let state = mock_app_state_with_collectors(collectors_bus0(false));
    store(&state, point("p1", "bus0"));
    let app = build_router(state);
    let res = app
        .oneshot(write_req(
            r#"{"writes":[{"id":"p1","value":1.0}]}"#,
            Some(TOKEN),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert_eq!(v["code"], 5101, "单写失败应返回 5101: {v}");
    assert!(v["data"].is_null(), "写失败时不应带 data: {v}");
}

/// ≥2 条走批写：白名单外 / 不存在点位作为**该项失败**独立返回，成功项不受影响
#[tokio::test]
async fn unified_write_multi_entry_per_item_failures() {
    let state = mock_app_state_with_collectors(collectors_bus0(true));
    store(&state, point("p1", "bus0"));
    let mut p2 = point("p2", "bus0");
    p2.writable = false;
    store(&state, p2);

    let app = build_router(state);
    let res = app
        .oneshot(write_req(
            r#"{"writes":[
                {"id":"p1","value":1.0},
                {"id":"p2","value":2.0},
                {"id":"ghost","value":3.0}
            ]}"#,
            Some(TOKEN),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    let v = &v["data"];
    assert_eq!(v["status"], "partial");
    assert_eq!(v["total"], 3);
    assert_eq!(v["succeeded"], 1);
    assert_eq!(v["failed"], 2);
    assert_eq!(v["results"][0]["status"], "ok");
    assert_eq!(v["results"][1]["status"], "error");
    assert!(v["results"][1]["error"]
        .as_str()
        .unwrap()
        .contains("写入白名单"));
    assert_eq!(v["results"][2]["status"], "error");
    assert!(v["results"][2]["error"].as_str().unwrap().contains("ghost"));
}

/// ≥2 条但整条总线无 collector → 每项独立失败，整体 status=error（HTTP 仍 200）
#[tokio::test]
async fn unified_write_multi_entry_missing_collector_is_per_item_error() {
    let state = mock_app_state();
    store(&state, point("p1", "bus0"));
    store(&state, point("p2", "bus0"));
    let app = build_router(state);
    let res = app
        .oneshot(write_req(
            r#"{"writes":[{"id":"p1","value":1.0},{"id":"p2","value":2.0}]}"#,
            Some(TOKEN),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    let v = &v["data"];
    assert_eq!(v["status"], "error");
    assert_eq!(v["succeeded"], 0);
    assert_eq!(v["failed"], 2);
    assert!(v["results"][0]["error"].as_str().unwrap().contains("bus0"));
    assert!(v["results"][1]["error"].as_str().unwrap().contains("bus0"));
}

/// 批写成功项逐条落审计 `point.write`，可经 /api/audit/logs 读回
#[tokio::test]
async fn unified_write_multi_entry_logs_audit_per_item() {
    let state = mock_app_state_with_collectors(collectors_bus0(true));
    store(&state, point("p1", "bus0"));
    store(&state, point("p2", "bus0"));
    let app = build_router(state);

    let res = app
        .clone()
        .oneshot(write_req(
            r#"{"writes":[{"id":"p1","value":1.0},{"id":"p2","value":2.0}]}"#,
            Some(TOKEN),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // 审计异步落盘：轮询等待最多 ~2s
    let mut found = 0usize;
    for _ in 0..40 {
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/audit/logs?action=point.write")
                    .header("authorization", TOKEN)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        found = v["data"]["logs"]
            .as_array()
            .map(|a| a.iter().filter(|e| e["action"] == "point.write").count())
            .unwrap_or(0);
        if found >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(found >= 2, "批写应逐条落审计 point.write，实际 {found} 条");
}
