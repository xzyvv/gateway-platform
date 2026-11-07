//! Windows 平台的主机指标采集实现。
//!
//! 数据来源：`GetSystemTimes`（CPU）、`GlobalMemoryStatusEx`（内存）、
//! `GetTickCount64`（运行时长）、`EnumProcesses`（进程数）、
//! `GetAdaptersAddresses` + `GetIfTable2`（网卡）。

use super::{cpu_usage_from_counters, NetInterfaceMetric};

#[repr(C)]
#[derive(Default)]
struct FileTime {
    low: u32,
    high: u32,
}

impl FileTime {
    fn to_u64(&self) -> u64 {
        ((self.high as u64) << 32) | self.low as u64
    }
}

/// `GetSystemTimes` 增量得到 CPU 使用率（%）
pub(super) fn cpu_usage_percent() -> Option<f64> {
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

/// `GlobalMemoryStatusEx` 得到内存使用率（%）
pub(super) fn mem_usage_percent() -> Option<f64> {
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

/// `GetTickCount64` 得到系统运行时长（秒）
pub(super) fn uptime_secs() -> Option<u64> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetTickCount64() -> u64;
    }
    // SAFETY: 无参数、无输出指针，返回自开机以来的毫秒数。
    Some(unsafe { GetTickCount64() } / 1000)
}

/// `EnumProcesses` 统计进程数
pub(super) fn process_count() -> Option<u32> {
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
pub(super) fn cpu_temp_celsius() -> Option<f64> {
    None
}

/// Windows 无 loadavg 概念，如实返回 None
pub(super) fn load_avg_1m() -> Option<f64> {
    None
}

/// 枚举本机网卡（不含 loopback）。
///
/// 两个系统 API 分工：`GetAdaptersAddresses` 给出接口名称、IPv4 与操作状态，
/// `GetIfTable2` 给出累计收发字节；两者用 LUID（接口的稳定唯一标识）关联。
pub(super) fn net_interfaces() -> Vec<NetInterfaceMetric> {
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
type IfTableRow = (u64, u64, u64);

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
type AdapterRow = (u64, String, String, bool);

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
