//! 设备运维 API（health / shadow / groups / assets）
//!
//! 持久化（groups / assets）：存 GatewayConfig.device_groups / device_assets
//! 内存态（health / shadow）：handler 内基于 devices 列表实时计算
//!
//! 路由：见 [device_ops_routes]（位于子模块 `routes`）

mod handlers;
mod routes;

pub(crate) use handlers::*;
pub(crate) use routes::device_ops_routes;
