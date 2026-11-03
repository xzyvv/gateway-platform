//! 内置模拟消费者（SimConsumer）API 集成测试
//!
//! 覆盖三个端点：GET /api/sim_consumer/status、POST /api/sim_consumer/start、
//! POST /api/sim_consumer/stop。
//!
//! 关键契约：
//! - 读接口匿名可读（mock 态 allow_anon_read=true），写接口必须带 token；
//! - `enabled`（配置意图）与 `running`（实际运行态）在状态里分别体现；
//! - 已在运行时再次 start → 409；未运行时 stop → 409（不能静默成功）；
//! - MQTT 已启用时拒绝启动，并给出可读的阻塞原因（mqtt_enabled）。

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use common::mock_state::mock_app_state;
use std::sync::Arc;
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

/// 把 mock 态配置里的 MQTT 打开（模拟「有真实北向通道」的现场）。
fn enable_mqtt(state: &Arc<gateway_api::web::AppState>) {
    let mut cfg = state.config.load_full().as_ref().clone();
    cfg.mqtt.enabled = true;
    state.config.store(Arc::new(cfg));
}

/// 契约：状态接口同时暴露「配置意图（enabled）」与「实际运行态（running）」，
/// 未启动时 running=false、started_at/uptime 为空，队列长度可读。
#[tokio::test]
async fn status_reports_config_intent_and_runtime_state() {
    let state = mock_app_state();
    let app = gateway_api::web::build_router(state);

    let (status, body) = call(&app, req("GET", "/api/sim_consumer/status", None, None)).await;
    assert_eq!(status, StatusCode::OK, "{}", body);
    assert_eq!(body["code"], 0);
    let data = &body["data"];
    assert_eq!(data["running"], false, "初始不得处于运行态");
    assert_eq!(data["enabled"], true, "配置意图应如实上报");
    assert!(data["started_at_ms"].is_null(), "未运行不得有启动时刻");
    assert!(data["uptime_ms"].is_null(), "未运行不得有运行时长");
    assert!(data["queue_len"].is_number(), "队列长度必须是数字");
    assert!(
        data["last_consumed_at_ms"].is_null(),
        "未消费过不得有消费时刻"
    );
    assert!(
        data["blocked_reason"].is_null(),
        "MQTT 未启用时不得有阻塞原因"
    );
}

