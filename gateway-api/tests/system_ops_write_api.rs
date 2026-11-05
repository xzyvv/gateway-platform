//! 系统运维写接口集成测试（决策清单 P1-4 收尾）
//!
//! 覆盖 `docs/api-surface.md` 中最后 6 个无测试写端点：
//! - POST /api/certificates/upload
//! - POST /api/certificates/renew
//! - POST /api/ntp/sync
//! - POST /api/ota/upload
//! - POST /api/watchdog/services/:name/restart
//! - POST /api/webhook/test

mod common;

use axum::body::Body;
use axum::http::{header, Request};
use serde_json::{json, Value};
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789abcdef";

fn req(method: &str, uri: &str, body: Option<Value>) -> Request<Body> {
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

fn req_raw(method: &str, uri: &str, bytes: Vec<u8>) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {}", TOKEN))
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(Body::from(bytes))
        .unwrap()
}

async fn json_body(resp: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

async fn call(app: &axum::Router, method: &str, uri: &str, body: Option<Value>) -> Value {
    let resp = app.clone().oneshot(req(method, uri, body)).await.unwrap();
    json_body(resp).await
}

async fn call_raw(app: &axum::Router, method: &str, uri: &str, bytes: Vec<u8>) -> Value {
    let resp = app
        .clone()
        .oneshot(req_raw(method, uri, bytes))
        .await
        .unwrap();
    json_body(resp).await
}

#[tokio::test]
async fn certificate_upload_then_renew_replaces_pair() {
    let state = common::mock_state::mock_app_state();
    let app = gateway_api::web::build_router(state);

    // Mock 态证书路径为占位文件名（相对 cwd），测试结束统一清理，避免污染工作区
    let cert_path = "dummy_cert.pem";
    let key_path = "dummy_key.pem";

    let body = call(
        &app,
        "POST",
        "/api/certificates/upload",
        Some(json!({
            "name": "现场证书",
            "cert_pem": "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n",
            "key_pem": "-----BEGIN PRIVATE KEY-----\nMIIB\n-----END PRIVATE KEY-----\n"
        })),
    )
    .await;
    assert_eq!(body["code"], 0, "上传证书失败: {body}");
    assert!(
        std::path::Path::new(cert_path).exists() && std::path::Path::new(key_path).exists(),
        "上传必须把证书与私钥都落到磁盘"
    );

    // 重新生成自签名证书：覆盖上面上传的占位内容
    let body = call(&app, "POST", "/api/certificates/renew", None).await;
    assert_eq!(body["code"], 0, "重新生成证书失败: {body}");

    // 列表应能解析出重新生成的证书（上传的占位内容无法解析，因此这里验证的是 renew 生效）
    let body = call(&app, "GET", "/api/certificates", None).await;
    assert_eq!(body["code"], 0);
    let certs = body["data"].as_array().unwrap();
    assert_eq!(certs.len(), 1);
    assert!(certs[0]["days_remaining"].as_i64().unwrap() > 0);
    assert_eq!(certs[0]["enabled"], true);

    let _ = std::fs::remove_file(cert_path);
    let _ = std::fs::remove_file(key_path);
}

#[tokio::test]
async fn ntp_sync_always_reports_synced_because_rtc_fallback_is_not_error() {
    let state = common::mock_state::mock_app_state();
    let app = gateway_api::web::build_router(state);

    // 设计约定：NTP 不可达时回落到 RTC，接口不报错（避免运维误判为致命故障）
    let body = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        call(&app, "POST", "/api/ntp/sync", None),
    )
    .await
    .expect("校时接口不应挂起超过 60s");
    assert_eq!(body["code"], 0, "校时接口不应返回错误: {body}");
    assert_eq!(body["data"]["status"], "synced");

    let body = call(&app, "GET", "/api/ntp/status", None).await;
    assert_eq!(body["code"], 0);
    assert!(
        body["data"]["clock_trusted"].as_bool().unwrap(),
        "校时后时钟应可信: {body}"
    );
}

#[tokio::test]
async fn ota_upload_rejects_empty_and_classifies_full_and_delta() {
    let state = common::mock_state::mock_app_state();
    let app = gateway_api::web::build_router(state);

    // 空固件 → 4000，不得触发后台处理任务
    let body = call_raw(&app, "POST", "/api/ota/upload", Vec::new()).await;
    assert_eq!(body["code"], 4000);
    assert!(body["message"].as_str().unwrap().contains("empty firmware"));

    // 普通固件 → full（响应被全局信封包裹，业务字段在 data 下）
    let body = call_raw(
        &app,
        "POST",
        "/api/ota/upload",
        b"not-a-real-firmware".to_vec(),
    )
    .await;
    assert_eq!(body["code"], 0, "上传固件失败: {body}");
    assert_eq!(body["data"]["status"], "uploading");
    assert_eq!(body["data"]["type"], "full");

    // GWDT 魔数 → delta
    let mut delta = b"GWDT".to_vec();
    delta.extend_from_slice(&[0x01, 0x02, 0x03, 0x04]);
    let body = call_raw(&app, "POST", "/api/ota/upload", delta).await;
    assert_eq!(body["code"], 0, "上传增量包失败: {body}");
    assert_eq!(body["data"]["status"], "uploading");
    assert_eq!(body["data"]["type"], "delta");
}

#[tokio::test]
async fn watchdog_restart_reloads_services_and_records_event() {
    let state = common::mock_state::mock_app_state();
    let app = gateway_api::web::build_router(state.clone());

    let body = call(&app, "POST", "/api/watchdog/services/gateway/restart", None).await;
    assert_eq!(body["code"], 0, "重启服务失败: {body}");

    let body = call(&app, "GET", "/api/watchdog/services", None).await;
    assert_eq!(body["code"], 0);

    // 自愈事件里必须留下这次手动重启（便于事后审计「谁在什么时候重启过」）
    let events = state.platform.heal_events.recent(10);
    let summary: Vec<String> = events
        .iter()
        .map(|e| format!("{}:{}:{}", e.id, e.target, e.result))
        .collect();
    assert!(
        events
            .iter()
            .any(|e| e.target == "gateway" && e.result == "success"),
        "应记录一条成功的自愈事件: {summary:?}"
    );
}

#[tokio::test]
async fn webhook_test_rejects_bad_scheme_and_private_target_without_network() {
    let state = common::mock_state::mock_app_state();
    let app = gateway_api::web::build_router(state);

    // 非 http(s) → 拒绝，但不抛出错误（前端拿到可读原因）
    let body = call(
        &app,
        "POST",
        "/api/webhook/test",
        Some(json!({ "url": "ftp://example.com/hook", "method": "POST" })),
    )
    .await;
    assert_eq!(body["code"], 0);
    assert_eq!(body["data"]["ok"], false);
    assert_eq!(body["data"]["status_text"], "Invalid URL");

    // 链路本地地址（云元数据端点）→ 无论 block_private 开关如何都拦截，
    // 且不发起任何网络请求（默认策略允许环回/私网，故这里必须选恒拒目标）
    let body = call(
        &app,
        "POST",
        "/api/webhook/test",
        Some(json!({
            "url": "http://169.254.169.254/latest/meta-data",
            "method": "POST"
        })),
    )
    .await;
    assert_eq!(body["code"], 0);
    assert_eq!(body["data"]["ok"], false);
    assert_eq!(body["data"]["status_text"], "Blocked Target");
    assert_eq!(body["data"]["status"], 0);
    assert!(
        body["data"]["response_body"].as_str().unwrap().is_empty(),
        "被拦截的目标不应有响应体: {body}"
    );
}
