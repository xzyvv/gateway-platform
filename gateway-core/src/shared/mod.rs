//! 跨层共享的纯工具与策略（无 I/O、无业务层依赖）
//!
//! - [`clock`]：系统时钟可信度判定（纯状态机，不读系统时间）；
//! - [`cron`]：5 段 cron 求值与时区解析；
//! - 路径解析：`default_xxx()`（env var 优先 + 平台默认，见 [`paths`]）；
//! - [`sqlite_maintenance`]：队列主库与配置库共用的增量 vacuum / 空闲页采样。

pub mod clock;
pub mod cron;
mod paths;
pub mod sqlite_maintenance;
pub use paths::*;
