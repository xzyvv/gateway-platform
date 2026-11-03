//! 设备基础 API：IO 控制 / OTA / NTP（从原 handlers.rs 迁移）
//!
//! D-03：原 `POST /api/discover` 是 `POST /api/protocol-tools/slave-scan` 的
//! 等价实现（同一扫描引擎、同一校验），已删除；前端改调 slave-scan 阻塞模式。

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Json},
};

use crate::web::types::{ApiError, AppState, IoControlRequest, OtaStatus};

pub(crate) async fn control_io(
    State(state): State<Arc<AppState>>,
    Path(index): Path<usize>,
    Json(req): Json<IoControlRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let on = match req.action.as_str() {
        "on" => true,
        "off" => false,
        other => {
            return Err(ApiError::bad_request(format!("invalid action: {}", other)));
        }
    };
    let mut gpio = state.platform.gpio.lock();
    let result = gpio.set_do(index, on);
    state.platform.metrics.record_point_write(result.is_ok());
    result.map_err(ApiError::from)?;
    // 数字量输出直接驱动现场设备，属物理动作，必须落审计
    state.platform.audit.log(
        "io.control",
        &format!("do:{}", index),
        if on { "set DO on" } else { "set DO off" },
        "api",
    );
    Ok(Json(
        serde_json::json!({"status": "ok", "index": index, "on": on}),
    ))
}

pub(crate) async fn io_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(state.platform.gpio.lock().status())
}

pub(crate) async fn ota_upload(
    State(state): State<Arc<AppState>>,
    body: axum::body::Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let ota = state.north.ota.clone();
    let data = body.to_vec();
    if data.is_empty() {
        return Err(ApiError::bad_request("empty firmware"));
    }
    let is_delta = data.len() >= 4 && &data[0..4] == b"GWDT";
    let upload_type = if is_delta { "delta" } else { "full" };
    // D3: OTA 升级为敏感操作，须落审计（action=ota.upgrade，供自愈事件/审计查询消费）
    state.platform.audit.log(
        "ota.upgrade",
        "gateway",
        &format!("upload {} firmware ({} bytes)", upload_type, data.len()),
        "api",
    );
    gateway_core::engine::supervision::spawn_catch_unwind("ota_process", async move {
        let result = if is_delta {
            ota.process_delta(data).await
        } else {
            ota.process_firmware(data).await
        };
        if let Err(e) = result {
            tracing::error!("OTA {} process failed: {}", upload_type, e);
        }
    });
    Ok(Json(
        serde_json::json!({"status": "uploading", "type": upload_type}),
    ))
}

pub(crate) async fn ota_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let s = state.north.ota.status();
    Json(OtaStatus {
        progress: s.progress,
        status: s.state.as_str().to_string(),
        error: s.error,
    })
}

pub(crate) async fn ntp_sync(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ApiError> {
    state
        .platform
        .ntp
        .sync_now()
        .await
        .map_err(ApiError::from)?;
    // 校时会影响全站时间戳/历史数据对齐，属运维敏感动作
    state
        .platform
        .audit
        .log("ntp.sync", "gateway", "manual ntp sync", "api");
    Ok(Json(serde_json::json!({"status": "synced"})))
}

pub(crate) async fn ntp_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(state.platform.ntp.status())
}

/// OTA 回滚：将固件回滚到上一个版本
pub(crate) async fn ota_rollback(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ApiError> {
    state.north.ota.rollback().map_err(ApiError::from)?;

    // D3: OTA 回滚为敏感操作，须落审计
    state
        .platform
        .audit
        .log("ota.rollback", "gateway", "rollback firmware", "api");

    Ok(Json(serde_json::json!({
        "status": "rolled_back",
    })))
}
