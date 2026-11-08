//! 监控与可观测性子模块
//!
//! 汇聚面向「运行时观测」的能力：健康监控、任务心跳、远程诊断、
//! OTel 导出。通用基础设施（error / logging / supervision 等）仍留在 `engine/` 顶层。
pub mod db_maintenance;
pub mod health;
pub mod heartbeat;
pub mod host;
pub mod otel;
pub mod remote_diag;
