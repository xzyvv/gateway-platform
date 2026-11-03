//! 数据点位相关 Web API（F3 批量编辑 / F5 手动触发采集 / 批量导入）

use std::sync::Arc;

use axum::extract::{Json, Path, State};

use axum::response::IntoResponse;
use serde::Deserialize;

use crate::web::types::{ApiError, ApiResponse, AppState};

/// 点位批量请求体（统一入口 `POST /api/points/batch`）
///
/// R3 合并：原 `/api/points/batch`（裸数组 upsert）与 `/api/points/batch/edit`
/// （增/改/删）合并为同一端点，按请求体形状分派：
/// - 直接传 `[{...point}, ...]` → 等价 `upsert`（同 id 覆盖，旧调用零改动）；
/// - 传对象 → 按 `action` 字段分派：`{"action":"upsert","points":[...]}` 或
///   `{"action":"edit","add":[...],"update":[...],"delete":["id"]}`。
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum PointsBatchReq {
    /// 兼容旧调用：裸点位数组
    BareUpsert(Vec<gateway_core::config::PointConfig>),
    /// 统一入口：带 action 的对象
    Action(PointsBatchAction),
}

/// 统一入口的 action 分支
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum PointsBatchAction {
    /// 批量插入/覆盖（同 id 覆盖）
    Upsert {
        points: Vec<gateway_core::config::PointConfig>,
    },
    /// 批量增/改/删（删除进回收站）
    Edit {
        #[serde(default)]
        add: Vec<gateway_core::config::PointConfig>,
        #[serde(default)]
        update: Vec<gateway_core::config::PointConfig>,
        #[serde(default)]
        delete: Vec<String>,
    },
}

/// 点位写入请求体（唯一写入口 `POST /api/points/write`）
///
/// 契约：`{ "writes": [ { "id": "<point_id>", "value": <f64> } ] }`。
/// 条目字段沿用既有批写接口的 `id`（前端无需改字段名），同时接受
/// `point_id` 作为等价别名。
#[derive(Debug, Deserialize)]
pub struct PointsWriteBatchReq {
    pub writes: Vec<PointWriteEntry>,
}

/// 写入的单个条目：点位 id + 目标值（`id` / `point_id` 两种写法等价）
#[derive(Debug, Deserialize)]
pub struct PointWriteEntry {
    #[serde(alias = "point_id")]
    pub id: String,
    pub value: f64,
}

/// 写入模式（B2 统一入口内部的分派口径）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriteMode {
    /// 单写：校验失败直接返回 ApiError（与 `send_command` 同一错误码）
    Single,
    /// 批写：逐条独立成败，单条失败不影响其它条
    Batch,
}

/// 单条写结果（与入参逐位对应）
#[derive(Debug)]
pub(crate) struct WriteItemResult {
    pub point_id: String,
    pub point_name: Option<String>,
    pub device_id: Option<String>,
    pub value: f64,
    pub error: Option<WriteItemError>,
}

/// 单条失败信息
#[derive(Debug)]
pub(crate) struct WriteItemError {
    pub message: String,
    pub quality: Option<gateway_core::engine::model::Quality>,
}

mod handlers;
mod routes;

pub(crate) use handlers::*;
pub(crate) use routes::points_routes;
