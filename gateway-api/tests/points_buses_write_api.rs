//! 点位与总线写接口集成测试（决策清单 P1-4 补差集）
//!
//! 覆盖 `docs/api-surface.md` 中此前无测试的 5 个写端点：
//! - PUT  /api/points/:id              编辑点位（字段校验 + 归属设备继承）
//! - POST /api/point-tools/offset/apply     批量地址偏移（只影响选中点位、下界钳制）
//! - POST /api/point-tools/clone/apply      跨设备克隆点位（重命名 + 跳过已存在）
//! - POST /api/buses/:name/pause       暂停总线采集
//! - POST /api/buses/:name/resume      恢复总线采集

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

/// 点位编辑用的完整载荷（`PointPayload` 是 deny_unknown_fields 且字段无默认值）。
fn point_payload(id: &str, name: &str, address_num: i64, interval: u64) -> serde_json::Value {
    json!({
        "id": id,
        "device_id": "dev1",
        "name": name,
        "bus": "bus0",
        "protocol": "modbus",
        "address": "40001",
        "slave_id": null,
        "address_num": address_num,
        "data_id": null,
        "function_code": 3,
        "data_type": "u16",
        "byte_order": null,
        "register_count": null,
        "interval": interval,
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
    })
}

/// 夹具：bus0 + dev1/dev2，并真实创建点位 p1（走 API，保证与现场一致）。
async fn state_with_points() -> (Arc<gateway_api::web::AppState>, axum::Router) {
    let state = common::mock_state::mock_app_state();
    let mut v = serde_json::to_value(current_config(&state)).unwrap();
    v["buses"] = json!([{ "port_name": "bus0", "protocol": "modbus" }]);
    v["devices"] = json!([
        {
            "id": "dev1", "name": "dev1", "template": "t1", "bus": "bus0",
            "protocol": "modbus", "address": "1", "slave_id": 1, "interval": 10
        },
        {
            "id": "dev2", "name": "dev2", "template": "t1", "bus": "bus0",
            "protocol": "modbus", "address": "2", "slave_id": 1, "interval": 10
        }
    ]);
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
        Some(point_payload("p1", "电压", 1, 10)),
    )
    .await;
    assert_eq!(body["code"], 0, "创建夹具点位失败: {body}");
    (state, app)
}

#[tokio::test]
async fn update_point_validates_fields_and_owner_device() {
    let (state, app) = state_with_points().await;

    // 合法修改：改名 + 地址 +1 + 单位清空（整行覆盖语义）
    let mut ok = point_payload("p1", "电压(改)", 2, 10);
    ok["unit"] = json!(null);
    let body = call(&app, "PUT", "/api/points/p1", Some(ok.clone())).await;
    assert_eq!(body["code"], 0, "编辑点位失败: {body}");
    let cfg = current_config(&state);
    assert_eq!(cfg.points[0].name.as_deref(), Some("电压(改)"));
    assert_eq!(cfg.points[0].address_num, Some(2));
    assert_eq!(cfg.points[0].unit, None, "unit 应被整行覆盖清空");

    // interval=0 → 4002
    let mut bad = ok.clone();
    bad["interval"] = json!(0);
    let body = call(&app, "PUT", "/api/points/p1", Some(bad)).await;
    assert_eq!(body["code"], 4002);
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("interval 必须大于 0"));

    // interval=60：北向关闭时不做周期约束（validate_cycle_conflict 在
    // !mqtt.enabled 时直接放行，见 config::validate::gate），所以这里应当成功；
    // 周期冲突本身由 gateway-core 的 gate 单测覆盖，不在写接口层重复断言。
    let mut long_interval = ok.clone();
    long_interval["interval"] = json!(60);
    let body = call(&app, "PUT", "/api/points/p1", Some(long_interval)).await;
    assert_eq!(body["code"], 0, "北向关闭时不应因采集周期拒绝编辑: {body}");

    // scale=0 → 4002（不能静默归零采集值）
    let mut bad = ok.clone();
    bad["scale"] = json!(0.0);
    let body = call(&app, "PUT", "/api/points/p1", Some(bad)).await;
    assert_eq!(body["code"], 4002);
    assert!(body["message"].as_str().unwrap().contains("scale 不能为 0"));

    // 不支持的协议 → 4002
    let mut bad = ok.clone();
    bad["protocol"] = json!("bogus-protocol");
    let body = call(&app, "PUT", "/api/points/p1", Some(bad)).await;
    assert_eq!(body["code"], 4002);
    assert!(body["message"].as_str().unwrap().contains("不支持的协议"));

    // 归属设备不存在 → 4004
    let mut bad = ok.clone();
    bad["device_id"] = json!("ghost-dev");
    let body = call(&app, "PUT", "/api/points/p1", Some(bad)).await;
    assert_eq!(body["code"], 4004);
    assert!(body["message"].as_str().unwrap().contains("不存在"));

    // 点位不存在 → 4004
    let body = call(&app, "PUT", "/api/points/nope", Some(ok)).await;
    assert_eq!(body["code"], 4004);
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("point not found"));
}

