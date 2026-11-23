//! 采集错误分类码与「失败原因」分桶计数器（从 `scoped.rs` 拆出）
//!
//! 拆分原因：C23 要求 `scoped.rs` 主文件 < 500 行；错误码与分桶计数是
//! 与「日桶统计」无关的独立职责，单独成模块后主文件只剩统计主体。
//!
//! 对外路径不变：`scoped.rs` 用 `pub use` 原样再导出，
//! `crate::engine::metrics::CollectErrorCode` 等引用无需改动。

use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

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
        self.render_with_prefix(metric, "", buf);
    }

    /// 同 [`Self::render`]，但在 `by_reason` 前追加额外标签（如 `bus="..."`）。
    ///
    /// `prefix` 需自带结尾逗号，例如 `bus="COM1",`。
    pub fn render_with_prefix(&self, metric: &str, prefix: &str, buf: &mut String) {
        use std::fmt::Write as _;
        for (i, label) in REASON_LABELS.iter().enumerate() {
            let _ = writeln!(
                buf,
                "{metric}{{{prefix}by_reason=\"{label}\"}} {}",
                self.slots[i].load(Ordering::Relaxed)
            );
        }
    }
}
