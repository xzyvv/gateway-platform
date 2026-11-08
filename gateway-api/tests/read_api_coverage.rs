//! 读接口覆盖补齐（API 测试覆盖率 38 个 GET 端点）
//!
//! 覆盖 `docs/api-surface.md` 中此前无测试的只读端点，按域分组：
//! - devices：health / shadows / control / topology / point-quality / backoff / :id/points
//! - diagnostics：bus-load / modbus-errors / stats / schedule-timeline / logs
//! - points：offset / clone/devices / :id/read
//! - protocol-tools：presets / convert rules / simulator / register-scan / auto-detect / diag
//! - system：config versions compare / cellular / north / ota / passthrough / disk history
//!   / watchdog events / uplink / health.live / health.ready
//!
//! 约定：只读接口大多返回 `{code,message,data}` 信封（`/health/*` 为裸 JSON）；
//! 需要配置数据的场景统一走「写配置到磁盘 → POST /api/config/reload」夹具。
//! 全程不发外部网络、不碰真实串口，`/api/points/:id/read` 用 mock collector 内部短路。

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use gateway_api::web::AppState;
use gateway_core::config::GatewayConfig;
use gateway_core::south::protocol::Collector;
use serde_json::{json, Value};
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789abcdef";

// ===== HTTP 辅助 =====

fn req(method: &str, uri: &str, body: Option<Value>) -> Request<Body> {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {}", TOKEN));
    finish(builder, body)
}

/// 不带 Authorization 的请求（验证鉴权中间件）。
fn req_anon(method: &str, uri: &str) -> Request<Body> {
    finish(Request::builder().method(method).uri(uri), None)
}

