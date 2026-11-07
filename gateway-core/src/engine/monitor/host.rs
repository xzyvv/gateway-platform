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

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::sync::atomic::{AtomicU64, Ordering};

use crate::engine::monitor::health;

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

/// 采集一次主机指标。磁盘使用率针对数据分区（`GATEWAY_DATA_DIR` 或平台默认盘）。
pub fn collect() -> HostMetrics {
    HostMetrics {
        cpu_usage_pct: cpu_usage_percent(),
        mem_usage_pct: mem_usage_percent(),
        disk_usage_pct: health::disk_usage_percent(&health::disk_check_path()).ok(),
        cpu_temp_c: cpu_temp_celsius(),
        load_avg_1m: load_avg_1m(),
        process_count: process_count(),
        uptime_secs: uptime_secs(),
        net_interfaces: net_interfaces(),
    }
}

/// 跨平台 CPU 使用率（%），供 `/api/status`、`/api/watchdog/services` 等
/// 轻量端点复用（这些端点只需要 CPU / 内存两项，不必付 `collect()` 的其余采集成本）。
///
/// 与 `collect()` 共用同一份进程级基线（`CPU_LAST_*`）：首次调用只建立基线返回
/// `None`，调用方应短暂等待后复采一次；之后按两次采样的增量计算。
/// 历史上这几处各写一份 `/proc/stat` 解析，Windows 上直接恒为 0。
pub fn cpu_usage_pct_now() -> Option<f64> {
    cpu_usage_percent()
}

/// 跨平台内存使用率（%）。读不到/不支持的平台返回 `None`。
pub fn mem_usage_pct_now() -> Option<f64> {
    mem_usage_percent()
}

// ============================================================
// Linux 实现
// ============================================================

/// 解析 `/proc/stat` 首行（`cpu` 汇总行），返回 (idle 累计, total 累计)
///
/// 字段顺序：user nice system idle iowait irq softirq steal [guest guest_nice]。
/// guest/guest_nice 已计入 user/nice，不能再累加，否则 total 虚高。
fn parse_proc_stat(text: &str) -> Option<(u64, u64)> {
    let line = text.lines().next()?;
    let mut fields = line.split_whitespace();
    if fields.next()? != "cpu" {
        return None;
    }
    let values: Vec<u64> = fields.filter_map(|v| v.parse().ok()).collect();
    if values.len() < 4 {
        return None;
    }
    let idle = values[3] + values.get(4).copied().unwrap_or(0); // idle + iowait
    let total: u64 = values.iter().take(8).sum();
    Some((idle, total))
}

/// 解析 `/proc/meminfo`，返回 (总内存, 可用内存)，单位 kB
fn parse_meminfo(text: &str) -> Option<(u64, u64)> {
    let mut total = None;
    let mut available = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            total = rest.split_whitespace().next().and_then(|v| v.parse().ok());
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            available = rest.split_whitespace().next().and_then(|v| v.parse().ok());
        }
    }
    let (total, available): (u64, u64) = (total?, available?);
    if total == 0 {
        return None;
    }
    Some((total, available))
}

/// 解析 `/proc/loadavg` 的 1 分钟负载
fn parse_loadavg(text: &str) -> Option<f64> {
    text.split_whitespace().next()?.parse().ok()
}

/// 解析 `/proc/uptime` 的运行秒数（首字段为浮点）
fn parse_uptime(text: &str) -> Option<u64> {
    let secs: f64 = text.split_whitespace().next()?.parse().ok()?;
    if secs < 0.0 {
        return None;
    }
    Some(secs as u64)
}

/// 解析 thermal zone 的毫摄氏度值
fn parse_thermal_milli(text: &str) -> Option<f64> {
    let milli: i64 = text.trim().parse().ok()?;
    // 0 或负值通常表示传感器未就绪，不当作有效温度
    if milli <= 0 {
        return None;
    }
    Some(milli as f64 / 1000.0)
}

