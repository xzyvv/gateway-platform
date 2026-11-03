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
mod tests {
    use super::*;

    const T0: i64 = 1_700_000_000_000; // 固定基准时间，避免依赖真实时钟

    /// 简化构造：默认无设备、无错误码（覆盖既有用例）
    fn sample<'a>(
        bus: &'a str,
        point: &'a str,
        success: bool,
        us: u64,
        now_ms: i64,
    ) -> CollectSample<'a> {
        CollectSample {
            bus,
            point,
            device: "",
            success,
            duration_us: us,
            error: None,
            now_ms,
        }
    }

    /// 带错误码的失败样本
    fn fail_sample<'a>(
        bus: &'a str,
        point: &'a str,
        error: CollectErrorCode,
        now_ms: i64,
    ) -> CollectSample<'a> {
        CollectSample {
            bus,
            point,
            device: "",
            success: false,
            duration_us: 1_000,
            error: Some(error),
            now_ms,
        }
    }

    /// 带设备归属的样本（在线设备数口径）
    fn dev_sample<'a>(
        bus: &'a str,
        point: &'a str,
        device: &'a str,
        success: bool,
        now_ms: i64,
    ) -> CollectSample<'a> {
        CollectSample {
            bus,
            point,
            device,
            success,
            duration_us: 1_000,
            error: None,
            now_ms,
        }
    }

    #[test]
    fn records_success_and_failure_per_scope() {
        let m = ScopedCollectMetrics::default();
        m.record(sample("bus-a", "p1", true, 2_000, T0));
        m.record(sample("bus-a", "p1", false, 8_000, T0 + 1_000));
        m.record(sample("bus-b", "p2", true, 4_000, T0 + 2_000));

        let buses = m.bus_snapshot(T0);
        assert_eq!(buses["bus-a"].total, 2);
        assert_eq!(buses["bus-a"].success, 1);
        assert_eq!(buses["bus-a"].failed, 1);
        assert_eq!(buses["bus-a"].success_rate(), Some(50.0));
        // (2ms + 8ms) / 2 = 5ms
        assert_eq!(buses["bus-a"].avg_latency_ms(), Some(5.0));
        assert_eq!(buses["bus-b"].success_rate(), Some(100.0));
        assert_eq!(buses["bus-b"].last_success_ms, T0 + 2_000);

        let points = m.point_snapshot(T0);
        assert_eq!(points.len(), 2);
        assert_eq!(points["p1"].total, 2);
    }

    #[test]
    fn empty_scope_is_skipped() {
        let m = ScopedCollectMetrics::default();
        m.record(sample("", "p1", true, 1_000, T0));
        assert!(m.bus_snapshot(T0).is_empty());
        assert_eq!(m.point_snapshot(T0).len(), 1);
    }

    #[test]
    fn window_filters_old_days() {
        let m = ScopedCollectMetrics::default();
        let day_ms = DAY_MS;
        m.record(sample("bus", "p", true, 1_000, T0));
        m.record(sample("bus", "p", false, 1_000, T0 + 3 * day_ms));

        // 只看最近 1 天：仅第二个桶（失败）计入
        let recent = m.bus_snapshot(T0 + 3 * day_ms);
        assert_eq!(recent["bus"].total, 1);
        assert_eq!(recent["bus"].failed, 1);
        // 覆盖 4 天：两次都计入
        let all = m.bus_snapshot(T0);
        assert_eq!(all["bus"].total, 2);
    }

    #[test]
    fn retention_caps_bucket_count() {
        let m = ScopedCollectMetrics::default();
        for d in 0..(RETENTION_DAYS as i64 + 5) {
            m.record(sample("bus", "p", true, 1_000, T0 + d * DAY_MS));
        }
        // 超期桶被裁剪：窗口从 T0 起算只能看到 RETENTION_DAYS 个
        let stats = m.bus_snapshot(T0);
        assert_eq!(stats["bus"].total, RETENTION_DAYS as u64);
    }

    #[test]
    fn empty_window_rates_are_none() {
        let m = ScopedCollectMetrics::default();
        m.record(sample("bus", "p", true, 1_000, T0));
        let future = m.bus_snapshot(T0 + 10 * DAY_MS);
        assert_eq!(future["bus"].total, 0);
        assert_eq!(future["bus"].success_rate(), None);
        assert_eq!(future["bus"].avg_latency_ms(), None);
        assert_eq!(future["bus"].timeout_rate(), None);
    }

    /// 每总线关键指标：在线设备数 / 超时率 / 最近错误码 / 重连次数。
    #[test]
    fn bus_snapshot_reports_key_bus_metrics() {
        let m = ScopedCollectMetrics::default();
        // dev-1 两个点位：一个成功一个超时 → 设备仍在线
        m.record(CollectSample {
            bus: "bus-a",
            point: "p1",
            device: "dev-1",
            success: true,
            duration_us: 1_000,
            error: None,
            now_ms: T0,
        });
        m.record(CollectSample {
            bus: "bus-a",
            point: "p2",
            device: "dev-1",
            success: false,
            duration_us: 2_000,
            error: Some(CollectErrorCode::Timeout),
            now_ms: T0 + 1_000,
        });
        // dev-2 仅失败 → 离线
        m.record(CollectSample {
            bus: "bus-a",
            point: "p3",
            device: "dev-2",
            success: false,
            duration_us: 3_000,
            error: Some(CollectErrorCode::Io),
            now_ms: T0 + 2_000,
        });
        m.inc_reconnect("bus-a");

        let s = m.bus_snapshot(T0)["bus-a"];
        assert_eq!(s.online_devices, 1, "dev-1 有点位成功即在线，dev-2 离线");
        assert_eq!(s.timeouts, 1);
        // 1 / 3 超时
        assert_eq!(s.timeout_rate(), Some(33.3));
        assert_eq!(s.last_error_code, Some(CollectErrorCode::Io));
        assert_eq!(s.last_error_code.unwrap().code(), "IO");
        assert_eq!(s.reconnects, 1);
        // 点位维度不携带设备/重连信息
        let point = m.point_snapshot(T0)["p1"];
        assert_eq!(point.online_devices, 0);
        assert_eq!(point.reconnects, 0);
    }

    /// 成功后保留「最近错误码」，便于排查已恢复的抖动。
    #[test]
    fn last_error_code_survives_recovery() {
        let m = ScopedCollectMetrics::default();
        m.record(CollectSample {
            bus: "bus",
            point: "p",
            device: "",
            success: false,
            duration_us: 1_000,
            error: Some(CollectErrorCode::ModbusException(2)),
            now_ms: T0,
        });
        m.record(sample("bus", "p", true, 1_000, T0 + 1_000));
        let s = m.bus_snapshot(T0)["bus"];
        assert_eq!(
            s.last_error_code,
            Some(CollectErrorCode::ModbusException(2))
        );
        assert_eq!(
            s.last_error_code.unwrap().code(),
            "EXC_02",
            "Modbus 异常码必须保留原始码值"
        );
        assert_eq!(s.timeouts, 0);
    }

    // ===== 四指标口径锁定 =====

    /// 错误码 → 机器可读字符串的映射必须稳定（前端按此展示「最近错误码」）。
    ///
    /// 口径一旦漂移，历史报表与告警规则里的错误码就全部对不上，故在此锁死。
    #[test]
    fn error_code_strings_are_stable() {
        assert_eq!(CollectErrorCode::Timeout.code(), "TIMEOUT");
        assert_eq!(CollectErrorCode::NoReply.code(), "NO_REPLY");
        assert_eq!(CollectErrorCode::Io.code(), "IO");
        assert_eq!(CollectErrorCode::Checksum.code(), "CRC");
        assert_eq!(CollectErrorCode::Decode.code(), "DECODE");
        assert_eq!(CollectErrorCode::BusClosed.code(), "BUS_CLOSED");
        assert_eq!(CollectErrorCode::Busy.code(), "BUSY");
        assert_eq!(CollectErrorCode::Config.code(), "CONFIG");
        assert_eq!(CollectErrorCode::Other.code(), "OTHER");
        // Modbus 异常码必须保留原始码值（十六进制、大写、两位）
        assert_eq!(CollectErrorCode::ModbusException(2).code(), "EXC_02");
        assert_eq!(CollectErrorCode::ModbusException(0x0B).code(), "EXC_0B");
    }

    /// 超时率口径：超时是 `failed` 的子集，按**总采集次数**为分母。
    ///
    /// 10 次采集里 3 次超时 → 30%；非超时的失败（IO 错误）不计入超时。
    #[test]
    fn timeout_rate_counts_only_timeout_failures() {
        let m = ScopedCollectMetrics::default();
        for i in 0..6 {
            m.record(sample("bus", "p", true, 1_000, T0 + i * 10));
        }
        for i in 0..3 {
            m.record(fail_sample(
                "bus",
                "p",
                CollectErrorCode::Timeout,
                T0 + 100 + i * 10,
            ));
        }
        // 1 次非超时失败：不计入超时，但计入 failed
        m.record(fail_sample("bus", "p", CollectErrorCode::Io, T0 + 200));

        let s = m.bus_snapshot(T0)["bus"];
        assert_eq!(s.total, 10);
        assert_eq!(s.failed, 4);
        assert_eq!(s.timeouts, 3, "只有 Timeout 计超时");
        assert_eq!(s.timeout_rate(), Some(30.0));
    }

    /// 超时率在窗口内无采集时必须为 `None`（不得用 0% 冒充「无超时」）。
    #[test]
    fn timeout_rate_is_none_without_samples() {
        let m = ScopedCollectMetrics::default();
        m.record(fail_sample("bus", "p", CollectErrorCode::Timeout, T0));
        // 取一个未来窗口：该总线没有落在窗口内的采集
        let future = m.bus_snapshot(T0 + 10 * DAY_MS);
        assert_eq!(future["bus"].total, 0);
        assert_eq!(future["bus"].timeout_rate(), None);
    }

    /// 在线设备数：设备任一点位最近一次采集成功即视为在线；
    /// 该设备所有点位都失败 → 离线。
    #[test]
    fn online_devices_follow_latest_point_result() {
        let m = ScopedCollectMetrics::default();
        // dev1：p1 成功 → 在线
        m.record(dev_sample("bus", "p1", "dev1", true, T0));
        // dev2：先成功后失败 → 最近一次失败 → 离线
        m.record(dev_sample("bus", "p2", "dev2", true, T0));
        m.record(dev_sample("bus", "p2", "dev2", false, T0 + 1));
        // dev3：从未成功 → 离线
        m.record(dev_sample("bus", "p3", "dev3", false, T0 + 2));

        let s = m.bus_snapshot(T0)["bus"];
        assert_eq!(s.online_devices, 1, "仅 dev1 有最近一次成功的点位");
    }

    /// 在线设备数按**设备**去重：同一设备的多个点位只计 1 台。
    #[test]
    fn online_devices_deduplicate_multiple_points() {
        let m = ScopedCollectMetrics::default();
        for i in 0..3 {
            m.record(dev_sample("bus", &format!("p{i}"), "dev1", true, T0 + i));
        }
        let s = m.bus_snapshot(T0)["bus"];
        assert_eq!(s.online_devices, 1, "同一设备的 3 个点位只算 1 台在线");
    }

    /// 未知设备（device 为空串）不参与在线设备计数。
    #[test]
    fn samples_without_device_do_not_count_online() {
        let m = ScopedCollectMetrics::default();
        m.record(sample("bus", "p", true, 1_000, T0));
        let s = m.bus_snapshot(T0)["bus"];
        assert_eq!(s.online_devices, 0, "设备未知的采集不得计入在线设备数");
        assert_eq!(s.success, 1, "但成功计数照常");
    }

    /// 重连次数是**累计值**：跨窗口不重置，且只影响总线维度。
    #[test]
    fn reconnect_count_accumulates_across_windows() {
        let m = ScopedCollectMetrics::default();
        m.inc_reconnect("bus");
        m.inc_reconnect("bus");
        m.inc_reconnect("bus");
        // 空总线名不建条目
        m.inc_reconnect("");

        let now = m.bus_snapshot(T0);
        assert_eq!(now["bus"].reconnects, 3);
        // 取一个没有任何采集的窗口：重连次数仍应保留（累计语义）
        let future = m.bus_snapshot(T0 + 10 * DAY_MS);
        assert_eq!(
            future["bus"].reconnects, 3,
            "重连次数是累计值，不随窗口清零"
        );
        assert_eq!(future["bus"].total, 0);
    }

    /// 最近错误码只取**最近一次失败**的码：先失败后成功 → 保留失败码（便于排查抖动）。
    #[test]
    fn last_error_code_keeps_the_latest_failure() {
        let m = ScopedCollectMetrics::default();
        m.record(fail_sample("bus", "p", CollectErrorCode::Checksum, T0));
        m.record(fail_sample("bus", "p", CollectErrorCode::Timeout, T0 + 1));
        let s = m.bus_snapshot(T0)["bus"];
        assert_eq!(
            s.last_error_code,
            Some(CollectErrorCode::Timeout),
            "应取最近一次失败的错误码"
        );
    }

    /// 失败但没有错误码（如质量码 Bad）时，不得覆盖已有的具体错误码。
    #[test]
    fn failure_without_code_keeps_previous_error_code() {
        let m = ScopedCollectMetrics::default();
        m.record(fail_sample("bus", "p", CollectErrorCode::Decode, T0));
        // 无错误码的失败（error=None）
        m.record(sample("bus", "p", false, 1_000, T0 + 1));
        let s = m.bus_snapshot(T0)["bus"];
        assert_eq!(
            s.last_error_code,
            Some(CollectErrorCode::Decode),
            "无错误码的失败不得把已知错误码清空"
        );
    }

    /// 总线之间互不串扰：每条总线的四指标独立统计。
    #[test]
    fn bus_metrics_are_isolated_per_bus() {
        let m = ScopedCollectMetrics::default();
        m.record(dev_sample("bus-a", "p1", "dev1", true, T0));
        m.record(fail_sample("bus-b", "p1", CollectErrorCode::Timeout, T0));
        m.inc_reconnect("bus-b");

        let snap = m.bus_snapshot(T0);
        assert_eq!(snap["bus-a"].online_devices, 1);
        assert_eq!(snap["bus-a"].timeouts, 0);
        assert_eq!(snap["bus-a"].last_error_code, None);
        assert_eq!(snap["bus-a"].reconnects, 0);

        assert_eq!(snap["bus-b"].online_devices, 0);
        assert_eq!(snap["bus-b"].timeouts, 1);
        assert_eq!(snap["bus-b"].timeout_rate(), Some(100.0));
        assert_eq!(
            snap["bus-b"].last_error_code,
            Some(CollectErrorCode::Timeout)
        );
        assert_eq!(snap["bus-b"].reconnects, 1);
    }

    // ===== by_reason 标签口径 =====

    /// `reason_index` 必须把每个错误码映射到唯一且不越界的桶下标。
    ///
    /// 桶下标一旦漂移，历史时序与 Grafana 图例就对不上，故在此锁死。
    #[test]
    fn reason_index_covers_unique_buckets() {
        let codes = [
            CollectErrorCode::Timeout,
            CollectErrorCode::NoReply,
            CollectErrorCode::Io,
            CollectErrorCode::Checksum,
            CollectErrorCode::Decode,
            CollectErrorCode::BusClosed,
            CollectErrorCode::Busy,
            CollectErrorCode::ModbusException(2),
            CollectErrorCode::Config,
            CollectErrorCode::Other,
        ];
        let mut indices: Vec<usize> = codes.iter().map(|c| c.reason_index()).collect();
        assert!(
            indices.iter().all(|i| *i < REASON_LABELS.len()),
            "reason_index 越界：{:?}",
            indices
        );
        // 10 个错误码必须恰好铺满 REASON_LABELS 的 10 个桶，不得重复
        indices.sort_unstable();
        let expected: Vec<usize> = (0..REASON_LABELS.len()).collect();
        assert_eq!(indices, expected, "reason_index 未覆盖全部桶或存在重复映射");
        // 标签值必须互不相同（否则不同原因会落到同一时序）
        let mut labels: Vec<&str> = REASON_LABELS.to_vec();
        labels.sort_unstable();
        let before = labels.len();
        labels.dedup();
        assert_eq!(labels.len(), before, "REASON_LABELS 存在重复取值");
    }

    /// `from_code` 必须与 `code()` 互为逆运算（Modbus 异常码除外，其异常码原值不可还原）。
    #[test]
    fn from_code_reverses_code() {
        for code in [
            CollectErrorCode::Timeout,
            CollectErrorCode::NoReply,
            CollectErrorCode::Io,
            CollectErrorCode::Checksum,
            CollectErrorCode::Decode,
            CollectErrorCode::BusClosed,
            CollectErrorCode::Busy,
            CollectErrorCode::Config,
            CollectErrorCode::Other,
        ] {
            assert_eq!(
                CollectErrorCode::from_code(&code.code()),
                Some(code),
                "from_code 应还原 {}",
                code.code()
            );
        }
        // Modbus 异常码：只还原到「异常」这一类，原始码值不还原
        assert_eq!(
            CollectErrorCode::from_code("EXC_02"),
            Some(CollectErrorCode::ModbusException(2))
        );
        // 未知错误码返回 None，由调用方回退到 Other
        assert_eq!(CollectErrorCode::from_code("NOT_A_CODE"), None);
        assert_eq!(CollectErrorCode::from_code("EXC_ZZ"), None);
    }

    /// `ReasonCounters` 按原因分桶累加，且导出时标签集合恒定（未发生的原因为 0）。
    #[test]
    fn reason_counters_bucket_and_render() {
        let c = ReasonCounters::default();
        c.inc(CollectErrorCode::Timeout);
        c.inc(CollectErrorCode::Timeout);
        c.inc(CollectErrorCode::Io);
        c.inc(CollectErrorCode::ModbusException(3));

        assert_eq!(c.load(CollectErrorCode::Timeout), 2);
        assert_eq!(c.load(CollectErrorCode::Io), 1);
        assert_eq!(c.load(CollectErrorCode::ModbusException(3)), 1);
        assert_eq!(c.load(CollectErrorCode::Busy), 0);

        let mut buf = String::new();
        c.render("gateway_collect_failures", &mut buf);
        assert!(buf.contains("gateway_collect_failures{by_reason=\"timeout\"} 2"));
        assert!(buf.contains("gateway_collect_failures{by_reason=\"io\"} 1"));
        assert!(buf.contains("gateway_collect_failures{by_reason=\"protocol_exception\"} 1"));
        // 标签集合恒定：未发生的原因为 0 也必须输出
        assert!(buf.contains("gateway_collect_failures{by_reason=\"busy\"} 0"));
        // 每个标签值恰好一行
        assert_eq!(
            buf.lines()
                .filter(|l| l.starts_with("gateway_collect_failures{"))
                .count(),
            REASON_LABELS.len()
        );
    }

    /// `ScopedCollectMetrics::render()`：只导出有失败的点位，并给出失败点总数。
    #[test]
    fn scoped_render_exports_failing_points_only() {
        let m = ScopedCollectMetrics::default();
        m.record(sample("bus", "ok-point", true, 1_000, T0));
        m.record(fail_sample(
            "bus",
            "bad-point",
            CollectErrorCode::Timeout,
            T0,
        ));
        m.record(fail_sample(
            "bus",
            "bad-point",
            CollectErrorCode::Timeout,
            T0 + 1,
        ));

        let mut buf = String::new();
        m.render(&mut buf);
        assert!(buf.contains("gateway_collect_failures_scoped{point=\"bad-point\"} 2"));
        assert!(!buf.contains("ok-point"), "无失败点位不得导出：\n{}", buf);
        assert!(buf.contains("gateway_collect_scoped_points_failing 1"));
    }
}
