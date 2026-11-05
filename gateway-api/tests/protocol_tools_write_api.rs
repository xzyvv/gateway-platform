//! 协议工具写接口集成测试（决策清单 P1-4 补差集）
//!
//! 覆盖 `docs/api-surface.md` 中此前无测试的 14 个写端点：
//! - 转换规则：POST / PUT / DELETE / POST toggle
//! - 抓包：POST /api/protocol-tools/packet-capture/clear
//! - 报文编辑器：POST /api/protocol-tools/packet-editor/send
//! - 回放会话：POST /api/protocol-tools/packet-replay/sessions、DELETE .../:id
//! - 自动探测：POST /api/protocol-tools/auto-detect/run/:id/stop
//! - 寄存器扫描：DELETE /api/protocol-tools/register-scan/:id、POST .../:id/suggestions
//! - 从站模拟器：POST start / stop、PUT /simulator/registers/:addr

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request};
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

async fn call(app: &axum::Router, method: &str, uri: &str, body: Option<Value>) -> Value {
    let resp = app.clone().oneshot(req(method, uri, body)).await.unwrap();
    json_body(resp).await
}

fn current_config(state: &Arc<gateway_api::web::AppState>) -> GatewayConfig {
    state.config.load_full().as_ref().clone()
}

fn rule_json(id: &str, name: &str, transform: &str) -> Value {
    json!({
        "id": id,
        "name": name,
        "source_proto": "modbus",
        "source_point_id": "p1",
        "source_point_name": "电压",
        "target_proto": "mqtt",
        "target_tag": "voltage",
        "transform": transform,
        "scale": null,
        "offset": null,
        "enum_map": null,
        "enabled": true,
        "last_converted_at_ms": null,
        "last_value": null,
        "created_at_ms": 0
    })
}

/// 夹具：bus0（modbus），用于报文下发/寄存器扫描等需要真实总线的分支。
async fn app_with_bus() -> (Arc<gateway_api::web::AppState>, axum::Router) {
    let state = common::mock_state::mock_app_state();
    let mut v = serde_json::to_value(current_config(&state)).unwrap();
    v["buses"] = json!([{ "port_name": "bus0", "protocol": "modbus" }]);
    let cfg: GatewayConfig = serde_json::from_value(v).expect("夹具配置应可反序列化");
    state.config.config_loader.save(&cfg).expect("写入夹具配置");

    let app = gateway_api::web::build_router(state.clone());
    let body = call(
        &app,
        "POST",
        "/api/config/reload",
        Some(json!({ "config": {} })),
    )
    .await;
    assert_eq!(body["code"], 0, "夹具配置 reload 失败: {body}");
    (state, app)
}

