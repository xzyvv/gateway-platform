//! 集成测试：关键路径端到端验证
//!
//! 覆盖不需要完整 AppState 的关键路径。
//! 通过 gateway_core::lib 引用源码模块或使用自包含逻辑。

mod common;

mod config_e2e {
    use super::common::*;

    /// 端到端：配置 JSON 解析 → 验证字段
    #[test]
    fn parse_config_json_and_validate_fields() {
        let json = r#"{
            "mqtt": { "enabled": false, "broker": "test.mqtt.com", "port": 1883, "topic": "test/topic" },
            "ntp": { "server": "pool.ntp.org", "sync_interval": 3600 },
            "buses": [],
            "points": []
        }"#;
        let path = write_temp_file("config.json", json);
        let content = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(v["mqtt"]["enabled"], false);
        assert_eq!(v["mqtt"]["broker"], "test.mqtt.com");
        assert_eq!(v["ntp"]["server"], "pool.ntp.org");
    }

    /// 端到端：通过 gateway_core::config 解析配置 → 验证字段
    #[test]
    fn parse_config_via_lib() {
        let json = r#"{
            "mqtt": { "enabled": false, "broker": "lib.test", "port": 1883, "topic": "t" },
            "ntp": { "server": "ntp.lib", "sync_interval": 3600 },
            "buses": [],
            "points": []
        }"#;
        let config: gateway_core::config::GatewayConfig = serde_json::from_str(json).unwrap();
        assert!(!config.mqtt.enabled);
        assert_eq!(config.mqtt.broker, "lib.test");
        assert_eq!(config.ntp.unwrap().server, "ntp.lib");
    }

    /// 端到端：缺字段的配置 JSON 可正常解析
    #[test]
    fn parse_minimal_config_json() {
        let json = r#"{
            "mqtt": { "enabled": false, "broker": "localhost", "port": 1883, "topic": "t" }
        }"#;
        let path = write_temp_file("config_min.json", json);
        let content = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(v["mqtt"]["enabled"], false);
        // 缺失的 buses/points 不影响 JSON 解析
        assert!(v.get("buses").is_none());
    }
}

mod sqlite_e2e {
    use super::common::*;

    /// 端到端：创建 SQLite DB → 写入 → 读取 → 验证
    #[test]
    fn sqlite_write_read_roundtrip() {
        let db_path = write_temp_file("test.sqlite", "");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute("CREATE TABLE test (id INTEGER, data TEXT)", [])
            .unwrap();
        conn.execute("INSERT INTO test VALUES (1, 'hello')", [])
            .unwrap();
        let result: String = conn
            .query_row("SELECT data FROM test WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(result, "hello");
    }

    /// 端到端：WAL checkpoint 不丢失数据
    #[test]
    fn wal_checkpoint_preserves_data() {
        let db_path = write_temp_file("wal_test.sqlite", "");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        conn.execute("CREATE TABLE queue (id INTEGER, payload TEXT)", [])
            .unwrap();
        for i in 0..100 {
            conn.execute(
                "INSERT INTO queue VALUES (?, ?)",
                rusqlite::params![i, format!("data{}", i)],
            )
            .unwrap();
        }
        // checkpoint
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
        // 验证数据完整
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM queue", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 100);
    }
}

mod ts_bindings_e2e {
    use std::fs;

    /// 端到端：ts-rs generated 文件存在且非空
    #[test]
    fn ts_bindings_files_exist() {
        let bindings_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../gateway-web/src/api/generated");
        if !bindings_dir.exists() {
            // CI 环境可能尚未生成
            return;
        }
        let ts_files: Vec<_> = fs::read_dir(&bindings_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "ts"))
            .collect();
        assert!(!ts_files.is_empty(), "generated/ should contain .ts files");
        for entry in &ts_files {
            let content = fs::read_to_string(entry.path()).unwrap();
            assert!(
                !content.is_empty(),
                "{} should not be empty",
                entry.path().display()
            );
        }
    }
}

// ============================================================
// M2-15: 鉴权中间件集成测试
// ============================================================
// 通过 mock_state 构造 AppState（4 个关键字段注入 mock），
// 调用 web::build_router 构造路由，用 tower::ServiceExt::oneshot 发请求。
// 验证：写入路由（POST /api/point-tools/batch）未携带 token 时返回 401。

mod auth_integration {
    use super::common::mock_state::mock_app_state;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use gateway_api::web::build_router;
    use tower::ServiceExt;

    /// 写入路由未携带 token 必须返回 401（require_auth 中间件 fail-closed）
    #[tokio::test]
    async fn write_route_without_token_returns_401() {
        let state = mock_app_state();
        let app = build_router(state);
        let req = Request::builder()
            .method("POST")
            .uri("/api/point-tools/batch")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    /// 写入路由携带正确 token 应放行进入 handler（不会 401）
    /// （handler 内部可能因 mock 行为返回其他状态码，但不应是 401）
    #[tokio::test]
    async fn write_route_with_valid_token_does_not_return_401() {
        let state = mock_app_state();
        let app = build_router(state.clone());
        let req = Request::builder()
            .method("POST")
            .uri("/api/point-tools/batch")
            .header("authorization", "Bearer test-token-0123456789abcdef")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_ne!(
            res.status(),
            StatusCode::UNAUTHORIZED,
            "valid token should not be rejected by auth middleware"
        );
    }

    /// 错误 token 必须返回 401（恒定时间比较后拒绝）
    #[tokio::test]
    async fn write_route_with_wrong_token_returns_401() {
        let state = mock_app_state();
        let app = build_router(state);
        let req = Request::builder()
            .method("POST")
            .uri("/api/point-tools/batch")
            .header("authorization", "Bearer wrong-token-xxxxxxxxxxxx")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }
}

// ============================================================
// 批量查找替换：配置未能落地时不得返回「假成功」
// ============================================================

mod find_replace_failure {
    use super::common::mock_state::mock_app_state;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use gateway_api::web::build_router;
    use gateway_core::config::ConfigLoader;
    use std::sync::Arc;
    use tower::ServiceExt;

    /// 回归缺陷维度：handler 内 `let _ = reload_points(...)` 吞掉错误——保存/校验/license
    /// 配额任一步失败时，接口仍返回 200 + `updated: N`，前端提示「已替换 N 处」，而
    /// 磁盘与运行时配置一处未改（磁盘满、只读 rootfs、配额超限时都会命中）。
    ///
    /// 复现方式：把配置落地路径指向一个非法路径（父目录是普通文件），使
    /// `ConfigLoader::save` 必定失败，断言接口返回 500 而非「替换成功」。
    #[tokio::test]
    async fn apply_reports_error_when_config_cannot_be_saved() {
        // 造一个「父路径为普通文件」的非法配置路径：写盘必然失败
        let tmp = tempfile::tempdir().expect("create temp dir");
        let blocker = tmp.path().join("not_a_dir");
        std::fs::write(&blocker, b"not a directory").unwrap();
        let bad_path = blocker.join("config.json");

        let mut state = mock_app_state();
        Arc::get_mut(&mut state)
            .expect("mock_app_state 刚创建，尚无其他持有者")
            .config
            .config_loader = ConfigLoader::new(bad_path.to_str().unwrap());
        let app = build_router(state);

        let req = Request::builder()
            .method("POST")
            .uri("/api/point-tools/find-replace/apply")
            .header("authorization", "Bearer test-token-0123456789abcdef")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"ids":["p1"],"field":"name","from":"a","to":"b"}"#,
            ))
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(
            res.status(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "配置未落地成功时必须回错误（修复前此处为 200 + updated 计数，即假成功）"
        );
    }
}

// ============================================================
// 回归：uplink 路由组鉴权（此前整组漏挂中间件，无 token 即可写配置/发测试上行）
// ============================================================

