//! 上行模板 API 处理函数（从 uplink.rs 拆分）

use super::*;

mod config;
mod preview;
mod test;

pub(crate) use config::*;
pub(crate) use preview::*;
pub(crate) use test::*;
