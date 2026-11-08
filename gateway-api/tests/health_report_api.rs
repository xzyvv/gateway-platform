//! 健康聚合报告 / 日志快照下载：路由与响应契约集成测试
//!
//! 这两个端点是"现场一屏看健康 + 故障后取日志"的入口，前端诊断包直接依赖它们，
//! 因此至少要保证：
//! - 路由真的挂在需要鉴权的 GET 组里（不是 404，也不是无鉴权）；
//! - `/api/health/report` 的字段形状稳定（前端按字段渲染，缺字段会整块空白）；
//!   该端点 P1-1 后走统一 ApiResponse 信封，业务字段在 `data` 下；
//! - `/api/diagnostics/logs/download` 回的是**纯文本**，不会被包成 JSON 信封——
//!   前端按 text 读取，多包一层就是一堆花括号。

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use common::mock_state::mock_app_state;
use gateway_api::web::AppState;
use gateway_core::config::{BusConfig, DeviceConfig, PointConfig};
use gateway_core::engine::metrics::CollectErrorCode;
use gateway_core::south::scheduler::DeviceStatus;
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789abcdef";

fn device(id: &str, bus: &str) -> DeviceConfig {
    DeviceConfig {
        id: id.to_string(),
        name: id.to_string(),
        template: "default".to_string(),
        bus: bus.to_string(),
        protocol: "modbus".to_string(),
        address: "1".to_string(),
        slave_id: Some(1),
        address_map: None,
        interval: None,
    }
}

fn point(id: &str, device_id: &str, bus: &str) -> PointConfig {
    PointConfig {
        id: id.to_string(),
        name: Some(id.to_uppercase()),
        device_id: Some(device_id.to_string()),
        bus: bus.to_string(),
        protocol: "modbus".to_string(),
        address: "1".to_string(),
        interval: 10,
        ..Default::default()
    }
}

fn state_with_config() -> Arc<AppState> {
    let state = mock_app_state();
    let mut cfg = state.config.load_full().as_ref().clone();
    cfg.devices = Some(vec![device("dev1", "COM_HEALTH")]);
    cfg.points = vec![point("dev1_t1", "dev1", "COM_HEALTH")];
    cfg.buses = vec![BusConfig {
        port_name: "COM_HEALTH".to_string(),
        ..BusConfig::default()
    }];
    state.config.config.store(Arc::new(cfg));
    state
}

async fn get_raw(state: Arc<AppState>, uri: &str) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let app = gateway_api::web::build_router(state);
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, headers, bytes.to_vec())
}

