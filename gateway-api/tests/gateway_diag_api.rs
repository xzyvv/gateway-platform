//! `/api/diagnostics/gateway` 集成测试
//!
//! 核心回归点：该端点曾用伪随机数生成 CPU 温度/使用率、内存、磁盘、负载、进程数，
//! 页面上的数字与设备真实状态无关。这里断言返回的是真实采集值（仅读取失败/不支持时
//! 才允许为 null），并校验百分比区间。

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use common::mock_state::mock_app_state;
use gateway_api::web::AppState;
use gateway_core::config::BusConfig;
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789abcdef";

/// 用给定总线配置替换 mock 状态里的 `buses`（用于串口探测断言）
fn state_with_buses(buses: Vec<BusConfig>) -> Arc<AppState> {
    let state = mock_app_state();
    let mut cfg = (*state.config.config.load_full()).clone();
    cfg.buses = buses;
    state.config.config.store(Arc::new(cfg));
    state
}

/// mock 环境下的一次真实采集请求
async fn fetch_diag(state: Arc<AppState>) -> serde_json::Value {
    let app = gateway_api::web::build_router(state);
    let request = Request::builder()
        .method("GET")
        .uri("/api/diagnostics/gateway")
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
        .body(Body::empty())
        .expect("build request");
    let resp = app.oneshot(request).await.expect("router call");
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .expect("read body");
    serde_json::from_slice(&bytes).expect("json body")
}

#[tokio::test]
async fn gateway_diag_returns_real_host_metrics() {
    let body = fetch_diag(mock_app_state()).await;
    assert_eq!(body["code"], 0, "接口应成功: {body}");
    let data = &body["data"];

    if cfg!(target_os = "linux") || cfg!(windows) {
        assert!(
            data["mem_usage"].is_number(),
            "内存使用率应为真实值: {data}"
        );
        assert!(data["processes"].is_number(), "进程数应为真实值: {data}");
        assert!(
            data["host_uptime_str"]
                .as_str()
                .map(|s| s != "-")
                .unwrap_or(false),
            "主机运行时长应可读取: {data}"
        );
        // P3-5：进程运行时长必须单独给出，且远小于主机时长（本次进程刚启动）
        let host = data["host_uptime_secs"].as_u64().unwrap_or(0);
        let process = data["process_uptime_secs"]
            .as_u64()
            .expect("进程运行时长应为秒级数字");
        assert!(
            process <= host,
            "进程运行时长 {process}s 不应超过主机开机时长 {host}s"
        );
        // 首次请求内部会热身 CPU 基线后复采，因此也应拿到使用率
        assert!(
            data["cpu_usage"].is_number(),
            "CPU 使用率应为真实值: {data}"
        );
    }

    // 百分比区间：真实采集不可能越界
    for key in ["cpu_usage", "mem_usage"] {
        if let Some(value) = data[key].as_f64() {
            assert!(
                (0.0..=100.0).contains(&value),
                "{key} 应在 0-100 之间，实际 {value}"
            );
        }
    }
    // 温度：若平台支持则必须是合理范围（-20~150℃），不应出现伪随机的固定区间值
    if let Some(temp) = data["cpu_temp"].as_f64() {
        assert!((-20.0..=150.0).contains(&temp), "CPU 温度不合理: {temp}");
    }
}

#[tokio::test]
async fn gateway_diag_does_not_fabricate_serial_status() {
    // 注入两条不可用的总线：本地串口节点不存在、TCP 端点无监听。
    // 状态必须如实为 error，而不是历史实现的「按序号 + 时间片伪随机 90% 为 ok」。
    let body = fetch_diag(state_with_buses(vec![
        BusConfig {
            port_name: "/dev/definitely-not-a-real-port".to_string(),
            ..BusConfig::default()
        },
        BusConfig {
            tcp_addr: Some("127.0.0.1:1".to_string()),
            ..BusConfig::default()
        },
    ]))
    .await;

    let ports = body["data"]["serial_ports"]
        .as_array()
        .expect("serial_ports 应为数组");
    assert_eq!(ports.len(), 2, "两条总线都应出现在诊断结果里: {body}");
    for port in ports {
        assert_eq!(
            port["status"].as_str(),
            Some("error"),
            "不可用的端点必须报 error: {port}"
        );
        let desc = port["desc"].as_str().unwrap_or_default();
        assert!(
            desc == "设备不存在" || desc == "连接失败",
            "失败原因应明确: {port}"
        );
    }
}
