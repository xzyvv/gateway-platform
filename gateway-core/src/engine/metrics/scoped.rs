//! 按总线 / 点位的真实采集统计（最近 31 天日桶）
//!
//! 背景：`/api/diagnostics/stats`、`/api/diagnostics/bus-load`、`/api/devices/health`
//! 三个端点此前用 `hash(设备ID + 时间片)` 生成成功率、延迟、负载、健康分——页面数字与
//! 设备真实运行状态无关。本结构把采集循环里**实际发生**的每次读取结果（成功/失败 +
//! 耗时）按总线与点位累计，供上述端点直接读取。
//!
//! 采用最近 31 个自然日（UTC）的日桶：日/周/月报表只需按起始日过滤即可，不需要历史库。
//! 日桶数量恒定（≤31），内存与点位数量同阶；进程重启后清零（历史报表请走持久化方案）。

use std::collections::{BTreeMap, HashMap};

use parking_lot::RwLock;

const DAY_MS: i64 = 86_400_000;
/// 保留天数：覆盖「月报」窗口（30 天）并留 1 天余量
const RETENTION_DAYS: usize = 31;
/// 按点位导出失败数时的上限：8,000 点场景下全部导出会让 Prometheus 序列数爆炸，
/// 故只导出失败数最多的前 N 个点（其余以 `gateway_collect_scoped_points_failing` 计数体现）。
const POINT_EXPORT_LIMIT: usize = 500;

/// 单日累计值
#[derive(Debug, Clone, Copy, Default)]
struct DayBucket {
    day: i64,
    total: u64,
    success: u64,
    failed: u64,
    duration_sum_us: u64,
    /// 超时次数（failed 的子集，用于「超时率」）
    timeouts: u64,
}

// 错误码与失败原因分桶已拆到 `scoped/error_codes.rs`（C23：主文件 < 500 行）。
// 对外路径保持不变：`pub use` 原样再导出，`crate::engine::metrics::CollectErrorCode`
// 等引用无需改动。
mod error_codes;
pub use error_codes::{CollectErrorCode, ReasonCounters, REASON_LABELS};

/// 一次采集结果的完整记录（参数较多，用结构体承载，避免超长参数列表）。
#[derive(Debug, Clone, Copy)]
pub struct CollectSample<'a> {
    pub bus: &'a str,
    pub point: &'a str,
    /// 点位所属设备；空串表示未知（不参与在线设备计数）
    pub device: &'a str,
    pub success: bool,
    pub duration_us: u64,
    /// 失败时的错误码；成功传 `None`
    pub error: Option<CollectErrorCode>,
    pub now_ms: i64,
}

/// 一个作用域（总线或点位）在指定窗口内的统计
#[derive(Debug, Clone, Copy, Default)]
pub struct ScopeStats {
    pub total: u64,
    pub success: u64,
    pub failed: u64,
    pub duration_sum_us: u64,
    /// 最近一次成功采集时间（毫秒时间戳，0=窗口内无成功）
    pub last_success_ms: i64,
    /// 最近一次失败采集时间（毫秒时间戳，0=窗口内无失败）
    pub last_failure_ms: i64,
    /// 窗口内超时次数（`failed` 的子集）
    pub timeouts: u64,
    /// 总线重连成功次数（进程启动以来累计，不随窗口重置；点位维度恒为 0）
    pub reconnects: u64,
    /// 最近一次失败的错误码（成功后保留，便于排查「最近一次错误」）
    pub last_error_code: Option<CollectErrorCode>,
    /// 在线设备数（最近一次采集成功的设备数；点位维度恒为 0）
    pub online_devices: u32,
}

impl ScopeStats {
    /// 成功率（%）；窗口内无采集返回 None（调用方渲染「—」，不得用 100 冒充）
    pub fn success_rate(&self) -> Option<f64> {
        if self.total == 0 {
            return None;
        }
        Some((self.success as f64 / self.total as f64 * 1000.0).round() / 10.0)
    }

    /// 平均延迟（毫秒）；窗口内无采集返回 None
    pub fn avg_latency_ms(&self) -> Option<f64> {
        if self.total == 0 {
            return None;
        }
        let avg_us = self.duration_sum_us as f64 / self.total as f64;
        Some((avg_us / 1000.0 * 10.0).round() / 10.0)
    }

    /// 超时率（%）；窗口内无采集返回 None
    pub fn timeout_rate(&self) -> Option<f64> {
        if self.total == 0 {
            return None;
        }
        Some((self.timeouts as f64 / self.total as f64 * 1000.0).round() / 10.0)
    }
}

