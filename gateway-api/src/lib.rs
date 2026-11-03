//! gateway-api 库入口
//!
//! Web 调试面（§5 网关 Web API，axum + tower）由 gateway-core 拆出为独立 crate，
//! 使底层引擎与 HTTP 框架解耦：引擎能力经 `gateway_core::*` 公共 API 访问，
//! web 自身仍以 `crate::web::` 路径组织。

// 生产代码禁用 unwrap/expect：与 gateway-core 同一门禁口径。
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod web;
