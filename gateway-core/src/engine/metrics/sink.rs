//! sink 指标操作（async fan-out 相关）
//!
//! P2-2：指标槽位由**编译期枚举** `SinkId` 固定，替代运行时的
//! `RwLock<HashMap<String, AtomicU64>>`：
//!
//! - 槽位固定 → 写路径退化为「数组下标 + 原子自增」，无写锁、无 `String` 分配；
//! - 枚举穷举 → sink 名写错在编译期报错（旧实现写错只会静默生成一个新的
//!   HashMap key，Prometheus 面板上表现为「指标消失」而无任何告警）；
//! - 全量预注册 → Prometheus / 前端接口恒定输出全部内置 sink（含 0 值），
//!   不会因为「该 sink 还没上报过」而出现空洞。
//!
//! 新增内置 sink 时只需在 `SinkId` 加一个变体，`ALL` / `as_str` / `index`
//! 由 match 强制穷举，漏改无法通过编译。

use super::Metrics;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

/// 内置 sink 标识（编译期穷举）。
///
/// `as_str()` 返回的字符串是 Prometheus 的 `sink` 标签值，也是前端
/// `/api/system` 里的 `name` 字段，保持与历史取值完全一致（`"opcua"` /
/// `"modbus"`），避免既有面板与告警规则失效。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SinkId {
    OpcUa,
    Modbus,
}

impl SinkId {
    /// 全部内置 sink（顺序即槽位顺序，同时作为遍历顺序）。
    pub const ALL: [SinkId; 2] = [SinkId::OpcUa, SinkId::Modbus];

    /// sink 数量（数组槽位长度）。
    pub const COUNT: usize = Self::ALL.len();

    /// Prometheus `sink` 标签值 / API `name` 字段。
    pub const fn as_str(self) -> &'static str {
        match self {
            SinkId::OpcUa => "opcua",
            SinkId::Modbus => "modbus",
        }
    }

    /// 固定槽位下标。变体与下标一一对应，新增变体时编译器会要求补全。
    const fn index(self) -> usize {
        match self {
            SinkId::OpcUa => 0,
            SinkId::Modbus => 1,
        }
    }
}

/// 每个 sink 的三组固定槽位指标。
#[derive(Debug)]
pub struct SinkMetrics {
    /// `gateway_sink_send_total`
    send: [AtomicU64; SinkId::COUNT],
    /// `gateway_sink_dropped_total`
    dropped: [AtomicU64; SinkId::COUNT],
    /// `gateway_sink_channel_depth`
    depth: [AtomicI64; SinkId::COUNT],
}

impl Default for SinkMetrics {
    fn default() -> Self {
        // `[AtomicU64; N]` 未实现 Default（仅有 0/1/…/32 长度的特化），
        // 用 `from_fn` 逐槽初始化，长度由 `SinkId::COUNT` 自动跟随。
        Self {
            send: std::array::from_fn(|_| AtomicU64::new(0)),
            dropped: std::array::from_fn(|_| AtomicU64::new(0)),
            depth: std::array::from_fn(|_| AtomicI64::new(0)),
        }
    }
}

impl Metrics {
    /// 记录一次 sink 投递（成功进入 mpsc）
    pub fn inc_sink_send(&self, sink: SinkId) {
        self.sink.send[sink.index()].fetch_add(1, Ordering::Relaxed);
    }

    /// 记录一次 sink 丢弃（mpsc 满）
    pub fn inc_sink_dropped(&self, sink: SinkId) {
        self.sink.dropped[sink.index()].fetch_add(1, Ordering::Relaxed);
    }

    /// 设置 sink 内部 mpsc 通道当前深度（由 sink task 定期上报）
    pub fn set_sink_channel_depth(&self, sink: SinkId, depth: i64) {
        self.sink.depth[sink.index()].store(depth.max(0), Ordering::Relaxed);
    }

    /// 读 sink send_total（snapshot）
    pub fn sink_send_total(&self, sink: SinkId) -> u64 {
        self.sink.send[sink.index()].load(Ordering::Relaxed)
    }

    /// 读 sink dropped_total（snapshot）
    pub fn sink_dropped_total(&self, sink: SinkId) -> u64 {
        self.sink.dropped[sink.index()].load(Ordering::Relaxed)
    }

    /// 读 sink 通道深度（snapshot）
    pub fn sink_channel_depth(&self, sink: SinkId) -> i64 {
        self.sink.depth[sink.index()].load(Ordering::Relaxed)
    }
}
