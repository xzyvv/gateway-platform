//! 工程文件整包导入导出（决策表 #8）集成测试
//!
//! 「工程文件」= 三层可移植子集（总线 / 北向 / 变量映射）+ 自描述信封，
//! 与 `GET /api/config/export`（整份配置，含 web 凭据）刻意区分。
//! 须验证：
//! - 路由鉴权 fail-closed（无 token → 401）
//! - 导出信封含 kind / schema_version / 三层且 mqtt 非空
//! - 导入成功应用三层（并可通过内存配置读回）
//! - kind 不符 / schema_version 超前 → 400
//! - 导入**不篡改**本机 web 鉴权与凭据（工程文件不携带 web）
//! - 导入落审计 project.import 且可经 /api/audit/logs 读回

mod common;

mod project_io {
    use super::common::mock_state::mock_app_state;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use gateway_api::web::build_router;
    use std::sync::Arc;
    use tower::ServiceExt;

    const TOKEN: &str = "Bearer test-token-0123456789abcdef";

    async fn body_json(res: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    }

    fn export_req(token: Option<&str>) -> Request<Body> {
        let mut b = Request::builder().method("GET").uri("/api/project/export");
        if let Some(t) = token {
            b = b.header("authorization", t);
        }
        b.body(Body::empty()).unwrap()
    }

    fn import_req(body: &serde_json::Value, token: Option<&str>) -> Request<Body> {
        let mut b = Request::builder()
            .method("POST")
            .uri("/api/project/import")
            .header("content-type", "application/json");
        if let Some(t) = token {
            b = b.header("authorization", t);
        }
        b.body(Body::from(body.to_string())).unwrap()
    }

    /// 一个合法的工程文件：1 条总线 + 1 个 MQTT 北向 + 1 个点位
    fn project_body() -> serde_json::Value {
        serde_json::json!({
            "kind": "gateway-project",
            "schema_version": 1,
            "exported_at": 1_700_000_000i64,
            "buses": [{ "port_name": "/dev/ttyS0", "protocol": "modbus" }],
            "northbound": {
                "mqtt": { "enabled": false, "broker": "proj.broker", "port": 1883, "topic": "proj/topic" }
            },
            "mappings": {
                "points": [{
                    "id": "proj_p1", "name": "P1", "bus": "/dev/ttyS0",
                    "protocol": "modbus", "address": "1", "address_num": 1, "interval": 10
                }]
            }
        })
    }

