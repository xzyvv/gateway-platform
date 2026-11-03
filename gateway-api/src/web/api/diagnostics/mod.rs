//! 诊断运维 API（bus-load / capture / modbus-errors / stats / task-diag / gateway）
//!
//! 与既有 `system::diagnostics_logs_ws` / `diagnostics_exec` 路由共存：
//! - 既有：`/api/diagnostics/log`（WebSocket tail）、`/api/diagnostics/exec`（白名单 shell）
//! - 本模块：`/api/diagnostics/<sub>` 子路径，无命名冲突
//!
//! 数据来源：
//! - 总线负载：采集引擎真实计数器（ScopedCollectMetrics）
//! - 统计：按总线点位日桶真实累计（stats.rs）
//! - 任务诊断：scheduler 快照 + collector 运行态真实派生（task_diag.rs）
//! - 网关自诊：真实主机指标（gateway_diag.rs）
//! - 总线嗅探：真实帧（capture.rs ← frame_log 环形日志，采集/调试事务单点记录）
//! - Modbus 异常码：静态知识库（异常码速查，设计如此）
//!
//! 路由：见 [diagnostics_routes]（位于子模块 `routes`）

mod handlers;
mod routes;

pub(crate) use handlers::*;
pub(crate) use routes::diagnostics_routes;