mod uplink_auth_integration {
    use super::common::mock_state::mock_app_state;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use gateway_api::web::build_router;
    use tower::ServiceExt;

    /// 无 token 写上行配置必须 401（require_auth）
    #[tokio::test]
    async fn uplink_config_write_without_token_returns_401() {
        let state = mock_app_state();
        let app = build_router(state);
        let req = Request::builder()
            .method("POST")
            .uri("/api/uplink/config")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    /// 无 token 触发上行测试（send=true 会向 MQTT 发布）必须 401
    #[tokio::test]
    async fn uplink_test_without_token_returns_401() {
        let state = mock_app_state();
        let app = build_router(state);
        let req = Request::builder()
            .method("POST")
            .uri("/api/uplink/test")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    /// 无 token 导入上行模板必须 401
    #[tokio::test]
    async fn uplink_template_import_without_token_returns_401() {
        let state = mock_app_state();
        let app = build_router(state);
        let req = Request::builder()
            .method("POST")
            .uri("/api/uplink/template/import")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    /// 无 token 清空上行测试历史必须 401
    #[tokio::test]
    async fn uplink_clear_history_without_token_returns_401() {
        let state = mock_app_state();
        let app = build_router(state);
        let req = Request::builder()
            .method("DELETE")
            .uri("/api/uplink/test/history")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    /// 携带正确 token 的写请求不应被 401 拦截（进入 handler，状态码由 mock 决定）
    #[tokio::test]
    async fn uplink_write_with_valid_token_not_401() {
        let state = mock_app_state();
        let app = build_router(state);
        let req = Request::builder()
            .method("POST")
            .uri("/api/uplink/config")
            .header("authorization", "Bearer test-token-0123456789abcdef")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_ne!(
            res.status(),
            StatusCode::UNAUTHORIZED,
            "valid token should not be rejected by auth middleware"
        );
    }

    /// 读接口在 allow_anon_read=true（mock 默认）下匿名 GET 不应 401
    #[tokio::test]
    async fn uplink_read_anon_not_401() {
        let state = mock_app_state();
        let app = build_router(state);
        let req = Request::builder()
            .method("GET")
            .uri("/api/uplink/config")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_ne!(
            res.status(),
            StatusCode::UNAUTHORIZED,
            "anon read enabled: GET should pass auth middleware"
        );
    }
}

// ============================================================
// 回归：配置敏感字段脱敏（P0-2 / P0-3）
// GET /api/config 与 /api/config/versions/:id 必须脱敏明文凭据；
// 保存路径必须把前端回传的占位值还原为真实值（不触碰即保留），
// 用户清空字段才是真正清除。
// ============================================================

mod config_redaction_integration {
    use super::common::mock_state::mock_app_state;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use gateway_api::web::build_router;
    use std::sync::Arc;
    use tower::ServiceExt;

    const TOKEN: &str = "test-token-0123456789abcdef";
    const REDACTED: &str = "\"********\"";

    async fn body_string(res: axum::response::Response) -> String {
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn post_config_req(cfg_json: String) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/api/config")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {}", TOKEN))
            .body(Body::from(cfg_json))
            .unwrap()
    }

    /// P0-2：GET /api/config 不得泄露 mqtt 明文凭据
    /// （`/api/config` 属敏感读路径：即便匿名只读开启也必须带 token，此处带主 token 读）
    #[tokio::test]
    async fn get_config_redacts_mqtt_credentials() {
        let state = mock_app_state();
        {
            let mut cfg = state.config.load_full().as_ref().clone();
            cfg.mqtt.username = Some("mqtt-user".to_string());
            cfg.mqtt.password = Some("mqtt-pass-s3cret".to_string());
            state.config.store(Arc::new(cfg));
        }
        let app = build_router(state);
        let req = Request::builder()
            .method("GET")
            .uri("/api/config")
            .header("authorization", format!("Bearer {}", TOKEN))
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_string(res).await;
        assert!(body.contains(REDACTED), "脱敏占位值缺失，响应: {}", body);
        assert!(
            !body.contains("mqtt-pass-s3cret"),
            "mqtt 密码泄露: {}",
            body
        );
        assert!(!body.contains("mqtt-user"), "mqtt 用户名泄露: {}", body);
    }

    /// P2-2：GET /api/config 必须使用 ApiResponse 信封（code/message/data），
    /// 不再是裸配置对象。旧实现顶层直接是 config，前端/脚本得靠"有没有 code"
    /// 猜是否解包，与其余接口不一致。
    #[tokio::test]
    async fn get_config_returns_api_response_envelope() {
        let state = mock_app_state();
        let app = build_router(state);
        let req = Request::builder()
            .method("GET")
            .uri("/api/config")
            .header("authorization", format!("Bearer {}", TOKEN))
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = body_string(res).await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["code"], 0, "成功响应 code 应为 0, got {v}");
        assert_eq!(v["message"], "success");
        assert!(v["data"].is_object(), "data 应为配置对象, got {v}");
        assert!(
            v["data"].get("buses").is_some(),
            "data 应包含配置字段 buses, got {v}"
        );
        assert!(
            v.get("buses").is_none(),
            "顶层不应再是裸配置对象（回归点）, got {v}"
        );
    }

    /// BUG-C/D：axum 提取器（Query / Json）拒绝必须走统一信封。
    ///
    /// 原实现里 `/api/audit/logs?limit=abc` 返回 400 + `text/plain`
    /// "Failed to deserialize query string: ..."，前端读到的是裸字符串：
    /// 没有 message 字段、没有业务码，错误提示只能显示 "Request failed with status 400"。
    /// 修复后 HTTP 状态码保持 400，body 改为 `{code, message, error}`。
    #[tokio::test]
    async fn extractor_rejection_uses_error_envelope() {
        let state = mock_app_state();
        let app = build_router(state);
        let req = Request::builder()
            .method("GET")
            .uri("/api/audit/logs?limit=abc")
            .header("authorization", format!("Bearer {}", TOKEN))
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(
            res.status(),
            StatusCode::BAD_REQUEST,
            "非法查询参数应保持 400"
        );
        let ct = res
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        assert!(
            ct.starts_with("application/json"),
            "提取器拒绝的 body 必须是 JSON 信封, got content-type={ct}"
        );
        let body = body_string(res).await;
        let v: serde_json::Value = serde_json::from_str(&body)
            .unwrap_or_else(|_| panic!("响应必须可解析为 JSON, got {body}"));
        assert_eq!(v["code"], 4000, "参数错误业务码应为 4000, got {v}");
        assert_eq!(v["message"], v["error"], "message 与 error 应一致, got {v}");
        // 注意：axum 的 Query 提取器走 serde_urlencoded，错误只带 serde 原文
        // （如 "invalid digit found in string"），不含字段名（JSON body 走
        // serde_path_to_error 才带路径）。这里只能断言「保留了具体失败原因」。
        assert!(
            v["message"]
                .as_str()
                .is_some_and(|m| !m.is_empty() && m.contains("invalid digit")),
            "message 应保留提取器的具体失败原因, got {v}"
        );
        assert!(
            !body.contains("Failed to deserialize query string"),
            "不应再透出 axum 原始文案, got {body}"
        );
    }

    /// P0-3：版本快照同样脱敏（快照可能来自 export 等带明文的内容）
    #[tokio::test]
    async fn config_version_snapshot_redacts_secrets() {
        let state = mock_app_state();
        let mut cfg = state.config.load_full().as_ref().clone();
        cfg.mqtt.username = Some("snap-user".to_string());
        cfg.mqtt.password = Some("snap-pass-s3cret".to_string());
        let body = serde_json::to_string(&cfg).unwrap();
        let app = build_router(state.clone());
        let res = app.oneshot(post_config_req(body)).await.unwrap();
        assert_eq!(
            res.status(),
            StatusCode::OK,
            "POST /api/config 应成功以创建快照"
        );
        // 快照读同属敏感读路径（必须带 token），且响应必须脱敏
        let app = build_router(state);
        let req = Request::builder()
            .method("GET")
            .uri("/api/config/versions/1")
            .header("authorization", format!("Bearer {}", TOKEN))
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        let body = body_string(res).await;
        assert!(
            !body.contains("snap-pass-s3cret"),
            "快照 mqtt 密码泄露: {}",
            body
        );
        assert!(body.contains(REDACTED), "快照脱敏占位值缺失: {}", body);
    }

    /// 保存路径还原占位值：前端不触碰敏感字段（回传 ********）时保留真实值
    #[tokio::test]
    async fn post_config_restores_redacted_secrets() {
        let state = mock_app_state();
        {
            let mut cfg = state.config.load_full().as_ref().clone();
            cfg.mqtt.username = Some("real-user".to_string());
            cfg.mqtt.password = Some("real-pass-s3cret".to_string());
            state.config.store(Arc::new(cfg));
        }
        let mut posted = state.config.load_full().as_ref().clone();
        posted.mqtt.username = Some("********".to_string());
        posted.mqtt.password = Some("********".to_string());
        let body = serde_json::to_string(&posted).unwrap();
        let app = build_router(state.clone());
        let res = app.oneshot(post_config_req(body)).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK, "回传占位值应保存成功");
        let saved = state.config.load_full();
        assert_eq!(saved.mqtt.username.as_deref(), Some("real-user"));
        assert_eq!(saved.mqtt.password.as_deref(), Some("real-pass-s3cret"));
    }

