//! 设备模板 handler
//!
//! 原实现寄居在 `api/history`（模板属设备域，不属历史域，容易让改端点的人多跑一处
//! grep），路由与 handler 一并迁入设备域；**URL 与鉴权策略保持不变**：
//! - GET    /api/templates        -> list_templates
//! - POST   /api/templates        -> save_template
//! - DELETE /api/templates/:name  -> delete_template

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Json},
};
use gateway_core::config::DeviceTemplate;

use crate::web::types::{ApiError, AppState};

pub(crate) async fn list_templates(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let templates: Vec<_> = state.config.templates.list();
    Json(templates)
}

pub(crate) async fn save_template(
    State(state): State<Arc<AppState>>,
    Json(template): Json<DeviceTemplate>,
) -> Result<impl IntoResponse, ApiError> {
    let name = template.name.clone();
    let point_count = template.points.len();
    state
        .config
        .templates
        .save_custom(template)
        .map_err(ApiError::from)?;
    // 模板是设备点位的展开来源，覆盖保存会影响引用它的全部设备，必须留痕
    state.platform.audit.log(
        "template.save",
        &format!("template:{}", name),
        &format!("points={}", point_count),
        "api",
    );
    Ok(Json(serde_json::json!({"status": "saved"})))
}

/// D5: 删除设备模板
pub(crate) async fn delete_template(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    state
        .config
        .templates
        .delete(&name)
        .map_err(ApiError::from)?;
    state.platform.audit.log(
        "template.delete",
        &format!("template:{}", name),
        "delete template",
        "api",
    );
    Ok(Json(serde_json::json!({"status": "deleted"})))
}
