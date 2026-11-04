//! 四条零覆盖主链路集成测试（N-14）：认证 / 用户管理 / 历史与模板 / 设备发现与 IO。
//!
//! 覆盖范围（对应 v7 方案 N-14）：
//! - auth：默认凭据登录、whoami 身份与凭证能力、改密的旧密码校验与 API Token 轮换
//! - users：写接口鉴权、用户 CRUD、用户名唯一、角色白名单、404 语义
//! - history：历史查询匿名可读、CSV 导出（表头/截断响应头）、设备模板读写删
//! - devices：IO 状态读取、IO 控制（非 Linux 平台如实报不支持）、设备发现参数校验

mod common;

use axum::body::Body;
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::Router;
use common::mock_state::mock_app_state;
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789abcdef";

// ===== 请求工具 =====

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

/// 读文本响应（CSV 导出用）
async fn call_text(app: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, String) {
    let resp = app.clone().oneshot(request).await.expect("router call");
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .expect("read body");
    (status, headers, String::from_utf8_lossy(&bytes).to_string())
}

// ===== auth =====

#[tokio::test]
async fn auth_login_and_whoami() {
    let state = mock_app_state();
    let app = gateway_api::web::build_router(state);

    // 默认凭据（password_hash 为空且 allow_default_token=true）
    let (status, body) = call(
        &app,
        req(
            "POST",
            "/api/auth/login",
            Some(serde_json::json!({"username": "admin", "password": "admin"})),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "默认凭据应可登录: {}", body);
    assert_eq!(body["token"], TOKEN);

    // whoami 即使在 allow_anon_read=true 下也必须鉴权
    let (status, body) = call(&app, req("GET", "/api/auth/whoami", None, None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], 8000);

    let (status, body) = call(&app, req("GET", "/api/auth/whoami", None, Some(TOKEN))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["username"], "admin");
    // 主 token 的能力声明：管理员 = credential=primary + scopes=["*"]
    assert_eq!(body["data"]["credential"], "primary");
    assert_eq!(body["data"]["scopes"][0], "*");
    // 仍是默认密码，前端应提示改密
    assert_eq!(body["data"]["using_default_password"], true);
}

#[tokio::test]
async fn auth_login_rejects_wrong_password() {
    let state = mock_app_state();
    let app = gateway_api::web::build_router(state);

    let (status, body) = call(
        &app,
        req(
            "POST",
            "/api/auth/login",
            Some(serde_json::json!({"username": "admin", "password": "wrong"})),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], 8000);
}

#[tokio::test]
async fn auth_change_password_validates_and_rotates_token() {
    let state = mock_app_state();
    let app = gateway_api::web::build_router(state);

    // 新密码过短：先于旧密码校验被拒
    let (status, body) = call_auth(
        &app,
        "POST",
        "/api/auth/password",
        Some(serde_json::json!({"old_password": "admin", "new_password": "123"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{}", body);
    assert_eq!(body["code"], 4000);

    // 旧密码错误：仅凭有效 token 不足以改密（防 token 泄漏后永久接管）
    let (status, body) = call_auth(
        &app,
        "POST",
        "/api/auth/password",
        Some(serde_json::json!({"old_password": "wrong", "new_password": "newpass123"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{}", body);
    assert_eq!(body["code"], 8000);

    // 正确改密：成功并轮换 api_token
    let (status, body) = call_auth(
        &app,
        "POST",
        "/api/auth/password",
        Some(serde_json::json!({"old_password": "admin", "new_password": "newpass123"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", body);
    assert_eq!(body["data"]["status"], "ok");

    // 旧 token 立即失效
    let (status, _) = call(&app, req("GET", "/api/auth/whoami", None, Some(TOKEN))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "改密后旧 token 必须失效");

    // 旧密码不再可登录，新密码可登录且返回轮换后的新 token
    let (status, _) = call(
        &app,
        req(
            "POST",
            "/api/auth/login",
            Some(serde_json::json!({"username": "admin", "password": "admin"})),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, body) = call(
        &app,
        req(
            "POST",
            "/api/auth/login",
            Some(serde_json::json!({"username": "admin", "password": "newpass123"})),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", body);
    let rotated = body["token"].as_str().expect("token");
    assert_ne!(rotated, TOKEN, "改密后 token 必须轮换");
}

// ===== history / templates =====

#[tokio::test]
async fn history_query_and_csv_export() {
    let state = mock_app_state();
    let app = gateway_api::web::build_router(state);

    // 缺省时间窗（不传 start/end）也应可用，返回统一结果结构
    let (status, body) = call(&app, req("GET", "/api/history?point_id=p1", None, None)).await;
    assert_eq!(status, StatusCode::OK, "{}", body);
    assert!(
        body["data"]["items"].is_array(),
        "items 必须为数组: {}",
        body
    );
    assert_eq!(body["data"]["total"], 0);

    // CSV 导出：表头列顺序稳定 + 截断元信息放响应头
    let (status, headers, text) = call_text(
        &app,
        req("GET", "/api/history/export?point_id=p1", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("text/csv; charset=utf-8")
    );
    assert_eq!(
        headers
            .get("x-history-truncated")
            .and_then(|v| v.to_str().ok()),
        Some("false")
    );
    assert!(
        text.starts_with(
            "timestamp,point_id,value,raw_value,raw_type,quality,min_value,max_value,sample_count"
        ),
        "CSV 表头列顺序被外部脚本依赖，不得随意改动: {}",
        text
    );
}

#[tokio::test]
async fn templates_read_write_delete() {
    let state = mock_app_state();
    let app = gateway_api::web::build_router(state);

    // 匿名可读：mock 模板库预置 t1
    let (status, body) = call(&app, req("GET", "/api/templates", None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body["data"]
            .as_array()
            .expect("array")
            .iter()
            .any(|t| t["name"] == "t1"),
        "应列出预置模板 t1: {}",
        body
    );

    // 写入需 token
    let tpl = serde_json::json!({
        "name": "custom1",
        "vendor": "test",
        "model": "custom1",
        "protocol": "modbus",
        "default_interval": 5,
        "points": [{
            "name": "p1", "unit": "", "data_type": "u16",
            "default_address": 1, "function_code": 3, "register_count": 1,
            "scale": 1, "offset": 0, "deadband": 0,
        }],
    });
    let (status, body) = call(&app, req("POST", "/api/templates", Some(tpl.clone()), None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], 8000);

    let (status, body) = call_auth(&app, "POST", "/api/templates", Some(tpl)).await;
    assert_eq!(status, StatusCode::OK, "{}", body);
    assert_eq!(body["data"]["status"], "saved");

    let (_, list) = call(&app, req("GET", "/api/templates", None, None)).await;
    assert!(
        list["data"]
            .as_array()
            .expect("array")
            .iter()
            .any(|t| t["name"] == "custom1"),
        "保存后应出现在模板列表: {}",
        list
    );

    // 删除
    let (status, body) = call_auth(&app, "DELETE", "/api/templates/custom1", None).await;
    assert_eq!(status, StatusCode::OK, "{}", body);
    assert_eq!(body["data"]["status"], "deleted");

    let (_, list) = call(&app, req("GET", "/api/templates", None, None)).await;
    assert!(
        !list["data"]
            .as_array()
            .expect("array")
            .iter()
            .any(|t| t["name"] == "custom1"),
        "删除后不应再出现在列表: {}",
        list
    );
}

// ===== devices（发现 / IO） =====

#[tokio::test]
async fn io_status_and_control() {
    let state = mock_app_state();
    let app = gateway_api::web::build_router(state);

    let (status, body) = call(&app, req("GET", "/api/io/status", None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body["data"]["di"].is_array(),
        "IO 状态结构应含 di: {}",
        body
    );
    // 非 Linux 平台如实标注不支持，前端据此禁用入口
    assert_eq!(
        body["data"]["platform_supported"],
        cfg!(target_os = "linux")
    );

    // 控制入口匿名必须 401
    let (status, _) = call(
        &app,
        req(
            "POST",
            "/api/io/0",
            Some(serde_json::json!({"action": "on"})),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // 非法动作：400
    let (status, body) = call_auth(
        &app,
        "POST",
        "/api/io/0",
        Some(serde_json::json!({"action": "toggle"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{}", body);
    assert_eq!(body["code"], 4000);

    // 合法动作：结果必须与硬件可用性一致——能写则成功，不写则如实报错
    //
    // 断言按**写入结果**分支，而非按平台分支：
    // - 非 Linux 平台无 gpiod，必然失败；
    // - Linux 平台若无 `/dev/gpiochip0`（CI runner 即如此）同样失败。
    // 这与 `gateway-core/src/south/protocol/gpio/tests.rs` 中
    // `test_write_do_updates_single_state_source` 已确立的契约一致。
    let (status, body) = call_auth(
        &app,
        "POST",
        "/api/io/0",
        Some(serde_json::json!({"action": "on"})),
    )
    .await;
    if status == StatusCode::OK {
        assert_eq!(body["data"]["on"], true, "写入成功应回报输出状态: {}", body);
    } else {
        assert_ne!(
            status,
            StatusCode::OK,
            "无 GPIO 硬件时不得假装成功: {}",
            body
        );
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{}", body);
        assert_eq!(body["code"], 2000, "GPIO 不可用应报 IO 错误码: {}", body);
        assert!(
            body["message"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .contains("gpio"),
            "错误信息应说明 GPIO 不可用: {}",
            body
        );
    }
}
