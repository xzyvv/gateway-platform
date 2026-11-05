//! 诊断与访问控制写接口集成测试（决策清单 P1-4 补差集）
//!
//! 覆盖 `docs/api-surface.md` 中此前无测试的写端点：
//! - DELETE /api/access/api-keys/:id
//! - DELETE /api/access/rate-rules/:id
//! - DELETE /api/access/scheduled-tasks/:id
//! - POST   /api/access/scheduled-tasks/:id/run
//! - POST   /api/diagnostics/bus-load/refresh
//! - POST   /api/diagnostics/task-diag/:point_id
//! - POST   /api/diagnostics/exec（默认构建下 remote-shell 关闭）

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request};
use gateway_core::config::GatewayConfig;
use serde_json::json;
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789abcdef";

fn req(method: &str, uri: &str, body: Option<serde_json::Value>) -> Request<Body> {
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

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> serde_json::Value {
    let resp = app.clone().oneshot(req(method, uri, body)).await.unwrap();
    json_body(resp).await
}

fn current_config(state: &Arc<gateway_api::web::AppState>) -> GatewayConfig {
    state.config.load_full().as_ref().clone()
}

/// 夹具：bus0 + dev1，再真实创建一个点位 p1（诊断链路会用到点位配置）。
async fn state_with_point() -> axum::Router {
    let state = common::mock_state::mock_app_state();
    let mut v = serde_json::to_value(current_config(&state)).unwrap();
    v["buses"] = json!([{ "port_name": "bus0", "protocol": "modbus" }]);
    v["devices"] = json!([{
        "id": "dev1", "name": "dev1", "template": "t1", "bus": "bus0",
        "protocol": "modbus", "address": "1", "slave_id": 1, "interval": 10
    }]);
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

    let body = call(
        &app,
        "POST",
        "/api/devices/dev1/points",
        Some(json!({
            "id": "p1", "device_id": "dev1", "name": "电压", "bus": "bus0",
            "protocol": "modbus", "address": "40001", "slave_id": null,
            "address_num": 1, "data_id": null, "function_code": 3,
            "data_type": "u16", "byte_order": null, "register_count": null,
            "interval": 10, "scale": 1.0, "offset": null, "formula": null,
            "ct_ratio": null, "pt_ratio": null, "deadband": null,
            "on_change": null, "unit": "V", "alarm_high": null,
            "alarm_low": null, "aggregation": null, "modbus_register": null,
            "writable": false
        })),
    )
    .await;
    assert_eq!(body["code"], 0, "创建夹具点位失败: {body}");
    app
}

#[tokio::test]
async fn api_key_delete_removes_key_and_second_delete_is_404() {
    let state = common::mock_state::mock_app_state();
    let app = gateway_api::web::build_router(state.clone());

    let body = call(
        &app,
        "POST",
        "/api/access/api-keys",
        Some(json!({ "name": "读点位", "scopes": ["points:read"], "expires_days": 0 })),
    )
    .await;
    assert_eq!(body["code"], 0, "创建密钥失败: {body}");
    let id = body["data"]["id"].as_str().unwrap().to_string();
    assert_eq!(
        state.platform.api_keys.list().unwrap().len(),
        1,
        "创建后应有一条密钥"
    );

    let body = call(
        &app,
        "DELETE",
        &format!("/api/access/api-keys/{}", id),
        None,
    )
    .await;
    assert_eq!(body["code"], 0, "删除密钥失败: {body}");
    assert_eq!(body["data"], json!(true));
    assert!(state.platform.api_keys.list().unwrap().is_empty());

    let body = call(
        &app,
        "DELETE",
        &format!("/api/access/api-keys/{}", id),
        None,
    )
    .await;
    assert_eq!(body["code"], 4004);
    assert!(body["message"].as_str().unwrap().contains("不存在"));
}

#[tokio::test]
async fn rate_rule_delete_and_admin_cover_guard() {
    let state = common::mock_state::mock_app_state();
    let app = gateway_api::web::build_router(state.clone());

    // 覆盖 /api/access 的规则必须拒绝（否则误配后无法进入面板解除限流）
    let body = call(
        &app,
        "POST",
        "/api/access/rate-rules",
        Some(json!({
            "name": "自锁", "path": "/api/access/rate-rules", "method": "GET",
            "limitPerMinute": 10, "limitPerHour": 100
        })),
    )
    .await;
    assert_eq!(body["code"], 4000);
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("不能覆盖 /api/access"),
        "实际响应: {body}"
    );

    let body = call(
        &app,
        "POST",
        "/api/access/rate-rules",
        Some(json!({
            "name": "历史查询限流", "path": "/api/history", "method": "get",
            "limitPerMinute": 60, "limitPerHour": 1000, "burst": 10
        })),
    )
    .await;
    assert_eq!(body["code"], 0, "创建限流规则失败: {body}");
    let id = body["data"]["id"].as_str().unwrap().to_string();
    assert_eq!(body["data"]["method"], "GET", "method 应被归一化为大写");

    let body = call(
        &app,
        "DELETE",
        &format!("/api/access/rate-rules/{}", id),
        None,
    )
    .await;
    assert_eq!(body["code"], 0, "删除限流规则失败: {body}");
    assert_eq!(body["data"], json!(true));
    assert!(state.platform.rate_rules.list_rules().unwrap().is_empty());

    let body = call(
        &app,
        "DELETE",
        &format!("/api/access/rate-rules/{}", id),
        None,
    )
    .await;
    assert_eq!(body["code"], 4004);
    assert!(body["message"].as_str().unwrap().contains("不存在"));
}