    /// 用户清空字段（MQTT 匿名连接语义）必须真正保存空值，不能被还原
    #[tokio::test]
    async fn post_config_with_cleared_secret_keeps_clear() {
        let state = mock_app_state();
        {
            let mut cfg = state.config.load_full().as_ref().clone();
            cfg.mqtt.password = Some("real-pass-s3cret".to_string());
            state.config.store(Arc::new(cfg));
        }
        let mut posted = state.config.load_full().as_ref().clone();
        posted.mqtt.password = None;
        let body = serde_json::to_string(&posted).unwrap();
        let app = build_router(state.clone());
        let res = app.oneshot(post_config_req(body)).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK, "清空密码应保存成功");
        let saved = state.config.load_full();
        assert!(
            saved.mqtt.password.is_none(),
            "清空密码被错误还原: {:?}",
            saved.mqtt.password
        );
    }
}

// ============================================================
// D1: 配置结构校验覆盖 —— 增量端点也必须 fail-fast
// ============================================================
// 回归：仅 POST /api/config 校验不够，批量导入等增量保存路径也必须在
// 落盘前拒绝非法配置（否则坏配置进入调度器，运行期才暴露）。

mod validation_integration {
    use super::common::mock_state::mock_app_state;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use gateway_api::web::build_router;
    use tower::ServiceExt;

    /// 批量导入一个 protocol 为空的非法点位 → 结构校验拦截，返回 400（不得 2xx 落盘）
    #[tokio::test]
    async fn batch_import_rejects_invalid_point_with_400() {
        let state = mock_app_state();
        let app = build_router(state);
        // protocol 为空 → validate_protocol 拒绝；其余为 PointConfig 必填字段
        let body = serde_json::json!([{
            "id": "bad_point",
            "name": "bad_point",
            "bus": "bus0",
            "protocol": "",
            "address": "0",
            "interval": 10
        }]);
        let req = Request::builder()
            .method("POST")
            .uri("/api/point-tools/batch")
            .header("authorization", "Bearer test-token-0123456789abcdef")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(
            res.status(),
            StatusCode::BAD_REQUEST,
            "非法配置应被结构校验 fail-fast 拒绝"
        );
    }
}

// ============================================================
// D3: 审计日志覆盖 —— 敏感操作必须写入真实 AuditLog 且可查询
// ============================================================
// 回归：原先 web 层仅调用 tracing 桩 `audit_log_imported()`，真实 AuditLog
// 从不落盘，`/api/audit/logs` 恒为空。现敏感操作（OTA / 配置变更）统一写入
// AuditLog，须能通过查询端点读回。

mod audit_integration {
    use super::common::mock_state::{mock_app_state, mock_app_state_with_buses};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use gateway_api::web::build_router;
    use gateway_core::config::BusConfig;
    use tower::ServiceExt;

    const TOKEN: &str = "Bearer test-token-0123456789abcdef";

    async fn body_json(res: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(res.into_body(), 65536).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// OTA 回滚成功后，审计查询端点应能读回 action=ota.rollback 的记录
    #[tokio::test]
    async fn ota_rollback_writes_queryable_audit_entry() {
        let state = mock_app_state();
        let app = build_router(state);

        // 1) 执行敏感操作：OTA 回滚（mock OTA 返回 Ok，不触碰磁盘）
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/ota/rollback")
                    .header("authorization", TOKEN)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // 2) 查询审计日志，必须读回该记录（证明写入的是真实 AuditLog，而非 tracing 桩）
        let res = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/audit/logs?action=ota.rollback")
                    .header("authorization", TOKEN)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        // P1-1：/api/audit/logs 已纳入统一 ApiResponse 信封
        let logs = v["data"]["logs"].as_array().expect("logs array");
        assert!(
            logs.iter().any(|e| e["action"] == "ota.rollback"),
            "审计查询应返回 ota.rollback 记录, got {v}"
        );
    }

    /// 删除设备（级联删测点）成功后，审计查询端点应能读回 action=config.delete 记录。
    /// 回归点：原 delete_device 只打 tracing::warn，审计日志中无删除痕迹（E1）。
    #[tokio::test]
    async fn device_delete_writes_queryable_audit_entry() {
        // N1：设备总线必须真实存在于配置里，这里预置一条 "b1" 供创建使用。
        let state = mock_app_state_with_buses(vec![BusConfig {
            port_name: "b1".to_string(),
            ..BusConfig::default()
        }]);
        let app = build_router(state);

        // 1) 先建一台设备（mock 配置初始 devices 为空，无设备可删）
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/devices")
                    .header("authorization", TOKEN)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"id":"dev_e1","name":"E1","template":"t1","bus":"b1","protocol":"modbus","address":"1","interval":1000}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        assert_eq!(v["code"], 0, "创建设备应成功, got {v}");

        // 2) 删除该设备（触发级联清理逻辑与审计落库）
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/devices/dev_e1")
                    .header("authorization", TOKEN)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        assert_eq!(v["code"], 0, "删除设备应成功, got {v}");

        // 3) 审计查询端点应能读回该记录（证明 config.delete 走真实 AuditLog）
        let res = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/audit/logs?action=config.delete")
                    .header("authorization", TOKEN)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        // P1-1：/api/audit/logs 已纳入统一 ApiResponse 信封
        let logs = v["data"]["logs"].as_array().expect("logs array");
        assert!(
            logs.iter().any(|e| e["action"] == "config.delete"),
            "审计查询应返回 config.delete 记录, got {v}"
        );
    }
}

// ============================================================
// 模板引擎集成测试：渲染产物必须可被 JSON 解析
// ============================================================
// 单元测试已覆盖字符串相等性；这里关注端到端语义：渲染输出能被 serde_json
// 正确解析为 JSON，确保上行 MQTT payload 不会因模板缺陷变成非法 JSON。

mod template_e2e {
    use gateway_core::north::template::{render_str, ExtraValue, TemplateEngine, TemplateVars};

