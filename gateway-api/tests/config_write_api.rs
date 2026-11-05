//! 配置写接口集成测试（决策清单 P1-4 补差集）
//!
//! 覆盖 `docs/api-surface.md` 中此前无测试的 4 个写端点：
//! - POST /api/config/diff        字段级差异
//! - PUT  /api/config/import      批量覆盖（含空设备列表拒绝分支）
//! - POST /api/config/reload      热重载（磁盘配置缺失 / 正常两条路径）
//! - POST /api/config/versions/:id/rollback  版本回滚（404 / 成功）

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use gateway_core::config::GatewayConfig;
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789abcdef";

fn req(method: &str, uri: &str, body: Option<serde_json::Value>) -> Request<Body> {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {}", TOKEN));
    match body {
        Some(v) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

/// 兼容两种响应形态：裸 JSON（diff / rollback）与统一信封 `{code,data}`（reload / import）。
fn payload(v: serde_json::Value) -> serde_json::Value {
    if v.get("code").is_some() {
        v.get("data").cloned().unwrap_or(serde_json::Value::Null)
    } else {
        v
    }
}

fn current_config(state: &Arc<gateway_api::web::AppState>) -> GatewayConfig {
    state.config.load_full().as_ref().clone()
}

#[tokio::test]
async fn config_diff_reports_only_changed_sections() {
    let state = common::mock_state::mock_app_state();
    let app = gateway_api::web::build_router(state.clone());
    let cfg = current_config(&state);

    // 1) 原样回传 → 无变化
    let resp = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/config/diff",
            Some(serde_json::to_value(&cfg).unwrap()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = payload(json_body(resp).await);
    assert_eq!(body["has_changes"], false);
    assert_eq!(body["changed_fields"], serde_json::json!([]));

    // 2) 只改 points → 差异只报 points，不夹带其它分节
    let mut changed = cfg.clone();
    changed.points = vec![gateway_core::config::PointConfig {
        id: "diff-p1".into(),
        bus: "bus0".into(),
        protocol: "modbus".into(),
        address: "1".into(),
        address_num: Some(1),
        slave_id: Some(1),
        interval: 10,
        ..Default::default()
    }];
    let resp = app
        .oneshot(req(
            "POST",
            "/api/config/diff",
            Some(serde_json::to_value(&changed).unwrap()),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = payload(json_body(resp).await);
    assert_eq!(body["has_changes"], true);
    assert_eq!(body["changed_fields"], serde_json::json!(["points"]));
}

#[tokio::test]
async fn config_import_rejects_config_without_devices() {
    let state = common::mock_state::mock_app_state();
    let app = gateway_api::web::build_router(state.clone());
    let cfg = current_config(&state);

    let resp = app
        .oneshot(req(
            "PUT",
            "/api/config/import",
            Some(serde_json::json!({ "config": cfg })),
        ))
        .await
        .unwrap();
    // 业务校验失败：HTTP 400 + code 4003
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = json_body(resp).await;
    assert_eq!(body["code"], 4003);
    assert!(
        body["message"].as_str().unwrap_or("").contains("至少需要一个设备"),
        "报错应说明缺设备，实际: {body}"
    );
    // 拒绝路径不得改动内存配置
    assert_eq!(
        current_config(&state).points.len(),
        0,
        "校验失败时不得部分应用配置"
    );
}

#[tokio::test]
async fn config_reload_returns_500_when_config_file_missing() {
    let state = common::mock_state::mock_app_state();
    let app = gateway_api::web::build_router(state.clone());

    // mock 状态未落盘 config.json，显式 reload 必须报错而不是静默生成默认配置
    // （临时目录里的同名文件可能由上一轮测试残留，先删除以保证「文件缺失」前提）
    let path = state.config.config_loader.path().to_path_buf();
    let _ = std::fs::remove_file(&path);
    assert!(!path.exists(), "前置条件：配置文件不存在");
    let resp = app
        .oneshot(req(
            "POST",
            "/api/config/reload",
            Some(serde_json::json!({ "config": {} })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = json_body(resp).await;
    assert!(
        body["message"].as_str().unwrap_or("").contains("reload failed"),
        "应提示 reload failed，实际: {body}"
    );
}

#[tokio::test]
async fn config_reload_applies_saved_config_from_disk() {
    let state = common::mock_state::mock_app_state();
    let cfg = current_config(&state);
    state
        .config
        .config_loader
        .save(&cfg)
        .expect("保存磁盘配置");

    let app = gateway_api::web::build_router(state.clone());
    let resp = app
        .oneshot(req(
            "POST",
            "/api/config/reload",
            Some(serde_json::json!({ "config": {} })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = payload(json_body(resp).await);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["applied"], serde_json::json!([]));
    assert_eq!(body["restart_required"], serde_json::json!([]));
}

#[tokio::test]
async fn config_rollback_to_missing_version_is_404() {
    let state = common::mock_state::mock_app_state();
    let app = gateway_api::web::build_router(state.clone());

    let resp = app
        .oneshot(req(
            "POST",
            "/api/config/versions/2000000/rollback",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = json_body(resp).await;
    assert!(
        body["message"]
            .as_str()
            .unwrap_or("")
            .contains("version 2000000 not found"),
        "应点名不存在的版本，实际: {body}"
    );
}

#[tokio::test]
async fn config_rollback_restores_saved_version() {
    let state = common::mock_state::mock_app_state();
    let cfg = current_config(&state);
    let vid = state
        .config
        .version_store
        .save_version(&cfg, "manual backup")
        .expect("写入版本快照");

    let app = gateway_api::web::build_router(state.clone());
    let resp = app
        .oneshot(req(
            "POST",
            &format!("/api/config/versions/{}/rollback", vid),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = payload(json_body(resp).await);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["rolled_back_to"], vid);
    // 回滚后内存配置应与快照一致（至少点位数一致），且落盘成功
    assert_eq!(
        current_config(&state).points.len(),
        cfg.points.len(),
        "回滚后内存配置应与版本快照一致"
    );
}
