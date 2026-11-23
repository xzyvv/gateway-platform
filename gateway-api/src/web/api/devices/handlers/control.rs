//! 远程控制（点位写入）handler
//!
//! 路由：
//! - GET  /api/devices/control/writable  -> list_writable_points
//! - GET  /api/devices/control/commands   -> list_commands
//! - POST /api/devices/control/send      -> send_command【已废弃】（DD-55，改用 /api/point-tools/write）

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use axum::Json;

use crate::web::api::points::{
    write_failure_code, write_failure_message, write_points_core, PointWriteEntry, WriteMode,
};
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
/// **【已废弃】**（DD-55）：前端写路径已统一到 `POST /api/point-tools/write`
/// （`usePointWrite.ts`），本端点前端已无调用方，属孤儿端点。
///
/// 处理方式：不删除（外部脚本可能仍在调用，删除即破坏契约），改为**标注废弃**——
/// 响应统一带 `Deprecation: true` 与 `Warning` 头，调用方可据此自动迁移；
/// 写业务本身仍走统一核心 `points::write_points_core`，行为与旧实现一致
/// （失败码 5101 与旧实现一致，Bug 221）。
pub(crate) async fn send_command(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SendCommandRequest>,
) -> Result<(HeaderMap, Json<ApiResponse<ControlCommand>>), ApiError> {
    let writes = [PointWriteEntry {
        id: req.point_id.clone(),
        value: req.value,
    }];
    let items = write_points_core(&state, &writes, WriteMode::Single).await?;
    let item = &items[0];

    if let Some(err) = &item.error {
        // Bug 221: 写失败必须返回非 0 业务码，前端统一走错误提示路径。
        // G-02: 按写结果三态区分业务码——5101 明确失败（可安全重试），
        // 5102 结果未知（设备可能已执行，禁止自动重试）。
        return Ok((
            deprecation_headers(),
            Json(ApiResponse::err(
                write_failure_code(err.outcome),
                write_failure_message(err),
            )),
        ));
    }

    let ts = chrono::Utc::now().timestamp_millis();
    Ok((
        deprecation_headers(),
        Json(ApiResponse::ok(ControlCommand {
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
        })),
    ))
}

/// 废弃端点响应头：标准 `Deprecation: true` + `Warning: 299 - "..."`。
///
/// 让外部脚本（而非只有浏览器前端）也能看到该端点已进入废弃期。
fn deprecation_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("deprecation"),
        HeaderValue::from_static("true"),
    );
    headers.insert(
        HeaderName::from_static("warning"),
        HeaderValue::from_static(
            "299 - \"POST /api/devices/control/send 已废弃，请改用 POST /api/point-tools/write\"",
        ),
    );
    headers
}