#[cfg(target_os = "linux")]
fn cpu_usage_percent() -> Option<f64> {
    let text = std::fs::read_to_string("/proc/stat").ok()?;
    let (idle, total) = parse_proc_stat(&text)?;
    cpu_usage_from_counters(idle, total)
}

#[cfg(target_os = "linux")]
fn mem_usage_percent() -> Option<f64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let (total, available) = parse_meminfo(&text)?;
    if total == 0 {
        return None;
    }
    let used = total.saturating_sub(available);
    Some((used as f64 / total as f64 * 100.0).clamp(0.0, 100.0))
}

#[cfg(target_os = "linux")]
fn load_avg_1m() -> Option<f64> {
    let text = std::fs::read_to_string("/proc/loadavg").ok()?;
    parse_loadavg(&text)
}

#[cfg(target_os = "linux")]
fn uptime_secs() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/uptime").ok()?;
    parse_uptime(&text)
}

/// 读取 CPU 温度：优先 thermal_zone，退化为 hwmon 的 temp*_input。
///
/// 多传感器取最大值——运维关心的是最热的那颗核心/封装。
#[cfg(target_os = "linux")]
fn cpu_temp_celsius() -> Option<f64> {
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
#[cfg(target_os = "linux")]
fn process_count() -> Option<u32> {
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
#[cfg(target_os = "linux")]
fn net_interfaces() -> Vec<NetInterfaceMetric> {
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
#[cfg(target_os = "linux")]
fn ipv4_by_interface() -> std::collections::HashMap<String, String> {
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

// ============================================================
// Windows 实现
// ============================================================

#[cfg(windows)]
#[repr(C)]
#[derive(Default)]
struct FileTime {
    low: u32,
    high: u32,
}

#[cfg(windows)]
impl FileTime {
    fn to_u64(&self) -> u64 {
        ((self.high as u64) << 32) | self.low as u64
    }
}

#[cfg(windows)]
fn cpu_usage_percent() -> Option<f64> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetSystemTimes(
            idle_time: *mut FileTime,
            kernel_time: *mut FileTime,
            user_time: *mut FileTime,
        ) -> i32;
    }

    let mut idle = FileTime::default();
    let mut kernel = FileTime::default();
    let mut user = FileTime::default();
    // SAFETY: 三个输出参数均指向栈上已初始化的 FileTime，GetSystemTimes 成功后写入。
    let ok = unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) };
    if ok == 0 {
        return None;
    }
    // kernel 已包含 idle，故 total = kernel + user
    let total = kernel.to_u64().saturating_add(user.to_u64());
    let idle = idle.to_u64();
    cpu_usage_from_counters(idle, total)
}

#[cfg(windows)]
fn mem_usage_percent() -> Option<f64> {
    #[repr(C)]
    #[derive(Default)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
    }

    let mut status = MemoryStatusEx {
        length: std::mem::size_of::<MemoryStatusEx>() as u32,
        ..MemoryStatusEx::default()
    };
    // SAFETY: status 为栈上已初始化的结构体，length 已按 ABI 填好，成功时写入。
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
    if ok == 0 {
        return None;
    }
    let pct = status.memory_load as f64;
    Some(pct.clamp(0.0, 100.0))
}

#[cfg(windows)]
fn uptime_secs() -> Option<u64> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetTickCount64() -> u64;
    }
    // SAFETY: 无参数、无输出指针，返回自开机以来的毫秒数。
    Some(unsafe { GetTickCount64() } / 1000)
}

#[cfg(windows)]
fn process_count() -> Option<u32> {
    #[link(name = "psapi")]
    extern "system" {
        fn EnumProcesses(process_ids: *mut u32, bytes: u32, needed: *mut u32) -> i32;
    }

    // 逐次扩容：进程数超过容量时 next_needed 会等于容量，需要更大缓冲
    let mut capacity: u32 = 1024;
    for _ in 0..3 {
        let mut ids = vec![0u32; capacity as usize];
        let mut needed: u32 = 0;
        let bytes = capacity * 4;
        // SAFETY: ids 分配了 capacity 个 u32，字节长度与 bytes 一致；
        // needed 为栈上输出参数，EnumProcesses 成功后写入实际字节数。
        let ok = unsafe { EnumProcesses(ids.as_mut_ptr(), bytes, &mut needed) };
        if ok == 0 {
            return None;
        }
        if needed < bytes {
            let count = needed / 4;
            return Some(count);
        }
        capacity = capacity.saturating_mul(4);
    }
    None
}

