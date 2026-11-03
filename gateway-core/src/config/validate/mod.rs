//! 配置校验（§3.1）—— 按职责拆分为三个子模块：
//!
//! - [`field`]：字段级校验器（波特率 / MQTT topic / QoS / 字节序 / 功能码 / 对端地址等），
//!   供各配置结构体的 `#[validate(...)]` 属性直接引用；
//! - [`gate`]：启停期门禁 `GatewayConfig::validate_all` 及其交叉规则，
//!   失败即 fail-fast，阻止无效配置启动；
//! - [`report`]：Web `POST /api/config/validate` 的完整校验报告（只读检查，不阻断启动）。
//!
//! 对外路径保持不变：`config/mod.rs` 通过 `pub(crate) use validate::*;` 继续把
//! `validate_*` 函数暴露给 `bus` / `mqtt` / `point` 等子模块，调用方无需改动。

mod field;
mod gate;
mod report;

pub use gate::validate_collection_config;

#[allow(unused_imports)]
pub(crate) use field::*;