#[tokio::test]
async fn point_offset_apply_touches_only_selected_and_clamps_at_zero() {
    let (state, app) = state_with_points().await;
    // 第二个点位：address_num=5
    let body = call(
        &app,
        "POST",
        "/api/devices/dev1/points",
        Some(point_payload("p2", "电流", 5, 10)),
    )
    .await;
    assert_eq!(body["code"], 0);

    let body = call(
        &app,
        "POST",
        "/api/point-tools/offset/apply",
        Some(json!({ "ids": ["p1"], "offset": 10 })),
    )
    .await;
    assert_eq!(body["code"], 0, "偏移失败: {body}");
    assert_eq!(body["data"]["updated"], 1);
    let cfg = current_config(&state);
    let p1 = cfg.points.iter().find(|p| p.id == "p1").unwrap();
    let p2 = cfg.points.iter().find(|p| p.id == "p2").unwrap();
    assert_eq!(p1.address_num, Some(11));
    assert_eq!(p2.address_num, Some(5), "未选中的点位不得被改动");

    // 负偏移越界 → 钳到 0，而不是回绕成 65535
    let body = call(
        &app,
        "POST",
        "/api/point-tools/offset/apply",
        Some(json!({ "ids": ["p1"], "offset": -20 })),
    )
    .await;
    assert_eq!(body["code"], 0);
    assert_eq!(body["data"]["updated"], 1);
    let cfg = current_config(&state);
    assert_eq!(
        cfg.points
            .iter()
            .find(|p| p.id == "p1")
            .unwrap()
            .address_num,
        Some(0)
    );
}

#[tokio::test]
async fn point_clone_apply_copies_to_target_and_skips_existing() {
    let (state, app) = state_with_points().await;

    let body = call(
        &app,
        "POST",
        "/api/point-tools/clone/apply",
        Some(json!({
            "source_device_id": "dev1",
            "target_device_ids": ["dev2"],
            "name_suffix": "_copy"
        })),
    )
    .await;
    assert_eq!(body["code"], 0, "克隆失败: {body}");
    assert!(body["data"]["total"].as_u64().unwrap() >= 1);
    let cfg = current_config(&state);
    let cloned = cfg
        .points
        .iter()
        .find(|p| p.id == "p1_dev2")
        .expect("目标设备上应出现克隆点位");
    assert_eq!(cloned.device_id.as_deref(), Some("dev2"));
    assert_eq!(cloned.name.as_deref(), Some("电压_copy"));
    assert_eq!(cloned.address_num, Some(1), "克隆应保留源地址");

    // 重复克隆：已存在的 id 跳过，不产生重复条目
    let before = current_config(&state).points.len();
    let body = call(
        &app,
        "POST",
        "/api/point-tools/clone/apply",
        Some(json!({
            "source_device_id": "dev1",
            "target_device_ids": ["dev2"],
            "name_suffix": "_copy"
        })),
    )
    .await;
    assert_eq!(body["code"], 0);
    assert_eq!(current_config(&state).points.len(), before);

    // 目标设备不存在 → 404
    let body = call(
        &app,
        "POST",
        "/api/point-tools/clone/apply",
        Some(json!({
            "source_device_id": "dev1",
            "target_device_ids": ["ghost-dev"],
            "name_suffix": ""
        })),
    )
    .await;
    assert_eq!(body["code"], 4004);
    assert!(body["message"].as_str().unwrap().contains("not found"));
}

#[tokio::test]
async fn bus_pause_resume_checks_bus_and_reports_unsupported_mock() {
    let (_state, app) = state_with_points().await;

    // 不存在的总线 → 404（在任何调度动作之前拦截）
    let body = call(&app, "POST", "/api/buses/ghost-bus/pause", None).await;
    assert_eq!(body["code"], 4004);
    assert!(
        body["message"].as_str().unwrap().contains("不在当前配置中"),
        "应提示总线不在配置中: {body}"
    );

    // 配置内总线：mock 调度器没有暂停能力，handler 必须把「不支持」变成可读 400
    let body = call(&app, "POST", "/api/buses/bus0/pause", None).await;
    assert_eq!(body["code"], 4000);
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("不支持暂停总线采集"),
        "应明确说明调度器不支持: {body}"
    );

    // resume 同样先校验总线存在性
    let body = call(&app, "POST", "/api/buses/ghost-bus/resume", None).await;
    assert_eq!(body["code"], 4004);
    let body = call(&app, "POST", "/api/buses/bus0/resume", None).await;
    assert_eq!(body["code"], 4000);
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("不支持恢复总线采集"));
}