/// 内存型虚拟化环境常见 CPU 温度不可得，如实返回 None
#[cfg(windows)]
fn cpu_temp_celsius() -> Option<f64> {
    None
}

/// Windows 无 loadavg 概念，如实返回 None
#[cfg(windows)]
fn load_avg_1m() -> Option<f64> {
    None
}

/// 枚举本机网卡（不含 loopback）。
///
/// 两个系统 API 分工：`GetAdaptersAddresses` 给出接口名称、IPv4 与操作状态，
/// `GetIfTable2` 给出累计收发字节；两者用 LUID（接口的稳定唯一标识）关联。
#[cfg(windows)]
fn net_interfaces() -> Vec<NetInterfaceMetric> {
    let stats = if_table_rows();
    let mut result: Vec<NetInterfaceMetric> = adapter_rows()
        .into_iter()
        .map(|(luid, name, ip, up)| {
            let (rx_bytes, tx_bytes) = stats
                .iter()
                .find(|(row_luid, _, _)| *row_luid == luid)
                .map(|(_, rx, tx)| (*rx, *tx))
                .unwrap_or((0, 0));
            NetInterfaceMetric {
                name,
                ip,
                up,
                rx_bytes,
                tx_bytes,
            }
        })
        .collect();
    result.sort_by(|a, b| a.name.cmp(&b.name));
    result
}

/// `GetIfTable2` 结果：(LUID, 累计接收字节, 累计发送字节)
#[cfg(windows)]
type IfTableRow = (u64, u64, u64);

#[cfg(windows)]
fn if_table_rows() -> Vec<IfTableRow> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        FreeMibTable, GetIfTable2, MIB_IF_TABLE2,
    };

    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    // SAFETY:
    // - table 是栈上输出参数，GetIfTable2 成功（返回 0）时由系统分配 `MIB_IF_TABLE2`
    //   （`Table` 是尾部变长数组，长度由 `NumEntries` 决定），必须用 `FreeMibTable` 释放。
    // - 读取期间只读；先把需要字段拷贝进 Vec，再释放，避免悬垂引用。
    unsafe {
        if GetIfTable2(&mut table) != 0 || table.is_null() {
            return Vec::new();
        }
        let header = &*table;
        let rows = std::slice::from_raw_parts(header.Table.as_ptr(), header.NumEntries as usize);
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            // 读取 union 字段需要 unsafe。
            out.push((row.InterfaceLuid.Value, row.InOctets, row.OutOctets));
        }
        FreeMibTable(table as *const core::ffi::c_void);
        out
    }
}

/// `GetAdaptersAddresses` 结果：(LUID, 接口名, IPv4, 是否 up)
#[cfg(windows)]
type AdapterRow = (u64, String, String, bool);