#[tokio::test]
async fn convert_rules_crud_validation_toggle() {
    let (state, app) = app_with_bus().await;

    // 创建（transform=raw 无需 scale/offset）
    let body = call(
        &app,
        "POST",
        "/api/protocol-tools/convert/rules",
        Some(rule_json("cv_it1", "电压转发", "raw")),
    )
    .await;
    assert_eq!(body["code"], 0, "创建转换规则失败: {body}");
    assert_eq!(body["data"]["id"], "cv_it1");
    assert_eq!(current_config(&state).convert_rules.unwrap().len(), 1);

    // 同 id 再建 → 4000
    let body = call(
        &app,
        "POST",
        "/api/protocol-tools/convert/rules",
        Some(rule_json("cv_it1", "重复", "raw")),
    )
    .await;
    assert_eq!(body["code"], 4000);
    assert!(body["message"].as_str().unwrap().contains("id 已存在"));

    // linear 缺 scale/offset → 4000，且不落盘
    let mut bad = rule_json("cv_it2", "线性", "linear");
    bad["source_point_id"] = json!("p2");
    let body = call(&app, "POST", "/api/protocol-tools/convert/rules", Some(bad)).await;
    assert_eq!(body["code"], 4000);
    assert!(body["message"].as_str().unwrap().contains("scale 必填"));
    assert_eq!(current_config(&state).convert_rules.unwrap().len(), 1);

    // 更新
    let mut upd = rule_json("cv_it1", "电压转发(改)", "raw");
    upd["target_tag"] = json!("voltage2");
    let body = call(
        &app,
        "PUT",
        "/api/protocol-tools/convert/rules/cv_it1",
        Some(upd),
    )
    .await;
    assert_eq!(body["code"], 0, "更新转换规则失败: {body}");
    let cfg = current_config(&state);
    assert_eq!(cfg.convert_rules.as_ref().unwrap()[0].name, "电压转发(改)");
    assert_eq!(
        cfg.convert_rules.as_ref().unwrap()[0].target_tag,
        "voltage2"
    );

    // 更新不存在的规则 → 4004
    let body = call(
        &app,
        "PUT",
        "/api/protocol-tools/convert/rules/cv_ghost",
        Some(rule_json("cv_ghost", "x", "raw")),
    )
    .await;
    assert_eq!(body["code"], 4004);

    // 切换启用状态：true → false
    let body = call(
        &app,
        "POST",
        "/api/protocol-tools/convert/rules/cv_it1/toggle",
        None,
    )
    .await;
    assert_eq!(body["code"], 0);
    assert_eq!(body["data"], json!(false));
    let body = call(
        &app,
        "POST",
        "/api/protocol-tools/convert/rules/cv_it1/toggle",
        None,
    )
    .await;
    assert_eq!(body["data"], json!(true));

    // 删除；再删 → 4004
    let body = call(
        &app,
        "DELETE",
        "/api/protocol-tools/convert/rules/cv_it1",
        None,
    )
    .await;
    assert_eq!(body["code"], 0, "删除转换规则失败: {body}");
    assert!(current_config(&state).convert_rules.unwrap().is_empty());
    let body = call(
        &app,
        "DELETE",
        "/api/protocol-tools/convert/rules/cv_it1",
        None,
    )
    .await;
    assert_eq!(body["code"], 4004);
    assert!(body["message"].as_str().unwrap().contains("rule not found"));
}

