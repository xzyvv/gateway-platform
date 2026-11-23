//! 设备基础 API：IO 控制（从原 handlers.rs 迁移）
//!
//! D-03：原 `POST /api/discover` 是 `POST /api/protocol-tools/slave-scan` 的
//! 等价实现（同一扫描引擎、同一校验），已删除；前端改调 slave-scan 阻塞模式。
//!
//! DD-44 归位说明：OTA（`/api/ota/*`）与 NTP（`/api/ntp/*`）handler 原先物理放在
//! 本文件，但它们操作的是**网关自身**（固件升级 / 时钟校准），与设备域无关，
//! 路由也一直由系统域注册（`system/routes.rs`）。现已移至
//! `system/handlers/ota_ntp.rs`，与证书 handler 同处系统域；本文件只保留设备域
//! 真正的能力：IO 控制与 IO 状态。

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Json},
};

use crate::web::types::{ApiError, AppState, IoControlRequest};

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