    /// 无 token 导出 → 401（config 管理路由 require_auth fail-closed）
    #[tokio::test]
    async fn export_without_token_returns_401() {
        let app = build_router(mock_app_state());
        let res = app.oneshot(export_req(None)).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    /// 无 token 导入 → 401
    #[tokio::test]
    async fn import_without_token_returns_401() {
        let app = build_router(mock_app_state());
        let res = app
            .oneshot(import_req(&project_body(), None))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    /// 导出成功：信封含 kind/schema_version，三层齐备且 mqtt 非空
    #[tokio::test]
    async fn export_returns_project_envelope() {
        let app = build_router(mock_app_state());
        let res = app.oneshot(export_req(Some(TOKEN))).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        assert_eq!(v["code"], 0);
        let data = &v["data"];
        assert_eq!(data["kind"], "gateway-project");
        assert_eq!(data["schema_version"], 1);
        assert!(data["exported_at"].as_i64().unwrap_or(0) > 0);
        assert!(data["buses"].is_array());
        assert!(data["mappings"]["points"].is_array());
        // mqtt 为必填北向项，导出必须给出（非 null），否则导入端无法区分"缺省"与"清空"
        assert!(data["northbound"]["mqtt"].is_object(), "got {v}");
    }

    /// 导入成功：三层被覆盖，且可通过内存配置读回
    #[tokio::test]
    async fn import_applies_three_layers() {
        let state = mock_app_state();
        let app = build_router(state.clone());
        let res = app
            .oneshot(import_req(&project_body(), Some(TOKEN)))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        assert_eq!(v["code"], 0, "工程导入应成功, got {v}");

        let cfg = state.config.load_full();
        assert_eq!(cfg.buses.len(), 1, "总线层应被替换");
        assert_eq!(cfg.buses[0].port_name, "/dev/ttyS0");
        assert_eq!(cfg.points.len(), 1, "变量映射层应被替换");
        assert_eq!(cfg.points[0].id, "proj_p1");
        assert_eq!(cfg.mqtt.broker, "proj.broker", "北向层应被替换");
    }

    /// kind 不符（误传普通 JSON）→ 400
    #[tokio::test]
    async fn import_wrong_kind_returns_400() {
        let app = build_router(mock_app_state());
        let mut body = project_body();
        body["kind"] = serde_json::json!("not-a-project");
        let res = app.oneshot(import_req(&body, Some(TOKEN))).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    /// schema_version 超前（来自更新版本的文件）→ 400，不允许静默降级
    #[tokio::test]
    async fn import_future_schema_version_returns_400() {
        let app = build_router(mock_app_state());
        let mut body = project_body();
        body["schema_version"] = serde_json::json!(9999);
        let res = app.oneshot(import_req(&body, Some(TOKEN))).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    /// 安全性回归：工程文件不携带 web，故导入**不得**篡改本机鉴权/凭据。
    /// 载荷中即便塞入 web.api_token 字段，也必须被当作未知字段忽略。
    #[tokio::test]
    async fn import_does_not_touch_local_web_credentials() {
        let state = mock_app_state();
        {
            let mut cfg = state.config.load_full().as_ref().clone();
            let mut web = cfg.web.clone().unwrap();
            web.api_token = "local-secret-token".to_string();
            web.password_hash = "local-password-hash".to_string();
            cfg.web = Some(web);
            state.config.store(Arc::new(cfg));
        }
        let app = build_router(state.clone());

        let mut body = project_body();
        // 试图夹带本机凭据覆盖字段（工程文件无此字段 → 应被忽略）
        body["web"] = serde_json::json!({
            "api_token": "hacked-token",
            "password_hash": "hacked-hash"
        });
        // 同时把 ntp 也夹带进来，确认同属"本机配置"而不被覆盖
        body["ntp"] = serde_json::json!({ "server": "hacked.ntp" });

        let res = app
            .oneshot(import_req(&body, Some("Bearer local-secret-token")))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let cfg = state.config.load_full();
        let web = cfg.web.as_ref().expect("web config preserved");
        assert_eq!(
            web.api_token, "local-secret-token",
            "api_token 不得被工程文件篡改"
        );
        assert_eq!(
            web.password_hash, "local-password-hash",
            "password_hash 不得被工程文件篡改"
        );
        assert_ne!(
            cfg.ntp.as_ref().map(|n| n.server.as_str()),
            Some("hacked.ntp"),
            "ntp 属本机配置，不应被工程文件覆盖"
        );
    }

    fn repo_path(rel: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join(rel)
    }

    fn repo_json(rel: &str) -> serde_json::Value {
        let text = std::fs::read_to_string(repo_path(rel))
            .unwrap_or_else(|e| panic!("读取 {} 失败: {}", rel, e));
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("解析 {} 失败: {}", rel, e))
    }

    fn builtin_template(rel: &str) -> gateway_core::config::DeviceTemplate {
        serde_json::from_value(repo_json(rel)).expect("内置设备模板应可反序列化")
    }

    /// 交付验收包：两套默认工程模板必须能真实导入。
    ///
    /// 模板放在 gateway-web/public/delivery-templates 供现场从「交付验收」页下载，
    /// 这里复用工程导入端点做端到端校验，避免模板与配置/设备模板字段漂移。
    #[tokio::test]
    async fn delivery_templates_import_successfully() {
        let state = mock_app_state();
        // mock 环境的模板目录是空临时目录，先把内置设备模板灌进去。
        state
            .config
            .templates
            .save_custom(builtin_template(
                "templates/builtin/universal_modbus_power_meter.json",
            ))
            .expect("保存 Modbus 设备模板");
        state
            .config
            .templates
            .save_custom(builtin_template(
                "templates/builtin/universal_iec104_station.json",
            ))
            .expect("保存 IEC104 设备模板");

        let app = build_router(state.clone());
        for rel in [
            "gateway-web/public/delivery-templates/modbus-power-meter.json",
            "gateway-web/public/delivery-templates/iec104-station.json",
        ] {
            let body = repo_json(rel);
            let res = app
                .clone()
                .oneshot(import_req(&body, Some(TOKEN)))
                .await
                .unwrap();
            let status = res.status();
            let v = body_json(res).await;
            assert_eq!(status, StatusCode::OK, "{rel} 应可导入, got {v}");
            assert_eq!(v["code"], 0, "{rel} 导入失败: {v}");

            let cfg = state.config.load_full();
            assert_eq!(cfg.buses.len(), 1, "{rel} 应写入 1 条总线");
            let devices = cfg.devices.as_ref().expect("模板必须带设备");
            assert_eq!(devices.len(), 1, "{rel} 应写入 1 台设备");

            // 设备模板能展开出点位，且 IEC104 使用点位 IOA 而非设备地址
            let points = state
                .config
                .templates
                .expand_devices(devices)
                .unwrap_or_else(|e| panic!("{rel} 设备模板展开失败: {e}"));
            assert!(!points.is_empty(), "{rel} 展开点位数不应为 0");
            if devices[0].protocol == "iec104" {
                let addresses: Vec<&str> = points.iter().map(|p| p.address.as_str()).collect();
                assert!(
                    addresses.contains(&"1001") && addresses.contains(&"2001"),
                    "{rel} IEC104 点位应以 IOA 寻址，实际: {:?}",
                    addresses
                );
            }
        }
    }

    /// 导入应落审计 project.import（后台线程异步落盘，轮询等待最多 ~2s）
    #[tokio::test]
    async fn import_writes_queryable_audit() {
        let app = build_router(mock_app_state());
        let res = app
            .clone()
            .oneshot(import_req(&project_body(), Some(TOKEN)))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let mut found = false;
        for _ in 0..40 {
            let res = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri("/api/audit/logs?action=project.import")
                        .header("authorization", TOKEN)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::OK);
            let v = body_json(res).await;
            // P1-1：/api/audit/logs 已纳入统一 ApiResponse 信封
            if v["data"]["logs"]
                .as_array()
                .map(|a| a.iter().any(|e| e["action"] == "project.import"))
                .unwrap_or(false)
            {
                found = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(found, "工程导入应落审计 project.import");
    }
}