    /// 端到端：ThingsBoard 风格 payload 渲染产物可被 serde_json 解析，字段值正确
    #[test]
    fn render_thingsboard_payload_produces_valid_json() {
        let template = r#"{"ts":${timestamp_ms},"values":{"${point_id}":${value}}}"#;
        let engine = TemplateEngine::new(template).unwrap();
        let mut vars = TemplateVars::new();
        vars.timestamp_ms = 1727936400000;
        vars.point_id = "voltage_a".into();
        vars.value = 220.5;

        let out = engine.render(&vars);
        let parsed: serde_json::Value =
            serde_json::from_str(&out).expect("rendered output must be valid JSON");
        assert_eq!(parsed["ts"], 1727936400000i64);
        assert_eq!(parsed["values"]["voltage_a"], 220.5);
    }

    /// 端到端：render_str 包装的字符串渲染后必须是合法 JSON 字符串字面量，
    /// 且反序列化结果等于原始字符串（覆盖引号 / 反斜杠 / 控制字符三类转义）
    #[test]
    fn render_str_escapes_produce_valid_json_string() {
        let raw = "a\"b\\c\nd\te"; // 包含 " \ \n \t
        let escaped = render_str(raw);
        let parsed: serde_json::Value =
            serde_json::from_str(&escaped).expect("render_str output must be valid JSON");
        assert_eq!(parsed.as_str().unwrap(), raw);
    }

    /// 端到端：InfluxDB 行协议（无引号包裹的 tag/field value）渲染产物不被 JSON 引号污染
    #[test]
    fn render_influxdb_line_protocol_payload() {
        let template =
            "measurement,device=${device_id},point=${point_id} value=${value} ${timestamp_ms}";
        let engine = TemplateEngine::new(template).unwrap();
        let mut vars = TemplateVars::new();
        vars.device_id = "m1".into();
        vars.point_id = "v".into();
        vars.value = 12.5;
        vars.timestamp_ms = 1700000000000;

        let out = engine.render(&vars);
        assert_eq!(
            out,
            "measurement,device=m1,point=v value=12.5 1700000000000"
        );
    }

    /// 端到端：extra 字段（字符串 + 数字）同时渲染并解析为合法 JSON
    #[test]
    fn render_extra_fields_round_trip() {
        let template = r#"{"line":"${line}","port":${port}}"#;
        let engine = TemplateEngine::new(template).unwrap();
        let vars = TemplateVars::new()
            .with_extra("line", ExtraValue::Str("L1".into()))
            .with_extra("port", ExtraValue::Num(502.0));

        let out = engine.render(&vars);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["line"], "L1");
        assert_eq!(parsed["port"], 502);
    }

    /// 端到端：render_bytes 与 render 输出一致（MQTT publish 走 bytes 路径）
    #[test]
    fn render_bytes_matches_render_for_complex_payload() {
        let template = r#"{"device":"${device_id}","value":${value},"ts":${timestamp_ms}}"#;
        let engine = TemplateEngine::new(template).unwrap();
        let mut vars = TemplateVars::new();
        vars.device_id = "m1".into();
        vars.value = 42.0;
        vars.timestamp_ms = 1000;

        let s = engine.render(&vars);
        let b = engine.render_bytes(&vars);
        assert_eq!(s.as_bytes(), b.as_slice());
    }
}

// ============================================================
// 密码哈希集成测试：PBKDF2 往返 + 异常输入 fail-closed
// ============================================================

mod crypto_password_e2e {
    use gateway_core::crypto::password::{hash_password, verify_password};

    /// 哈希字符串格式符合 PHC 约定：pbkdf2$<iter>$<salt_b64>$<hash_b64>
    #[test]
    fn hash_password_produces_phc_format() {
        let stored = hash_password("any-password");
        assert!(stored.starts_with("pbkdf2$100000$"));
        let parts: Vec<&str> = stored.split('$').collect();
        assert_eq!(parts.len(), 4);
        assert!(!parts[2].is_empty(), "salt should not be empty");
        assert!(!parts[3].is_empty(), "hash should not be empty");
    }

    /// 同一明文两次哈希应产生不同结果（盐随机），但都能验证通过
    #[test]
    fn hash_password_salt_is_random() {
        let p1 = hash_password("same-password");
        let p2 = hash_password("same-password");
        assert_ne!(p1, p2, "salt should differ between calls");
        assert!(verify_password("same-password", &p1));
        assert!(verify_password("same-password", &p2));
    }

    /// 错误密码必须被拒绝
    #[test]
    fn verify_rejects_wrong_password() {
        let stored = hash_password("correct-password");
        assert!(!verify_password("wrong-password", &stored));
    }

    /// 畸形哈希字符串必须 fail-closed（返回 false，不 panic）
    #[test]
    fn verify_rejects_malformed_hash_without_panic() {
        // 非法 schema 前缀
        assert!(!verify_password("anything", "bcrypt$100$abc$yyy"));
        // parts 不足
        assert!(!verify_password("anything", "pbkdf2$100$abc"));
        // iter 非数字
        assert!(!verify_password("anything", "pbkdf2$abc$xxx$yyy"));
        // 空字符串
        assert!(!verify_password("anything", ""));
    }
}

// ============================================================
// 只读路由匿名访问集成测试：allow_anon_read=true 的副作用边界
// ============================================================
// 现有 auth_integration 覆盖了写入路由 401/放行；这里聚焦"只读模式"
// 的边界：allow_anon_read=true 时，匿名 GET 读路由不被 401 拦截。

mod readonly_routes_e2e {
    use super::common::mock_state::mock_app_state;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use gateway_api::web::build_router;
    use tower::ServiceExt;

    /// allow_anon_read=true 时，无 token GET /api/version 应放行（不返回 401）
    /// handler 内部可能因 mock 行为返回其他状态码，但不应是 401
    #[tokio::test]
    async fn anonymous_get_read_route_passes_auth() {
        let state = mock_app_state();
        let app = build_router(state);
        let req = Request::builder()
            .method("GET")
            .uri("/api/version")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_ne!(
            res.status(),
            StatusCode::UNAUTHORIZED,
            "allow_anon_read=true → anonymous GET should pass auth middleware"
        );
    }
}

// ============================================================
// 点位写接口（POST /api/point-tools/write）集成测试
// ============================================================
// 第 1 期冗余收口：原 `POST /api/points/:id/write` 与 `POST /api/point-tools/write-batch`
// 已删除，写入统一走 `POST /api/point-tools/write`（1 条走单写、≥2 条走批写）。
// 须验证：写路由鉴权 fail-closed、点位/collector 不存在返回 404、
// 写成功返回 {status:"ok"}、写失败返回 code=5101、并落审计 point.write。

mod point_write_integration {
    use super::common::mock_state::{
        mock_app_state, mock_app_state_with_collectors, MockCollector,
    };
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use gateway_api::web::{build_router, AppState};
    use gateway_core::config::PointConfig;
    use gateway_core::south::protocol::Collector;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tower::ServiceExt;

    const TOKEN: &str = "Bearer test-token-0123456789abcdef";

    /// 构造一个位于 bus0 的合法点位
    fn test_point() -> PointConfig {
        PointConfig {
            id: "p1".to_string(),
            name: Some("P1".to_string()),
            bus: "bus0".to_string(),
            protocol: "modbus".to_string(),
            address: "1".to_string(),
            interval: 10,
            // S11: 测试点位默认允许写入（白名单拒绝用例见 write_non_whitelisted_point_returns_400）
            writable: true,
            ..Default::default()
        }
    }

    /// 把点位写入内存配置（mock 默认 points 为空）
    fn store_point(state: &Arc<AppState>, point: PointConfig) {
        let mut cfg = state.config.load_full().as_ref().clone();
        cfg.points.push(point);
        state.config.store(Arc::new(cfg));
    }

    /// 单条写入请求（统一写入口，writes 长度 1 → 单写分支）
    fn write_req(point_id: &str, value: f64, token: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/api/point-tools/write")
            .header("content-type", "application/json");
        if let Some(t) = token {
            builder = builder.header("authorization", t);
        }
        builder
            .body(Body::from(format!(
                "{{\"writes\":[{{\"id\":\"{}\",\"value\":{}}}]}}",
                point_id, value
            )))
            .unwrap()
    }

