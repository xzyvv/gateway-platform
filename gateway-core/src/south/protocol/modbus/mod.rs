//! Modbus 采集器（§9 批量读取规则）
//!
//! 纯 Rust 实现 Modbus RTU 协议，含：
//! - CRC16 校验（统一从 protocol_util 引入）
//! - FC01/FC02 读线圈/离散输入，FC03/FC04 读保持/输入寄存器
//! - 按 (bus, slave_id, function_code) 分组合并读取
//! - 多寄存器数据类型 + 字节序解码
//!
//! 从站地址扫描不在本模块：由 `gateway-api` 的 `slave_scan` 协议工具直接发探测帧实现。
//!
//! 模块结构：
//! - frames.rs: 帧构建 / 响应解析
//! - batch.rs: 读合并与写分组算法
//! - exception.rs: 异常码分级
//! - merge.rs: 合并参数自适应统计 + 写分组辅助
//! - collector.rs: 采集器固有方法（重试读 / 批量读）
//! - trait_impl.rs: `Collector` trait 实现（单点读写 / 多点写）
//! - transaction/: TCP 事务池

pub mod transaction;

mod batch;
mod collector;
mod exception;
pub(crate) mod frames;
mod merge;
mod trait_impl;

pub(crate) use crate::south::protocol::convert_value;
pub(crate) use crate::south::protocol::reverse_convert;
pub(crate) use batch::{
    encode_register_values, merge_batches, merge_write_batches, parse_point_value, write_kind_of,
    WriteKind, MAX_WRITE_COILS, MAX_WRITE_REGISTERS,
};
// 供 gateway-api 的协议工具复用（帧构造单一来源），其余解析函数仍限 crate 内
pub use frames::{build_read_request, rtu_frame};
pub(crate) use frames::{
    build_write_multi_coils, build_write_multi_registers, extract_modbus_frame,
    parse_coils_response, parse_read_response,
};

pub use exception::ModbusExceptionCode;

use std::sync::Arc;

use parking_lot::Mutex;

use self::merge::MergeStats;

use super::{clone_collector_error, Collector, CollectorError, CollectorResult};
use crate::south::bus::BusManager;

/// Modbus 采集器
pub struct ModbusCollector {
    bus: Arc<BusManager>,
    /// 合并参数自适应状态（决策表 #3 轻量实现 A）
    merge_stats: Arc<Mutex<MergeStats>>,
    // Bug 186 (N16): single-point read uses _with_retry variant.
    /// None 时不累计指标（保持向后兼容测试场景）。
    metrics: Option<Arc<crate::engine::metrics::Metrics>>,
}

impl ModbusCollector {
    pub fn new(_bus_name: &str, bus: Arc<BusManager>) -> Self {
        Self {
            bus,
            merge_stats: Arc::new(Mutex::new(MergeStats::new())),
            // Bug 186 (N16): single-point read uses _with_retry variant.
            metrics: None,
        }
    }
}

#[cfg(test)]
mod tests;