fn finish(builder: axum::http::request::Builder, body: Option<Value>) -> Request<Body> {
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

async fn call_status(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let resp = app.clone().oneshot(req(method, uri, body)).await.unwrap();
    let status = resp.status().as_u16();
    (status, json_body(resp).await)
}

async fn call_anon(app: &axum::Router, method: &str, uri: &str) -> (u16, Value) {
    let resp = app.clone().oneshot(req_anon(method, uri)).await.unwrap();
    let status = resp.status().as_u16();
    (status, json_body(resp).await)
}

// ===== 配置夹具 =====

fn current_config(state: &Arc<AppState>) -> GatewayConfig {
    state.config.load_full().as_ref().clone()
}

/// 总线 bus0 + 设备 dev1（模板 t1）+ 显式可写点位 p1（挂在 dev1 下）。
/// 点位 id 与模板展开的 `dev1_p1` 不冲突，因此同时覆盖「显式点位」与「模板展开」两条路径。
fn fixture_config(state: &Arc<AppState>) -> GatewayConfig {
    let mut v = serde_json::to_value(current_config(state)).unwrap();
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
    v["points"] = json!([{
        "id": "p1",
        "name": "点位1",
        "device_id": "dev1",
        "bus": "bus0",
        "protocol": "modbus",
        "address": "1",
        "slave_id": 1,
        "address_num": 10,
        "function_code": 3,
        "data_type": "u16",
        "interval": 10,
        "writable": true
    }]);
    serde_json::from_value(v).expect("夹具配置应可反序列化")
}

/// 落盘 + 走真实 reload 入口，让后续读接口面对与现场一致的「已加载配置」。
async fn reload_fixture(state: &Arc<AppState>, app: &axum::Router) {
    let cfg = fixture_config(state);
    state.config.config_loader.save(&cfg).expect("写入夹具配置");
    let body = call(
        app,
        "POST",
        "/api/config/reload",
        Some(json!({ "config": {} })),
    )
    .await;
    assert_eq!(body["code"], 0, "夹具配置 reload 失败: {body}");
}

async fn state_with_device() -> (Arc<AppState>, axum::Router) {
    let state = common::mock_state::mock_app_state();
    let app = gateway_api::web::build_router(state.clone());
    reload_fixture(&state, &app).await;
    (state, app)
}

/// 额外注入一个假 collector（按总线名索引），让 `/api/points/:id/read` 与协议诊断
/// 能在离线环境走完 handler，而不是 404「collector 未找到」。
async fn state_with_collector() -> (Arc<AppState>, axum::Router) {
    let mut collectors: HashMap<String, Arc<dyn Collector>> = HashMap::new();
    collectors.insert(
        "bus0".to_string(),
        Arc::new(common::mock_state::MockCollector::new("bus0")),
    );
    let state = common::mock_state::mock_app_state_with_collectors(collectors);
    let app = gateway_api::web::build_router(state.clone());
    reload_fixture(&state, &app).await;
    (state, app)
}

// ============================================================
// devices
// ============================================================

#[tokio::test]
async fn devices_health_and_shadows() {
    let (_state, app) = state_with_device().await;

    let body = call(&app, "GET", "/api/devices/health", None).await;
    assert_eq!(body["code"], 0, "{body}");
    let list = body["data"].as_array().expect("health data 应为数组");
    assert_eq!(list.len(), 1, "夹具只有 dev1: {body}");
    assert_eq!(list[0]["device_id"], "dev1");
    // 窗口内无采集：分数为 null、等级 unknown（不编造 100 分）
    assert!(list[0]["score"].is_null(), "{body}");
    assert_eq!(list[0]["level"], "unknown");

    let body = call(&app, "GET", "/api/devices/shadows", None).await;
    assert_eq!(body["code"], 0, "{body}");
    let list = body["data"].as_array().expect("shadows data 应为数组");
    assert_eq!(list.len(), 1, "{body}");
    assert_eq!(list[0]["device_id"], "dev1");
    assert_eq!(list[0]["protocol"], "modbus");
    // 无运行时快照 → uncertain + 0（前端显示「—」）
    assert_eq!(list[0]["last_quality"], "uncertain");
    assert_eq!(list[0]["last_update_at"], 0);
}

#[tokio::test]
async fn devices_control_lists() {
    let (_state, app) = state_with_device().await;

    // 写入白名单只返回显式 writable=true 的点位
    let body = call(&app, "GET", "/api/devices/control/writable", None).await;
    assert_eq!(body["code"], 0, "{body}");
    let list = body["data"].as_array().expect("writable data 应为数组");
    assert_eq!(list.len(), 1, "{body}");
    assert_eq!(list[0]["id"], "p1");
    assert_eq!(list[0]["device_id"], "dev1");

    // 命令历史来自审计库（可能有既存记录，故只校验形状与状态取值）
    let body = call(&app, "GET", "/api/devices/control/commands", None).await;
    assert_eq!(body["code"], 0, "{body}");
    let commands = body["data"].as_array().expect("commands data 应为数组");
    for c in commands {
        assert!(c["point_id"].is_string(), "缺 point_id: {body}");
        assert!(
            c["status"] == "success" || c["status"] == "failed",
            "状态取值非法: {body}"
        );
    }
}

#[tokio::test]
async fn devices_topology_point_quality_and_backoff() {
    let (_state, app) = state_with_device().await;

    let body = call(&app, "GET", "/api/devices/topology", None).await;
    assert_eq!(body["code"], 0, "{body}");
    assert_eq!(body["data"]["stats"]["bus_count"], 1, "{body}");
    assert_eq!(body["data"]["stats"]["device_count"], 1, "{body}");
    assert!(!body["data"]["nodes"].as_array().expect("nodes").is_empty());
    assert!(!body["data"]["links"].as_array().expect("links").is_empty());

    // 无运行时状态：质量列表为空，统计全 0（空态如实表达）
    let body = call(&app, "GET", "/api/devices/point-quality", None).await;
    assert_eq!(body["code"], 0, "{body}");
    assert!(body["data"].as_array().expect("point-quality").is_empty());

    let body = call(&app, "GET", "/api/devices/point-quality/stats", None).await;
    assert_eq!(body["code"], 0, "{body}");
    assert_eq!(body["data"]["total"], 0, "{body}");
    assert_eq!(body["data"]["good"], 0);
    assert_eq!(body["data"]["bad"], 0);

    let body = call(&app, "GET", "/api/devices/backoff", None).await;
    assert_eq!(body["code"], 0, "{body}");
    assert!(body["data"].as_array().expect("backoff").is_empty());
}

#[tokio::test]
async fn device_points_by_device() {
    let (_state, app) = state_with_device().await;

    let uri = format!("/api/devices/{}/points", "dev1");
    let body = call(&app, "GET", &uri, None).await;
    assert_eq!(body["code"], 0, "{body}");
    let points = body["data"].as_array().expect("points 应为数组");
    assert!(!points.is_empty(), "模板 t1 应展开出点位: {body}");
    assert!(points.iter().all(|p| p["device_id"] == "dev1"), "{body}");

    // BUG-E：设备不存在必须 404，而不是 200 + 空数组
    let uri = format!("/api/devices/{}/points", "nope");
    let (status, body) = call_status(&app, "GET", &uri, None).await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body["code"], 4004, "{body}");
}

