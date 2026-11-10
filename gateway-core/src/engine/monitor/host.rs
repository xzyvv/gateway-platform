//! 主机运行时指标采集（真实值）
//!
//! 历史背景：`/api/diagnostics/gateway` 曾用伪随机数生成 CPU 温度/使用率、内存、磁盘、
//! 负载、进程数，页面上的数字与设备真实状态无关——运维会据此误判设备是否过热、容量是否够用。
//! 本模块改为直接读取操作系统计数器。
//!
//! | 指标 | Linux | Windows |
//! |---|---|---|
//! | CPU 使用率 | `/proc/stat` 增量 | `GetSystemTimes` 增量 |
//! | 内存使用率 | `/proc/meminfo` | `GlobalMemoryStatusEx` |
//! | 磁盘使用率 | `statvfs` | `GetDiskFreeSpaceExW` |
//! | CPU 温度 | `/sys/class/thermal` | 不支持 → `None` |
//! | 负载均值 | `/proc/loadavg` | 无对应概念 → `None` |
//! | 进程数 | `/proc` 数字目录计数 | `EnumProcesses` |
//! | 运行时长 | `/proc/uptime` | `GetTickCount64` |
//! | 网卡列表 | `/sys/class/net` + `getifaddrs` | `GetAdaptersAddresses` + `GetIfTable2` |
//!
//! 所有标量均为 `Option`：读不到或不支持时返回 `None`，由 API 层渲染为「—」。
//! 宁可显示未知，也不要编造一个看起来合理的数字。
//!
//! ## 模块结构（O-6 拆分）
//!
//! | 文件 | 职责 |
//! |---|---|
//! | `host.rs`（本文件） | 公共类型、CPU 基线状态、跨平台入口 `collect()` |
//! | `host/parsers.rs` | 纯文本解析函数（`/proc/*` 文本 → 数值） |
//! | `host/linux.rs` | Linux 采集实现 |
//! | `host/windows.rs` | Windows 采集实现 |
//! | `host/other.rs` | 其它平台：全部如实返回未知 |
//! | `host/tests.rs` | 单元测试 |

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::sync::atomic::{AtomicU64, Ordering};

use crate::engine::monitor::health;

mod parsers;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use self::linux as platform;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use self::windows as platform;

#[cfg(all(not(target_os = "linux"), not(windows)))]
mod other;
#[cfg(all(not(target_os = "linux"), not(windows)))]
use other as platform;

/// 单块网卡的运行时指标
#[derive(Debug, Clone, PartialEq)]
pub struct NetInterfaceMetric {
    /// 接口名（如 eth0）
    pub name: String,
    /// IPv4 地址；无地址时为 "-"
    pub ip: String,
    /// 是否处于 up 状态
    pub up: bool,
    /// 累计接收字节
    pub rx_bytes: u64,
    /// 累计发送字节
    pub tx_bytes: u64,
}

/// 主机指标快照
#[derive(Debug, Clone, Default)]
pub struct HostMetrics {
    /// CPU 使用率（%）。首次采样无基线时为 None
    pub cpu_usage_pct: Option<f64>,
    /// 内存使用率（%）
    pub mem_usage_pct: Option<f64>,
    /// 数据分区磁盘使用率（%）
    pub disk_usage_pct: Option<f64>,
    /// CPU 温度（°C）
    pub cpu_temp_c: Option<f64>,
    /// 1 分钟平均负载
    pub load_avg_1m: Option<f64>,
    /// 进程数
    pub process_count: Option<u32>,
    /// 系统运行时长（秒）
    pub uptime_secs: Option<u64>,
    /// 网卡列表（不含 loopback）
    pub net_interfaces: Vec<NetInterfaceMetric>,
}

/// CPU 时间片基线（上一次采样的 idle / total 累计值，单位随平台不同）
static CPU_LAST_IDLE: AtomicU64 = AtomicU64::new(0);
static CPU_LAST_TOTAL: AtomicU64 = AtomicU64::new(0);
static CPU_HAS_BASELINE: AtomicU64 = AtomicU64::new(0);

/// 纯函数：由前后两次累计计数求 CPU 使用率；无增量返回 `None`。
///
/// 与全局基线分离，便于确定性测试；全局状态只由 [`cpu_usage_from_counters`] 维护。
fn usage_pct(prev_idle: u64, prev_total: u64, idle: u64, total: u64) -> Option<f64> {
    let d_total = total.saturating_sub(prev_total);
    if d_total == 0 {
        return None;
    }
    let d_idle = idle.saturating_sub(prev_idle);
    Some(((1.0 - d_idle as f64 / d_total as f64) * 100.0).clamp(0.0, 100.0))
}

/// 由累计 idle / total 计数与上次基线计算使用率，并刷新基线。
///
/// CPU 使用率是区间量：没有上一次采样就无法得出有意义的百分比，因此首次调用只建立
/// 基线并返回 `None`。调用方可先调一次 [`collect`] 再短暂等待后复采，避免首屏显示「—」。
fn cpu_usage_from_counters(idle: u64, total: u64) -> Option<f64> {
    let has_baseline = CPU_HAS_BASELINE.load(Ordering::Relaxed) != 0;
    let last_idle = CPU_LAST_IDLE.load(Ordering::Relaxed);
    let last_total = CPU_LAST_TOTAL.load(Ordering::Relaxed);

    if !has_baseline {
        store_baseline(idle, total);
        return None;
    }
    let usage = usage_pct(last_idle, last_total, idle, total);
    // 计数器尚未推进（Windows 系统时间约 15ms 更新一次，并发采样可能落在同一 tick）。
    // 此时保留旧基线，否则会把基线推进到同一时刻，后续调用依然算不出增量。
    if usage.is_some() {
        store_baseline(idle, total);
    }
    usage
}

fn store_baseline(idle: u64, total: u64) {
    CPU_LAST_IDLE.store(idle, Ordering::Relaxed);
    CPU_LAST_TOTAL.store(total, Ordering::Relaxed);
    CPU_HAS_BASELINE.store(1, Ordering::Relaxed);
}

/// 采集一次主机指标。磁盘使用率针对数据分区（`GATEWAY_DATA_DIR` 或队列库所在目录）。
pub fn collect() -> HostMetrics {
    HostMetrics {
        cpu_usage_pct: platform::cpu_usage_percent(),
        mem_usage_pct: platform::mem_usage_percent(),
        disk_usage_pct: health::disk_usage_percent(&health::disk_check_path()).ok(),
        cpu_temp_c: platform::cpu_temp_celsius(),
        load_avg_1m: platform::load_avg_1m(),
        process_count: platform::process_count(),
        uptime_secs: platform::uptime_secs(),
        net_interfaces: platform::net_interfaces(),
    }
}

/// 跨平台 CPU 使用率（%），供 `/api/status`、`/api/watchdog/services` 等
/// 轻量端点复用（这些端点只需要 CPU / 内存两项，不必付 `collect()` 的其余采集成本）。
///
/// 与 `collect()` 共用同一份进程级基线（`CPU_LAST_*`）：首次调用只建立基线返回
/// `None`，调用方应短暂等待后复采一次；之后按两次采样的增量计算。
/// 历史上这几处各写一份 `/proc/stat` 解析，Windows 上直接恒为 0。
pub fn cpu_usage_pct_now() -> Option<f64> {
    platform::cpu_usage_percent()
}

/// 跨平台内存使用率（%）。读不到/不支持的平台返回 `None`。
pub fn mem_usage_pct_now() -> Option<f64> {
    platform::mem_usage_percent()
}

#[cfg(test)]
mod tests;
