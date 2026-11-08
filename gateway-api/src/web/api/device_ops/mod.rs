//! 设备运维 API（health / shadow / groups / assets）handler 库
//!
//! 持久化（groups / assets）：存 GatewayConfig.device_groups / device_assets
//! 内存态（health / shadow）：handler 内基于 devices 列表实时计算
//!
//! **本子域不再注册路由**：`/api/devices*` 的唯一注册点是
//! `web/api/devices/routes.rs`（同一前缀只能有一个注册点，否则改端点要在多个文件里翻）。
//! 本目录只提供 handler 与共享判据（如 `acc_quality` / `quality_code`）。

mod handlers;

pub(crate) use handlers::*;
