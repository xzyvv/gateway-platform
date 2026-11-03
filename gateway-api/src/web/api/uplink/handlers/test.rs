//! 上行测试发送 + 测试历史 handler

use super::*;
use crate::web::services::render_template;
use gateway_core::engine::model::DataPoint;

/// clamp 历史查询 limit，防止前端传超大值拖垮 SQLite
pub(crate) fn clamp_history_limit(input: usize) -> usize {
    input.min(200)
}

pub async fn test_uplink(
    State(state): State<Arc<AppState>>,
    Json(req): Json<TestUplinkRequest>,
) -> Resp {
    let vars = build_preview_vars(&req.vars);
    let rendered = match render_template(&req.template, &vars) {
        Ok(r) => r,
        Err(e) => {
            return ok(TestUplinkResponse {
                ok: false,
                rendered: String::new(),
                bytes: 0,
                publish: None,
                error: Some(format!("模板编译失败：{}", e)),
                history_id: None,
            });
        }
    };
    let bytes = rendered.len();

    if !req.send {
        return ok(TestUplinkResponse {
            ok: true,
            rendered,
            bytes,
            publish: None,
            error: None,
            history_id: None,
        });
    }

    // 取 `gateway.mqtt()` 的**实时**值：热重载可能替换/移除 MQTT 客户端，
    // `state.north.mqtt` 是启动期快照，重载后会指向已被替换的旧客户端
    // （重载为禁用时它还会误报「客户端可用」）。
    let mqtt = match state.north.gateway.mqtt() {
        Some(m) => m,
        None => {
            return ok(TestUplinkResponse {
                ok: false,
                rendered,
                bytes,
                publish: None,
                error: Some("MQTT 客户端未启动（mqtt.enabled=false）".into()),
                history_id: None,
            });
        }
    };

    let dp = DataPoint::good(
        req.vars
            .get("point_id")
            .and_then(|v| v.as_str())
            .unwrap_or("test_point"),
        req.vars
            .get("device_id")
            .and_then(|v| v.as_str())
            .unwrap_or("test_device"),
        req.vars
            .get("value")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0),
        req.vars
            .get("value")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0),
        "f64",
    );
    let started = std::time::Instant::now();
    let published = mqtt.publish_bypass_queue(&[dp]).await;
    let latency = started.elapsed().as_millis() as u64;

    let publish = PublishResult {
        topic: mqtt.config().topic.clone(),
        qos: mqtt.config().qos,
        delivered: published,
        latency_ms: latency,
    };

    let err_msg = if published {
        None
    } else {
        Some("发送失败（broker 不可达或断路器开启）".to_string())
    };

    // 持久化测试历史（便于前端 UplinkTester 翻查历史对比）
    let vars_json = serde_json::to_string(&req.vars).unwrap_or_else(|_| "{}".into());
    let history_id =
        match state
            .config
            .uplink_test_history
            .save(gateway_core::config::NewUplinkTestHistory {
                template: &req.template,
                rendered: &rendered,
                vars_json: &vars_json,
                send: true,
                delivered: published,
                latency_ms: latency,
                error: err_msg.as_deref(),
            }) {
            Ok(id) => Some(id),
            Err(e) => {
                tracing::warn!("failed to save uplink test history: {}", e);
                None
            }
        };

    state.platform.audit.log(
        "uplink.test",
        &mqtt.config().topic,
        &format!("delivered={}, {}ms", published, latency),
        "api",
    );

    ok(TestUplinkResponse {
        ok: published,
        rendered,
        bytes,
        publish: Some(publish),
        error: err_msg,
        history_id,
    })
}

pub async fn list_test_history(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListHistoryQuery>,
) -> Resp {
    let limit = clamp_history_limit(q.limit);
    match state.config.uplink_test_history.list_recent(limit) {
        Ok(items) => ok(items),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            50002,
            format!("查询测试历史失败：{}", e),
        ),
    }
}

pub async fn clear_test_history(State(state): State<Arc<AppState>>) -> Resp {
    match state.config.uplink_test_history.clear() {
        Ok(_) => {
            state.platform.audit.log(
                "uplink.test.history.clear",
                "all",
                "清空上行测试历史",
                "api",
            );
            ok(serde_json::json!({ "cleared": true }))
        }
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            50003,
            format!("清空测试历史失败：{}", e),
        ),
    }
}
