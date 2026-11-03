//! 统计报表 / 总线负载 / 设备健康评分：真实数据与 null 语义集成测试
//!
//! 背景：三个端点此前用 `hash(设备ID + 时间片)` 生成成功率、延迟、负载、健康分。
//! 本测试验证：
//! - 采集循环写入的真实计数（成功/失败/耗时）会被统计报表、总线负载、健康评分读取；
//! - 窗口内无采集时相关字段为 null（前端渲染「—」），不返回 0/100 之类的伪造值；
//! - 总线负载的速率是区间量：首帧无基线必须为 null，第二帧才出现真实速率。

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use common::mock_state::mock_app_state;
use gateway_api::web::AppState;
use gateway_core::config::{BusConfig, DeviceConfig, PointConfig};
use gateway_core::engine::metrics::CollectErrorCode;
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789abcdef";

fn device(id: &str, name: &str, bus: &str) -> DeviceConfig {
    DeviceConfig {
        id: id.to_string(),
        name: name.to_string(),
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

fn state_with(
    devices: Vec<DeviceConfig>,
    points: Vec<PointConfig>,
    buses: Vec<BusConfig>,
) -> Arc<AppState> {
    let state = mock_app_state();
    let mut cfg = state.config.load_full().as_ref().clone();
    cfg.devices = Some(devices);
    cfg.points = points;
    cfg.buses = buses;
    state.config.config.store(Arc::new(cfg));
    state
}

async fn get(state: Arc<AppState>, uri: &str) -> (StatusCode, serde_json::Value) {
    let app = gateway_api::web::build_router(state);
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 65536).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

// ===== 统计报表 =====

#[tokio::test]
async fn stats_report_reads_real_counters() {
    let state = state_with(
        vec![device("dev1", "dev1", "COM_STATS")],
        vec![
            point("dev1_t1", "dev1", "COM_STATS"),
            point("dev1_t2", "dev1", "COM_STATS"),
        ],
        vec![BusConfig {
            port_name: "COM_STATS".to_string(),
            ..BusConfig::default()
        }],
    );

    // dev1_t1: 3 成功 + 1 失败；dev1_t2: 2 成功 → 总 6，成功 5
    for _ in 0..3 {
        state.platform.metrics.record_collect_scoped(
            "COM_STATS",
            "dev1_t1",
            "dev1",
            true,
            4_000,
            None,
        );
    }
    state.platform.metrics.record_collect_scoped(
        "COM_STATS",
        "dev1_t1",
        "dev1",
        false,
        20_000,
        None,
    );
    for _ in 0..2 {
        state.platform.metrics.record_collect_scoped(
            "COM_STATS",
            "dev1_t2",
            "dev1",
            true,
            4_000,
            None,
        );
    }

    let (status, body) = get(state, "/api/diagnostics/stats?period=day").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let data = &body["data"];

    assert_eq!(data["summary"]["total_reads"], 6, "{data}");
    let rate = data["summary"]["success_rate"].as_f64().unwrap();
    assert!(
        (rate - 83.3).abs() < 0.2,
        "成功率应为 5/6≈83.3，实际 {rate}"
    );
    // (3×4ms + 1×20ms + 2×4ms) / 6 = 6.67ms → 四舍五入 7ms
    assert_eq!(data["summary"]["avg_latency_ms"], 7, "{data}");

    let dev = &data["device_stats"][0];
    assert_eq!(dev["device_name"], "dev1");
    assert_eq!(dev["total_reads"], 6);
    assert_eq!(dev["success"], 5);
    assert_eq!(dev["failed"], 1);

    let bus = &data["bus_stats"][0];
    assert_eq!(bus["bus"], "COM_STATS");
    assert_eq!(bus["total_reads"], 6);
    assert_eq!(bus["device_count"], 1, "{data}");

    // 错误分布来自真实全局计数：1 次采集失败，且未归类为串口/解码错误
    assert_eq!(data["error_dist"].as_array().unwrap().len(), 1, "{data}");
    assert_eq!(data["error_dist"][0]["error_type"], "其他采集失败");
    assert_eq!(data["error_dist"][0]["count"], 1);
}

#[tokio::test]
async fn stats_report_returns_null_without_collection() {
    let state = state_with(
        vec![device("dev1", "dev1", "COM_EMPTY")],
        vec![point("dev1_t1", "dev1", "COM_EMPTY")],
        vec![BusConfig {
            port_name: "COM_EMPTY".to_string(),
            ..BusConfig::default()
        }],
    );

    let (status, body) = get(state, "/api/diagnostics/stats?period=month").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let data = &body["data"];

    assert_eq!(data["summary"]["total_reads"], 0);
    assert!(
        data["summary"]["success_rate"].is_null(),
        "无采集必须是 null: {data}"
    );
    assert!(
        data["summary"]["avg_latency_ms"].is_null(),
        "无采集必须是 null: {data}"
    );
    assert!(data["device_stats"][0]["success_rate"].is_null(), "{data}");
    assert!(data["bus_stats"][0]["success_rate"].is_null(), "{data}");
    assert_eq!(data["error_dist"].as_array().unwrap().len(), 0);
}

// ===== 每总线四指标（在线设备数 / 超时率 / 最近错误码 / 重连次数）=====

/// 四指标必须由真实采集记录驱动（不是占位 0 / null）。
#[tokio::test]
async fn stats_report_exposes_per_bus_key_metrics() {
    let state = state_with(
        vec![
            device("dev1", "dev1", "COM_METRIC"),
            device("dev2", "dev2", "COM_METRIC"),
        ],
        vec![
            point("dev1_t1", "dev1", "COM_METRIC"),
            point("dev2_t1", "dev2", "COM_METRIC"),
        ],
        vec![BusConfig {
            port_name: "COM_METRIC".to_string(),
            ..BusConfig::default()
        }],
    );

    // dev1_t1：成功 → dev1 在线
    state.platform.metrics.record_collect_scoped(
        "COM_METRIC",
        "dev1_t1",
        "dev1",
        true,
        4_000,
        None,
    );
    // dev1_t2：超时失败
    state.platform.metrics.record_collect_scoped(
        "COM_METRIC",
        "dev1_t2",
        "dev1",
        false,
        30_000,
        Some(CollectErrorCode::Timeout),
    );
    // dev2_t1：IO 失败 → dev2 离线
    state.platform.metrics.record_collect_scoped(
        "COM_METRIC",
        "dev2_t1",
        "dev2",
        false,
        5_000,
        Some(CollectErrorCode::Io),
    );
    // 一次总线重连
    state.platform.metrics.inc_bus_reconnect("COM_METRIC");

    let (status, body) = get(state, "/api/diagnostics/stats?period=day").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let bus = &body["data"]["bus_stats"][0];

    assert_eq!(bus["bus"], "COM_METRIC", "{body}");
    assert_eq!(
        bus["online_devices"], 1,
        "只有 dev1 有成功采集 → 在线设备数 1: {body}"
    );
    // 3 次采集里 1 次超时 → 33.3%
    let rate = bus["timeout_rate"].as_f64().expect("timeout_rate 应有值");
    assert!(
        (rate - 33.3).abs() < 0.2,
        "超时率应约 33.3，实际 {rate}: {body}"
    );
    assert_eq!(
        bus["last_error_code"], "IO",
        "最近一次失败是 IO 错误: {body}"
    );
    assert_eq!(bus["reconnect_count"], 1, "{body}");
}

/// 四指标在窗口内无采集时必须为 null / 0（不得伪造数值）。
#[tokio::test]
async fn stats_report_key_metrics_are_empty_without_collection() {
    let state = state_with(
        vec![device("dev1", "dev1", "COM_NOMETRIC")],
        vec![point("dev1_t1", "dev1", "COM_NOMETRIC")],
        vec![BusConfig {
            port_name: "COM_NOMETRIC".to_string(),
            ..BusConfig::default()
        }],
    );

    let (status, body) = get(state, "/api/diagnostics/stats?period=day").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let bus = &body["data"]["bus_stats"][0];

    assert_eq!(bus["online_devices"], 0, "{body}");
    assert!(
        bus["timeout_rate"].is_null(),
        "无采集时超时率必须是 null: {body}"
    );
    assert!(
        bus["last_error_code"].is_null(),
        "无失败时最近错误码必须是 null: {body}"
    );
    assert_eq!(bus["reconnect_count"], 0, "{body}");
}

// ===== 总线负载 =====

#[tokio::test]
async fn bus_load_rates_need_two_samples_and_are_real() {
    let state = state_with(
        Vec::new(),
        Vec::new(),
        vec![BusConfig {
            port_name: "BUS_RATE_TEST".to_string(),
            ..BusConfig::default()
        }],
    );

    for _ in 0..5 {
        state
            .platform
            .metrics
            .record_collect_scoped("BUS_RATE_TEST", "p1", "", true, 3_000, None);
    }

    // 首帧：没有上一帧累计值，速率类字段必须为 null
    let (status, first) = get(state.clone(), "/api/diagnostics/bus-load").await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let bus0 = &first["data"][0];
    assert!(bus0["tx_rate"].is_null(), "首帧不得伪造速率: {bus0}");
    assert!(bus0["rx_rate"].is_null(), "首帧不得伪造速率: {bus0}");
    assert!(bus0["avg_latency_ms"].is_null(), "{bus0}");
    assert!(bus0["utilization"].is_null(), "{bus0}");
    assert!(
        bus0["backlog"].is_null(),
        "队列为全局单队列，不能按总线拆分: {bus0}"
    );
    assert_eq!(bus0["history"].as_array().unwrap().len(), 0);

    // 再采集 10 次后第二次请求：区间内真实速率 > 0
    tokio::time::sleep(Duration::from_millis(80)).await;
    for _ in 0..10 {
        state
            .platform
            .metrics
            .record_collect_scoped("BUS_RATE_TEST", "p1", "", true, 2_000, None);
    }

    let (status, second) = get(state, "/api/diagnostics/bus-load").await;
    assert_eq!(status, StatusCode::OK, "{second}");
    let bus1 = &second["data"][0];
    assert!(bus1["tx_rate"].as_f64().unwrap() > 0.0, "{bus1}");
    assert!(bus1["rx_rate"].as_f64().unwrap() > 0.0, "{bus1}");
    assert_eq!(bus1["avg_latency_ms"], 2, "{bus1}");
    assert_eq!(bus1["collision_rate"].as_f64().unwrap(), 0.0, "{bus1}");
    assert!(bus1["utilization"].as_f64().unwrap() > 0.0, "{bus1}");
    assert_eq!(bus1["history"].as_array().unwrap().len(), 1, "{bus1}");
}

// ===== 设备健康评分 =====

#[tokio::test]
async fn device_health_uses_real_counters() {
    let state = state_with(
        vec![device("dev1", "dev1", "COM_HEALTH")],
        vec![point("dev1_t1", "dev1", "COM_HEALTH")],
        Vec::new(),
    );

    // 9 成功 + 1 失败 → 成功率 90%，平均延迟 (9×5ms + 1×55ms)/10 = 10ms
    for _ in 0..9 {
        state.platform.metrics.record_collect_scoped(
            "COM_HEALTH",
            "dev1_t1",
            "dev1",
            true,
            5_000,
            None,
        );
    }
    state.platform.metrics.record_collect_scoped(
        "COM_HEALTH",
        "dev1_t1",
        "dev1",
        false,
        55_000,
        None,
    );

    let (status, body) = get(state, "/api/devices/health").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let h = &body["data"][0];
    assert_eq!(h["device_id"], "dev1");
    assert_eq!(h["success_rate"], 90.0, "{h}");
    assert_eq!(h["avg_latency_ms"], 10, "{h}");
    assert_eq!(h["error_count"], 1, "{h}");
    assert_eq!(h["offline_count"], 0, "mock 调度器无点位状态: {h}");
    assert!(
        h["last_seen_at"].is_number(),
        "有成功采集就应有最后在线时间: {h}"
    );
    let score = h["score"].as_i64().unwrap();
    // 100 - 10×0.6 - 10/10 - 1×2 = 91
    assert_eq!(score, 91, "{h}");
    assert_eq!(h["level"], "excellent", "{h}");

    // 第 1 期冗余收口：`GET /api/devices/health/summary` 已删除，
    // 汇总口径由前端从 `/api/devices/health` 列表派生（后端只有一份判据）。
    let list = body["data"].as_array().expect("health 列表");
    assert_eq!(list.len(), 1);
    let excellent = list.iter().filter(|h| h["level"] == "excellent").count();
    assert_eq!(excellent, 1, "{list:?}");
    let unknown = list.iter().filter(|h| h["level"] == "unknown").count();
    assert_eq!(unknown, 0, "{list:?}");
    let scores: Vec<i64> = list.iter().filter_map(|h| h["score"].as_i64()).collect();
    assert_eq!(scores.len(), 1, "{list:?}");
    assert_eq!(scores[0], 91, "{list:?}");
}

#[tokio::test]
async fn device_health_without_data_is_unknown_not_zero() {
    let state = state_with(
        vec![device("dev1", "dev1", "COM_UNKNOWN")],
        vec![point("dev1_t1", "dev1", "COM_UNKNOWN")],
        Vec::new(),
    );

    let (status, body) = get(state, "/api/devices/health").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let h = &body["data"][0];
    assert!(h["score"].is_null(), "无采集不得给出 0 分: {h}");
    assert_eq!(h["level"], "unknown", "{h}");
    assert!(h["success_rate"].is_null(), "{h}");
    assert!(h["last_seen_at"].is_null(), "{h}");

    // 第 1 期冗余收口：汇总由列表派生（`/api/devices/health/summary` 已删除）。
    let list = body["data"].as_array().expect("health 列表");
    let unknown = list.iter().filter(|h| h["level"] == "unknown").count();
    assert_eq!(unknown, 1, "{list:?}");
    // 无采集时无有效评分，平均分应为空（而非 0）
    assert!(
        list.iter().all(|h| h["score"].is_null()),
        "无采集不得给出 0 分: {list:?}"
    );
}