/// 契约：启动 → 状态转 running=true 且带启动时刻/运行时长；停止 → 回到
/// running=false。启停都要等 run 循环真正退出，故停止后状态立即可读。
#[tokio::test]
async fn start_then_stop_toggles_running_state() {
    let state = mock_app_state();
    let app = gateway_api::web::build_router(state);

    // 启动（带运行参数）
    let (status, body) = call_auth(
        &app,
        "POST",
        "/api/sim_consumer/start",
        Some(serde_json::json!({ "batchSize": 20, "intervalMs": 50, "logEvery": 5 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", body);
    assert_eq!(body["code"], 0);
    let data = &body["data"];
    assert_eq!(data["running"], true, "启动后应处于运行态");
    assert_eq!(data["batch_size"], 20, "运行参数应被应用");
    assert_eq!(data["interval_ms"], 50, "运行参数应被应用");
    assert_eq!(data["log_every"], 5, "运行参数应被应用");
    assert!(data["started_at_ms"].is_number(), "运行中应有启动时刻");
    assert!(data["uptime_ms"].is_number(), "运行中应有运行时长");

    // 停止
    let (status, body) = call_auth(&app, "POST", "/api/sim_consumer/stop", None).await;
    assert_eq!(status, StatusCode::OK, "{}", body);
    assert_eq!(body["data"]["running"], false, "停止后应回到非运行态");
    assert!(
        body["data"]["started_at_ms"].is_null(),
        "停止后不得有启动时刻"
    );

    // 状态接口同步反映
    let (status, body) = call(&app, req("GET", "/api/sim_consumer/status", None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["running"], false);
}

/// 契约：重复启停是**冲突**而非静默成功——已在运行时再 start 返回 409，
/// 未运行时 stop 返回 409（调用方据此判断状态迁移是否合法）。
#[tokio::test]
async fn duplicate_start_and_stop_return_conflict() {
    let state = mock_app_state();
    let app = gateway_api::web::build_router(state);

    // 未运行就 stop → 409
    let (status, body) = call_auth(&app, "POST", "/api/sim_consumer/stop", None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{}", body);
    assert_ne!(body["code"], 0);

    // 启动
    let (status, _) = call_auth(&app, "POST", "/api/sim_consumer/start", None).await;
    assert_eq!(status, StatusCode::OK);

    // 已运行再 start → 409
    let (status, body) = call_auth(&app, "POST", "/api/sim_consumer/start", None).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "已在运行时重复启动应 409: {}",
        body
    );
    assert_ne!(body["code"], 0);

    // 清理：停掉，避免遗留运行中的消费者
    let (status, _) = call_auth(&app, "POST", "/api/sim_consumer/stop", None).await;
    assert_eq!(status, StatusCode::OK);
}

/// 契约：MQTT 已启用时拒绝启动（两个消费者会分流同一批数据 → 静默丢数），
/// 状态接口给出可读的阻塞原因 mqtt_enabled。
#[tokio::test]
async fn mqtt_enabled_blocks_start_with_readable_reason() {
    let state = mock_app_state();
    enable_mqtt(&state);
    let app = gateway_api::web::build_router(state);

    // 启动被拒
    let (status, body) = call_auth(&app, "POST", "/api/sim_consumer/start", None).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "MQTT 启用时应拒绝启动: {}",
        body
    );
    assert_ne!(body["code"], 0);
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("MQTT"),
        "错误信息应说明 MQTT 冲突原因: {}",
        body["message"]
    );

    // 状态接口给出阻塞原因
    let (status, body) = call(&app, req("GET", "/api/sim_consumer/status", None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["data"]["blocked_reason"], "mqtt_enabled",
        "阻塞原因须是可读标识: {}",
        body["data"]["blocked_reason"]
    );
    assert_eq!(body["data"]["running"], false, "被挡住时不得处于运行态");
}

/// 契约：写接口必须鉴权（token 缺失 → 401），读接口匿名可读。
#[tokio::test]
async fn write_endpoints_require_token() {
    let state = mock_app_state();
    let app = gateway_api::web::build_router(state);

    let (status, body) = call(&app, req("POST", "/api/sim_consumer/start", None, None)).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "启动接口必须要求 token: {}",
        body
    );
    assert_ne!(body["code"], 0);

    let (status, body) = call(&app, req("POST", "/api/sim_consumer/stop", None, None)).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "停止接口必须要求 token: {}",
        body
    );
    assert_ne!(body["code"], 0);
}

/// 契约：「是否存在会消费队列的北向消费者」随模拟消费者的**实际运行态**翻转。
///
/// 背景：内置模拟消费者没有 MQTT 客户端，但同样会 dequeue → ack 队列。
/// 若队列健康判定只看 MQTT，模拟消费者在跑时会被误判成「无消费者（本地历史存量）」，
/// 与后台积压告警监控给出互斥结论。这里锁定 `NorthRefs::has_north_consumer()`
/// 的运行期语义——它是队列健康判定的唯一口径来源。
#[tokio::test]
async fn north_consumer_presence_follows_sim_consumer_runtime_state() {
    let state = mock_app_state();
    // mock 态无 MQTT 客户端，故消费者存在与否完全由模拟消费者决定
    assert!(
        !state.north.has_north_consumer(),
        "初始未启动，不得判定为有消费者"
    );

    // 启动 → 有消费者
    let (status, body) = call_auth(
        &gateway_api::web::build_router(state.clone()),
        "POST",
        "/api/sim_consumer/start",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", body);
    assert!(
        state.north.has_north_consumer(),
        "模拟消费者运行中，应判定为有北向消费者"
    );

    // 停止 → 无消费者
    let (status, body) = call_auth(
        &gateway_api::web::build_router(state.clone()),
        "POST",
        "/api/sim_consumer/stop",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", body);
    assert!(
        !state.north.has_north_consumer(),
        "模拟消费者已停，不得判定为有北向消费者"
    );
}
