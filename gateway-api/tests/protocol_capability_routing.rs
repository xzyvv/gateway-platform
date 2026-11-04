//! C1.1 能力位路由契约（B3 的端到端钉子）
//!
//! slave-scan / register-scan / auto-detect 三个调试工具自 B3 起不再按协议名硬编码，
//! 而是查注册表能力位路由。本文件验证两条边界：
//! - 协议声明支持 → 请求被受理并返回**可执行结果**（200 + 结果体），不是 500；
//! - 协议未声明支持（已知协议的能力位 false，或注册表里查不到的协议名）
//!   → 明确 400 `unsupported_protocol` + 现场可读的中文提示。
//!
//! 物理层用注入的超时假总线替代：能力位判断发生在真正下发之前，
//! 因此「超时」不会混淆「协议不支持」与「设备没回」两类结论。

mod common;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use common::mock_state::mock_app_state_with_debug_bus;
use gateway_api::web::debug_bus::{BusErrorKind, DebugBusTrait, ExchangeOutcome};
use gateway_api::web::AppState;
use gateway_core::config::{BusConfig, DeviceConfig};
use serde_json::{json, Value};
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789abcdef";

/// 恒超时的假总线：只用来证明请求已越过能力位门禁并进入真实执行路径。
struct AlwaysTimeout;

impl DebugBusTrait for AlwaysTimeout {
    fn exchange<'a>(
        &'a self,
        _request: &'a [u8],
        _timeout_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<ExchangeOutcome, BusErrorKind>> + Send + 'a>> {
        Box::pin(async { Err(BusErrorKind::Timeout) })
    }
}

fn bus_cfg(port: &str, protocol: &str) -> BusConfig {
    serde_json::from_value(json!({ "port_name": port, "protocol": protocol }))
        .expect("BusConfig 最少字段应可反序列化")
}

fn device(id: &str, bus: &str, protocol: &str, slave_id: u8) -> DeviceConfig {
    serde_json::from_value(json!({
        "id": id,
        "name": id,
        "template": "t1",
        "bus": bus,
        "protocol": protocol,
        "address": slave_id.to_string(),
        "slave_id": slave_id,
    }))
    .expect("DeviceConfig 最少字段应可反序列化")
}

/// 构造注入假总线 + 指定设备表的 AppState。
///
/// `mock_app_state_with_debug_bus` 只接收总线表，auto-detect 还需按 device_id
/// 解析 bus/slave，故在独占所有权阶段把 devices 写回配置快照。
fn state_with(buses: Vec<BusConfig>, devices: Vec<DeviceConfig>) -> Arc<AppState> {
    let mut state = mock_app_state_with_debug_bus(buses, Arc::new(AlwaysTimeout));
    let mut cfg = state.config.load_full().as_ref().clone();
    cfg.devices = Some(devices);
    Arc::get_mut(&mut state)
        .expect("刚构造的 AppState 应独占")
        .config
        .config
        .store(Arc::new(cfg));
    state
}

fn request(method: &str, uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {}", TOKEN))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("构造请求")
}

async fn call(state: Arc<AppState>, req: Request<Body>) -> (StatusCode, Value) {
    let router = gateway_api::web::build_router(state);
    let resp = router.oneshot(req).await.expect("router 应可调用");
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .expect("读取响应体");
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

fn assert_unsupported(status: StatusCode, body: &Value, proto: &str) {
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "未声明能力必须 400，body={body}"
    );
    assert_ne!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        body["code"], 4002,
        "业务码应为 unsupported_protocol，body={body}"
    );
    assert_eq!(body["error"], "unsupported_protocol", "body={body}");
    let msg = body["message"].as_str().unwrap_or_default();
    assert!(msg.contains(proto), "提示应点明协议 {proto}，实际: {msg}");
    assert!(
        msg.contains("未声明"),
        "提示应说明「未声明支持」而非笼统失败，实际: {msg}"
    );
}

// ===== slave-scan =====

