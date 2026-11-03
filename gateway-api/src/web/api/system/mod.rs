//! Web API 系统端点：健康检查、版本、状态、配置管理、指标

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};

use crate::web::types::{ApiError, ApiResponse, AppState, RunStatus, VersionInfo};
use gateway_core::config::GatewayConfig;

pub(crate) mod handlers;
mod routes;

/// 两个版本之间的字段级差异对比查询参数
#[derive(serde::Deserialize)]
pub(crate) struct CompareParams {
    pub compare_to: i64,
}

/// 北向状态构建的输入参数（替代 13 个位置参数，提升可读性）
///
/// N-7：`*_connected` / `*_last_error` 一律来自 connector 的 `health()`，
/// 不再由 Web 层按配置推断——断连、端点 4xx/5xx 等
/// 真实故障才能在状态页可见。
#[derive(Debug, Clone)]
pub(crate) struct NorthStatusParams {
    pub mqtt_enabled: bool,
    pub mqtt_connected: bool,
    pub mqtt_circuit_open: bool,
    pub mqtt_last_error: Option<String>,
    pub mqtt_broker: String,
    pub webhook_enabled: bool,
    /// None = 无连接器可问（未启用，或连接器未成功初始化）
    pub webhook_connected: Option<bool>,
    pub webhook_last_error: Option<String>,
    pub webhook_url: String,
    pub wh_success: u64,
    pub wh_failure: u64,
    pub queue_len: usize,
    pub mqtt_success: u64,
    /// N-7：RealtimeBus 广播溢出（背压）累计丢弃批次数
    pub lagged_dropped_batches: u64,
    /// 北向 Server 存活列表（原独立端点 `/api/north/servers` 的内容，
    /// 第 1 期冗余收口并入本响应）
    pub servers: Vec<NorthServerInfo>,
}

/// 北向 Server 存活信息
#[derive(Debug, Clone)]
pub(crate) struct NorthServerInfo {
    pub name: String,
    pub running: bool,
}

/// Build the north status JSON from primitive inputs (testable without AppState).
pub(crate) fn build_north_status_json(
    p: &NorthStatusParams,
    metrics: &gateway_core::engine::metrics::Metrics,
) -> serde_json::Value {
    let mut channels: Vec<serde_json::Value> = Vec::new();
    channels.push(serde_json::json!({
        "name": "mqtt",
        "type": "mqtt",
        "enabled": p.mqtt_enabled,
        "connected": p.mqtt_connected,
        "queue_length": p.queue_len,
        "extra": {
            "broker": p.mqtt_broker,
            "circuit_open": p.mqtt_circuit_open,
            "last_error": p.mqtt_last_error,
        },
    }));
    channels.push(serde_json::json!({
        "name": "webhook",
        "type": "http",
        "enabled": p.webhook_enabled,
        "connected": p.webhook_connected,
        "queue_length": 0,
        "extra": {
            "url": p.webhook_url,
            "success_count": p.wh_success,
            "failed_count": p.wh_failure,
            "avg_latency_ms": serde_json::Value::Null,
            "last_error": p.webhook_last_error,
        },
    }));

    let healthy = channels.iter().all(|c| {
        let enabled = c["enabled"].as_bool().unwrap_or(false);
        if !enabled {
            return true;
        }
        c["connected"].as_bool().unwrap_or(true)
    });

    let mqtt_connected_i = if p.mqtt_enabled && p.mqtt_connected {
        1
    } else {
        0
    };
    // N-7：已连接数改由 health() 实测值聚合；无连接器可问（None）按未连接计，
    // 避免「配置已启用但实际不连通」被算成已连接。
    let http_connected_i = if p.webhook_enabled && p.webhook_connected == Some(true) {
        1
    } else {
        0
    };
    let total_sent = p.mqtt_success + p.wh_success;

    let aggregated = serde_json::json!({
        "mqtt": if p.mqtt_enabled { 1 } else { 0 },
        "http": if p.webhook_enabled { 1 } else { 0 },
        "mqtt_connected": mqtt_connected_i,
        "http_connected": http_connected_i,
        "pending_messages": p.queue_len,
        "total_messages_sent": total_sent,
        // N-7：背压丢帧累计（RealtimeBus Lagged）。持续增长 = 实时推送存在数据缺口。
        "lagged_dropped_batches": p.lagged_dropped_batches,
    });

    // ===== sink 指标（async fan-out 相关）=====
    // 从全局 metrics 读取每个 sink 的 send_total / dropped_total / channel_depth
    // metrics 由参数传入（1.2 注入化）
    let mut sink_names: Vec<String> = metrics.sink_names();
    // 合并 dropped map 中出现但 send_total 没有的 sink name
    {
        let dropped_keys: Vec<String> = metrics.sink_dropped_total_keys();
        for n in dropped_keys {
            if !sink_names.contains(&n) {
                sink_names.push(n);
            }
        }
    }
    sink_names.sort();
    let sinks: Vec<serde_json::Value> = sink_names
        .iter()
        .map(|name| {
            serde_json::json!({
                "name": name,
                "send_total": metrics.sink_send_total(name),
                "dropped_total": metrics.sink_dropped_total(name),
                "channel_depth": metrics.sink_channel_depth(name),
            })
        })
        .collect();

    let servers: Vec<serde_json::Value> = p
        .servers
        .iter()
        .map(|s| {
            serde_json::json!({
                "name": s.name,
                "running": s.running,
            })
        })
        .collect();

    serde_json::json!({
        "healthy": healthy,
        "channels": channels,
        "sinks": sinks,
        "aggregated": aggregated,
        "servers": servers,
    })
}

