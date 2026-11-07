//! 主机指标采集用的纯解析函数。
//!
//! 这些函数只做「文本 → 数值」的转换，不触碰系统 API，也不读写全局状态，
//! 因此可以脱离平台实现独立测试（Linux 读取 `/proc/*`，其余平台读不到这些文本）。

/// 解析 `/proc/stat` 首行（`cpu` 汇总行），返回 (idle 累计, total 累计)
///
/// 字段顺序：user nice system idle iowait irq softirq steal [guest guest_nice]。
/// guest/guest_nice 已计入 user/nice，不能再累加，否则 total 虚高。
pub(super) fn parse_proc_stat(text: &str) -> Option<(u64, u64)> {
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
pub(super) fn parse_meminfo(text: &str) -> Option<(u64, u64)> {
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
pub(super) fn parse_loadavg(text: &str) -> Option<f64> {
    text.split_whitespace().next()?.parse().ok()
}

/// 解析 `/proc/uptime` 的运行秒数（首字段为浮点）
pub(super) fn parse_uptime(text: &str) -> Option<u64> {
    let secs: f64 = text.split_whitespace().next()?.parse().ok()?;
    if secs < 0.0 {
        return None;
    }
    Some(secs as u64)
}

/// 解析 thermal zone 的毫摄氏度值
pub(super) fn parse_thermal_milli(text: &str) -> Option<f64> {
    let milli: i64 = text.trim().parse().ok()?;
    // 0 或负值通常表示传感器未就绪，不当作有效温度
    if milli <= 0 {
        return None;
    }
    Some(milli as f64 / 1000.0)
}