#[derive(Debug, Default)]
struct Series {
    buckets: Vec<DayBucket>,
    last_success_ms: i64,
    last_failure_ms: i64,
    last_error_code: Option<CollectErrorCode>,
    /// 总线重连成功次数（累计值）
    reconnects: u64,
    /// 设备 -> 点位 -> 最近一次是否成功（仅总线维度维护，用于在线设备数）
    devices: HashMap<String, HashMap<String, bool>>,
    /// 点位所属总线（仅点位维度维护，导出 `bus` 标签用）
    bus: String,
    /// 点位所属设备（仅点位维度维护，导出 `device` 标签用）
    device: String,
}

/// 按总线 / 点位累计的采集统计
#[derive(Debug, Default)]
pub struct ScopedCollectMetrics {
    by_bus: RwLock<HashMap<String, Series>>,
    by_point: RwLock<HashMap<String, Series>>,
}

impl ScopedCollectMetrics {
    /// 记录一次采集结果。
    ///
    /// `bus` / `point` / `device` 为空串时跳过对应维度（互不影响）。
    pub fn record(&self, sample: CollectSample<'_>) {
        let day = sample.now_ms.div_euclid(DAY_MS);
        if !sample.bus.is_empty() {
            let mut map = self.by_bus.write();
            let series = map.entry(sample.bus.to_string()).or_default();
            record_into(series, day, &sample);
            if !sample.device.is_empty() && !sample.point.is_empty() {
                series
                    .devices
                    .entry(sample.device.to_string())
                    .or_default()
                    .insert(sample.point.to_string(), sample.success);
            }
        }
        if !sample.point.is_empty() {
            let mut map = self.by_point.write();
            let series = map.entry(sample.point.to_string()).or_default();
            record_into(series, day, &sample);
            // ROI-08：点位序列记住所属总线/设备，导出时带上标签。
            if series.bus.is_empty() && !sample.bus.is_empty() {
                series.bus = sample.bus.to_string();
            }
            if !sample.device.is_empty() {
                series.device = sample.device.to_string();
            }
        }
    }

    /// 记录一次总线重连成功（累计值，供「重连次数」展示）。
    pub fn inc_reconnect(&self, bus: &str) {
        if bus.is_empty() {
            return;
        }
        self.by_bus
            .write()
            .entry(bus.to_string())
            .or_default()
            .reconnects += 1;
    }

    /// 总线维度在 `since_ms` 之后的汇总
    pub fn bus_snapshot(&self, since_ms: i64) -> HashMap<String, ScopeStats> {
        self.by_bus
            .read()
            .iter()
            .map(|(k, s)| (k.clone(), s.sum_since(since_ms)))
            .collect()
    }

    /// 点位维度在 `since_ms` 之后的汇总
    pub fn point_snapshot(&self, since_ms: i64) -> HashMap<String, ScopeStats> {
        self.by_point
            .read()
            .iter()
            .map(|(k, s)| (k.clone(), s.sum_since(since_ms)))
            .collect()
    }

    /// 导出按点位的采集失败数（Prometheus）。
    ///
    /// 口径说明（日桶窗口，非严格单调 counter）：
    /// - 数值 = 该点位在**保留窗口（≤31 天日桶）**内的失败次数之和；
    ///   随日桶滚动与超期裁剪，该值**可能下降**，故 TYPE 声明为 gauge。
    /// - 只导出失败数最多的前 [`POINT_EXPORT_LIMIT`] 个点位，其余失败点位的
    ///   数量由 `gateway_collect_scoped_points_failing` 体现（避免序列数爆炸）。
    /// - ROI-08：每条序列带上所属 `bus` / `device` 标签，便于按设备或总线筛选；
    ///   序列总数仍受 [`POINT_EXPORT_LIMIT`] 限制。
    pub fn render(&self, buf: &mut String) {
        use std::fmt::Write as _;
        let mut rows: Vec<(String, u64, String, String)> = {
            let map = self.by_point.read();
            map.iter()
                .map(|(point, series)| {
                    let failed = series.buckets.iter().map(|b| b.failed).sum();
                    (
                        point.clone(),
                        failed,
                        series.bus.clone(),
                        series.device.clone(),
                    )
                })
                .filter(|(_, failed, _, _)| *failed > 0)
                .collect()
        };
        // 失败数降序；同数按点位名升序，保证导出顺序稳定可复现。
        rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let failing_points = rows.len() as u64;

        let _ = writeln!(
            buf,
            "# HELP gateway_collect_failures_scoped Collection failures per point (sum over retained daily buckets)\n\
             # TYPE gateway_collect_failures_scoped gauge"
        );
        for (point, failed, bus, device) in rows.iter().take(POINT_EXPORT_LIMIT) {
            let _ = writeln!(
                buf,
                "gateway_collect_failures_scoped{{{}}} {}",
                point_labels(bus, device, point),
                failed
            );
        }
        let _ = writeln!(
            buf,
            "# HELP gateway_collect_scoped_points_failing Number of points with collection failures (may exceed exported series limit)\n\
             # TYPE gateway_collect_scoped_points_failing gauge\n\
             gateway_collect_scoped_points_failing {}",
            failing_points,
        );
    }

