//! 诊断 handler 聚合
//!
//! 路由：
//! - GET  /api/diagnostics/bus-load                -> list_bus_loads
//! - GET  /api/diagnostics/bus-load/{bus}          -> get_bus_load
//! - POST /api/diagnostics/bus-load/refresh         -> refresh_bus_loads
//! - GET  /api/diagnostics/modbus-errors           -> list_modbus_errors
//! - GET  /api/diagnostics/modbus-errors/issues     -> list_common_issues
//! - GET  /api/diagnostics/stats                   -> get_stats_report
//! - GET  /api/diagnostics/schedule-timeline       -> get_schedule_timeline
//! - POST /api/diagnostics/task-diag/{point_id}     -> diagnose_point
//! - GET  /api/diagnostics/gateway                 -> get_gateway_diag
//! - GET  /api/diagnostics/logs/download           -> download_logs
//! - GET  /api/diagnostics/logs/archives           -> list_log_archives
//!
//! 抓包已统一到 `/api/protocol-tools/packet-capture/*`（单一实现，含结构化解析、
//! 设备归属、配对与真正的清空）；原 `/api/diagnostics/capture/*` 双实现已删除。

pub mod bus_load;
pub mod gateway_diag;
pub mod log_download;
pub mod modbus_errors;
pub mod schedule_timeline;
pub mod stats;
pub mod task_diag;

pub(crate) use bus_load::*;
pub(crate) use gateway_diag::*;
pub(crate) use log_download::*;
pub(crate) use modbus_errors::*;
pub(crate) use schedule_timeline::*;
pub(crate) use stats::*;
pub(crate) use task_diag::*;
