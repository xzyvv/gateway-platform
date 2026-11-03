//! 设备/测点配置 CRUD API（Q16-Q18）
//!
//! 供 Go+Vue 调试软件调用，支持：
//! - 设备增删改查（POST/GET/PUT/DELETE /api/devices[/:id]）
//! - 测点增删改查（POST/GET /api/devices/:id/points, PUT/DELETE /api/points/:id）
//! - 配置导入导出（GET/PUT /api/config）
//! - 触发热重载（POST /api/config/reload）
//!
//! 设计要点：
//! - 响应体统一 `ApiResponse<T>`（code/message/data），便于 Vue axios 拦截器统一处理
//! - 写操作完成保存后立即触发热重载（基于 `state.north.gateway.reload_config()` 增量应用）
//! - 校验函数（validate_device/validate_point/validate_config）实现 Q17 配置校验
//!
//! 已知假设（依赖前置补丁 Q14/Q15）：
//!   - `DeviceConfig.id: String` 已添加（原项目仅 `name`，本模块按任务约定使用 `id`）
//!   - `PointConfig.device_id: Option<String>` 已添加
//!
//! 若上述字段未补齐，本文件相关行将编译失败——按任务说明无需在本补丁内新增字段。
//!
//! 路由注册：见 [config_routes]（位于子模块 `routes`）。

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};

pub(crate) use crate::web::services::VALID_PROTOCOLS;
use crate::web::types::{ApiError, AppState};
use gateway_core::config::{DeviceConfig, GatewayConfig, PointConfig, PointTrashEntry};

pub(crate) mod handlers;
mod routes;

// 统一响应体 ApiResponse 已移至 web::types，供所有 API 模块共享
pub use crate::web::types::ApiResponse;

// ===== 请求体 =====
// 协议白名单统一来自 south::protocol::registry::VALID_PROTOCOLS，
// 新增协议只需在 registry.rs 修改一处，避免多处硬编码不同步。

pub(crate) use handlers::*;
pub(crate) use routes::config_routes;
