//! 访问控制域（限流规则 / API 密钥 / 计划任务）集成测试
//!
//! 覆盖范围：
//! - 读接口匿名可访问、写接口必须带主 token（鉴权分层）
//! - 限流规则 CRUD、入参校验（含防自锁）、规则真实生效（429 + Retry-After）
//! - 访问日志/统计落库（异步旁路写，故用轮询等待）
//! - API 密钥一次性返回完整 Key、scope 校验、停用后失效
//! - 计划任务 CRUD、nextRunAt 计算、run-now 结果回写

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use common::mock_state::mock_app_state;
use gateway_api::web::AppState;
use gateway_core::config::RateRule;
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

/// 发请求并解析响应体（非 JSON 时返回 Null）
async fn call(app: &Router, request: Request<Body>) -> (StatusCode, serde_json::Value) {
    let resp = app.clone().oneshot(request).await.expect("router call");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .expect("read body");
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

/// 带主 token 调用
async fn call_auth(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    call(app, req(method, uri, body, Some(TOKEN))).await
}

fn rule_body(name: &str, path: &str, method: &str, per_minute: u32) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "path": path,
        "method": method,
        "limitPerMinute": per_minute,
        "limitPerHour": 1000,
        "burst": 0,
        "enabled": true,
    })
}

#[tokio::test]
async fn read_endpoints_require_token_under_sensitive_path() {
    let state = mock_app_state();
    let app = gateway_api::web::build_router(state);

    // `/api/access/*` 属敏感读路径（密钥与限流日志即凭据）：即便 allow_anon_read=true，
    // 匿名读也必须被拦成 401（见 middleware::auth::is_sensitive_read_path）
    let (status, _) = call(&app, req("GET", "/api/access/rate-rules", None, None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    for uri in [
        "/api/access/rate-rules",
        "/api/access/rate-stats",
        "/api/access/rate-logs",
        "/api/access/api-keys",
        "/api/access/scheduled-tasks",
    ] {
        let (status, body) = call_auth(&app, "GET", uri, None).await;
        assert_eq!(status, StatusCode::OK, "{} 带 token 读应放行", uri);
        assert_eq!(body["code"], 0, "{} 返回统一响应体", uri);
        assert!(
            body["data"].is_array() || body["data"].is_object(),
            "{} data 非空",
            uri
        );
    }
}

#[tokio::test]
async fn write_endpoints_require_token() {
    let state = mock_app_state();
    let app = gateway_api::web::build_router(state);

    let (status, body) = call(
        &app,
        req(
            "POST",
            "/api/access/rate-rules",
            Some(rule_body("r", "/api/points", "GET", 10)),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], 8000);
}

#[tokio::test]
async fn rate_rule_crud_roundtrip() {
    let state = mock_app_state();
    let app = gateway_api::web::build_router(state);

    // 创建：method 归一化为大写
    let (status, body) = call_auth(
        &app,
        "POST",
        "/api/access/rate-rules",
        Some(rule_body("点位读取", "/api/points", "get", 30)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "创建失败: {}", body);
    let id = body["data"]["id"].as_str().expect("id").to_string();
    assert_eq!(body["data"]["method"], "GET");
    assert_eq!(body["data"]["limitPerMinute"], 30);

    // 列表（/api/access 为敏感读路径，需带 token）
    let (_, list) = call_auth(&app, "GET", "/api/access/rate-rules", None).await;
    assert_eq!(list["data"].as_array().map(|a| a.len()), Some(1));

    // 更新
    let uri = format!("/api/access/rate-rules/{}", id);
    let (status, updated) = call_auth(
        &app,
        "PUT",
        &uri,
        Some(rule_body("点位读取", "/api/points", "GET", 60)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["data"]["limitPerMinute"], 60);

    // 删除，再删一次应为 404
    let (status, _) = call_auth(&app, "DELETE", &uri, None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = call_auth(&app, "DELETE", &uri, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], 4004);
}

#[tokio::test]
async fn rate_rule_rejects_bad_input() {
    let state = mock_app_state();
    let app = gateway_api::web::build_router(state);

    // 限额为 0
    let (status, body) = call_auth(
        &app,
        "POST",
        "/api/access/rate-rules",
        Some(rule_body("r", "/api/points", "GET", 0)),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], 4000);

    // 覆盖管理入口 → 防自锁
    for path in ["/api/access/*", "/api/*", "/api/access/rate-rules"] {
        let (status, body) = call_auth(
            &app,
            "POST",
            "/api/access/rate-rules",
            Some(rule_body("bad", path, "GET", 10)),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{} 应被拒", path);
        assert!(
            body["message"].as_str().unwrap_or("").contains("管理入口"),
            "{} 错误信息应说明原因: {}",
            path,
            body
        );
    }

    // 非法方法
    let (status, _) = call_auth(
        &app,
        "POST",
        "/api/access/rate-rules",
        Some(rule_body("bad", "/api/points", "HEAD", 10)),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn rate_rule_actually_limits_and_logs() {
    let state = mock_app_state();
    // 直接落库（不走 HTTP），保证首个请求就能加载到规则
    state
        .platform
        .rate_rules
        .create_rule(RateRule {
            id: String::new(),
            name: "限制点位读取".into(),
            path: "/api/points".into(),
            method: "GET".into(),
            limit_per_minute: 1,
            limit_per_hour: 1000,
            burst: 0,
            enabled: true,
            created_at: 0,
            updated_at: 0,
        })
        .expect("seed rule");
    let app = gateway_api::web::build_router(state.clone());

    // 第一次放行（不论业务结果如何，不应被限流）
    let first = app
        .clone()
        .oneshot(req("GET", "/api/points", None, None))
        .await
        .expect("first");
    assert_ne!(first.status(), StatusCode::TOO_MANY_REQUESTS);

    // 第二次超出 1 次/分钟 → 429 + Retry-After
    let second = app
        .clone()
        .oneshot(req("GET", "/api/points", None, None))
        .await
        .expect("second");
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        second
            .headers()
            .get(header::RETRY_AFTER)
            .map(|v| v.to_str().unwrap_or("")),
        Some("1")
    );

    // 统计与日志：日志由 spawn_blocking 旁路写入，轮询等待
    let logs = wait_for_logs(&state, 2);
    assert!(
        logs.iter().any(|l| l.path == "/api/points" && l.limited),
        "应记录被限流请求: {:?}",
        logs
    );
    let stats = state.platform.rate_rules.stats().expect("stats");
    assert!(stats.limited_requests >= 1, "限流计数应增加: {:?}", stats);
}

#[tokio::test]
async fn api_key_create_returns_full_key_once() {
    let state = mock_app_state();
    let app = gateway_api::web::build_router(state);

    let (status, body) = call_auth(
        &app,
        "POST",
        "/api/access/api-keys",
        Some(
            serde_json::json!({ "name": "对接方A", "scopes": ["points:read"], "expiresDays": 30 }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "创建失败: {}", body);

    let key = body["data"]["key"].as_str().expect("完整 key").to_string();
    let preview = body["data"]["keyPreview"]
        .as_str()
        .expect("预览")
        .to_string();
    assert!(key.starts_with("gwk_"));
    assert_eq!(key.len(), 44, "gwk_ + 40 hex");
    assert_eq!(preview, format!("{}…{}", &key[..8], &key[key.len() - 4..]));
    assert!(body["data"]["expiresAt"].is_number());

    // 列表不再暴露完整 Key（/api/access 为敏感读路径，需带 token）
    let (_, list) = call_auth(&app, "GET", "/api/access/api-keys", None).await;
    let items = list["data"].as_array().expect("array");
    assert_eq!(items.len(), 1);
    assert!(items[0].get("key").is_none(), "列表不应返回完整 Key");
    assert_eq!(items[0]["keyPreview"], preview);
    assert_eq!(items[0]["callCount"], 0);
}

#[tokio::test]
async fn api_key_scope_and_lifecycle() {
    let state = mock_app_state();
    let app = gateway_api::web::build_router(state);

    let (_, created) = call_auth(
        &app,
        "POST",
        "/api/access/api-keys",
        Some(serde_json::json!({ "name": "只读", "scopes": ["points:read"] })),
    )
    .await;
    let id = created["data"]["id"].as_str().expect("id").to_string();
    let key = created["data"]["key"].as_str().expect("key").to_string();

    // 只读密钥可读
    let (status, _) = call(&app, req("GET", "/api/access/rate-stats", None, Some(&key))).await;
    assert_eq!(status, StatusCode::OK);

    // 只读密钥写 → 403/8003
    let (status, body) = call(
        &app,
        req(
            "POST",
            "/api/access/rate-rules",
            Some(rule_body("r", "/api/points", "GET", 10)),
            Some(&key),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], 8003);

    // 主 token 停用密钥后，密钥立即失效（401，与错误 token 同返，防枚举）
    let uri = format!("/api/access/api-keys/{}", id);
    let (status, _) = call_auth(
        &app,
        "PUT",
        &uri,
        Some(serde_json::json!({ "enabled": false })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = call(
        &app,
        req(
            "POST",
            "/api/access/rate-rules",
            Some(rule_body("r", "/api/points", "GET", 10)),
            Some(&key),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], 8000);
}

#[tokio::test]
async fn api_key_with_write_scope_can_write() {
    let state = mock_app_state();
    let app = gateway_api::web::build_router(state);

    let (_, created) = call_auth(
        &app,
        "POST",
        "/api/access/api-keys",
        Some(serde_json::json!({ "name": "可写", "scopes": ["points:write"] })),
    )
    .await;
    let key = created["data"]["key"].as_str().expect("key").to_string();

    // points:write 密钥可写点位域（POST 语义的检索接口，入参简单且无副作用）
    let (status, body) = call(
        &app,
        req(
            "POST",
            "/api/points/find-replace/search",
            Some(serde_json::json!({ "keyword": "不存在", "scope": "all" })),
            Some(&key),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "点位域写 scope 应放行: {}", body);
    assert_eq!(body["code"], 0);
    assert!(body["data"].is_array(), "返回结果数组");

    // 跨域写被拒：同一密钥不能改限流规则（系统域写操作仅主 token）
    let (status, body) = call(
        &app,
        req(
            "POST",
            "/api/access/rate-rules",
            Some(rule_body("r", "/api/devices", "GET", 20)),
            Some(&key),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "跨域写应被拒: {}", body);
    assert_eq!(body["code"], 8003);
}

#[tokio::test]
async fn api_key_rejects_unknown_scope() {
    let state = mock_app_state();
    let app = gateway_api::web::build_router(state);

    let (status, body) = call_auth(
        &app,
        "POST",
        "/api/access/api-keys",
        Some(serde_json::json!({ "name": "x", "scopes": ["root:all"] })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["message"]
        .as_str()
        .unwrap_or("")
        .contains("未知权限范围"));
}

#[tokio::test]
async fn sched_task_crud_and_run_now() {
    let state = mock_app_state();
    let app = gateway_api::web::build_router(state);

    // 创建：读点位任务，nextRunAt 应为未来时刻
    let (status, body) = call_auth(
        &app,
        "POST",
        "/api/access/scheduled-tasks",
        Some(serde_json::json!({
            "name": "五分钟读点",
            "type": "read_point",
            "target": "missing-point",
            "cron": "*/5 * * * *",
            "enabled": true,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "创建失败: {}", body);
    let id = body["data"]["id"].as_str().expect("id").to_string();
    assert_eq!(body["data"]["type"], "read_point");
    let next = body["data"]["nextRunAt"].as_i64().expect("nextRunAt");
    assert!(next > chrono::Utc::now().timestamp_millis());

    // 停用 → nextRunAt 清空
    let uri = format!("/api/access/scheduled-tasks/{}", id);
    let (status, updated) = call_auth(
        &app,
        "PUT",
        &uri,
        Some(serde_json::json!({
            "name": "五分钟读点",
            "type": "read_point",
            "target": "missing-point",
            "cron": "*/5 * * * *",
            "enabled": false,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(updated["data"]["nextRunAt"].is_null(), "停用任务不排程");

    // run-now：目标点位不存在 → 执行失败但接口本身 200，并回写任务记录
    let (status, run) = call_auth(&app, "POST", &format!("{}/run", uri), None).await;
    assert_eq!(status, StatusCode::OK, "run-now 应返回 200: {}", run);
    assert_eq!(run["data"]["status"], "error");
    assert!(run["data"]["error"].is_string());

    let (_, list) = call_auth(&app, "GET", "/api/access/scheduled-tasks", None).await;
    let task = &list["data"][0];
    assert_eq!(task["lastStatus"], "error");
    assert_eq!(task["runCount"], 1);
    assert!(task["lastRunAt"].is_number());

    // 删除
    let (status, _) = call_auth(&app, "DELETE", &uri, None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call_auth(&app, "DELETE", &uri, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn sched_task_rejects_bad_input() {
    let state = mock_app_state();
    let app = gateway_api::web::build_router(state);

    let cases = [
        serde_json::json!({ "name": "x", "type": "read_point", "target": "p1", "cron": "not-a-cron" }),
        serde_json::json!({ "name": "x", "type": "read_point", "target": "", "cron": "*/5 * * * *" }),
        serde_json::json!({ "name": "x", "type": "restart_gateway", "cron": "*/5 * * * *" }),
        serde_json::json!({ "name": "", "type": "sync_ntp", "cron": "*/5 * * * *" }),
    ];
    for case in cases {
        let (status, body) = call_auth(
            &app,
            "POST",
            "/api/access/scheduled-tasks",
            Some(case.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{} 应被拒: {}", case, body);
    }
}

// ===== 辅助：等待异步访问日志落库 =====

fn wait_for_logs(state: &Arc<AppState>, min: usize) -> Vec<gateway_core::config::ApiAccessLog> {
    for _ in 0..100 {
        let logs = state
            .platform
            .rate_rules
            .recent_access_logs(50)
            .expect("logs");
        if logs.len() >= min {
            return logs;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    state
        .platform
        .rate_rules
        .recent_access_logs(50)
        .expect("logs")
}
