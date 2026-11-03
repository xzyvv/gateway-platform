//! 调度时序诊断 handler（F-11）
//!
//! 数据来源：`Scheduler::timeline`（采集循环尾部写入的**真实执行窗口**）。
//! 此前运维页的甘特图是拿「配置采集间隔 + 平均延迟」前端推算出来的，
//! 还会自行给出「检测到 N 处时间重叠」的结论——本端点把它换成真实快照。

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;

use crate::web::types::{ApiError, ApiResponse, AppState, ScheduleTimeline};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct TimelineQuery {
    /// 统计窗口（毫秒）。缺省 10s；钳制到 1s～60s，避免超大窗口拉出无效样本
    #[serde(default)]
    pub window_ms: Option<u64>,
}

/// GET /api/diagnostics/schedule-timeline
pub(crate) async fn get_schedule_timeline(
    State(state): State<Arc<AppState>>,
    Query(q): Query<TimelineQuery>,
) -> Result<Json<ApiResponse<ScheduleTimeline>>, ApiError> {
    let window_ms = q.window_ms.unwrap_or(10_000).clamp(1_000, 60_000);
    Ok(Json(ApiResponse::ok(
        state.south.scheduler.schedule_timeline(window_ms),
    )))
}
