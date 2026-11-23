//! 设备 API handler 聚合
//!
//! 拆分自原 `handlers.rs`（发现 / IO）与 `handlers_ops.rs`（排行 / 控制 / 拓扑 / 质量）。
//! 数据结构统一放在 `super::model`。
//!
//! DD-44：OTA / NTP handler 已迁至 `system/handlers/ota_ntp.rs`（网关自身能力，
//! 路由 `/api/ota/*`、`/api/ntp/*` 由系统域注册），设备域不再持有。
//!
//! 路由：
//! - POST /api/devices/discover            （已下线：改用 /api/protocol-tools/slave-scan）
//! - POST /api/devices/io/{index}           -> control_io
//! - GET  /api/devices/io                    -> io_status
//! - GET  /api/devices/control/writable      -> list_writable_points
//! - GET  /api/devices/control/commands      -> list_commands
//! - POST /api/devices/control/send          -> send_command（DD-55：前端已无调用方，标注废弃）
//! - GET  /api/devices/topology              -> device_topology（含聚合统计）
//! - GET  /api/devices/point-quality          -> list_point_quality
//! - GET  /api/devices/point-quality/stats   -> point_quality_stats
//! - GET  /api/devices/backoff               -> list_point_backoff

mod control;
mod discovery;
mod point_quality;
mod templates;
mod topology;

pub(crate) use control::*;
pub(crate) use discovery::*;
pub(crate) use point_quality::*;
pub(crate) use templates::*;
pub(crate) use topology::*;