pub(crate) use handlers::*;
pub(crate) use routes::system_routes;

#[cfg(test)]
mod north_tests {
    use super::{build_north_status_json, NorthServerInfo, NorthStatusParams};

    fn default_args() -> NorthStatusParams {
        NorthStatusParams {
            mqtt_enabled: true,
            mqtt_connected: true,
            mqtt_circuit_open: false,
            mqtt_last_error: None,
            mqtt_broker: "tcp://broker:1883".into(),
            webhook_enabled: true,
            webhook_connected: Some(true),
            webhook_last_error: None,
            webhook_url: "https://hook/api".into(),
            wh_success: 10,
            wh_failure: 2,
            queue_len: 5,
            mqtt_success: 100,
            lagged_dropped_batches: 0,
            servers: vec![NorthServerInfo {
                name: "opcua".into(),
                running: true,
            }],
        }
    }

    /// 第 1 期冗余收口：原 `/api/north/servers` 的内容并入本响应。
    #[test]
    fn test_servers_included() {
        let p = default_args();
        let v = build_north_status_json(&p, &gateway_core::engine::metrics::Metrics::new());
        let servers = v["servers"].as_array().unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0]["name"].as_str().unwrap(), "opcua");
        assert!(servers[0]["running"].as_bool().unwrap());
    }

    #[test]
    fn test_has_two_channels() {
        let p = default_args();
        let v = build_north_status_json(&p, &gateway_core::engine::metrics::Metrics::new());
        assert_eq!(v["channels"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_channel_types() {
        let p = default_args();
        let v = build_north_status_json(&p, &gateway_core::engine::metrics::Metrics::new());
        let chs = v["channels"].as_array().unwrap();
        assert_eq!(chs[0]["type"].as_str().unwrap(), "mqtt");
        assert_eq!(chs[1]["type"].as_str().unwrap(), "http");
    }

    #[test]
    fn test_mqtt_extra_fields() {
        let p = default_args();
        let v = build_north_status_json(&p, &gateway_core::engine::metrics::Metrics::new());
        let mqtt = &v["channels"].as_array().unwrap()[0];
        assert_eq!(
            mqtt["extra"]["broker"].as_str().unwrap(),
            "tcp://broker:1883"
        );
        assert!(!mqtt["extra"]["circuit_open"].as_bool().unwrap());
        assert_eq!(mqtt["queue_length"].as_u64().unwrap(), 5u64);
    }

    #[test]
    fn test_webhook_extra_fields() {
        let p = default_args();
        let v = build_north_status_json(&p, &gateway_core::engine::metrics::Metrics::new());
        let wh = &v["channels"].as_array().unwrap()[1];
        assert_eq!(wh["extra"]["url"].as_str().unwrap(), "https://hook/api");
        assert_eq!(wh["extra"]["success_count"].as_u64().unwrap(), 10u64);
        assert_eq!(wh["extra"]["failed_count"].as_u64().unwrap(), 2u64);
        assert!(wh["extra"]["avg_latency_ms"].is_null());
    }

    #[test]
    fn test_aggregated_counts() {
        let p = default_args();
        let v = build_north_status_json(&p, &gateway_core::engine::metrics::Metrics::new());
        let agg = &v["aggregated"];
        assert_eq!(agg["mqtt"].as_u64().unwrap(), 1u64);
        assert_eq!(agg["http"].as_u64().unwrap(), 1u64);
        assert_eq!(agg["mqtt_connected"].as_u64().unwrap(), 1u64);
        assert_eq!(agg["http_connected"].as_u64().unwrap(), 1u64);
        assert_eq!(agg["pending_messages"].as_u64().unwrap(), 5u64);
        assert_eq!(agg["total_messages_sent"].as_u64().unwrap(), 110u64);
    }

    #[test]
    fn test_healthy_when_mqtt_connected() {
        let p = default_args();
        let v = build_north_status_json(&p, &gateway_core::engine::metrics::Metrics::new());
        assert!(v["healthy"].as_bool().unwrap());
    }

    #[test]
    fn test_unhealthy_when_mqtt_disconnected() {
        let mut p = default_args();
        p.mqtt_connected = false;
        let v = build_north_status_json(&p, &gateway_core::engine::metrics::Metrics::new());
        assert!(!v["healthy"].as_bool().unwrap());
    }

    /// N-7：Webhook 的 connected 必须来自 connector `health()`，断连时
    /// 通道显示未连接、健康判定为 false，且最近错误可见。
    #[test]
    fn test_webhook_health_surfaces_disconnect_and_last_error() {
        let mut p = default_args();
        p.webhook_connected = Some(false);
        p.webhook_last_error = Some("http status 503 Service Unavailable".into());
        let v = build_north_status_json(&p, &gateway_core::engine::metrics::Metrics::new());
        assert!(!v["healthy"].as_bool().unwrap());
        assert_eq!(v["channels"][1]["connected"].as_bool(), Some(false));
        assert_eq!(
            v["channels"][1]["extra"]["last_error"].as_str().unwrap(),
            "http status 503 Service Unavailable"
        );
        assert_eq!(v["aggregated"]["http_connected"].as_u64().unwrap(), 0);
    }

    /// N-7：实时广播背压丢帧必须出现在状态响应里（否则运维看不到数据缺口）。
    #[test]
    fn test_lagged_dropped_batches_exposed() {
        let mut p = default_args();
        p.lagged_dropped_batches = 7;
        let v = build_north_status_json(&p, &gateway_core::engine::metrics::Metrics::new());
        assert_eq!(
            v["aggregated"]["lagged_dropped_batches"].as_u64().unwrap(),
            7
        );
    }

    #[test]
    fn test_healthy_when_all_disabled() {
        let v = build_north_status_json(
            &NorthStatusParams {
                mqtt_enabled: false,
                mqtt_connected: false,
                mqtt_circuit_open: false,
                mqtt_last_error: None,
                mqtt_broker: String::new(),
                webhook_enabled: false,
                webhook_connected: None,
                webhook_last_error: None,
                webhook_url: String::new(),
                wh_success: 0,
                wh_failure: 0,
                queue_len: 0,
                mqtt_success: 0,
                lagged_dropped_batches: 0,
                servers: Vec::new(),
            },
            &gateway_core::engine::metrics::Metrics::new(),
        );
        assert!(v["healthy"].as_bool().unwrap());
    }

    #[test]
    fn test_total_messages_sent_sum() {
        let v = build_north_status_json(
            &NorthStatusParams {
                mqtt_enabled: true,
                mqtt_connected: true,
                mqtt_circuit_open: false,
                mqtt_last_error: None,
                mqtt_broker: String::from("b"),
                webhook_enabled: true,
                webhook_connected: Some(true),
                webhook_last_error: None,
                webhook_url: String::from("u"),
                wh_success: 7,
                wh_failure: 3,
                queue_len: 0,
                mqtt_success: 50,
                lagged_dropped_batches: 0,
                servers: Vec::new(),
            },
            &gateway_core::engine::metrics::Metrics::new(),
        );
        assert_eq!(
            v["aggregated"]["total_messages_sent"].as_u64().unwrap(),
            57u64
        );
    }

    #[test]
    fn test_circuit_open_propagated() {
        let v = build_north_status_json(
            &NorthStatusParams {
                mqtt_enabled: true,
                mqtt_connected: true,
                mqtt_circuit_open: true,
                mqtt_last_error: None,
                mqtt_broker: String::from("b"),
                webhook_enabled: false,
                webhook_connected: None,
                webhook_last_error: None,
                webhook_url: String::new(),
                wh_success: 0,
                wh_failure: 0,
                queue_len: 0,
                mqtt_success: 0,
                lagged_dropped_batches: 0,
                servers: Vec::new(),
            },
            &gateway_core::engine::metrics::Metrics::new(),
        );
        assert!(v["channels"][0]["extra"]["circuit_open"].as_bool().unwrap());
    }
}
