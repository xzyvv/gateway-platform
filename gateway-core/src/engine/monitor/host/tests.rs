//! `host` 的单元测试（从原单文件拆出）。
//!
//! O-6 拆分：engine/monitor/host.rs 原 902 行。
//!
//! 拆分的直接原因：单文件行数超出可维护阈值。测试与被测代码同属一个模块，
//! 因此放在同名子目录下（`host/tests.rs`），`use super::*` 的可见性语义不变。
use super::parsers::{
    parse_loadavg, parse_meminfo, parse_proc_stat, parse_thermal_milli, parse_uptime,
};
use super::platform::net_interfaces;
#[cfg(target_os = "linux")]
use super::platform::{
    ipv4_by_interface, load_avg_1m, mem_usage_percent, process_count, uptime_secs,
};
use super::{collect, usage_pct};

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
    let text =
        "MemTotal:       16000000 kB\nMemFree:         1000000 kB\nMemAvailable:    6000000 kB\n";
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