#[tokio::test]
async fn buses_paused_list() {
    let (_state, app) = state_with_device().await;

    let body = call(&app, "GET", "/api/buses/paused", None).await;
    assert_eq!(body["code"], 0, "{body}");
    assert!(body["data"]["paused_buses"]
        .as_array()
        .expect("paused_buses 应为数组")
        .is_empty());
}

// ============================================================
// diagnostics
// ============================================================

#[tokio::test]
async fn diagnostics_bus_load_list_and_detail() {
    let (_state, app) = state_with_device().await;

    let body = call(&app, "GET", "/api/diagnostics/bus-load", None).await;
    assert_eq!(body["code"], 0, "{body}");
    let list = body["data"].as_array().expect("bus-load 应为数组");
    assert_eq!(list.len(), 1, "{body}");
    assert_eq!(list[0]["bus"], "bus0");
    assert_eq!(list[0]["bus_type"], "modbus");

    let uri = format!("/api/diagnostics/bus-load/{}", "bus0");
    let body = call(&app, "GET", &uri, None).await;
    assert_eq!(body["code"], 0, "{body}");
    assert_eq!(body["data"]["bus"], "bus0");

    let uri = format!("/api/diagnostics/bus-load/{}", "nope");
    let (status, body) = call_status(&app, "GET", &uri, None).await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body["code"], 4004, "{body}");
}

#[tokio::test]
async fn diagnostics_modbus_error_catalog() {
    let (_state, app) = state_with_device().await;

    let body = call(&app, "GET", "/api/diagnostics/modbus-errors", None).await;
    assert_eq!(body["code"], 0, "{body}");
    let list = body["data"].as_array().expect("modbus-errors 应为数组");
    assert!(!list.is_empty(), "错误码字典不应为空: {body}");
    assert!(list[0]["code"].is_string());
    assert!(list[0]["meaning"].is_string());

    let body = call(&app, "GET", "/api/diagnostics/modbus-errors/issues", None).await;
    assert_eq!(body["code"], 0, "{body}");
    let list = body["data"].as_array().expect("issues 应为数组");
    assert!(!list.is_empty(), "常见问题清单不应为空: {body}");
    assert!(list[0]["symptom"].is_string());
    assert!(list[0]["checks"].is_array());
}

#[tokio::test]
async fn diagnostics_stats_report() {
    let (_state, app) = state_with_device().await;

    let body = call(&app, "GET", "/api/diagnostics/stats", None).await;
    assert_eq!(body["code"], 0, "{body}");
    assert_eq!(body["data"]["period"], "day");
    assert!(body["data"]["start_at_ms"].is_i64());
    assert!(body["data"]["end_at_ms"].is_i64());
    assert!(body["data"]["summary"].is_object());
    assert!(body["data"]["bus_stats"].is_array());
}

#[tokio::test]
async fn diagnostics_schedule_timeline() {
    let (_state, app) = state_with_device().await;

    let body = call(&app, "GET", "/api/diagnostics/schedule-timeline", None).await;
    assert_eq!(body["code"], 0, "{body}");
    // 缺省窗口 10s（handler 钳制 1s~60s）
    assert_eq!(body["data"]["window_ms"], 10_000, "{body}");
    // mock scheduler 未运行 → running=false + 空分组，不伪造甘特图
    assert_eq!(body["data"]["running"], false);
    assert!(body["data"]["groups"]
        .as_array()
        .expect("groups")
        .is_empty());
}

