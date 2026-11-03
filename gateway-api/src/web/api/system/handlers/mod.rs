//! API System handlers

// 重新导出父模块（system）的导入，供子 handler 通过 use super::* 引用
pub(crate) use super::*;

pub mod certificates;
pub mod config;
pub mod status;
pub mod watchdog;
pub mod webhook;

// 将 config/status 子模块的 handler 函数提升到 handlers 命名空间
pub(crate) use certificates::*;
pub(crate) use config::*;
pub(crate) use status::*;
pub(crate) use watchdog::*;
pub(crate) use webhook::*;
