//! 启动期共享支撑（启动编排已迁至 gatewayd 二进制）
//!
//! 完整的 13 步启动流程位于 `gatewayd::bootstrap`；本模块仅保留被引擎内部
//! （`north::server`、`gateway_api::web`、`engine::logging`）复用的几类支撑：
//! - [`app`]：`GatewayRuntime` 组件管理器（总线 / MQTT / NTP 热更新），北向从站与 Web 调试面均依赖它；
//! - 路径解析：`default_xxx()`（env var 优先 + 平台默认，见 [paths]）；
//! - [`sqlite_maintenance`]：队列主库与配置库共用的增量 vacuum / 空闲页采样。
//!
//! 之所以留在 gateway-core 而非并入 gatewayd：它们被引擎侧反向引用，
//! 迁入二进制会形成 core -> bin 的非法依赖。

pub mod app;
pub mod clock;
pub mod cron;
mod paths;
pub mod sqlite_maintenance;
pub use paths::*;
