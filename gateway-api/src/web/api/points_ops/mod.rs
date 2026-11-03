//! 点位运维 API（virtual / find-replace / offset / clone）
//!
//! 虚拟点位：持久化在 GatewayConfig.virtual_points，公式派生
//! 批量操作：直接修改 PointConfig 并触发热重载
//!
//! 路由：见 [points_ops_routes]（位于子模块 `routes`）

mod handlers;
mod routes;

pub(crate) use handlers::*;
pub(crate) use routes::points_ops_routes;