#[tokio::test]
async fn packet_editor_send_reports_bus_and_frame_errors() {
    let (_state, app) = app_with_bus().await;

    // 缺 bus → 4001，明确要求指定总线
    let body = call(
        &app,
        "POST",
        "/api/protocol-tools/packet-editor/send",
        Some(json!({ "hex": "01030000000101" })),
    )
    .await;
    assert_eq!(body["code"], 4001);
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("缺少必填参数 bus"));

    // 非法 hex → 4000（参数校验先于总线准入）
    let body = call(
        &app,
        "POST",
        "/api/protocol-tools/packet-editor/send",
        Some(json!({ "hex": "zz", "bus": "bus0" })),
    )
    .await;
    assert_eq!(body["code"], 4000);

    // 总线不存在 → 4004
    let body = call(
        &app,
        "POST",
        "/api/protocol-tools/packet-editor/send",
        Some(json!({ "hex": "010300000001", "bus": "ghost-bus" })),
    )
    .await;
    assert_eq!(body["code"], 4004);

    // 配置内总线但端口不可用（mock 无串口/网口）→ 4503，HTTP 200 且带现场指引
    let body = call(
        &app,
        "POST",
        "/api/protocol-tools/packet-editor/send",
        Some(json!({ "hex": "010300000001", "bus": "bus0", "timeout_ms": 200 })),
    )
    .await;
    assert_eq!(body["code"], 4503, "body={body}");
    assert!(!body["message"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn packet_capture_clear_is_idempotent() {
    let (_state, app) = app_with_bus().await;
    for _ in 0..2 {
        let body = call(
            &app,
            "POST",
            "/api/protocol-tools/packet-capture/clear",
            None,
        )
        .await;
        assert_eq!(body["code"], 0, "清空抓包失败: {body}");
    }
}

#[tokio::test]
async fn replay_session_save_list_delete() {
    let (_state, app) = app_with_bus().await;

    let session = json!({
        "id": "sess_it1",
        "name": "现场回放",
        "device_name": "1号表",
        "duration_ms": 120,
        "packet_count": 1,
        "created_at_ms": 1700000000000i64,
        "packets": [{
            "id": "pk1",
            "time_ms": 0,
            "direction": "tx",
            "hex": "010300000001",
            "parsed": "读寄存器"
        }]
    });
    let body = call(
        &app,
        "POST",
        "/api/protocol-tools/packet-replay/sessions",
        Some(session),
    )
    .await;
    assert_eq!(body["code"], 0, "保存回放会话失败: {body}");
    assert_eq!(body["data"]["id"], "sess_it1");

    let body = call(
        &app,
        "GET",
        "/api/protocol-tools/packet-replay/sessions",
        None,
    )
    .await;
    assert_eq!(body["code"], 0);
    assert!(body["data"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["id"] == "sess_it1"));

    let body = call(
        &app,
        "DELETE",
        "/api/protocol-tools/packet-replay/sessions/sess_it1",
        None,
    )
    .await;
    assert_eq!(body["code"], 0, "删除回放会话失败: {body}");
    let body = call(
        &app,
        "GET",
        "/api/protocol-tools/packet-replay/sessions",
        None,
    )
    .await;
    assert!(!body["data"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["id"] == "sess_it1"));
}

#[tokio::test]
async fn register_scan_delete_and_suggestions_guard() {
    let (_state, app) = app_with_bus().await;

    // 总线端口不可用 → 扫描拒绝启动（4503），不得登记一个永远「运行中」的空任务
    let body = call(
        &app,
        "POST",
        "/api/protocol-tools/register-scan",
        Some(json!({
            "bus": "bus0",
            "slave_id": 1,
            "reg_type": "holding",
            "start_addr": 40001,
            "end_addr": 40003,
            "timeout_ms": 50,
            "interval_ms": 20
        })),
    )
    .await;
    assert_eq!(body["code"], 4503, "body={body}");
    assert!(body["message"].as_str().unwrap().contains("不可用"));

    // 未登记的扫描 → 生成建议必须 404，而不是返回一份编造的空建议
    let body = call(
        &app,
        "POST",
        "/api/protocol-tools/register-scan/scan_ghost/suggestions",
        None,
    )
    .await;
    assert_eq!(body["code"], 4004);
    assert!(body["message"].as_str().unwrap().contains("scan not found"));

    // 删除未知 id 是幂等的（DELETE 语义：目标不存在也算达成）
    let body = call(
        &app,
        "DELETE",
        "/api/protocol-tools/register-scan/scan_ghost",
        None,
    )
    .await;
    assert_eq!(body["code"], 0, "body={body}");
}

#[tokio::test]
async fn auto_detect_stop_is_idempotent() {
    let (_state, app) = app_with_bus().await;
    for _ in 0..2 {
        let body = call(
            &app,
            "POST",
            "/api/protocol-tools/auto-detect/run/detect_it1/stop",
            None,
        )
        .await;
        assert_eq!(body["code"], 0, "停止探测失败: {body}");
    }
}

#[tokio::test]
async fn simulator_start_update_register_stop() {
    let (_state, app) = app_with_bus().await;

    // port=0 让系统分配端口，避免与现场 1502 冲突
    let body = call(
        &app,
        "POST",
        "/api/protocol-tools/simulator/start",
        Some(json!({ "slave_id": 1, "port": 0 })),
    )
    .await;
    assert_eq!(body["code"], 0, "启动从站模拟器失败: {body}");

    // 重复启动 → 4000，提示先停止
    let body = call(
        &app,
        "POST",
        "/api/protocol-tools/simulator/start",
        Some(json!({ "slave_id": 1, "port": 0 })),
    )
    .await;
    assert_eq!(body["code"], 4000);
    assert!(body["message"].as_str().unwrap().contains("已在运行"));

    // 更新已存在的寄存器
    let body = call(
        &app,
        "PUT",
        "/api/protocol-tools/simulator/registers/0",
        Some(json!({ "value": 42 })),
    )
    .await;
    assert_eq!(body["code"], 0, "更新模拟寄存器失败: {body}");

    // 不存在的寄存器 → 4004
    let body = call(
        &app,
        "PUT",
        "/api/protocol-tools/simulator/registers/9999",
        Some(json!({ "value": 1 })),
    )
    .await;
    assert_eq!(body["code"], 4004);

    let body = call(&app, "POST", "/api/protocol-tools/simulator/stop", None).await;
    assert_eq!(body["code"], 0, "停止从站模拟器失败: {body}");
}
