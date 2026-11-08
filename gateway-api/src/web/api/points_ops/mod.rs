//! 点位运维 API（虚拟点位 / 地址偏移 / 克隆 / 查找替换）handler 库
//!
//! 虚拟点位：持久化在 GatewayConfig.virtual_points，公式派生
//! 批量操作：直接修改 PointConfig 并触发热重载
//!
//! **本子域不再注册路由**：`/api/points*` 的唯一注册点是
//! `web/api/points/routes.rs`（同一前缀只能有一个注册点，否则改端点要在多个文件里翻）。

mod handlers;

pub(crate) use handlers::*;
