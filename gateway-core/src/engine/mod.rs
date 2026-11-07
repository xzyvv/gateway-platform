//! 核心基础设施模块
//!
//! 汇聚日志、指标、错误类型等跨模块共享的基础设施；监控与可观测性见 `monitor` 子模块。

pub mod alarm;
pub mod audit;
pub mod bus;
pub mod error;
pub mod logging;
pub mod metrics;
pub mod monitor;
pub mod ports;
pub mod shutdown;
pub mod supervision;
pub mod util;

pub use util::{encoding, model};

/// 重新导出 Diagnostics 便于外部使用
pub use monitor::remote_diag::Diagnostics;