#[tokio::test]
async fn slave_scan_声明支持时返回可执行结果() {
    let state = state_with(vec![bus_cfg("cap_modbus_slave", "modbus")], Vec::new());

    let (status, body) = call(
        state,
        request(
            "POST",
            "/api/protocol-tools/slave-scan",
            json!({ "bus": "cap_modbus_slave", "range_start": 1, "range_end": 1,
                    "timeout_ms": 200, "interval_ms": 20 }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body={body}");
    assert_eq!(body["code"], 0, "body={body}");
    let results = body["data"].as_array().expect("应返回探测结果数组");
    assert_eq!(results.len(), 1, "单地址扫描应返回 1 条结果: {body}");
    assert_eq!(results[0]["slave_id"], 1);
    assert_eq!(
        results[0]["responded"], false,
        "假总线恒超时，应如实标记不在线"
    );
}

#[tokio::test]
async fn slave_scan_未声明支持时明确拒绝且非500() {
    let state = state_with(vec![bus_cfg("cap_iec104_slave", "iec104")], Vec::new());

    let (status, body) = call(
        state,
        request(
            "POST",
            "/api/protocol-tools/slave-scan",
            json!({ "bus": "cap_iec104_slave", "range_start": 1, "range_end": 2 }),
        ),
    )
    .await;

    assert_unsupported(status, &body, "iec104");
}

#[tokio::test]
async fn slave_scan_注册表外协议明确拒绝且非500() {
    let state = state_with(vec![bus_cfg("cap_unknown_slave", "proto_xyz")], Vec::new());

    let (status, body) = call(
        state,
        request(
            "POST",
            "/api/protocol-tools/slave-scan",
            json!({ "bus": "cap_unknown_slave", "range_start": 1, "range_end": 1 }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "body={body}");
    assert_eq!(body["code"], 4002, "body={body}");
    assert_eq!(body["error"], "unsupported_protocol", "body={body}");
    let msg = body["message"].as_str().unwrap_or_default();
    assert!(msg.contains("未在注册表中登记"), "实际: {msg}");
}

// ===== register-scan =====

#[tokio::test]
async fn register_scan_声明支持时返回可执行结果() {
    let state = state_with(vec![bus_cfg("cap_modbus_reg", "modbus")], Vec::new());

    let (status, body) = call(
        state,
        request(
            "POST",
            "/api/protocol-tools/register-scan",
            json!({ "bus": "cap_modbus_reg", "slave_id": 1, "reg_type": "holding",
                    "start_addr": 0, "end_addr": 0, "timeout_ms": 200, "interval_ms": 20 }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body={body}");
    assert_eq!(body["code"], 0, "body={body}");
    let id = body["data"].as_str().expect("应返回扫描任务 id");
    assert!(id.starts_with("scan_"), "id 形如 scan_*，实际: {id}");
}

/// mc：按软元件地址逐点探测（能力位 true → 200 + id；具体探测在后台任务内执行）
#[tokio::test]
async fn register_scan_mc_声明支持时返回可执行结果() {
    let state = state_with(vec![bus_cfg("cap_mc_reg", "mc")], Vec::new());

    let (status, body) = call(
        state,
        request(
            "POST",
            "/api/protocol-tools/register-scan",
            json!({ "bus": "cap_mc_reg", "slave_id": 0, "reg_type": "holding",
                    "start_addr": 0, "end_addr": 0, "timeout_ms": 200, "interval_ms": 20 }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body={body}");
    assert_eq!(body["code"], 0, "body={body}");
    let id = body["data"].as_str().expect("应返回扫描任务 id");
    assert!(id.starts_with("scan_"), "id 形如 scan_*，实际: {id}");
}

/// fins：按节点号 + 软元件地址逐点探测
///
/// fins 处于「退役观察」：已移出默认发行集，注册表条目按
/// `#[cfg(feature = "fins")]` 门控。两种构建各钉一条：
/// - 显式 `--features fins`（存量项目/回归）：协议已登记 → 200 + 可执行结果；
/// - 默认发行集：协议未登记 → 400 `unsupported_protocol` +「未在注册表中登记」。
///
/// 不按「协议名是否存在」硬断言 200，是因为那等于假设 fins 在默认构建里
/// 仍然可用——而这正是「退役观察」要杜绝的口径漂移。
#[cfg(feature = "fins")]
#[tokio::test]
async fn register_scan_fins_声明支持时返回可执行结果() {
    let state = state_with(vec![bus_cfg("cap_fins_reg", "fins")], Vec::new());

    let (status, body) = call(
        state,
        request(
            "POST",
            "/api/protocol-tools/register-scan",
            json!({ "bus": "cap_fins_reg", "slave_id": 1, "reg_type": "holding",
                    "start_addr": 0, "end_addr": 0, "timeout_ms": 200, "interval_ms": 20 }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body={body}");
    assert_eq!(body["code"], 0, "body={body}");
    let id = body["data"].as_str().expect("应返回扫描任务 id");
    assert!(id.starts_with("scan_"), "id 形如 scan_*，实际: {id}");
}

/// fins 未随默认发行集交付时，调试面必须明确拒绝「协议未登记」，
/// 而不是把它当成「协议不支持寄存器扫描」或直接 500。
#[cfg(not(feature = "fins"))]
#[tokio::test]
async fn register_scan_fins_未随默认发行集交付时明确拒绝() {
    let state = state_with(vec![bus_cfg("cap_fins_reg", "fins")], Vec::new());

    let (status, body) = call(
        state,
        request(
            "POST",
            "/api/protocol-tools/register-scan",
            json!({ "bus": "cap_fins_reg", "slave_id": 1, "reg_type": "holding",
                    "start_addr": 0, "end_addr": 0, "timeout_ms": 200, "interval_ms": 20 }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "body={body}");
    assert_eq!(body["code"], 4002, "body={body}");
    assert_eq!(body["error"], "unsupported_protocol", "body={body}");
    let msg = body["message"].as_str().unwrap_or_default();
    assert!(msg.contains("fins"), "提示应点明协议 fins，实际: {msg}");
    assert!(
        msg.contains("未在注册表中登记"),
        "fins 未随默认发行集交付，提示应说明「未在注册表中登记」，实际: {msg}"
    );
}

#[tokio::test]
async fn register_scan_未声明支持时明确拒绝且非500() {
    let state = state_with(vec![bus_cfg("cap_iec104_reg", "iec104")], Vec::new());

    let (status, body) = call(
        state,
        request(
            "POST",
            "/api/protocol-tools/register-scan",
            json!({ "bus": "cap_iec104_reg", "slave_id": 1, "reg_type": "holding",
                    "start_addr": 0, "end_addr": 1 }),
        ),
    )
    .await;

    assert_unsupported(status, &body, "iec104");
}

// ===== auto-detect =====

#[tokio::test]
async fn auto_detect_声明支持时返回可执行结果() {
    let state = state_with(
        vec![bus_cfg("cap_modbus_auto", "modbus")],
        vec![device("cap_dev_modbus", "cap_modbus_auto", "modbus", 1)],
    );

    let (status, body) = call(
        state,
        request(
            "POST",
            "/api/protocol-tools/auto-detect/run",
            json!({ "device_id": "cap_dev_modbus" }),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body={body}");
    assert_eq!(body["code"], 0, "body={body}");
    let id = body["data"].as_str().expect("应返回探测任务 id");
    assert!(id.starts_with("detect_"), "id 形如 detect_*，实际: {id}");
}

#[tokio::test]
async fn auto_detect_未声明支持时明确拒绝且非500() {
    let state = state_with(
        vec![bus_cfg("cap_iec104_auto", "iec104")],
        vec![device("cap_dev_iec104", "cap_iec104_auto", "iec104", 1)],
    );

    let (status, body) = call(
        state,
        request(
            "POST",
            "/api/protocol-tools/auto-detect/run",
            json!({ "device_id": "cap_dev_iec104" }),
        ),
    )
    .await;

    assert_unsupported(status, &body, "iec104");
}
