//! gateway-core 库入口
//!
//! 引擎能力（配置/采集/北向/队列等，Web API 已拆至 gateway-api）；完整启动流程（13 步）已迁至
//! `gatewayd` 二进制 crate，本库仅保留启动期共享支撑 [`support`]：
//! 1. 加载配置  2. 日志  3. Watchdog  4. NTP
//! 5. GPIO  6. BusManager  7. Collector  8. DataQueue
//! 9. MQTT  10. Lua  11. EdgeProcessor+Pipeline
//! 12. Scheduler  13. Web Server + HA

// 注：已移除全局 allow(dead_code)/unused_imports，代码库无死代码。
// 有意保留的接口使用定向 #[allow(dead_code)] 并附说明。
//
// 生产代码禁用 unwrap/expect：非测试构建下将 clippy::unwrap_used / clippy::expect_used
// 提升为 deny，防止新增的「不可能失败」假设悄悄引入 panic 路径。
// 经审计确认安全的不变量点使用定向 #[allow(...)] 并附说明（见 serial_task.rs、
// config/schema.rs）。license/mod.rs::issuer_support 的豁免点已于 W2-#18
// 用 `test-support` feature 移出生产构建（仅测试目标编译）。
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod config;
pub mod crypto;
pub mod domain;
pub mod engine;
pub mod license;
pub mod north;
pub mod pipeline;
pub mod platform;
pub mod queue;
pub mod south;
pub mod support;