#[tokio::test]
async fn diagnostics_log_archives() {
    let (_state, app) = state_with_device().await;

    let body = call(&app, "GET", "/api/diagnostics/logs/archives", None).await;
    assert_eq!(body["code"], 0, "{body}");
    let archives = body["data"]["archives"]
        .as_array()
        .expect("archives 应为数组");
    assert_eq!(body["data"]["count"], archives.len());
}

#[tokio::test]
async fn diagnostics_logs_ws_rejects_plain_get() {
    let (_state, app) = state_with_device().await;

    // `/api/diagnostics/logs` 是 WebSocket tail：普通 GET 不带 Upgrade 头时
    // axum 的 WebSocketUpgrade 提取器直接拒绝（400），不会进入日志流逻辑。
    let (status, _body) = call_status(&app, "GET", "/api/diagnostics/logs", None).await;
    assert_eq!(status, 400, "非升级请求应被拒绝");
}

// ============================================================
// points
// ============================================================

#[tokio::test]
async fn points_offset_and_clone_devices() {
    let (_state, app) = state_with_device().await;

    let body = call(&app, "GET", "/api/point-tools/offset", None).await;
    assert_eq!(body["code"], 0, "{body}");
    let list = body["data"].as_array().expect("offset 应为数组");
    assert_eq!(list.len(), 1, "{body}");
    assert_eq!(list[0]["id"], "p1");
    assert_eq!(list[0]["old_addr"], 10);
    assert_eq!(list[0]["new_addr"], 10);

    let body = call(&app, "GET", "/api/point-tools/clone/devices", None).await;
    assert_eq!(body["code"], 0, "{body}");
    let list = body["data"].as_array().expect("clone devices 应为数组");
    assert_eq!(list.len(), 1, "{body}");
    assert_eq!(list[0]["id"], "dev1");
    assert_eq!(list[0]["point_count"], 1, "显式点位 p1 应计入 dev1: {body}");
}

#[tokio::test]
async fn point_read_now_uses_injected_collector() {
    let (_state, app) = state_with_collector().await;

    // 假 collector 直返 Good 数据点：不发起任何现场采集，验证 handler 的成功响应形状
    let uri = format!("/api/points/{}/read", "p1");
    let body = call(&app, "GET", &uri, None).await;
    assert_eq!(body["code"], 0, "{body}");
    assert_eq!(body["data"]["status"], "ok", "{body}");
    assert_eq!(body["data"]["point_id"], "p1");
    assert!(body["data"]["value"].is_number());
}

// ============================================================
// protocol-tools
// ============================================================

#[tokio::test]
async fn protocol_tools_presets_and_convert_rules() {
    let (_state, app) = state_with_device().await;

    let body = call(
        &app,
        "GET",
        "/api/protocol-tools/packet-editor/presets",
        None,
    )
    .await;
    assert_eq!(body["code"], 0, "{body}");
    let list = body["data"].as_array().expect("presets 应为数组");
    assert!(!list.is_empty(), "内置帧预设不应为空: {body}");
    assert!(list[0]["name"].is_string());
    assert!(list[0]["hex"].is_string());

    // 未配置转换规则 → 空列表
    let body = call(&app, "GET", "/api/protocol-tools/convert/rules", None).await;
    assert_eq!(body["code"], 0, "{body}");
    assert!(body["data"].as_array().expect("rules").is_empty());
}

#[tokio::test]
async fn protocol_tools_simulator_registers() {
    let (_state, app) = state_with_device().await;

    let body = call(&app, "GET", "/api/protocol-tools/simulator/registers", None).await;
    assert_eq!(body["code"], 0, "{body}");
    let list = body["data"].as_array().expect("sim registers 应为数组");
    assert!(!list.is_empty(), "从站模拟寄存器表不应为空: {body}");
    assert!(list[0]["addr"].is_number());
}