#[cfg(windows)]
fn adapter_rows() -> Vec<AdapterRow> {
    use windows_sys::Win32::Foundation::ERROR_BUFFER_OVERFLOW;
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER,
        GAA_FLAG_SKIP_MULTICAST, IF_TYPE_SOFTWARE_LOOPBACK, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows_sys::Win32::Networking::WinSock::AF_UNSPEC;

    const FLAGS: u32 = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;

    // 用 u64 为单位分配，保证 8 字节对齐（`IP_ADAPTER_ADDRESSES_LH` 含 u64 字段）。
    let mut size: u32 = 16 * 1024;
    let mut buf: Vec<u64> = Vec::new();
    let mut ok = false;
    for _ in 0..3 {
        buf = vec![0u64; (size as usize).div_ceil(8)];
        // SAFETY: buf 的容量不小于 size 字节且地址 8 字节对齐；调用成功后缓冲区
        // 内是系统填充的链表（Next 指向同一缓冲区中的下一个节点，末节点为空）。
        // 失败且为 ERROR_BUFFER_OVERFLOW 时，size 被写成所需字节数。
        let ret = unsafe {
            GetAdaptersAddresses(
                AF_UNSPEC as u32,
                FLAGS,
                std::ptr::null(),
                buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH,
                &mut size,
            )
        };
        if ret == 0 {
            ok = true;
            break;
        }
        if ret != ERROR_BUFFER_OVERFLOW {
            // 其余错误（如 ERROR_NO_DATA）视为无网卡，如实返回空。
            return Vec::new();
        }
    }
    if !ok {
        return Vec::new();
    }

    let mut out = Vec::new();
    let mut cursor = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    // SAFETY: 以上调用成功，buf 内的链表节点均由系统填充；Next 为空表示链表结束。
    unsafe {
        while !cursor.is_null() {
            let adapter = &*cursor;
            if adapter.IfType != IF_TYPE_SOFTWARE_LOOPBACK {
                // 读取 union 字段（Luid）需要 unsafe。
                let name = pwstr_to_string(adapter.FriendlyName)
                    .filter(|s| !s.is_empty())
                    .or_else(|| pstr_to_string(adapter.AdapterName))
                    .unwrap_or_else(|| format!("if{}", adapter.Luid.Value));
                let ip = first_ipv4(adapter.FirstUnicastAddress).unwrap_or_else(|| "-".to_string());
                out.push((
                    adapter.Luid.Value,
                    name,
                    ip,
                    adapter.OperStatus == IfOperStatusUp,
                ));
            }
            cursor = adapter.Next;
        }
    }
    out
}

/// UTF-16（NUL 结尾）转 String；空串或空指针返回 None。
///
/// # Safety
/// `p` 必须指向以 0 结尾、长度为 `len` 的 UTF-16 序列（系统 API 返回的 FriendlyName）。
#[cfg(windows)]
unsafe fn pwstr_to_string(p: *const u16) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let mut len = 0usize;
    while *p.add(len) != 0 {
        len += 1;
    }
    if len == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, len)))
}