#[tokio::test]
async fn health_report_returns_aggregate_shape() {
    let (status, _headers, bytes) = get_raw(state_with_config(), "/api/health/report").await;
    assert_eq!(status, StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("health report is json");
    // P1-1：统一 ApiResponse 信封
    assert_eq!(body["code"], 0, "{body}");
    let body = &body["data"];

    // 没有 MQTT / 无积压 / 无设备运行态时不该报 critical
    let overall = body["status"].as_str().unwrap_or_default();
    assert!(
        overall == "ok" || overall == "degraded",
        "unexpected status: {overall}"
    );

    assert_eq!(body["components"]["process"], "ok");
    assert!(body["components"]["queue"].is_string());
    assert_eq!(body["summary"]["total_devices"], 1);
    assert_eq!(body["summary"]["bus_count"], 1);
    assert!(body["summary"]["queue_len"].is_number());
    assert!(body["generated_at_ms"].is_number());

    let buses = body["buses"].as_array().expect("buses is array");
    assert_eq!(buses.len(), 1);
    // 未配置 tcp_addr 时用 port_name 作为展示名
    assert_eq!(buses[0]["bus"], "COM_HEALTH");
    assert_eq!(buses[0]["protocol"], "modbus");
    // 没有采集运行态 → 在线数为 0，但不能因此被判成"总线全离线"（total_devices 也是 0）
    assert_eq!(buses[0]["online_devices"], 0);
    assert_eq!(buses[0]["total_devices"], 0);

    assert!(body["problems"].is_array());
    assert!(
        !body["problems"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["code"] == "BUS_ALL_OFFLINE"),
        "无设备的总线不应报全部离线"
    );
}

/// P9 回归：`/health/ready` 必须产出 `storage` 组件，且磁盘压力时为 degraded。
///
/// 背景：磁盘剩余空间低于阈值时网关会**阻断入队**、丢弃低优先级点——这是真实的
/// 生产影响。此前既没有对外信号（降采样标志只写不读），后端也从不产生 `storage`
/// 组件，现场只能从"点没上来"反推。
#[tokio::test]
async fn health_ready_exposes_storage_component_reflecting_disk_pressure() {
    let (state, gateway) = common::mock_state::mock_app_state_with_gateway();

    // 正常态：storage = ok
    let (status, _headers, bytes) = get_raw(state.clone(), "/health/ready").await;
    let raw = body_as_str(&bytes);
    assert_eq!(status, StatusCode::OK, "{raw}");
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("health ready is json");
    assert_eq!(body["components"]["storage"], "ok");

    // 注入磁盘压力（剩余 5%）→ storage = degraded
    gateway.disk_status().record(5.0, true);
    let (status, _headers, bytes) = get_raw(state, "/health/ready").await;
    let raw = body_as_str(&bytes);
    // 有组件降级 → ready 返回 503（不可用就绪），这与既有语义一致
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{raw}");
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("health ready is json");
    assert_eq!(body["components"]["storage"], "degraded");
    assert_eq!(body["status"], "degraded");
}

/// 磁盘水位分级必须在 `/api/health/report` 里可见（现场要能提前知道"快满了"）。
///
/// 背景：此前磁盘只有"已阻断"一档信号，等发现时已经在丢点。分级后
/// `warn`（<25%）/ `critical`（<15%）都应产生对应的问题码，
/// 而 `blocking`（<10%）由 `storage` 组件 degraded 表达。
#[tokio::test]
async fn health_report_exposes_disk_watermark_level() {
    let (state, gateway) = common::mock_state::mock_app_state_with_gateway();

    // 正常态：level = ok，不报磁盘问题
    gateway.disk_status().record(80.0, false);
    let (status, _headers, bytes) = get_raw(state.clone(), "/api/health/report").await;
    assert_eq!(status, StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("health report is json");
    let body = &body["data"];
    assert_eq!(body["summary"]["disk_level"], "ok");
    assert_eq!(body["summary"]["disk_free_pct"], 80.0);
    assert_eq!(body["summary"]["disk_thresholds"]["warn"], 25.0);
    assert_eq!(body["summary"]["disk_thresholds"]["critical"], 15.0);
    assert_eq!(body["summary"]["disk_thresholds"]["blocking"], 10.0);
    assert!(
        !body["problems"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["code"]
                .as_str()
                .unwrap_or_default()
                .starts_with("DISK_SPACE")),
        "正常磁盘不应报磁盘问题"
    );

    // warn 档（20%）：报 DISK_SPACE_LOW，但 /health/ready 仍应 200
    // （磁盘还能正常写入，此时重启网关反而丢数据）
    gateway.disk_status().record(20.0, false);
    let (status, _headers, bytes) = get_raw(state.clone(), "/api/health/report").await;
    assert_eq!(status, StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("health report is json");
    let body = &body["data"];
    assert_eq!(body["summary"]["disk_level"], "warn");
    let probs = body["problems"].as_array().unwrap();
    let low = probs
        .iter()
        .find(|p| p["code"] == "DISK_SPACE_LOW")
        .expect("应有 DISK_SPACE_LOW");
    assert_eq!(low["level"], "warning");

    let (status, _headers, _bytes) = get_raw(state.clone(), "/health/ready").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "warn 档磁盘仍能写入，不应判定为不可就绪"
    );

    // critical 档（12%）：报 DISK_SPACE_CRITICAL，等级为 critical
    gateway.disk_status().record(12.0, false);
    let (status, _headers, bytes) = get_raw(state, "/api/health/report").await;
    assert_eq!(status, StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("health report is json");
    let body = &body["data"];
    assert_eq!(body["summary"]["disk_level"], "critical");
    let probs = body["problems"].as_array().unwrap();
    let crit = probs
        .iter()
        .find(|p| p["code"] == "DISK_SPACE_CRITICAL")
        .expect("应有 DISK_SPACE_CRITICAL");
    assert_eq!(crit["level"], "critical");
}

/// 把响应字节转成可读文本（断言失败时便于排查）
fn body_as_str(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

#[tokio::test]
async fn log_download_returns_plain_text_not_json_envelope() {
    let (status, headers, bytes) =
        get_raw(mock_app_state(), "/api/diagnostics/logs/download?lines=20").await;
    assert_eq!(status, StatusCode::OK, "日志下载不应 404/500");

    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(
        content_type.starts_with("text/plain"),
        "content-type 应为 text/plain，实际 {content_type}"
    );
    assert!(
        headers.contains_key(header::CONTENT_DISPOSITION),
        "应带 attachment 头，便于浏览器直接下载"
    );

    // 日志文件不存在时允许空 body，但绝不能是 JSON 信封
    let text = String::from_utf8(bytes).expect("日志正文应为 UTF-8");
    assert!(
        text.is_empty() || !text.trim_start().starts_with('{'),
        "日志正文被包成了 JSON：{text}"
    );
}

// ===== 每总线四指标 =====

/// 构造带设备运行态的 AppState：`/api/health/report` 的 `online_devices`
/// 取自调度器运行态（`consecutive_failures == 0` 视为在线），与统计报表的
/// 「采集窗口内成功过的设备数」口径不同，故此处需注入设备状态。
fn state_with_runtime_devices() -> Arc<AppState> {
    let devices = vec![DeviceStatus {
        point_id: "dev1_t1".to_string(),
        bus: "COM_HEALTH".to_string(),
        device_id: "dev1".to_string(),
        consecutive_failures: 0,
        quality: "good".to_string(),
        ..Default::default()
    }];
    let (state, _gw) = common::mock_state::mock_app_state_for_self_heal(Vec::new(), devices);
    let mut cfg = state.config.load_full().as_ref().clone();
    cfg.devices = Some(vec![device("dev1", "COM_HEALTH")]);
    cfg.points = vec![point("dev1_t1", "dev1", "COM_HEALTH")];
    cfg.buses = vec![BusConfig {
        port_name: "COM_HEALTH".to_string(),
        ..BusConfig::default()
    }];
    state.config.config.store(Arc::new(cfg));
    state
}

/// `/api/health/report` 的每总线四指标必须由真实数据驱动。
///
/// 字段缺失会让前端整块空白（前端按字段名渲染），故在此锁定字段名与取值。
#[tokio::test]
async fn health_report_exposes_per_bus_key_metrics() {
    let state = state_with_runtime_devices();
    // dev1_t1 成功
    state.platform.metrics.record_collect_scoped(
        "COM_HEALTH",
        "dev1_t1",
        "dev1",
        true,
        4_000,
        None,
    );
    // 一次超时失败
    state.platform.metrics.record_collect_scoped(
        "COM_HEALTH",
        "dev1_t2",
        "dev1",
        false,
        30_000,
        Some(CollectErrorCode::Timeout),
    );
    // 一次重连
    state.platform.metrics.inc_bus_reconnect("COM_HEALTH");

    let (status, _headers, bytes) = get_raw(state, "/api/health/report").await;
    assert_eq!(status, StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("health report is json");
    // P1-1：统一 ApiResponse 信封
    let body = &body["data"];
    let bus = &body["buses"][0];

    assert_eq!(bus["bus"], "COM_HEALTH");
    assert_eq!(
        bus["online_devices"], 1,
        "运行态无连续失败的设备 → 在线设备数 1: {body}"
    );
    let rate = bus["timeout_rate"].as_f64().expect("timeout_rate 应有值");
    assert!(
        (rate - 50.0).abs() < 0.2,
        "2 次采集 1 次超时 → 超时率 50%，实际 {rate}: {body}"
    );
    assert_eq!(bus["last_error_code"], "TIMEOUT", "{body}");
    assert_eq!(bus["reconnect_count"], 1, "{body}");
}

/// 无采集时四指标不得伪造数值（null / 0）。
#[tokio::test]
async fn health_report_key_metrics_are_empty_without_collection() {
    let (status, _headers, bytes) = get_raw(state_with_config(), "/api/health/report").await;
    assert_eq!(status, StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("health report is json");
    // P1-1：统一 ApiResponse 信封
    let body = &body["data"];
    let bus = &body["buses"][0];

    assert_eq!(bus["online_devices"], 0, "{body}");
    assert!(
        bus["timeout_rate"].is_null(),
        "无采集超时率必须 null: {body}"
    );
    assert!(bus["last_error_code"].is_null(), "{body}");
    assert_eq!(bus["reconnect_count"], 0, "{body}");
}