    async fn body_string(res: axum::response::Response) -> String {
        let bytes = axum::body::to_bytes(res.into_body(), 65536).await.unwrap();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    async fn body_json(res: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(res.into_body(), 65536).await.unwrap();
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    }

    /// 无 token → 401（写路由 require_auth fail-closed）
    #[tokio::test]
    async fn write_without_token_returns_401() {
        let app = build_router(mock_app_state());
        let res = app.oneshot(write_req("p1", 1.0, None)).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    /// 携带正确 token，但目标点位不存在 → 404（handler 校验点位存在性）
    #[tokio::test]
    async fn write_unknown_point_returns_404() {
        let app = build_router(mock_app_state());
        let res = app
            .oneshot(write_req("nope", 1.0, Some(TOKEN)))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        let body = body_string(res).await;
        assert!(body.contains("nope"), "应提示点位不存在: {body}");
    }

    /// 携带 token，点位存在但所属总线无 collector → 404
    #[tokio::test]
    async fn write_point_without_collector_returns_404() {
        let state = mock_app_state();
        store_point(&state, test_point());
        let app = build_router(state);
        let res = app
            .oneshot(write_req("p1", 1.0, Some(TOKEN)))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        let body = body_string(res).await;
        assert!(body.contains("bus0"), "应提示 collector 缺失: {body}");
    }

    /// 携带 token，点位存在且 collector 写成功 → 200 {status:"ok"}
    #[tokio::test]
    async fn write_success_returns_status_ok() {
        let mut collectors: HashMap<String, Arc<dyn Collector>> = HashMap::new();
        collectors.insert("bus0".to_string(), Arc::new(MockCollector::new("bus0")));
        let state = mock_app_state_with_collectors(collectors);
        store_point(&state, test_point());
        let app = build_router(state);
        let res = app
            .oneshot(write_req("p1", 42.5, Some(TOKEN)))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        // 统一写入口已纳入统一 ApiResponse 信封
        let v = &v["data"];
        assert_eq!(v["status"], "ok");
        assert_eq!(v["total"], 1);
        assert_eq!(v["succeeded"], 1);
        assert_eq!(v["results"][0]["point_id"], "p1");
        assert_eq!(v["results"][0]["value"], 42.5);
    }

    /// collector 写失败 → HTTP 仍返回 200（业务失败不映射为 HTTP 错误），
    /// 但统一信封给出 code=5101 + data=null（D-04 起与 `/api/devices/control/send`
    /// 走同一条链路，失败码由 send_command 统一返回）
    #[tokio::test]
    async fn write_failure_returns_status_error() {
        let mut collectors: HashMap<String, Arc<dyn Collector>> = HashMap::new();
        collectors.insert(
            "bus0".to_string(),
            Arc::new(MockCollector::new("bus0").with_write_result(false)),
        );
        let state = mock_app_state_with_collectors(collectors);
        store_point(&state, test_point());
        let app = build_router(state);
        let res = app
            .oneshot(write_req("p1", 1.0, Some(TOKEN)))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        assert_eq!(v["code"], 5101);
        assert!(v["data"].is_null(), "写失败时不应带 data: {v}");
    }

    /// S11: 未标记 writable 的点位拒绝写入（400），不得下发到设备
    #[tokio::test]
    async fn write_non_whitelisted_point_returns_400() {
        let mut collectors: HashMap<String, Arc<dyn Collector>> = HashMap::new();
        collectors.insert("bus0".to_string(), Arc::new(MockCollector::new("bus0")));
        let state = mock_app_state_with_collectors(collectors);
        let mut p = test_point();
        p.writable = false;
        store_point(&state, p);
        let app = build_router(state);
        let res = app
            .oneshot(write_req("p1", 1.0, Some(TOKEN)))
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            StatusCode::BAD_REQUEST,
            "白名单外点位必须拒绝写入"
        );
    }

    /// S11: 批量写入中未标记 writable 的点位记为该项失败，其余点位不受影响
    #[tokio::test]
    async fn batch_write_marks_non_whitelisted_as_failed() {
        let mut collectors: HashMap<String, Arc<dyn Collector>> = HashMap::new();
        collectors.insert("bus0".to_string(), Arc::new(MockCollector::new("bus0")));
        let state = mock_app_state_with_collectors(collectors);
        store_point(&state, test_point()); // p1: writable=true
        let mut p2 = test_point();
        p2.id = "p2".to_string();
        p2.writable = false;
        store_point(&state, p2);
        let app = build_router(state);
        let req = Request::builder()
            .method("POST")
            .uri("/api/point-tools/write")
            .header("content-type", "application/json")
            .header("authorization", TOKEN)
            .body(Body::from(
                "{\"writes\":[{\"id\":\"p1\",\"value\":1.0},{\"id\":\"p2\",\"value\":2.0}]}",
            ))
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        // 统一写入口已纳入统一 ApiResponse 信封
        let v = &v["data"];
        assert_eq!(v["succeeded"], 1);
        assert_eq!(v["failed"], 1);
        assert_eq!(v["results"][0]["status"], "ok");
        assert_eq!(v["results"][1]["status"], "error");
    }

    /// S11: 远程控制接口对白名单外点位返回 400
    #[tokio::test]
    async fn send_command_rejects_non_whitelisted_point() {
        let mut collectors: HashMap<String, Arc<dyn Collector>> = HashMap::new();
        collectors.insert("bus0".to_string(), Arc::new(MockCollector::new("bus0")));
        let state = mock_app_state_with_collectors(collectors);
        let mut p = test_point();
        p.writable = false;
        store_point(&state, p);
        let app = build_router(state);
        let req = Request::builder()
            .method("POST")
            .uri("/api/devices/control/send")
            .header("content-type", "application/json")
            .header("authorization", TOKEN)
            .body(Body::from("{\"point_id\":\"p1\",\"value\":1.0}"))
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    /// 写成功必须落审计 point.write，并可经 /api/audit/logs 读回
    #[tokio::test]
    async fn write_success_logs_queryable_audit() {
        let mut collectors: HashMap<String, Arc<dyn Collector>> = HashMap::new();
        collectors.insert("bus0".to_string(), Arc::new(MockCollector::new("bus0")));
        let state = mock_app_state_with_collectors(collectors);
        store_point(&state, test_point());
        let app = build_router(state);

        let res = app
            .clone()
            .oneshot(write_req("p1", 7.0, Some(TOKEN)))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // 审计由后台线程异步落盘（AuditLog::log 仅入队），此处轮询等待最多 ~2s；
        // 若直接查询会与写入线程产生竞态（首查可能为空）。
        let mut found = false;
        for _ in 0..40 {
            let res = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri("/api/audit/logs?action=point.write")
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
                .map(|a| a.iter().any(|e| e["action"] == "point.write"))
                .unwrap_or(false)
            {
                found = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(found, "写点位应落审计 point.write");
    }
}

// ============================================================
// 配置备份接口（POST /api/config/backup）集成测试
// ============================================================
// 回归：调试 WEB 计划任务的「备份配置」（scheduledTasks.ts → POST /api/config/backup）
// 此前后端无任何 backup 路由，请求必然 404（且被 try/catch 静默吞掉）。
// 现补 backup_config（在网关侧落一个可回滚的版本快照），须验证：
// 写路由鉴权 fail-closed、返回 version_id、快照出现在 /api/config/versions、
// 且落审计 config.backup。

mod config_backup_integration {
    use super::common::mock_state::mock_app_state;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use gateway_api::web::build_router;
    use tower::ServiceExt;

    const TOKEN: &str = "Bearer test-token-0123456789abcdef";

    fn backup_req(token: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().method("POST").uri("/api/config/backup");
        if let Some(t) = token {
            builder = builder.header("authorization", t);
        }
        builder.body(Body::empty()).unwrap()
    }

    async fn body_json(res: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(res.into_body(), 65536).await.unwrap();
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    }

    /// 无 token → 401（config 管理写路由 require_auth fail-closed）
    #[tokio::test]
    async fn backup_without_token_returns_401() {
        let app = build_router(mock_app_state());
        let res = app.oneshot(backup_req(None)).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    /// 正常备份 → 200，data 带自增 version_id（证明快照已落盘）
    #[tokio::test]
    async fn backup_success_returns_version_id() {
        let app = build_router(mock_app_state());
        let res = app.oneshot(backup_req(Some(TOKEN))).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        assert_eq!(v["code"], 0, "备份应成功, got {v}");
        assert_eq!(v["data"]["status"], "ok");
        let vid = v["data"]["version_id"].as_i64().expect("version_id");
        assert!(vid > 0, "version_id 应为自增正整数, got {vid}");
    }

    /// 备份后，快照应出现在 /api/config/versions 列表且被标记为当前版本
    #[tokio::test]
    async fn backup_creates_listed_version_snapshot() {
        let app = build_router(mock_app_state());

        let res = app.clone().oneshot(backup_req(Some(TOKEN))).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let res = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/config/versions")
                    .header("authorization", TOKEN)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        // P1-1：/api/config/versions 已纳入统一 ApiResponse 信封
        let versions = v["data"]["versions"].as_array().expect("versions array");
        assert!(
            versions.iter().any(|e| e["message"] == "manual backup"),
            "备份应生成 note=manual backup 的版本快照, got {v}"
        );
        assert_eq!(
            versions[0]["is_current"], true,
            "最新版本应被标记 is_current"
        );
    }

    /// 备份应落审计 config.backup（后台线程异步落盘，轮询等待最多 ~2s）
    #[tokio::test]
    async fn backup_writes_queryable_audit() {
        let app = build_router(mock_app_state());
        let res = app.clone().oneshot(backup_req(Some(TOKEN))).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let mut found = false;
        for _ in 0..40 {
            let res = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri("/api/audit/logs?action=config.backup")
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
                .map(|a| a.iter().any(|e| e["action"] == "config.backup"))
                .unwrap_or(false)
            {
                found = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(found, "备份配置应落审计 config.backup");
    }
}

// ============================================================
// 批量写点位（POST /api/point-tools/write，≥2 条走批写分支）集成测试
// ============================================================
// 决策表 #1：Modbus FC15/FC16 多点写入口的 API 层。按 bus 分组调用
// Collector::write_multi（支持批量写的协议减少事务数，其余走默认逐点扇出）。
// 须验证：写路由鉴权 fail-closed、空 writes 返回 400、全成功/部分失败/全失败
// 三种 status、results 与入参逐位对应、以及成功项落审计 point.write。
//
// 第 1 期冗余收口：原 `POST /api/point-tools/write-batch` 已删除，批写统一走
// `POST /api/point-tools/write`（≥2 条自动走批写分支）。故本模块所有请求
// 均提交 ≥2 条 writes，确保进入批写语义。

mod point_write_multi_integration {
    use super::common::mock_state::{
        mock_app_state, mock_app_state_with_collectors, MockCollector,
    };
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use gateway_api::web::{build_router, AppState};
    use gateway_core::config::PointConfig;
    use gateway_core::south::protocol::Collector;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tower::ServiceExt;

    const TOKEN: &str = "Bearer test-token-0123456789abcdef";

    fn point(id: &str, bus: &str) -> PointConfig {
        PointConfig {
            id: id.to_string(),
            name: Some(id.to_uppercase()),
            bus: bus.to_string(),
            protocol: "modbus".to_string(),
            address: "1".to_string(),
            interval: 10,
            // S11: 批量写测试的点位需在白名单内
            writable: true,
            ..Default::default()
        }
    }

    fn store(state: &Arc<AppState>, p: PointConfig) {
        let mut cfg = state.config.load_full().as_ref().clone();
        cfg.points.push(p);
        state.config.store(Arc::new(cfg));
    }

    fn batch_req(body: &str, token: Option<&str>) -> Request<Body> {
        let mut b = Request::builder()
            .method("POST")
            .uri("/api/point-tools/write")
            .header("content-type", "application/json");
        if let Some(t) = token {
            b = b.header("authorization", t);
        }
        b.body(Body::from(body.to_string())).unwrap()
    }

    async fn body_json(res: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(res.into_body(), 65536).await.unwrap();
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    }

    /// 无 token → 401（写路由 require_auth fail-closed）
    #[tokio::test]
    async fn batch_write_without_token_returns_401() {
        let app = build_router(mock_app_state());
        let res = app
            .oneshot(batch_req(r#"{"writes":[{"id":"p1","value":1.0}]}"#, None))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    /// 空 writes → 400（显式拒绝，避免歧义语义）
    #[tokio::test]
    async fn batch_write_empty_returns_400() {
        let app = build_router(mock_app_state());
        let res = app
            .oneshot(batch_req(r#"{"writes":[]}"#, Some(TOKEN)))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    /// 全部成功 → status=ok，results 与入参逐位对应（含乱序入参）
    #[tokio::test]
    async fn batch_write_all_ok_preserves_order() {
        let mut collectors: HashMap<String, Arc<dyn Collector>> = HashMap::new();
        collectors.insert("bus0".to_string(), Arc::new(MockCollector::new("bus0")));
        let state = mock_app_state_with_collectors(collectors);
        store(&state, point("pa", "bus0"));
        store(&state, point("pb", "bus0"));
        let app = build_router(state);
        let res = app
            .oneshot(batch_req(
                r#"{"writes":[{"id":"pb","value":2.0},{"id":"pa","value":1.0}]}"#,
                Some(TOKEN),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        // P1-1：统一写入口已纳入统一 ApiResponse 信封
        let v = &v["data"];
        assert_eq!(v["status"], "ok");
        assert_eq!(v["total"], 2);
        assert_eq!(v["succeeded"], 2);
        assert_eq!(v["failed"], 0);
        assert_eq!(v["results"][0]["point_id"], "pb");
        assert_eq!(v["results"][0]["value"], 2.0);
        assert_eq!(v["results"][1]["point_id"], "pa");
        assert_eq!(v["results"][1]["value"], 1.0);
    }

    /// 部分失败（点位不存在）→ status=partial，失败项不影响成功项
    #[tokio::test]
    async fn batch_write_partial_failure() {
        let mut collectors: HashMap<String, Arc<dyn Collector>> = HashMap::new();
        collectors.insert("bus0".to_string(), Arc::new(MockCollector::new("bus0")));
        let state = mock_app_state_with_collectors(collectors);
        store(&state, point("pa", "bus0"));
        let app = build_router(state);
        let res = app
            .oneshot(batch_req(
                r#"{"writes":[{"id":"pa","value":1.0},{"id":"ghost","value":9.0}]}"#,
                Some(TOKEN),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        // P1-1：统一写入口已纳入统一 ApiResponse 信封
        let v = &v["data"];
        assert_eq!(v["status"], "partial");
        assert_eq!(v["succeeded"], 1);
        assert_eq!(v["failed"], 1);
        assert_eq!(v["results"][0]["status"], "ok");
        assert_eq!(v["results"][1]["status"], "error");
        assert!(v["results"][1]["error"].as_str().unwrap().contains("ghost"));
    }

    /// 点位存在但总线无 collector → 该项失败，全失败时 status=error
    #[tokio::test]
    async fn batch_write_missing_collector_is_per_item_error() {
        let state = mock_app_state();
        store(&state, point("pa", "bus0"));
        store(&state, point("pb", "bus0"));
        let app = build_router(state);
        let res = app
            .oneshot(batch_req(
                r#"{"writes":[{"id":"pa","value":1.0},{"id":"pb","value":2.0}]}"#,
                Some(TOKEN),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        // P1-1：统一写入口已纳入统一 ApiResponse 信封
        let v = &v["data"];
        assert_eq!(v["status"], "error");
        assert_eq!(v["succeeded"], 0);
        assert_eq!(v["failed"], 2);
        assert!(v["results"][0]["error"].as_str().unwrap().contains("bus0"));
    }

    /// collector 写失败 → 该项 status=error，整体 status=error
    #[tokio::test]
    async fn batch_write_collector_failure() {
        let mut collectors: HashMap<String, Arc<dyn Collector>> = HashMap::new();
        collectors.insert(
            "bus0".to_string(),
            Arc::new(MockCollector::new("bus0").with_write_result(false)),
        );
        let state = mock_app_state_with_collectors(collectors);
        store(&state, point("pa", "bus0"));
        store(&state, point("pb", "bus0"));
        let app = build_router(state);
        let res = app
            .oneshot(batch_req(
                r#"{"writes":[{"id":"pa","value":1.0},{"id":"pb","value":2.0}]}"#,
                Some(TOKEN),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        // P1-1：统一写入口已纳入统一 ApiResponse 信封
        let v = &v["data"];
        assert_eq!(v["status"], "error");
        assert_eq!(v["failed"], 2);
        assert_eq!(v["results"][0]["status"], "error");
    }

    /// 成功项必须落审计 point.write，并可经 /api/audit/logs 读回
    #[tokio::test]
    async fn batch_write_success_logs_queryable_audit() {
        let mut collectors: HashMap<String, Arc<dyn Collector>> = HashMap::new();
        collectors.insert("bus0".to_string(), Arc::new(MockCollector::new("bus0")));
        let state = mock_app_state_with_collectors(collectors);
        store(&state, point("pa", "bus0"));
        store(&state, point("pb", "bus0"));
        let app = build_router(state);
        let res = app
            .clone()
            .oneshot(batch_req(
                r#"{"writes":[{"id":"pa","value":1.0},{"id":"pb","value":2.0}]}"#,
                Some(TOKEN),
            ))
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
                        .uri("/api/audit/logs?action=point.write")
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
                .map(|a| a.iter().any(|e| e["action"] == "point.write"))
                .unwrap_or(false)
            {
                found = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(found, "批量写成功应落审计 point.write");
    }
}

// ============================================================
// 点位软删除 / 回收站 / 恢复 / 彻底删除
// ============================================================
// 设计：已删点位不留在 cfg.points（引擎/配额/导出零改动），而是搬进独立的
// 顶层 point_trash 数组。这里覆盖主链路：删除 → 回收站 → 恢复 → 重复恢复 →
// 单条彻底删除 → 清空 → 批量编辑删除 → 列表倒序 → 写路由鉴权。

mod point_trash_integration {
    use super::common::mock_state::mock_app_state;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use gateway_api::web::{build_router, AppState};
    use gateway_core::config::{PointConfig, PointTrashEntry};
    use std::sync::Arc;
    use tower::ServiceExt;

    const TOKEN: &str = "Bearer test-token-0123456789abcdef";

    fn test_point(id: &str) -> PointConfig {
        PointConfig {
            id: id.to_string(),
            name: Some(id.to_string()),
            bus: "bus0".to_string(),
            protocol: "modbus".to_string(),
            address: id.to_string(),
            slave_id: Some(1),
            address_num: Some(0),
            function_code: Some(3),
            interval: 10,
            ..Default::default()
        }
    }

    fn store_points(state: &Arc<AppState>, points: Vec<PointConfig>) {
        let mut cfg = state.config.load_full().as_ref().clone();
        cfg.points = points;
        state.config.store(Arc::new(cfg));
    }

    fn req(method: &str, uri: &str, token: Option<&str>, body: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(t) = token {
            builder = builder.header("authorization", t);
        }
        if body.is_some() {
            builder = builder.header("content-type", "application/json");
        }
        builder
            .body(
                body.map(|b| Body::from(b.to_string()))
                    .unwrap_or_else(Body::empty),
            )
            .unwrap()
    }

    async fn body_json(res: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    }

    /// 删除 → 回收站 1 条、活动点位少 1 条；恢复 → 回到 points 且回收站清空
    #[tokio::test]
    async fn delete_restore_roundtrip() {
        let state = mock_app_state();
        store_points(&state, vec![test_point("p1")]);
        let app = build_router(state.clone());

        // 1) 软删除
        let res = app
            .clone()
            .oneshot(req("DELETE", "/api/points/p1", Some(TOKEN), None))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        assert_eq!(v["code"], 0, "删除应成功, got {v}");

        let cfg = state.config.load_full().as_ref().clone();
        assert!(cfg.points.is_empty(), "活动点位应少 1 条");
        let trash = cfg.point_trash.as_ref().expect("应写入回收站");
        assert_eq!(trash.len(), 1);
        assert_eq!(trash[0].point.id, "p1");
        assert!(trash[0].deleted_at_ms > 0, "删除时间应为 Unix 毫秒");

        // 2) 列表
        let res = app
            .clone()
            .oneshot(req("GET", "/api/point-trash", Some(TOKEN), None))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        assert_eq!(v["code"], 0);
        assert_eq!(v["data"][0]["point"]["id"], "p1");

        // 3) 恢复
        let res = app
            .clone()
            .oneshot(req("POST", "/api/points/p1/restore", Some(TOKEN), None))
            .await
            .unwrap();
        let v = body_json(res).await;
        assert_eq!(v["code"], 0, "恢复应成功, got {v}");
        assert_eq!(v["data"]["id"], "p1");

        let cfg = state.config.load_full().as_ref().clone();
        assert_eq!(cfg.points.len(), 1, "点位应回到活动列表");
        assert!(cfg.point_trash.is_none(), "回收站应清空（不落盘）");

        // 4) 再次恢复同一 id → 回收站已无该条目
        let res = app
            .oneshot(req("POST", "/api/points/p1/restore", Some(TOKEN), None))
            .await
            .unwrap();
        let v = body_json(res).await;
        assert_eq!(v["code"], 4004, "重复恢复应返回 4004, got {v}");
    }

    /// 恢复时活动列表已存在同 id → 4009（不覆盖现有点位）
    #[tokio::test]
    async fn restore_existing_id_returns_4009() {
        let state = mock_app_state();
        store_points(&state, vec![test_point("p1")]);
        let app = build_router(state.clone());

        app.clone()
            .oneshot(req("DELETE", "/api/points/p1", Some(TOKEN), None))
            .await
            .unwrap();
        // 人工把同 id 点位加回活动列表，模拟「回收站里还有旧副本、id 已被重新占用」
        let mut cfg = state.config.load_full().as_ref().clone();
        cfg.points.push(test_point("p1"));
        state.config.store(Arc::new(cfg));

        let res = app
            .oneshot(req("POST", "/api/points/p1/restore", Some(TOKEN), None))
            .await
            .unwrap();
        let v = body_json(res).await;
        assert_eq!(v["code"], 4009, "同 id 已存在应返回 4009, got {v}");
    }

    /// 删除不存在的点位仍返回 4004
    #[tokio::test]
    async fn delete_unknown_point_returns_4004() {
        let app = build_router(mock_app_state());
        let res = app
            .oneshot(req("DELETE", "/api/points/nope", Some(TOKEN), None))
            .await
            .unwrap();
        let v = body_json(res).await;
        assert_eq!(v["code"], 4004);
    }

    /// 单条彻底删除 + 清空回收站
    #[tokio::test]
    async fn purge_single_and_purge_all() {
        let state = mock_app_state();
        store_points(&state, vec![test_point("p1"), test_point("p2")]);
        let app = build_router(state.clone());

        for id in ["p1", "p2"] {
            let res = app
                .clone()
                .oneshot(req(
                    "DELETE",
                    &format!("/api/points/{id}"),
                    Some(TOKEN),
                    None,
                ))
                .await
                .unwrap();
            assert_eq!(body_json(res).await["code"], 0);
        }
        let cfg = state.config.load_full().as_ref().clone();
        assert_eq!(cfg.point_trash.as_ref().map(|t| t.len()), Some(2));

        // 单条彻底删除
        let res = app
            .clone()
            .oneshot(req("DELETE", "/api/point-trash/p1", Some(TOKEN), None))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        let cfg = state.config.load_full().as_ref().clone();
        let ids: Vec<String> = cfg
            .point_trash
            .as_ref()
            .map(|t| t.iter().map(|e| e.point.id.clone()).collect())
            .unwrap_or_default();
        assert_eq!(ids, vec!["p2".to_string()]);

        // 彻底删除不存在的条目 → 4004
        let res = app
            .clone()
            .oneshot(req("DELETE", "/api/point-trash/nope", Some(TOKEN), None))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 4004);

        // 清空回收站
        let res = app
            .clone()
            .oneshot(req("DELETE", "/api/point-trash", Some(TOKEN), None))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);
        assert!(state.config.load_full().as_ref().point_trash.is_none());
    }

    /// 批量编辑 delete 分支同样进入回收站（deleted 计数语义不变）
    #[tokio::test]
    async fn batch_edit_delete_moves_points_to_trash() {
        let state = mock_app_state();
        store_points(&state, vec![test_point("p1"), test_point("p2")]);
        let app = build_router(state.clone());

        let res = app
            .oneshot(req(
                "POST",
                "/api/point-tools/batch",
                Some(TOKEN),
                Some(r#"{"action":"edit","delete":["p1"]}"#),
            ))
            .await
            .unwrap();
        let v = body_json(res).await;
        // R3：/api/point-tools/batch 已合并 batch/edit，edit 分支仍走统一 ApiResponse 信封
        assert_eq!(v["data"]["deleted"], 1, "deleted 计数语义不变, got {v}");

        let cfg = state.config.load_full().as_ref().clone();
        assert_eq!(cfg.points.len(), 1);
        assert_eq!(cfg.points[0].id, "p2");
        let trash = cfg.point_trash.as_ref().expect("批量删除也须进回收站");
        assert_eq!(trash.len(), 1);
        assert_eq!(trash[0].point.id, "p1");
    }

    /// 回收站列表按删除时间倒序
    #[tokio::test]
    async fn trash_list_sorted_desc_by_deleted_at() {
        let state = mock_app_state();
        let mut cfg = state.config.load_full().as_ref().clone();
        cfg.point_trash = Some(vec![
            PointTrashEntry {
                point: test_point("old"),
                deleted_at_ms: 100,
            },
            PointTrashEntry {
                point: test_point("new"),
                deleted_at_ms: 300,
            },
            PointTrashEntry {
                point: test_point("mid"),
                deleted_at_ms: 200,
            },
        ]);
        state.config.store(Arc::new(cfg));

        let app = build_router(state);
        let res = app
            .oneshot(req("GET", "/api/point-trash", Some(TOKEN), None))
            .await
            .unwrap();
        let v = body_json(res).await;
        let ids: Vec<&str> = v["data"]
            .as_array()
            .expect("data array")
            .iter()
            .map(|e| e["point"]["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["new", "mid", "old"]);
    }

    /// 回归：点位 id 恰好为 `trash` 时，`DELETE /api/points/trash` 必须删除该点位，
    /// 而不是命中「清空回收站」——静态段 `trash` 与参数段 `:id` 的遮蔽曾导致
    /// 「想删一个点位，结果把回收站清空」。
    ///
    /// 修复方式：回收站接口整体迁到 `/api/point-trash*`，`/api/points` 下不再有
    /// `trash` 静态段，`:id` 对任意 id 一致生效。
    #[tokio::test]
    async fn delete_point_with_id_trash_does_not_purge_trash() {
        let state = mock_app_state();
        let mut cfg = state.config.load_full().as_ref().clone();
        cfg.points = vec![test_point("trash"), test_point("p2")];
        // 回收站里预置一条无关记录：若被清空，说明命中了「清空回收站」处理器。
        // 时间必须落在 TTL（30 天）内，否则会被 push_trash 的过期清理顺手删掉，
        // 让本用例失去区分力。
        let recent_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        cfg.point_trash = Some(vec![PointTrashEntry {
            point: test_point("keep"),
            deleted_at_ms: recent_ms - 1_000,
        }]);
        state.config.store(Arc::new(cfg));
        let app = build_router(state.clone());

        let res = app
            .oneshot(req("DELETE", "/api/points/trash", Some(TOKEN), None))
            .await
            .unwrap();
        assert_eq!(body_json(res).await["code"], 0);

        let cfg = state.config.load_full().as_ref().clone();
        assert_eq!(
            cfg.points.len(),
            1,
            "只应删除 id=trash 的点位: {:?}",
            cfg.points
        );
        assert_eq!(cfg.points[0].id, "p2");
        let ids: Vec<&str> = cfg
            .point_trash
            .as_ref()
            .expect("回收站应仍存在")
            .iter()
            .map(|e| e.point.id.as_str())
            .collect();
        assert!(ids.contains(&"keep"), "原有回收站条目不能被清空: {ids:?}");
        assert!(ids.contains(&"trash"), "被删除的点位应进入回收站: {ids:?}");
    }

    /// 回归：点位 id 恰好叫保留字（`write`/`batch`/`virtual`/`offset`/`clone`/
    /// `find-replace`）时仍能被删除。
    ///
    /// 迁移前这些词都是 `/api/points` 下的静态段，而 matchit **静态段优先且不回落**：
    /// `DELETE /api/point-tools/virtual` 会命中虚拟点位节点并返回 405；`clone` 更糟——
    /// 该节点只是路径前缀（只有 `clone/devices`、`clone/apply` 两个子路由），
    /// 请求直接 404。两种结果都意味着这批点位在管理端「删不掉」。
    ///
    /// 修复方式：工具端点整体迁到 `/api/point-tools/*`，`/api/points` 下除 `:id` 外
    /// 不再注册任何静态段（理由见 `web/api/points/routes.rs` 文件头）。
    #[tokio::test]
    async fn point_ids_shadowed_by_static_segments_are_deletable() {
        for id in [
            "write",
            "batch",
            "virtual",
            "offset",
            "clone",
            "find-replace",
        ] {
            let state = mock_app_state();
            store_points(&state, vec![test_point(id), test_point("p2")]);
            let app = build_router(state.clone());

            let res = app
                .oneshot(req(
                    "DELETE",
                    &format!("/api/points/{id}"),
                    Some(TOKEN),
                    None,
                ))
                .await
                .unwrap();
            assert_eq!(
                res.status(),
                StatusCode::OK,
                "id={id} 的删除请求被静态段遮蔽（期望 200，实际 {}）",
                res.status()
            );
            assert_eq!(body_json(res).await["code"], 0, "id={id} 删除失败");

            let cfg = state.config.load_full().as_ref().clone();
            let left: Vec<&str> = cfg.points.iter().map(|p| p.id.as_str()).collect();
            assert_eq!(left, vec!["p2"], "id={id} 删除后应只剩 p2");
        }
    }

    /// 回收站写路由必须要求 token（fail-closed）
    #[tokio::test]
    async fn trash_write_routes_require_token() {
        let app = build_router(mock_app_state());
        for (method, uri) in [
            ("DELETE", "/api/point-trash"),
            ("DELETE", "/api/point-trash/p1"),
            ("POST", "/api/points/p1/restore"),
        ] {
            let res = app
                .clone()
                .oneshot(req(method, uri, None, None))
                .await
                .unwrap();
            assert_eq!(
                res.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {uri} 未携带 token 必须 401"
            );
        }
    }
}
