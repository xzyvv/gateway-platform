//! 南向（采集层）：总线 + 协议 + 调度器
//!
//! 统一管理所有南向采集相关模块：
//! - bus：串口/TCP 总线管理（BusManager + SerialManager）
//! - protocol：协议采集器（Modbus / DLT645 / CJT188 / OPC UA / IEC104 / BACnet）
//! - scheduler：采集调度（按协议 + 总线分组）

pub mod bus;
pub mod frame_log;
pub mod protocol;
pub mod scheduler;