#[tokio::test]
async fn protocol_tools_scan_results_and_auto_detect() {
    let (_state, app) = state_with_device().await;

    // 未发起扫描 → 空结果列表。真实扫描成功路径需要串口/总线硬件，
    // 这里只覆盖离线可得的「查询态」语义（不得伪造扫描结果）。
    let body = call(&app, "GET", "/api/protocol-tools/register-scan", None).await;
    assert_eq!(body["code"], 0, "{body}");
    assert!(body["data"].as_array().expect("register-scan").is_empty());

    // 未启动过的探测任务 → data=null（真实探测需真实发帧，离线不做）
    let uri = format!("/api/protocol-tools/auto-detect/run/{}", "no-such-run");
    let body = call(&app, "GET", &uri, None).await;
    assert_eq!(body["code"], 0, "{body}");
    assert!(body["data"].is_null(), "未知探测任务应返回 null: {body}");
}

#[tokio::test]
async fn protocol_diag_requires_running_collector() {
    // 无任何运行实例 → 404（现场「设备还没配好就点诊断」的如实提示）
    let (_state, app) = state_with_device().await;
    let uri = format!("/api/protocols/{}/diag", "s7comm");
    let (status, body) = call_status(&app, "GET", &uri, None).await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body["code"], 4004, "{body}");

    // 注入 mock collector 后走通成功路径：协议未实现 diag → executed=false + 原因，
    // 且总线未暂停时不执行真实诊断（不抢采集链路）
    let (_state, app) = state_with_collector().await;
    let uri = format!("/api/protocols/{}/diag", "mock");
    let body = call(&app, "GET", &uri, None).await;
    assert_eq!(body["code"], 0, "{body}");
    assert_eq!(body["data"]["protocol"], "mock", "{body}");
    assert_eq!(body["data"]["supported"], false);
    let entries = body["data"]["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 1, "{body}");
    assert_eq!(entries[0]["executed"], false);
    assert_eq!(entries[0]["bus"], "bus0");
}

// ============================================================
// system
// ============================================================

#[tokio::test]
async fn config_version_compare() {
    let (_state, app) = state_with_device().await;

    // id == compare_to：无需版本记录即可返回「无变化」比较结果
    let uri = format!("/api/config/versions/{}/compare?compare_to={}", 7, 7);
    let body = call(&app, "GET", &uri, None).await;
    assert_eq!(body["code"], 0, "{body}");
    assert_eq!(body["data"]["has_changes"], false, "{body}");
    assert_eq!(body["data"]["from"], 7);
    assert_eq!(body["data"]["to"], 7);
    assert!(body["data"]["changed_fields"]
        .as_array()
        .expect("changed_fields")
        .is_empty());

    // 版本不存在 → 404（临时库里没有该版本）
    let uri = format!("/api/config/versions/{}/compare?compare_to={}", 99, 100);
    let (status, body) = call_status(&app, "GET", &uri, None).await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body["code"], 4004, "{body}");
}

#[tokio::test]
async fn system_status_snapshots() {
    let (_state, app) = state_with_device().await;

    // 蜂窝模块未注入 → enabled=false 占位，但平台能力如实下发
    let body = call(&app, "GET", "/api/cellular/status", None).await;
    assert_eq!(body["code"], 0, "{body}");
    assert_eq!(body["data"]["enabled"], false);
    assert!(body["data"]["status"].is_null());

    // OTA mock 恒为 Idle
    let body = call(&app, "GET", "/api/ota/status", None).await;
    assert_eq!(body["code"], 0, "{body}");
    assert_eq!(body["data"]["status"], "idle");
    assert_eq!(body["data"]["progress"], 0);
    assert!(body["data"]["error"].is_null());

    // 透传未启用 → enabled=false + 空会话
    let body = call(&app, "GET", "/api/passthrough/sessions", None).await;
    assert_eq!(body["code"], 0, "{body}");
    assert_eq!(body["data"]["enabled"], false);
    assert!(body["data"]["sessions"]
        .as_array()
        .expect("sessions")
        .is_empty());

    // 北向状态：固定下发 mqtt + http 两条通道口径
    let body = call(&app, "GET", "/api/north/status", None).await;
    assert_eq!(body["code"], 0, "{body}");
    let channels = body["data"]["channels"].as_array().expect("channels");
    assert_eq!(channels.len(), 2, "{body}");
    assert_eq!(channels[0]["type"], "mqtt");
    assert_eq!(channels[1]["type"], "http");
    assert!(body["data"]["aggregated"].is_object());
}

