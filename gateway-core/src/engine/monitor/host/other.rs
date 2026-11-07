//! 其它平台的主机指标采集实现：全部如实返回未知。
//!
//! 与 Windows 的区别是 Windows 至少能读到 CPU / 内存 / 进程数 / 网卡；
//! 这里没有任何对应的系统 API 可用，因此所有指标返回 `None` / 空列表，
//! 由 API 层渲染为「—」。宁可显示未知，也不要编造一个看起来合理的数字。

use super::NetInterfaceMetric;

/// 其它平台无 CPU 使用率计数器
pub(super) fn cpu_usage_percent() -> Option<f64> {
    None
}

/// 其它平台无内存计数器
pub(super) fn mem_usage_percent() -> Option<f64> {
    None
}

/// 其它平台无 CPU 温度传感器接口
pub(super) fn cpu_temp_celsius() -> Option<f64> {
    None
}

/// 其它平台无 loadavg 概念
pub(super) fn load_avg_1m() -> Option<f64> {
    None
}

/// 其它平台无进程枚举接口
pub(super) fn process_count() -> Option<u32> {
    None
}

/// 其它平台无运行时长接口
pub(super) fn uptime_secs() -> Option<u64> {
    None
}

/// 其它平台无网卡枚举接口
pub(super) fn net_interfaces() -> Vec<NetInterfaceMetric> {
    Vec::new()
}