/// ANSI（NUL 结尾）转 String；空串或空指针返回 None。
#[cfg(windows)]
fn pstr_to_string(p: *const u8) -> Option<String> {
    if p.is_null() {
        return None;
    }
    // SAFETY: AdapterName 按 API 约定指向 NUL 结尾的短字符串（适配器 GUID）。
    let text = unsafe { std::ffi::CStr::from_ptr(p as *const std::ffi::c_char) }
        .to_string_lossy()
        .into_owned();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// 取该适配器的第一个 IPv4 地址（多地址时按 API 链表顺序取首个）。
///
/// # Safety
/// `cur` 必须为 `GetAdaptersAddresses` 返回的单播地址链表（或空指针）。
#[cfg(windows)]
unsafe fn first_ipv4(
    mut cur: *mut windows_sys::Win32::NetworkManagement::IpHelper::IP_ADAPTER_UNICAST_ADDRESS_LH,
) -> Option<String> {
    use windows_sys::Win32::Networking::WinSock::AF_INET;

    while !cur.is_null() {
        let item = &*cur;
        let sa = item.Address.lpSockaddr;
        if !sa.is_null() && (*sa).sa_family == AF_INET {
            // sockaddr_in 布局：sa_family(2) + sin_port(2) + sin_addr(4)，
            // 即 sa_data[2..6] 为大端序 IPv4 的四个八位组。
            let d = (*sa).sa_data;
            return Some(format!(
                "{}.{}.{}.{}",
                d[2] as u8, d[3] as u8, d[4] as u8, d[5] as u8
            ));
        }
        cur = item.Next;
    }
    None
}

// ============================================================
// 其它平台：全部如实返回未知
// ============================================================

#[cfg(all(not(target_os = "linux"), not(windows)))]
fn cpu_usage_percent() -> Option<f64> {
    None
}

#[cfg(all(not(target_os = "linux"), not(windows)))]
fn mem_usage_percent() -> Option<f64> {
    None
}

#[cfg(all(not(target_os = "linux"), not(windows)))]
fn cpu_temp_celsius() -> Option<f64> {
    None
}

#[cfg(all(not(target_os = "linux"), not(windows)))]
fn load_avg_1m() -> Option<f64> {
    None
}

#[cfg(all(not(target_os = "linux"), not(windows)))]
fn process_count() -> Option<u32> {
    None
}

#[cfg(all(not(target_os = "linux"), not(windows)))]
fn uptime_secs() -> Option<u64> {
    None
}

#[cfg(all(not(target_os = "linux"), not(windows)))]
fn net_interfaces() -> Vec<NetInterfaceMetric> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_proc_stat_handles_typical_line() {
        let text = "cpu  100 20 30 400 50 5 5 0 0 0\ncpu0 1 2 3 4 5 6 7 8 9 10\n";
        let (idle, total) = parse_proc_stat(text).expect("应能解析");
        // idle = idle(400) + iowait(50) = 450
        assert_eq!(idle, 450);
        // total = user..steal 前 8 项 = 100+20+30+400+50+5+5+0 = 610
        assert_eq!(total, 610);
        // guest/guest_nice 不计入 total，避免重复累加
    }

    #[test]
    fn parse_proc_stat_rejects_short_or_foreign_lines() {
        assert!(parse_proc_stat("intr 123 456\n").is_none());
        assert!(parse_proc_stat("cpu 1 2 3\n").is_none());
        assert!(parse_proc_stat("").is_none());
    }

    #[test]
    fn parse_meminfo_computes_used_memory() {
        let text = "MemTotal:       16000000 kB\nMemFree:         1000000 kB\nMemAvailable:    6000000 kB\n";
        let (total, available) = parse_meminfo(text).expect("应能解析");
        assert_eq!(total, 16_000_000);
        assert_eq!(available, 6_000_000);
        let used_pct = (total - available) as f64 / total as f64 * 100.0;
        assert!((used_pct - 62.5).abs() < 0.01);
    }

    #[test]
    fn parse_meminfo_requires_available_field() {
        // 老内核无 MemAvailable：返回 None，由调用方降级，而不是按 0 计算
        let text = "MemTotal: 1000 kB\nMemFree: 200 kB\n";
        assert!(parse_meminfo(text).is_none());
        assert!(parse_meminfo("MemTotal: 0 kB\nMemAvailable: 0 kB\n").is_none());
    }

    #[test]
    fn parse_loadavg_and_uptime() {
        assert_eq!(parse_loadavg("0.52 0.58 0.59 1/400 12345"), Some(0.52));
        assert!(parse_loadavg("").is_none());
        assert_eq!(parse_uptime("12345.67 45678.90"), Some(12345));
        assert!(parse_uptime("abc 1.0").is_none());
    }

    #[test]
    fn parse_thermal_milli_rejects_non_values() {
        assert_eq!(parse_thermal_milli("45000\n"), Some(45.0));
        assert_eq!(parse_thermal_milli("  0  "), None);
        assert_eq!(parse_thermal_milli("-5000"), None);
        assert_eq!(parse_thermal_milli("n/a"), None);
    }

    #[test]
    fn cpu_usage_needs_baseline_then_reports_delta() {
        // 200 个时间片里 50 个 idle → 使用率 75%（纯函数，不依赖进程级基线）
        let usage = usage_pct(
            10_000_000_000,
            20_000_000_000,
            10_000_000_050,
            20_000_000_200,
        )
        .expect("有增量应返回使用率");
        assert!((usage - 75.0).abs() < 0.01, "usage={usage}");
    }

    #[test]
    fn cpu_usage_is_clamped_and_handles_zero_delta() {
        assert!(usage_pct(100, 200, 100, 200).is_none(), "无增量应返回 None");
        // idle 增量大于总增量（计数回绕/异常）时钳到 0，不出现负使用率
        assert_eq!(usage_pct(0, 0, 300, 200), Some(0.0));
    }

    /// 冒烟：在本机真实读取，确认采集链路没有返回"永远未知"。
    /// 只断言跨平台可直接读取的项；CPU 使用率依赖进程级基线，故由纯函数用例覆盖。
    #[test]
    fn collect_reads_real_platform_metrics() {
        let metrics = collect();
        if cfg!(target_os = "linux") || cfg!(windows) {
            assert!(metrics.mem_usage_pct.is_some(), "内存使用率应可读取");
            assert!(metrics.uptime_secs.is_some(), "运行时长应可读取");
            assert!(metrics.process_count.is_some(), "进程数应可读取");
            let mem = metrics.mem_usage_pct.unwrap_or(-1.0);
            assert!(
                (0.0..=100.0).contains(&mem),
                "内存使用率应在 0-100 之间: {mem}"
            );
        }
        if cfg!(target_os = "linux") {
            // Linux 一定有至少一个非 loopback 接口（容器内通常是 eth0）
            assert!(!metrics.net_interfaces.is_empty(), "应能枚举网卡");
            assert!(
                metrics.net_interfaces.iter().all(|n| n.name != "lo"),
                "loopback 不参与展示"
            );
        }
    }

    /// 网卡枚举的形状校验：字段非空、不含回环；Windows 下必须能真实枚举到网卡。
    #[test]
    fn net_interfaces_are_well_formed() {
        let interfaces = net_interfaces();
        for iface in &interfaces {
            assert!(!iface.name.is_empty(), "接口名不应为空");
            assert!(!iface.ip.is_empty(), "IP 列应为地址或 \"-\"");
            assert!(
                !iface.name.to_ascii_lowercase().contains("loopback"),
                "回环接口不参与展示: {}",
                iface.name
            );
        }
        if cfg!(windows) {
            // Windows 主机必然有至少一块网卡（物理或虚拟）；枚举为空说明系统 API 链路异常，
            // 此时页面只剩误导性的「暂无网络接口」，应当让测试失败。
            assert!(!interfaces.is_empty(), "Windows 应能枚举到网卡");
        }
    }

    /// Linux 专属：`/proc` 真实读取链路必须能拿到数据。
    ///
    /// 这些分支在非 Linux 平台上直接返回 `None`，因此只在 Linux 上断言。
    /// CI 的 `linux-runtime` job 专门跑这一组，确保 Linux 采集链路在
    /// 真实内核接口上不退化（解析逻辑本身由纯函数用例覆盖）。
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_proc_readers_return_real_values() {
        let load = load_avg_1m().expect("Linux 应能读 /proc/loadavg");
        assert!(load >= 0.0, "负载不应为负: {load}");

        let uptime = uptime_secs().expect("Linux 应能读 /proc/uptime");
        assert!(uptime > 0, "运行时长应大于 0: {uptime}");

        let mem = mem_usage_percent().expect("Linux 应能读 /proc/meminfo");
        assert!(
            (0.0..=100.0).contains(&mem),
            "内存使用率应在 0-100 之间: {mem}"
        );

        let procs = process_count().expect("Linux 应能枚举 /proc 进程");
        assert!(procs > 0, "进程数应大于 0: {procs}");

        // /proc/stat 必须可读（CPU 使用率本身依赖进程级基线，由纯函数用例覆盖）
        let stat = std::fs::read_to_string("/proc/stat").expect("Linux 应能读 /proc/stat");
        assert!(
            parse_proc_stat(&stat).is_some(),
            "/proc/stat 首行应为可解析的 cpu 汇总行"
        );
    }

    /// Linux 专属：`getifaddrs` 必须能取到回环接口的 IPv4 地址。
    ///
    /// 取不到说明 libc 调用或 AF_INET 过滤写错了——现场 IPv4 展示会整列为空。
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_ipv4_lookup_finds_loopback() {
        let addrs = ipv4_by_interface();
        let lo = addrs.get("lo").expect("Linux 回环接口应有 IPv4 地址");
        assert_eq!(lo, "127.0.0.1", "回环接口 IPv4 应为 127.0.0.1");
    }
}
