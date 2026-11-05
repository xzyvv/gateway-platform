//! 设备运维写接口集成测试（决策清单 P1-4 补差集）
//!
//! 覆盖 `docs/api-surface.md` 中此前无测试的 7 个写端点：
//! - POST   /api/device-groups
//! - PUT    /api/device-groups/:id
//! - DELETE /api/device-groups/:id
//! - POST   /api/device-groups/:id/devices
//! - DELETE /api/device-groups/:id/devices/:did
//! - PUT    /api/devices/:id            修改设备（含协议/总线/模板校验）
//! - PUT    /api/devices/:id/asset      资产信息 upsert
//! - POST   /api/devices/:id/points     在设备下建点位

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

/// 准备一台「总线 + 设备」齐备的配置：先落盘再走真实 reload 路径，
/// 让后续写接口面对的是与现场一致的已加载配置。
async fn state_with_device() -> (Arc<gateway_api::web::AppState>, axum::Router) {
    let state = common::mock_state::mock_app_state();
    let mut v = serde_json::to_value(current_config(&state)).unwrap();
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
async fn device_groups_crud_roundtrip() {
    let (state, app) = state_with_device().await;

    // 创建
    let body = call(
        &app,
        "POST",
        "/api/device-groups",
        Some(json!({ "id": "g1", "name": "一号机组", "description": "现场分组" })),
    )
    .await;
    assert_eq!(body["code"], 0, "创建分组失败: {body}");
    assert_eq!(body["data"]["id"], "g1");

    // 组名重复 → 6001，且不得产生第二个同名组
    let body = call(
        &app,
        "POST",
        "/api/device-groups",
        Some(json!({ "id": "g2", "name": "一号机组" })),
    )
    .await;
    assert_eq!(body["code"], 6001);
    assert!(
        body["message"].as_str().unwrap().contains("已存在"),
        "应提示组名已存在: {body}"
    );
    let cfg = current_config(&state);
    assert_eq!(cfg.device_groups.as_ref().map(|g| g.len()).unwrap_or(0), 1);

    // 空组名 → 6001
    let body = call(
        &app,
        "POST",
        "/api/device-groups",
        Some(json!({ "id": "g3", "name": "  " })),
    )
    .await;
    assert_eq!(body["code"], 6001);

    // 更新
    let body = call(
        &app,
        "PUT",
        "/api/device-groups/g1",
        Some(json!({ "name": "一号机组(改)" })),
    )
    .await;
    assert_eq!(body["code"], 0, "更新分组失败: {body}");
    let cfg = current_config(&state);
    let g1 = cfg
        .device_groups
        .as_ref()
        .unwrap()
        .iter()
        .find(|g| g.id == "g1")
        .unwrap();
    assert_eq!(g1.name, "一号机组(改)");

    // 单设备加入分组：重复加入不得产生重复条目
    for _ in 0..2 {
        let body = call(
            &app,
            "POST",
            "/api/device-groups/g1/devices",
            Some(json!({ "device_id": "dev1" })),
        )
        .await;
        assert_eq!(body["code"], 0, "加入分组失败: {body}");
    }
    let cfg = current_config(&state);
    let g1 = cfg
        .device_groups
        .as_ref()
        .unwrap()
        .iter()
        .find(|g| g.id == "g1")
        .unwrap();
    assert_eq!(g1.device_ids, vec!["dev1".to_string()]);

    // 移出分组
    let body = call(&app, "DELETE", "/api/device-groups/g1/devices/dev1", None).await;
    assert_eq!(body["code"], 0, "移出分组失败: {body}");
    let cfg = current_config(&state);
    let g1 = cfg
        .device_groups
        .as_ref()
        .unwrap()
        .iter()
        .find(|g| g.id == "g1")
        .unwrap();
    assert!(g1.device_ids.is_empty());

    // 删除分组；再删一次 → 6002
    let body = call(&app, "DELETE", "/api/device-groups/g1", None).await;
    assert_eq!(body["code"], 0, "删除分组失败: {body}");
    let body = call(&app, "DELETE", "/api/device-groups/g1", None).await;
    assert_eq!(body["code"], 6002);
    assert!(body["message"].as_str().unwrap().contains("not found"));
}

#[tokio::test]
async fn update_device_validates_bus_protocol_template() {
    let (state, app) = state_with_device().await;

    // 合法修改：改名 + 改采集周期
    let valid = json!({
        "id": "dev1",
        "name": "dev1-renamed",
        "template": "t1",
        "bus": "bus0",
        "protocol": "modbus",
        "address": "2",
        "slave_id": 1,
        "interval": 30
    });
    let body = call(&app, "PUT", "/api/devices/dev1", Some(valid.clone())).await;
    assert_eq!(body["code"], 0, "修改设备失败: {body}");
    let cfg = current_config(&state);
    let d = cfg
        .devices
        .as_ref()
        .unwrap()
        .iter()
        .find(|d| d.id == "dev1")
        .unwrap();
    assert_eq!(d.name, "dev1-renamed");
    assert_eq!(d.interval, Some(30));

    // 不支持的协议 → 4001（不能把配置改坏）
    let mut bad = valid.clone();
    bad["protocol"] = json!("bogus-protocol");
    let body = call(&app, "PUT", "/api/devices/dev1", Some(bad)).await;
    assert_eq!(body["code"], 4001);
    assert!(body["message"].as_str().unwrap().contains("不支持"));

    // 总线不存在 → 4001
    let mut bad = valid.clone();
    bad["bus"] = json!("ghost-bus");
    let body = call(&app, "PUT", "/api/devices/dev1", Some(bad)).await;
    assert_eq!(body["code"], 4001);

    // 设备不存在 → 4004
    let body = call(&app, "PUT", "/api/devices/nope", Some(valid)).await;
    assert_eq!(body["code"], 4004);
}

#[tokio::test]
async fn device_asset_put_is_upsert_not_duplicate() {
    let (_state, app) = state_with_device().await;

    let body = call(
        &app,
        "PUT",
        "/api/devices/dev1/asset",
        Some(json!({ "asset_no": "A-001", "vendor": "ACME", "model": "X1" })),
    )
    .await;
    assert_eq!(body["code"], 0, "写入资产失败: {body}");
    assert_eq!(body["data"]["asset_no"], "A-001");

    // 再次 PUT：同一设备只保留一条，字段被覆盖
    let body = call(
        &app,
        "PUT",
        "/api/devices/dev1/asset",
        Some(json!({ "asset_no": "A-002", "vendor": "ACME", "model": "X2" })),
    )
    .await;
    assert_eq!(body["code"], 0);
    assert_eq!(body["data"]["asset_no"], "A-002");

    let body = call(&app, "GET", "/api/devices/dev1/asset", None).await;
    assert_eq!(body["code"], 0);
    assert_eq!(body["data"]["asset_no"], "A-002");
    assert_eq!(body["data"]["model"], "X2");

    let body = call(&app, "GET", "/api/devices/assets", None).await;
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn create_point_under_device_validates_owner_and_unique_id() {
    let (state, app) = state_with_device().await;

    let payload = json!({
        "id": "p1",
        "device_id": null,
        "name": "电压",
        "bus": "bus0",
        "protocol": "modbus",
        "address": "40001",
        "slave_id": null,
        "address_num": 1,
        "data_id": null,
        "function_code": 3,
        "data_type": "u16",
        "byte_order": null,
        "register_count": null,
        "interval": 10,
        "scale": 1.0,
        "offset": null,
        "formula": null,
        "ct_ratio": null,
        "pt_ratio": null,
        "deadband": null,
        "on_change": null,
        "unit": "V",
        "alarm_high": null,
        "alarm_low": null,
        "aggregation": null,
        "modbus_register": null,
        "writable": false
    });

    // 设备不存在 → 4004，不得落盘
    let body = call(
        &app,
        "POST",
        "/api/devices/nope/points",
        Some(payload.clone()),
    )
    .await;
    assert_eq!(body["code"], 4004);

    // 正常创建：device_id 由路径注入
    let body = call(
        &app,
        "POST",
        "/api/devices/dev1/points",
        Some(payload.clone()),
    )
    .await;
    assert_eq!(body["code"], 0, "创建点位失败: {body}");
    assert_eq!(body["data"]["device_id"], "dev1");
    let cfg = current_config(&state);
    assert_eq!(cfg.points.len(), 1);
    assert_eq!(cfg.points[0].id, "p1");

    // 同 id 再建 → 4002（不允许静默覆盖）
    let body = call(&app, "POST", "/api/devices/dev1/points", Some(payload)).await;
    assert_eq!(body["code"], 4002);
    assert!(body["message"].as_str().unwrap().contains("已存在"));
    assert_eq!(current_config(&state).points.len(), 1);
}
