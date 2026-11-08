//! 远程控制（点位写入）handler
//!
//! 路由：
//! - GET  /api/devices/control/writable  -> list_writable_points
//! - GET  /api/devices/control/commands   -> list_commands
//! - POST /api/devices/control/send      -> send_command

use std::sync::Arc;

use axum::extract::State;
use axum::Json;

use crate::web::api::points::{write_points_core, PointWriteEntry, WriteMode};
use crate::web::types::{ApiError, ApiResponse, AppState};

use super::super::model::{ControlCommand, SendCommandRequest, WritablePoint};

/// GET /api/devices/control/writable — 可写入点位列表
pub(crate) async fn list_writable_points(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<WritablePoint>>>, ApiError> {
    let cfg = state.config.load_full();
    let mut writable: Vec<WritablePoint> = Vec::new();
    for p in &cfg.points {
        // S11: 写入白名单——仅返回显式标记 writable=true 的点位，
        // 避免前端/调用方把只读测点当成可控点下发到设备。
        if !p.writable {
            continue;
        }
        let device_name = cfg
            .devices
            .as_ref()
            .and_then(|d| {
                d.iter()
                    .find(|x| x.id == p.device_id.clone().unwrap_or_default())
            })
            .map(|d| d.name.clone())
            .unwrap_or_else(|| p.bus.clone());
        writable.push(WritablePoint {
            id: p.id.clone(),
            name: p.name.clone().unwrap_or_else(|| p.id.clone()),
            data_type: p.data_type.clone().unwrap_or_else(|| "float".to_string()),
            device_id: p.device_id.clone().unwrap_or_default(),
            device_name,
        });
    }
    Ok(Json(ApiResponse::ok(writable)))
}

/// GET /api/devices/control/commands — 控制命令历史
///
/// 从审计日志中提取控制相关操作。审计 detail 格式为 `{name}={value}`
/// （send_command 写入），失败记录带 ` [失败: ...]` 后缀——解析时拆出真实
/// 写入值与成败状态（此前 write_value 恒 0.0、status 恒 success，属假数据）。
pub(crate) async fn list_commands(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<ControlCommand>>>, ApiError> {
    use gateway_core::engine::audit::AuditFilter;
    let entries = state
        .platform
        .audit
        .list(AuditFilter {
            action: Some("point.write".to_string()),
            target: None,
            source: None,
            ts_from: None,
            ts_to: None,
            limit: Some(50),
        })
        .map_err(ApiError::from)?;

    let commands: Vec<ControlCommand> = entries
        .into_iter()
        .map(|e| {
            // 失败后缀：send_command Err 分支补落审计时追加（含中文冒号与空格前缀）
            let (status, detail) = match e.detail.find(" [失败: ") {
                Some(pos) => ("failed".to_string(), e.detail[..pos].to_string()),
                None => ("success".to_string(), e.detail.clone()),
            };
            // 从末尾拆 name=value：点位名可能含 '='，value 必是尾部数字
            let (point_name, write_value) = match detail.rsplit_once('=') {
                Some((name, v)) => (name.to_string(), v.parse::<f64>().unwrap_or(0.0)),
                None => (detail.clone(), 0.0),
            };
            ControlCommand {
                id: format!("cmd-{}", e.ts),
                device_id: e.target.clone(),
                device_name: e.target.clone(),
                // 审计未存 point_id（detail 只有点位名），point_id 暂以点位名填充
                point_id: point_name.clone(),
                point_name,
                write_value,
                operator: e.source.clone(),
                status,
                created_at: e.ts,
                executed_at: Some(e.ts),
                result: Some(e.detail),
            }
        })
        .collect();

    Ok(Json(ApiResponse::ok(commands)))
}

/// POST /api/devices/control/send — 下发控制命令
///
/// 写业务（查点 → writable 白名单 → 下发 → 指标/审计）统一收敛到
/// `points::write_points_core`（单写模式），本 handler 只负责把结果适配成
/// `ControlCommand` 响应；失败码 5101 与旧实现一致（Bug 221）。
pub(crate) async fn send_command(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SendCommandRequest>,
) -> Result<ApiResponse<ControlCommand>, ApiError> {
    let writes = [PointWriteEntry {
        id: req.point_id.clone(),
        value: req.value,
    }];
    let items = write_points_core(&state, &writes, WriteMode::Single).await?;
    let item = &items[0];

    if let Some(err) = &item.error {
        // Bug 221: 写失败必须返回非 0 业务码，前端统一走错误提示路径。
        return Ok(ApiResponse::err(5101, format!("写入失败：{}", err.message)));
    }

    let ts = chrono::Utc::now().timestamp_millis();
    Ok(ApiResponse::ok(ControlCommand {
        id: format!("cmd-{}", ts),
        device_id: item.device_id.clone().unwrap_or_default(),
        device_name: item.device_id.clone().unwrap_or_default(),
        point_id: item.point_id.clone(),
        point_name: item
            .point_name
            .clone()
            .unwrap_or_else(|| item.point_id.clone()),
        write_value: req.value,
        operator: "api".to_string(),
        status: "success".to_string(),
        created_at: ts,
        executed_at: Some(ts),
        result: Some(format!("写入成功：{}", req.value)),
    }))
}
