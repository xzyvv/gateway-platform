//! r43-CFG1 回归：POST /api/config 回传脱敏占位值时的版本快照一致性
//!
//! 场景：前端 `GET /api/config` 拿到的是脱敏后的配置（敏感字段为 `"********"`），
//! 用户不改凭据、原样回传保存。旧实现把**入参**（含占位串）直接写入版本快照，
//! 导致版本库里凭据被写成 `"********"`；回滚到该版本时会读到错误凭据。
//!
//! 本测试断言：版本快照中保存的是**真实凭据**（还原自旧配置），而非占位串。

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use gateway_core::config::{BusConfig, ConfigLoader, GatewayConfig, PointConfig};
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789abcdef";
const REAL_MQTT_PW: &str = "real-mqtt-password";
const REAL_OPCUA_USER: &str = "opcua-operator";
const REAL_OPCUA_PW: &str = "real-opcua-password";
const REDACTED: &str = "********";

fn req(method: &str, uri: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {}", TOKEN))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// 一份通过 `validate_collection_config` 的最小配置（mqtt 默认 disabled，故不校验 broker）。
fn base_config() -> GatewayConfig {
    GatewayConfig {
        buses: vec![BusConfig::default()],
        points: vec![PointConfig {
            id: "p1".into(),
            bus: "bus0".into(),
            protocol: "modbus".into(),
            address: "1".into(),
            address_num: Some(1),
            slave_id: Some(1),
            interval: 10,
            ..Default::default()
        }],
        ..Default::default()
    }
}

#[tokio::test]
async fn post_config_version_snapshot_keeps_real_secrets() {
    let mut state = common::mock_state::mock_app_state();

    // 注入临时 config 路径：确保落盘成功——版本快照在 save 成功后才写入。
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg_path = dir.path().join("config.json");
    {
        let s = Arc::get_mut(&mut state).expect("fresh Arc，无其他持有者");
        s.config.config_loader = ConfigLoader::new(cfg_path.clone());
        // 模拟「线上已加载」的当前配置：带真实 mqtt 密码与真实 api_token。
        let mut cur = base_config();
        let mut web = s.web.clone();
        web.api_token = TOKEN.to_string();
        cur.web = Some(web);
        cur.mqtt.password = Some(REAL_MQTT_PW.to_string());
        // P0-3：总线级 OPC UA 凭据同样走脱敏/还原链路。
        cur.buses[0].protocol = "opcua".into();
        cur.buses[0].opcua = Some(gateway_core::config::OpcUaClientConfig {
            auth: "username".into(),
            username: Some(REAL_OPCUA_USER.into()),
            password: Some(REAL_OPCUA_PW.into()),
            ..Default::default()
        });
        s.config.config.store(Arc::new(cur));
    }

    let app = gateway_api::web::build_router(state.clone());

    // 模拟前端「GET（脱敏）→ 原样回传 POST」：两处凭据都退化为占位串。
    let mut submitted = base_config();
    let mut web = state.web.clone();
    web.api_token = REDACTED.to_string();
    submitted.web = Some(web);
    submitted.mqtt.password = Some(REDACTED.to_string());
    submitted.buses[0].protocol = "opcua".into();
    submitted.buses[0].opcua = Some(gateway_core::config::OpcUaClientConfig {
        auth: "username".into(),
        username: Some(REDACTED.into()),
        password: Some(REDACTED.into()),
        ..Default::default()
    });

    let resp = app
        .oneshot(req(
            "POST",
            "/api/config",
            serde_json::to_value(&submitted).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "POST /api/config 应保存成功（否则无法验证版本快照）"
    );

    // 版本快照必须记录真实凭据，而不是回传的 "********" 占位串。
    let versions = state
        .config
        .version_store
        .list_versions()
        .expect("list_versions");
    let latest = versions.first().expect("应写入版本快照");
    let snapshot = state
        .config
        .version_store
        .rollback(latest.id)
        .expect("rollback")
        .expect("version exists");

    let snap_web = snapshot.web.expect("快照应含 web 配置");
    assert_eq!(
        snap_web.api_token, TOKEN,
        "版本快照不得把脱敏占位串当作 api_token 落库"
    );
    assert_eq!(
        snapshot.mqtt.password.as_deref(),
        Some(REAL_MQTT_PW),
        "版本快照不得把脱敏占位串当作 mqtt 密码落库"
    );

    // 总线级 OPC UA 凭据：按 port_name 匹配还原，不得把占位串写进快照。
    let snap_opcua = snapshot.buses[0]
        .opcua
        .as_ref()
        .expect("快照应保留 buses[].opcua");
    assert_eq!(
        snap_opcua.username.as_deref(),
        Some(REAL_OPCUA_USER),
        "buses[].opcua.username 未被还原"
    );
    assert_eq!(
        snap_opcua.password.as_deref(),
        Some(REAL_OPCUA_PW),
        "buses[].opcua.password 未被还原"
    );
}
