//! 按总线 / 点位的真实采集统计（最近 31 天日桶）
//!
//! 背景：`/api/diagnostics/stats`、`/api/diagnostics/bus-load`、`/api/devices/health`
//! 三个端点此前用 `hash(设备ID + 时间片)` 生成成功率、延迟、负载、健康分——页面数字与
//! 设备真实运行状态无关。本结构把采集循环里**实际发生**的每次读取结果（成功/失败 +
//! 耗时）按总线与点位累计，供上述端点直接读取。
//!
//! 采用最近 31 个自然日（UTC）的日桶：日/周/月报表只需按起始日过滤即可，不需要历史库。
//! 日桶数量恒定（≤31），内存与点位数量同阶；进程重启后清零（历史报表请走持久化方案）。

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

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

/// 采集错误分类码（统计层稳定口径，不暴露具体协议的 `CollectorError` 类型）。
///
/// 由采集层（south）在记录结果时映射传入，`engine` 不反向依赖协议实现，
/// 保持分层约束。`code()` 输出即报表 / 前端展示用的机器可读错误码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectErrorCode {
    /// 采集超时（组级预算耗尽 / 协议栈无响应）
    Timeout,
    /// 设备无应答
    NoReply,
    /// 链路 IO 故障
    Io,
    /// 帧校验失败
    Checksum,
    /// 帧 / BCD 解码失败
    Decode,
    /// 总线句柄已关闭
    BusClosed,
    /// 资源忙（并发闸门饱和 / 排队被拒）
    Busy,
    /// Modbus 异常响应码
    ModbusException(u8),
    /// 配置错误
    Config,
    /// 其他（含协议不支持、质量码为 Bad 但无原始错误）
    Other,
}

impl CollectErrorCode {
    /// 稳定的机器可读错误码（不随内部枚举改名而漂移）
    pub fn code(self) -> String {
        match self {
            Self::Timeout => "TIMEOUT".to_string(),
            Self::NoReply => "NO_REPLY".to_string(),
            Self::Io => "IO".to_string(),
            Self::Checksum => "CRC".to_string(),
            Self::Decode => "DECODE".to_string(),
            Self::BusClosed => "BUS_CLOSED".to_string(),
            Self::Busy => "BUSY".to_string(),
            Self::ModbusException(c) => format!("EXC_{c:02X}"),
            Self::Config => "CONFIG".to_string(),
            Self::Other => "OTHER".to_string(),
        }
    }

    /// `by_reason` 标签下标（与 [`REASON_LABELS`] 顺序严格一致）。
    ///
    /// 分桶计数器按下标累加，标签值由 [`REASON_LABELS`] 统一给出，
    /// 避免「标签名与桶下标」两处定义漂移。
    pub fn reason_index(self) -> usize {
        match self {
            Self::Timeout => 0,
            Self::NoReply => 1,
            Self::Io => 2,
            Self::Checksum => 3,
            Self::Decode => 4,
            Self::BusClosed => 5,
            Self::Busy => 6,
            Self::ModbusException(_) => 7,
            Self::Config => 8,
            Self::Other => 9,
        }
    }

    /// 错误码字符串 → 枚举（[`CollectErrorCode::code`] 的逆运算）。
    ///
    /// 供采集层把 `quality_reason` 里携带的稳定错误码还原成枚举
    /// （如 `modbus_read_failed:TIMEOUT` → `CollectErrorCode::Timeout`）。
    /// 无法识别时返回 `None`，由调用方回退到 `Other`。
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "TIMEOUT" => Some(Self::Timeout),
            "NO_REPLY" => Some(Self::NoReply),
            "IO" => Some(Self::Io),
            "CRC" => Some(Self::Checksum),
            "DECODE" => Some(Self::Decode),
            "BUS_CLOSED" => Some(Self::BusClosed),
            "BUSY" => Some(Self::Busy),
            "CONFIG" => Some(Self::Config),
            "OTHER" => Some(Self::Other),
            other => other
                .strip_prefix("EXC_")
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                .map(Self::ModbusException),
        }
    }
}

/// `by_reason` 标签取值集合（Prometheus 侧标签值口径）。
///
/// 与 `CollectErrorCode::reason_index()` 一一对应，顺序不可调整。
/// 相比 `code()`（大写、含 Modbus 异常码原值）此处为**小写枚举**，
/// 且协议异常响应统一归入 `protocol_exception`（不按异常码再分桶，
/// 避免标签基数随异常码增长）。
///
/// ⚠️ 标签值**不得含协议名**：`gateway_collect_failures` 在 sink 段之前渲染，
/// 若标签值出现 "modbus" 等协议名，会让协议名在导出文本中先于 sink 标签行出现，
/// 违反 r31-PM1 约定（见 `prometheus.rs` 中 sink 段注释）。
pub const REASON_LABELS: [&str; 10] = [
    "timeout",
    "no_reply",
    "io",
    "crc",
    "decode",
    "bus_closed",
    "busy",
    "protocol_exception",
    "config",
    "other",
];

/// 失败原因分桶计数器（Prometheus `by_reason` 标签的数据源）。
///
/// 总量仍由各指标的原子计数器持有（`gateway_errors_total` / OTel / 压测脚本
/// 都直读原子总量），本结构只负责按原因拆分，**不改变既有指标语义**。
#[derive(Debug, Default)]
pub struct ReasonCounters {
    slots: [AtomicU64; REASON_LABELS.len()],
}

impl ReasonCounters {
    /// 累加一次失败（按原因分桶）
    pub fn inc(&self, code: CollectErrorCode) {
        self.slots[code.reason_index()].fetch_add(1, Ordering::Relaxed);
    }

    /// 读取某原因的累计值
    pub fn load(&self, code: CollectErrorCode) -> u64 {
        self.slots[code.reason_index()].load(Ordering::Relaxed)
    }

    /// 导出为 Prometheus 文本：`metric{by_reason="..."} value`。
    ///
    /// 所有标签值恒定输出（未发生的原因为 0），避免序列集合抖动。
    pub fn render(&self, metric: &str, buf: &mut String) {
        use std::fmt::Write as _;
        for (i, label) in REASON_LABELS.iter().enumerate() {
            let _ = writeln!(
                buf,
                "{metric}{{by_reason=\"{label}\"}} {}",
                self.slots[i].load(Ordering::Relaxed)
            );
        }
    }
}

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
    pub fn render(&self, buf: &mut String) {
        use std::fmt::Write as _;
        let mut rows: Vec<(String, u64)> = {
            let map = self.by_point.read();
            map.iter()
                .map(|(point, series)| {
                    let failed = series.buckets.iter().map(|b| b.failed).sum();
                    (point.clone(), failed)
                })
                .filter(|(_, failed)| *failed > 0)
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
        for (point, failed) in rows.iter().take(POINT_EXPORT_LIMIT) {
            let _ = writeln!(
                buf,
                "gateway_collect_failures_scoped{{point=\"{}\"}} {}",
                escape_label_value(point),
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
}

/// 转义 Prometheus 标签值中的特殊字符（`\`、`"`、换行）。
fn escape_label_value(value: &str) -> String {
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
