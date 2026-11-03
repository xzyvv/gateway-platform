//! 内置模拟消费者 handler
//!
//! 三个端点：
//! - GET  /api/sim_consumer/status —— 状态快照（配置意图 + 实际运行态 + 计数 + 队列深度）
//! - POST /api/sim_consumer/start  —— 启动（可带运行参数）
//! - POST /api/sim_consumer/stop   —— 停止
//!
//! 关键约束（与 core 的实现一致）：
//! - 与 MQTT **互斥**：MQTT 启用时拒绝启动——两个消费者会分流同一批队列数据，
//!   被模拟消费者分走的那部分被「消费掉但不上报」，现场表现为静默丢数；
//! - 运行参数只能在**未运行**时修改（core 侧 `update_params` 会报错）；
//! - `stop()` 会等到 run 循环真正退出才返回，故返回后队列不会再被消费。

use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use serde::Deserialize;

use crate::web::error::ApiError;
use crate::web::types::{ApiResponse, AppState, SimConsumerStatus};

/// 启动请求体（全部可选）：未提供的字段保持原值。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SimConsumerStartReq {
    /// 单批出队条数。
    #[serde(default)]
    pub(crate) batch_size: Option<usize>,
    /// 队列为空时的轮询间隔（毫秒）。
    #[serde(default)]
    pub(crate) interval_ms: Option<u64>,
    /// 每消费满多少条打一条 INFO 摘要日志（0 = 不打）。
    #[serde(default)]
    pub(crate) log_every: Option<usize>,
}

/// 阻塞原因常量：MQTT 已启用，模拟消费者不能与之并存。
const BLOCKED_BY_MQTT: &str = "mqtt_enabled";

/// MQTT 是否已启用（按**配置意图**判定，而非客户端是否成功连上）。
///
/// 为什么看配置而不是运行态：MQTT 客户端初始化失败（broker 不可达）时
/// 运行态是 None，但配置意图仍是「要上送」。此时若放行模拟消费者，
/// 队列数据会被吃掉却没上报——正是本组件要避免的静默丢数。
fn mqtt_enabled(state: &Arc<AppState>) -> bool {
    state.config.load_full().mqtt.enabled
}

/// 组装状态快照。
async fn build_status(state: &Arc<AppState>) -> SimConsumerStatus {
    let consumer = &state.north.sim_consumer;
    let stats = consumer.stats();
    let cfg = consumer.config();
    let queue_len = state.south.queue.len().await;
    let blocked = mqtt_enabled(state);
    SimConsumerStatus {
        running: stats.running,
        enabled: cfg.enabled,
        consumed: stats.consumed,
        batches: stats.batches,
        queue_len,
        last_consumed_at_ms: stats.last_consumed_at_ms,
        started_at_ms: stats.started_at_ms,
        uptime_ms: stats.uptime_ms,
        batch_size: cfg.batch_size,
        interval_ms: cfg.interval_ms,
        log_every: cfg.log_every,
        // 只有「没在跑 + MQTT 挡着」时才给出阻塞原因：已在运行时不存在阻塞。
        blocked_reason: if !stats.running && blocked {
            Some(BLOCKED_BY_MQTT.to_string())
        } else {
            None
        },
    }
}

/// GET /api/sim_consumer/status —— 状态快照
pub(crate) async fn get_sim_consumer_status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<SimConsumerStatus>>, ApiError> {
    Ok(Json(ApiResponse::ok(build_status(&state).await)))
}

/// POST /api/sim_consumer/start —— 启动（可带运行参数）
pub(crate) async fn start_sim_consumer(
    State(state): State<Arc<AppState>>,
    body: Option<Json<SimConsumerStartReq>>,
) -> Result<Json<ApiResponse<SimConsumerStatus>>, ApiError> {
    let consumer = &state.north.sim_consumer;

    if mqtt_enabled(&state) {
        return Err(ApiError::conflict(
            "MQTT 已启用：模拟消费者与 MQTT 会分流同一批队列数据导致静默丢数，拒绝启动",
        ));
    }
    if consumer.is_running() {
        return Err(ApiError::conflict("模拟消费者已在运行"));
    }

    let req = body.map(|Json(b)| b).unwrap_or_default();
    consumer
        .update_params(req.batch_size, req.interval_ms, req.log_every)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;

    if !consumer.start() {
        // 竞态：检查与启动之间被别的请求抢先启动。
        return Err(ApiError::conflict("模拟消费者已在运行"));
    }
    state.platform.audit.log(
        "sim_consumer.start",
        "sim_consumer",
        &format!(
            "sim consumer started (batch={}, interval={}ms)",
            consumer.config().batch_size,
            consumer.config().interval_ms
        ),
        "web",
    );
    Ok(Json(ApiResponse::ok(build_status(&state).await)))
}

/// POST /api/sim_consumer/stop —— 停止（等待 run 循环真正退出）
pub(crate) async fn stop_sim_consumer(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<SimConsumerStatus>>, ApiError> {
    let consumer = &state.north.sim_consumer;
    if !consumer.is_running() {
        return Err(ApiError::conflict("模拟消费者未在运行"));
    }
    consumer.stop().await;
    state.platform.audit.log(
        "sim_consumer.stop",
        "sim_consumer",
        "sim consumer stopped by web api",
        "web",
    );
    Ok(Json(ApiResponse::ok(build_status(&state).await)))
}