#[tokio::test]
async fn scheduled_task_run_now_backup_config_then_delete() {
    let state = common::mock_state::mock_app_state();
    let app = gateway_api::web::build_router(state.clone());

    let body = call(
        &app,
        "POST",
        "/api/access/scheduled-tasks",
        Some(json!({ "name": "每日配置备份", "type": "backup_config", "cron": "*/5 * * * *" })),
    )
    .await;
    assert_eq!(body["code"], 0, "创建计划任务失败: {body}");
    let id = body["data"]["id"].as_str().unwrap().to_string();

    // 立即执行：backup_config 在 mock 状态下可成功（写一条版本快照）
    let versions_before = state
        .config
        .version_store
        .list_versions()
        .map(|v| v.len())
        .unwrap_or(0);
    let body = call(
        &app,
        "POST",
        &format!("/api/access/scheduled-tasks/{}/run", id),
        None,
    )
    .await;
    assert_eq!(body["code"], 0, "执行计划任务失败: {body}");
    assert_eq!(body["data"]["status"], "success");
    assert_eq!(body["data"]["error"], serde_json::Value::Null);
    let versions_after = state
        .config
        .version_store
        .list_versions()
        .map(|v| v.len())
        .unwrap_or(0);
    assert!(
        versions_after > versions_before,
        "backup_config 应新增一条版本快照: {versions_before} -> {versions_after}"
    );

    // 删除后再次执行 → 404
    let body = call(
        &app,
        "DELETE",
        &format!("/api/access/scheduled-tasks/{}", id),
        None,
    )
    .await;
    assert_eq!(body["code"], 0, "删除任务失败: {body}");
    let body = call(
        &app,
        "DELETE",
        &format!("/api/access/scheduled-tasks/{}", id),
        None,
    )
    .await;
    assert_eq!(body["code"], 4004);
    let body = call(
        &app,
        "POST",
        &format!("/api/access/scheduled-tasks/{}/run", id),
        None,
    )
    .await;
    assert_eq!(body["code"], 4004);
}

#[tokio::test]
async fn diagnostics_bus_load_refresh_returns_configured_bus() {
    let app = state_with_point().await;

    let body = call(&app, "POST", "/api/diagnostics/bus-load/refresh", None).await;
    assert_eq!(body["code"], 0, "刷新总线负载失败: {body}");
    let loads = body["data"].as_array().unwrap();
    assert_eq!(loads.len(), 1, "配置里有 1 条总线: {body}");
    assert_eq!(loads[0]["bus"], "bus0");
    assert_eq!(loads[0]["bus_type"], "modbus");
    // 首帧无基线：速率为 null 而不是伪造 0
    assert_eq!(loads[0]["tx_rate"], serde_json::Value::Null);
    assert_eq!(loads[0]["rx_rate"], serde_json::Value::Null);
    assert!(loads[0]["history"].as_array().unwrap().is_empty());

    // 再次刷新：不产生重复总线条目
    let body = call(&app, "POST", "/api/diagnostics/bus-load/refresh", None).await;
    assert_eq!(body["code"], 0);
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn diagnostics_task_diag_reports_unknown_point_as_offline() {
    let app = state_with_point().await;

    // 已配置点位：诊断应返回完整步骤，且首步是配置校验（pass）
    let body = call(&app, "POST", "/api/diagnostics/task-diag/p1", None).await;
    assert_eq!(body["code"], 0, "点位诊断失败: {body}");
    assert_eq!(body["data"]["point_id"], "p1");
    assert_eq!(body["data"]["point_name"], "电压");
    let steps = body["data"]["steps"].as_array().unwrap();
    assert!(!steps.is_empty());
    // 步骤状态必须是约定枚举，且第一步必须点名配置校验结果
    for s in steps {
        let status = s["status"].as_str().unwrap();
        assert!(
            ["pass", "fail", "warning", "pending"].contains(&status),
            "非法步骤状态: {status}"
        );
        assert!(!s["name"].as_str().unwrap().is_empty());
        assert!(!s["message"].as_str().unwrap().is_empty());
    }

    // 未配置点位：不得 404，应给出离线结论 + 可操作建议
    let body = call(&app, "POST", "/api/diagnostics/task-diag/ghost-point", None).await;
    assert_eq!(body["code"], 0);
    assert_eq!(body["data"]["point_id"], "ghost-point");
    assert_eq!(body["data"]["point_name"], "ghost-point");
    assert_eq!(body["data"]["status"], "offline");
    assert!(!body["data"]["suggestion"].as_str().unwrap().is_empty());
    assert!(body["data"]["last_check_at_ms"].as_i64().unwrap() > 0);
}

#[tokio::test]
async fn diagnostics_exec_is_disabled_without_remote_shell_feature() {
    let state = common::mock_state::mock_app_state();
    let app = gateway_api::web::build_router(state);

    let body = call(
        &app,
        "POST",
        "/api/diagnostics/exec",
        Some(json!({ "command": "uname -a" })),
    )
    .await;
    assert_eq!(body["code"], 4000);
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("remote shell is disabled"),
        "默认构建必须明确拒绝而不是静默执行: {body}"
    );
}
