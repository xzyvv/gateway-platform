//! 协议工具 API（auto-detect / packet-capture / packet-editor / packet-replay /
//! protocol-convert / protocol-test / register-scan / slave-scan / slave-simulator）
//!
//! 与既有 diagnostics::capture（总线嗅探，含 ParsedPacket 子对象）共存：
//! - 既有：/api/diagnostics/capture/*（ParsedPacket 结构）
//! - 本模块：/api/protocol-tools/packet-capture/*（Packet 结构，含 deviceName/matchedId/delayMs/ascii）
//!
//! 数据来源：
//! - auto-detect：真实总线验证式探测（当前配置下的连通性/从站/功能码/耗时/字节序）
//! - packet-editor / packet-replay / slave-scan / register-scan / protocol-test：真实总线触达
//! - packet-capture：真实帧（frame_log 环形日志，经 web::debug_bus 门面读取）
//! - protocol-convert / slave-simulator：内存态（static Mutex）
//! - 单步调试：无独立后端——前端 StepDebug.vue 直调 packet-editor/send 真实下发
//!
//! 路由：见 [protocol_tools_routes]（位于子模块 `routes`）

mod handlers;
mod routes;
mod session;
mod sse;

pub(crate) use handlers::*;
pub(crate) use routes::protocol_tools_routes;
pub(crate) use session::{acquire_bus, audit_send, error_response, DebugError, DebugLease};
pub(crate) use sse::{accepts_sse, sse_response, IntoSseEvent};