#[tokio::test]
async fn health_disk_history_and_watchdog_events() {
    let (_state, app) = state_with_device().await;

    let body = call(&app, "GET", "/api/health/disk/history", None).await;
    assert_eq!(body["code"], 0, "{body}");
    let samples = body["data"]["samples"].as_array().expect("samples");
    assert_eq!(body["data"]["count"], samples.len());

    // 无自愈动作 → 空事件流
    let body = call(&app, "GET", "/api/watchdog/events", None).await;
    assert_eq!(body["code"], 0, "{body}");
    assert!(body["data"].as_array().expect("watchdog events").is_empty());
}

#[tokio::test]
async fn uplink_read_endpoints() {
    let (_state, app) = state_with_device().await;

    let body = call(&app, "GET", "/api/uplink/vars", None).await;
    assert_eq!(body["code"], 0, "{body}");
    let vars = body["data"].as_array().expect("vars 应为数组");
    assert!(!vars.is_empty(), "模板变量清单不应为空: {body}");
    assert_eq!(vars[0]["name"], "timestamp_ms");

    // 未配置 payload_template → enabled=false，但导出结构固定
    let body = call(&app, "GET", "/api/uplink/template/export", None).await;
    assert_eq!(body["code"], 0, "{body}");
    assert_eq!(body["data"]["enabled"], false, "{body}");
    assert_eq!(body["data"]["encoding"], "json_lines");
    assert_eq!(body["data"]["max_bytes"], 4096);
    assert!(body["data"]["exported_at"].is_string());

    // 未做过上行测试 → 空历史
    let body = call(&app, "GET", "/api/uplink/test/history", None).await;
    assert_eq!(body["code"], 0, "{body}");
    assert!(body["data"].as_array().expect("test history").is_empty());
}

#[tokio::test]
async fn health_probes_are_public_and_raw() {
    let (_state, app) = state_with_device().await;

    // 探针不套信封、免鉴权：带 token 与不带 token 都应可读
    let body = call(&app, "GET", "/health/live", None).await;
    assert_eq!(body["status"], "ok", "{body}");
    assert_eq!(body["service"], "gateway-platform");
    assert_eq!(body["discoverable"], true);
    assert!(body.get("code").is_none(), "live 不应被包信封: {body}");

    let body = call(&app, "GET", "/health/ready", None).await;
    assert_eq!(body["status"], "ok", "{body}");
    assert!(body["components"].is_object(), "{body}");
    assert!(body["components"]["process"] == "ok");
    assert!(body["queue_len"].is_number());
    assert!(body["ota_state"].is_string());

    let (status, body) = call_anon(&app, "GET", "/health/live").await;
    assert_eq!(status, 200, "健康探针必须免鉴权: {body}");
}

#[tokio::test]
async fn anon_read_disabled_requires_token() {
    // mock 默认 allow_anon_read=true；这里改为 false，验证读接口鉴权确实生效
    let state = common::mock_state::mock_app_state();
    let mut cfg = current_config(&state);
    cfg.web.as_mut().expect("mock 配置含 web").allow_anon_read = false;
    state.config.config_loader.save(&cfg).expect("写入鉴权夹具");
    let app = gateway_api::web::build_router(state.clone());

    // reload 自身始终要求 token
    let body = call(
        &app,
        "POST",
        "/api/config/reload",
        Some(json!({ "config": {} })),
    )
    .await;
    assert_eq!(body["code"], 0, "夹具配置 reload 失败: {body}");

    let (status, body) = call_anon(&app, "GET", "/api/devices/health").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED.as_u16(), "{body}");
    assert_eq!(body["code"], 8000, "{body}");

    // 带 token 同样可读
    let body = call(&app, "GET", "/api/devices/health", None).await;
    assert_eq!(body["code"], 0, "{body}");

    // 探针不受 allow_anon_read 影响
    let (status, body) = call_anon(&app, "GET", "/health/ready").await;
    assert_eq!(status, 200, "{body}");
}