    /// 导出设备在线状态（ROI-08）。
    ///
    /// 口径与 `online_devices` 一致：设备下任一点位「最近一次采集成功」即为在线。
    /// 序列数等于已知设备数，因此由配置规模决定；热重载时会裁剪已移除设备。
    pub fn render_device_online(&self, buf: &mut String) {
        use std::fmt::Write as _;
        // 同一设备理论上只属于一条总线；跨总线重名时按「任一在线即在线」合并。
        let mut devices: BTreeMap<String, bool> = BTreeMap::new();
        {
            let map = self.by_bus.read();
            for series in map.values() {
                for (device, points) in &series.devices {
                    let online = points.values().any(|&ok| ok);
                    devices
                        .entry(device.clone())
                        .and_modify(|state| *state |= online)
                        .or_insert(online);
                }
            }
        }
        let _ = writeln!(
            buf,
            "# HELP gateway_device_online Device online state (1 = any point's last collection succeeded)\n\
             # TYPE gateway_device_online gauge"
        );
        for (device, online) in devices {
            let _ = writeln!(
                buf,
                "gateway_device_online{{device=\"{}\"}} {}",
                escape_label_value(&device),
                u8::from(online)
            );
        }
    }

    /// 裁剪已从配置中删除的设备（热重载调用），避免在线状态序列长期残留。
    pub fn retain_devices(&self, keep: &std::collections::HashSet<String>) {
        for series in self.by_bus.write().values_mut() {
            series.devices.retain(|device, _| keep.contains(device));
        }
    }
}

/// 组装点位序列的标签串（空标签值省略，保持无总线/无设备场景的旧输出）。
fn point_labels(bus: &str, device: &str, point: &str) -> String {
    let mut labels = String::new();
    if !bus.is_empty() {
        labels.push_str("bus=\"");
        labels.push_str(&escape_label_value(bus));
        labels.push_str("\",");
    }
    if !device.is_empty() {
        labels.push_str("device=\"");
        labels.push_str(&escape_label_value(device));
        labels.push_str("\",");
    }
    labels.push_str("point=\"");
    labels.push_str(&escape_label_value(point));
    labels.push('"');
    labels
}

/// 转义 Prometheus 标签值中的特殊字符（`\`、`"`、换行）。
pub(crate) fn escape_label_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            _ => out.push(ch),
        }
    }
    out
}

impl Series {
    fn sum_since(&self, since_ms: i64) -> ScopeStats {
        let since_day = since_ms.div_euclid(DAY_MS);
        let mut stats = ScopeStats::default();
        for bucket in self.buckets.iter().filter(|b| b.day >= since_day) {
            stats.total += bucket.total;
            stats.success += bucket.success;
            stats.failed += bucket.failed;
            stats.duration_sum_us += bucket.duration_sum_us;
            stats.timeouts += bucket.timeouts;
        }
        stats.last_success_ms = self.last_success_ms;
        stats.last_failure_ms = self.last_failure_ms;
        stats.last_error_code = self.last_error_code;
        stats.reconnects = self.reconnects;
        stats.online_devices = self.online_devices();
        stats
    }

    /// 在线设备数：存在任一点位「最近一次采集成功」的设备数量。
    fn online_devices(&self) -> u32 {
        let n = self
            .devices
            .values()
            .filter(|points| points.values().any(|&ok| ok))
            .count();
        u32::try_from(n).unwrap_or(u32::MAX)
    }
}

fn record_into(series: &mut Series, day: i64, sample: &CollectSample<'_>) {
    let timed_out = !sample.success && sample.error == Some(CollectErrorCode::Timeout);
    match series.buckets.last_mut() {
        Some(bucket) if bucket.day == day => {
            bucket.total += 1;
            bucket.duration_sum_us += sample.duration_us;
            if sample.success {
                bucket.success += 1;
            } else {
                bucket.failed += 1;
            }
            bucket.timeouts += u64::from(timed_out);
        }
        _ => {
            series.buckets.push(DayBucket {
                day,
                total: 1,
                success: u64::from(sample.success),
                failed: u64::from(!sample.success),
                duration_sum_us: sample.duration_us,
                timeouts: u64::from(timed_out),
            });
            if series.buckets.len() > RETENTION_DAYS {
                series.buckets.remove(0);
            }
        }
    }
    if sample.success {
        series.last_success_ms = sample.now_ms;
    } else {
        series.last_failure_ms = sample.now_ms;
        if sample.error.is_some() {
            series.last_error_code = sample.error;
        }
    }
}

#[cfg(test)]
mod tests;
