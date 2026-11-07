//! Linux 平台的主机指标采集实现。
//!
//! 数据来源：`/proc/stat`、`/proc/meminfo`、`/proc/loadavg`、`/proc/uptime`、
//! `/sys/class/thermal`（退化到 `/sys/class/hwmon`）、`/sys/class/net` + `getifaddrs`。

use super::parsers::{
    parse_loadavg, parse_meminfo, parse_proc_stat, parse_thermal_milli, parse_uptime,
};
use super::{cpu_usage_from_counters, NetInterfaceMetric};

/// 读取 `/proc/stat` 增量得到 CPU 使用率（%）
pub(super) fn cpu_usage_percent() -> Option<f64> {
    let text = std::fs::read_to_string("/proc/stat").ok()?;
    let (idle, total) = parse_proc_stat(&text)?;
    cpu_usage_from_counters(idle, total)
}

/// 读取 `/proc/meminfo` 得到内存使用率（%）
pub(super) fn mem_usage_percent() -> Option<f64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let (total, available) = parse_meminfo(&text)?;
    if total == 0 {
        return None;
    }
    let used = total.saturating_sub(available);
    Some((used as f64 / total as f64 * 100.0).clamp(0.0, 100.0))
}

/// 读取 `/proc/loadavg` 的 1 分钟负载
pub(super) fn load_avg_1m() -> Option<f64> {
    let text = std::fs::read_to_string("/proc/loadavg").ok()?;
    parse_loadavg(&text)
}

/// 读取 `/proc/uptime` 的运行秒数
pub(super) fn uptime_secs() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/uptime").ok()?;
    parse_uptime(&text)
}

/// 读取 CPU 温度：优先 thermal_zone，退化为 hwmon 的 temp*_input。
///
/// 多传感器取最大值——运维关心的是最热的那颗核心/封装。
pub(super) fn cpu_temp_celsius() -> Option<f64> {
    let mut hottest: Option<f64> = None;
    if let Ok(dir) = std::fs::read_dir("/sys/class/thermal") {
        for entry in dir.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("thermal_zone") {
                continue;
            }
            let value = std::fs::read_to_string(entry.path().join("temp"))
                .ok()
                .and_then(|s| parse_thermal_milli(&s));
            if let Some(t) = value {
                hottest = Some(hottest.map_or(t, |h| h.max(t)));
            }
        }
    }
    if hottest.is_some() {
        return hottest;
    }
    // 容器/虚拟化环境常无 thermal_zone，回退 hwmon
    if let Ok(dir) = std::fs::read_dir("/sys/class/hwmon") {
        for entry in dir.flatten() {
            if let Ok(files) = std::fs::read_dir(entry.path()) {
                for file in files.flatten() {
                    let name = file.file_name().to_string_lossy().into_owned();
                    if !(name.starts_with("temp") && name.ends_with("_input")) {
                        continue;
                    }
                    let value = std::fs::read_to_string(file.path())
                        .ok()
                        .and_then(|s| parse_thermal_milli(&s));
                    if let Some(t) = value {
                        hottest = Some(hottest.map_or(t, |h| h.max(t)));
                    }
                }
            }
        }
    }
    hottest
}

/// 统计 `/proc` 下的纯数字目录数即为进程数
pub(super) fn process_count() -> Option<u32> {
    let dir = std::fs::read_dir("/proc").ok()?;
    let count = dir
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            !name.is_empty() && name.chars().all(|c| c.is_ascii_digit())
        })
        .count();
    u32::try_from(count).ok()
}

/// 网卡列表：/sys/class/net 提供状态与字节计数，getifaddrs 提供 IPv4 地址
pub(super) fn net_interfaces() -> Vec<NetInterfaceMetric> {
    let mut addresses = ipv4_by_interface();
    let mut result = Vec::new();
    let Ok(dir) = std::fs::read_dir("/sys/class/net") else {
        return result;
    };
    for entry in dir.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // loopback 对运维无意义，噪声项
        if name == "lo" {
            continue;
        }
        let base = entry.path();
        // 计数器/状态读失败时不能静默当 0 / down：运维会把「读不到」误读成
        // 「网卡流量为 0」「网卡已 down」，从而误判现场。失败时留痕。
        let rx_bytes = match std::fs::read_to_string(base.join("statistics/rx_bytes")) {
            Ok(s) => s.trim().parse().unwrap_or(0),
            Err(e) => {
                tracing::warn!("host metrics: read rx_bytes of {} failed: {}", name, e);
                0
            }
        };
        let tx_bytes = match std::fs::read_to_string(base.join("statistics/tx_bytes")) {
            Ok(s) => s.trim().parse().unwrap_or(0),
            Err(e) => {
                tracing::warn!("host metrics: read tx_bytes of {} failed: {}", name, e);
                0
            }
        };
        let up = match std::fs::read_to_string(base.join("operstate")) {
            Ok(s) => s.trim() == "up",
            Err(e) => {
                tracing::warn!("host metrics: read operstate of {} failed: {}", name, e);
                false
            }
        };
        let ip = addresses.remove(&name).unwrap_or_else(|| "-".to_string());
        result.push(NetInterfaceMetric {
            name,
            ip,
            up,
            rx_bytes,
            tx_bytes,
        });
    }
    result.sort_by(|a, b| a.name.cmp(&b.name));
    result
}

/// 用 getifaddrs 取各接口的 IPv4 地址（同一接口多地址时取第一个）
pub(super) fn ipv4_by_interface() -> std::collections::HashMap<String, String> {
    use std::collections::HashMap;
    use std::ffi::CStr;

    let mut map = HashMap::new();
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY:
    // - getifaddrs 成功时返回 0 并填充 head 指向的链表；失败不写 head。
    // - 链表节点由系统分配，freeifaddrs 统一释放；遍历期间不修改链表。
    // - ifa_name 指向 NUL 结尾字符串，CStr::from_ptr 仅在其生命周期内读取。
    // - ifa_addr 已判空且 sa_family 校验为 AF_INET 后才按 sockaddr_in 解释。
    unsafe {
        if libc::getifaddrs(&mut head) != 0 {
            return map;
        }
        let mut cursor = head;
        while !cursor.is_null() {
            let ifa = &*cursor;
            if !ifa.ifa_addr.is_null() && (*ifa.ifa_addr).sa_family as i32 == libc::AF_INET {
                let name = CStr::from_ptr(ifa.ifa_name).to_string_lossy().into_owned();
                let addr = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                let octets = u32::from_be(addr.sin_addr.s_addr).to_be_bytes();
                map.entry(name).or_insert_with(|| {
                    format!("{}.{}.{}.{}", octets[0], octets[1], octets[2], octets[3])
                });
            }
            cursor = ifa.ifa_next;
        }
        libc::freeifaddrs(head);
    }
    map
}
