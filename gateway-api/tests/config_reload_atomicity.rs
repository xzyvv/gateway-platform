//! 配置热更新**原子性**测试。
//!
//! 所有配置写回都走唯一路径 `reload_then_save`（expand → reload → store → save）：
//! 只有 core 侧热重载成功后才替换内存快照、再落盘。因此任何前置校验失败或
//! 热重载被拒时，内存与磁盘都必须**保持原样**，不能出现「半新半旧」。
//!
//! 覆盖三条此前无测试的路径：
//! 1. core 热重载返回错误（总线/采集器重建失败）→ 内存快照指针都不应被替换；
//! 2. 深层结构校验失败（通过 handler 的浅校验、被 `validate_all` 拦下）→ 同上；
//! 3. 并发导入时读者永远看不到混合快照（ArcSwap 换的是整份配置，而非逐字段改）。

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use gateway_api::web::AppState;
use gateway_core::config::GatewayConfig;
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

async fn json_body(resp: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

/// 一份能通过全部校验的配置：总线 + 引用模板 t1 的设备（mock 状态已提供该模板）。
///
/// `topic` / `username` 是两个互不相干的字段，并发用例用它们判定「快照是否混版」。
fn fixture(state: &Arc<AppState>, topic: &str, username: &str) -> GatewayConfig {
    let mut v = serde_json::to_value(state.config.load_full().as_ref()).expect("序列化当前配置");
    v["buses"] = json!([{ "port_name": "bus0", "protocol": "modbus" }]);
    v["devices"] = json!([{
        "id": "dev1",
        "name": "dev1",
        "template": "t1",
        "bus": "bus0",
        "protocol": "modbus",
        "address": "1",
        "slave_id": 1,
        "interval": 10
    }]);
    v["mqtt"]["topic"] = json!(topic);
    if let Some(web) = v.get_mut("web") {
        web["username"] = json!(username);
    }
    serde_json::from_value(v).expect("夹具配置应可反序列化")
}

/// 读盘上的配置字节（原子性的磁盘侧证据）
fn disk_bytes(state: &Arc<AppState>) -> Vec<u8> {
    std::fs::read(state.config.config_loader.path()).expect("mock 状态应已落盘一份配置")
}

/// core 侧热重载被拒：内存与磁盘都必须是旧配置。
#[tokio::test]
async fn reload_failure_leaves_memory_and_disk_untouched() {
    let (state, gw) = common::mock_state::mock_app_state_with_gateway();
    let app = gateway_api::web::build_router(state.clone());

    let mem_before = state.config.load_full();
    let disk_before = disk_bytes(&state);

    // 模拟 core 重建总线失败（真实现返回 GatewayError，不应用任何分节）
    gw.fail_next_reload("bus rebuild failed");

    let resp = app
        .clone()
        .oneshot(req(
            "PUT",
            "/api/config/import",
            Some(json!({ "config": fixture(&state, "gw/new", "operator") })),
        ))
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "热重载被拒必须作为失败返回，不能静默成功"
    );
    let body = json_body(resp).await;
    assert!(
        body["message"]
            .as_str()
            .unwrap_or("")
            .contains("bus rebuild failed"),
        "应把 core 的失败原因透出给调用方，实际: {body}"
    );

    let mem_after = state.config.load_full();
    assert!(
        Arc::ptr_eq(&mem_before, &mem_after),
        "失败路径不得替换内存配置快照"
    );
    assert!(
        mem_after.devices.as_ref().is_none_or(|d| d.is_empty()),
        "内存配置不得被部分应用"
    );
    assert_eq!(
        disk_bytes(&state),
        disk_before,
        "失败路径不得写盘（磁盘配置必须逐字节不变）"
    );

    // 失败是一次性的：紧接着的同一份配置应能正常导入（模拟 core 恢复）
    let resp = app
        .clone()
        .oneshot(req(
            "PUT",
            "/api/config/import",
            Some(json!({ "config": fixture(&state, "gw/new", "operator") })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "core 恢复后应可正常导入");
}

/// 深层结构校验失败（通过了 handler 的浅校验）同样不得改动内存与磁盘。
#[tokio::test]
async fn deep_validation_failure_leaves_memory_and_disk_untouched() {
    let state = common::mock_state::mock_app_state();
    let app = gateway_api::web::build_router(state.clone());

    let mem_before = state.config.load_full();
    let disk_before = disk_bytes(&state);

    // devices 非空 → 通过 import handler 的浅校验；点位协议非法 → 被 validate_all 拦下
    let mut cfg = fixture(&state, "gw/new", "operator");
    cfg.points = vec![serde_json::from_value(json!({
        "id": "bad-p1",
        "bus": "bus0",
        "protocol": "not_a_protocol",
        "address": "1",
        "interval": 10
    }))
    .expect("点位应可反序列化")];

    let resp = app
        .oneshot(req(
            "PUT",
            "/api/config/import",
            Some(json!({ "config": cfg })),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = json_body(resp).await;
    assert_eq!(
        body["code"], 4000,
        "深层校验失败应返回统一错误结构（bad_request），实际: {body}"
    );

    let mem_after = state.config.load_full();
    assert!(
        Arc::ptr_eq(&mem_before, &mem_after),
        "校验失败不得替换内存配置快照"
    );
    assert_eq!(
        disk_bytes(&state),
        disk_before,
        "校验失败不得写盘（磁盘配置必须逐字节不变）"
    );
}

/// 并发导入期间，读者看到的每份快照都必须是完整的一版（不会 A 的字段 + B 的字段）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_imports_never_expose_mixed_snapshot() {
    let state = common::mock_state::mock_app_state();
    let app = gateway_api::web::build_router(state.clone());

    // 基线：先成功导入 A，保证读者观察期间快照始终是 A 或 B 之一
    let resp = app
        .clone()
        .oneshot(req(
            "PUT",
            "/api/config/import",
            Some(json!({ "config": fixture(&state, "gw/A", "adminA") })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "基线导入失败");

    let stop = Arc::new(AtomicBool::new(false));
    let reader_state = state.clone();
    let reader_stop = stop.clone();
    let reader = tokio::spawn(async move {
        let mut checks = 0usize;
        while !reader_stop.load(Ordering::Relaxed) {
            let cfg = reader_state.config.load_full();
            let topic = cfg.mqtt.topic.clone();
            let username = cfg
                .web
                .as_ref()
                .map(|w| w.username.clone())
                .unwrap_or_default();
            let is_a = topic == "gw/A" && username == "adminA";
            let is_b = topic == "gw/B" && username == "adminB";
            assert!(
                is_a || is_b,
                "读到半新半旧的快照: topic={topic} username={username}"
            );
            checks += 1;
            tokio::task::yield_now().await;
        }
        // 至少读到几次，否则本用例没有实际观察意义
        assert!(checks > 0, "读者一次都没读到快照");
    });

    for i in 0..12 {
        let (topic, user) = if i % 2 == 0 {
            ("gw/B", "adminB")
        } else {
            ("gw/A", "adminA")
        };
        let resp = app
            .clone()
            .oneshot(req(
                "PUT",
                "/api/config/import",
                Some(json!({ "config": fixture(&state, topic, user) })),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "第 {i} 次导入失败");
    }

    stop.store(true, Ordering::Relaxed);
    reader.await.expect("读者任务不应 panic");
}
