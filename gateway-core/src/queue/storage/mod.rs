//! 存储 SPI：抽象队列存储后端
//!
//! - `QueueStorage` trait 定义队列存储的统一接口
//! - `SqliteStorage`: 生产环境 SQLite WAL 后端（现有实现封装）
//! - `MemoryStorage`: 纯内存后端（测试 / 轻量场景，无持久化）
//!
//! 所有方法保证线程安全，调用方通过 `spawn_blocking` 调用以避免阻塞 tokio worker。
//! depth / in_flight 计数器由实现方内部维护，通过 `len()` / `in_flight_count()` 读取。

mod memory;
mod sqlite;

pub use memory::MemoryStorage;
pub use sqlite::SqliteStorage;

use serde::Serialize;

use crate::engine::error::GatewayResult;
use crate::engine::model::DataPoint;

use crate::queue::core::QueuedItem;

/// 历史样本：原始行与时间桶聚合结果**共用同一形状**。
///
/// P2-1/P2-3：`/api/history` 与 `/api/history/export` 都消费这一结构。
/// 前端按 `min_value`/`sample_count` 是否为 `null`/`1` 区分原始行与聚合桶，
/// 因此不为两种粒度定义两套类型——避免前端「按形状猜测」的分支。
#[derive(Debug, Clone, Serialize)]
pub struct HistorySample {
    /// 时间戳（毫秒）：原始行 = 采集时刻；聚合桶 = 桶起点
    pub timestamp: i64,
    /// 代表值：原始行 = 采集值；聚合桶 = 桶内平均值
    pub value: f64,
    /// 桶内最小值（原始行为 `None`）
    pub min_value: Option<f64>,
    /// 桶内最大值（原始行为 `None`）
    pub max_value: Option<f64>,
    /// 归入本行的样本数（原始行恒为 1）
    pub sample_count: i64,
    /// 质量标签：原始行透传落库 quality；聚合桶固定 `"aggregated"`
    pub quality: String,
    /// 原始值（仅原始行有；聚合桶为 `None`）
    pub raw_value: Option<f64>,
    /// 原始类型（仅原始行有；聚合桶为 `None`）
    pub raw_type: Option<String>,
}

impl HistorySample {
    /// 原始行 → 样本：`sample_count=1`，无桶统计（min/max 为 None）。
    pub(crate) fn from_raw(item: QueuedItem) -> Self {
        Self {
            timestamp: item.timestamp,
            value: item.value,
            min_value: None,
            max_value: None,
            sample_count: 1,
            quality: item.quality,
            raw_value: Some(item.raw_value),
            raw_type: Some(item.raw_type),
        }
    }
}

/// 历史查询结果（含分页/截断元信息，供 `/api/history` 直接序列化）。
///
/// 语义约定：查询始终保留窗口内**最新**的 `limit` 条，因此
/// `truncated == has_more`（都是"窗口内还有更早数据未返回"）。
/// 保留两个字段是为了前端能直接消费 `has_more` 语义，而不必理解截断口径。
#[derive(Debug, Clone, Serialize)]
pub struct HistoryQueryResult {
    /// 返回的样本，按时间升序
    pub items: Vec<HistorySample>,
    /// 窗口内匹配的样本总数（原始行数 / 聚合桶数）
    pub total: i64,
    /// 实际返回条数（= `items.len()`）
    pub returned: usize,
    /// 是否因 `limit` 只返回了窗口内最新的部分数据
    pub truncated: bool,
    /// 与 `truncated` 同义（"是否还有更早的数据"）
    pub has_more: bool,
    /// 实际粒度：`"raw"` 或 `"second:N"`（N 为桶宽秒数）
    pub granularity: String,
}

impl HistoryQueryResult {
    /// 由"已裁剪到 limit 的样本 + 窗口匹配总数"构造结果，自动推导截断标记。
    pub fn new(items: Vec<HistorySample>, total: i64, granularity: impl Into<String>) -> Self {
        let returned = items.len();
        let truncated = total > returned as i64;
        Self {
            items,
            total,
            returned,
            truncated,
            has_more: truncated,
            granularity: granularity.into(),
        }
    }

    /// 空结果（"窗口内没有数据"），供不支持查询的后端/桩实现使用。
    pub fn empty() -> Self {
        Self::new(Vec::new(), 0, "raw")
    }
}

/// 快照折叠结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotCollapse {
    /// 本周期被折叠（标记已送达但不上行）的行数。
    pub superseded: usize,
    /// 本周期出队上界：只发送 `id <= cutoff` 的行；`i64::MAX` 表示不限制。
    ///
    /// 折叠时取当时表内 `MAX(id)`，把「本周期」的数据边界钉死——排空期间新到的
    /// 样本（id 更大）留到下个周期，避免同一点位在一个周期内被上行两次。
    pub cutoff: i64,
}

impl SnapshotCollapse {
    /// 不折叠、不限制上界（内存后端不支持折叠，退化为逐样本）。
    ///
    /// 注意：SQLite 后端折叠失败时**不走这里**——上报循环会跳过该周期，
    /// 避免退化为 `i64::MAX` 后同一点位一个周期内被上行两次。
    pub fn unrestricted() -> Self {
        Self {
            superseded: 0,
            cutoff: i64::MAX,
        }
    }
}

/// 队列存储后端 trait
///
/// 实现方负责内部同步（Mutex / 原子操作），调用方无需额外加锁。
pub trait QueueStorage: Send + Sync {
    /// 批量入队
    ///
    /// 内部包含 stale in_flight 清理 + 队列深度限制（enforce_queue_limit）。
    /// 返回入队数量。
    fn enqueue_batch(&self, points: &[DataPoint]) -> GatewayResult<usize>;

    /// 批量出队（标记 in_flight），按 id 升序返回。
    fn dequeue_batch(&self, limit: usize) -> GatewayResult<Vec<QueuedItem>>;

    /// 快照出队：每个 `(device_id, point_id)` 只返回最新一行，且 `id <= cutoff`。
    ///
    /// 折叠（把更旧的行标记 delivered）由 [`QueueStorage::collapse_snapshot`] 每周期
    /// 执行一次，不随批次重复。`cutoff` 由折叠结果给出，用于固定本周期数据边界。
    /// 默认实现退化为普通出队（内存后端 / 测试桩没有折叠能力，`cutoff` 被忽略）。
    fn dequeue_snapshot_batch(&self, limit: usize, cutoff: i64) -> GatewayResult<Vec<QueuedItem>> {
        let _ = cutoff;
        self.dequeue_batch(limit)
    }

    /// 快照折叠：把「非最新」的未送达行标记为已送达（不上行）。
    ///
    /// 由上报循环每周期调用一次，返回折叠行数与本周期出队上界。
    /// 默认实现不折叠、不限制上界（即退化为逐样本上行）。
    fn collapse_snapshot(&self) -> GatewayResult<SnapshotCollapse> {
        Ok(SnapshotCollapse::unrestricted())
    }

    /// 确认已发送，删除记录。
    fn ack_batch(&self, ids: &[i64]) -> GatewayResult<()>;

    /// 释放 in_flight 标志（不删除），让数据可被重新 dequeue。
    fn release_in_flight(&self, ids: &[i64]) -> GatewayResult<()>;

    /// 清理超时 stale in_flight（cutoff_ts 之前的 in_flight=1 置为 0）。
    /// 返回释放数量。
    fn cleanup_stale_in_flight(&self, cutoff_ts: i64) -> GatewayResult<usize>;

    /// 当前队列深度（含 in_flight）。
    fn len(&self) -> usize;

    /// 队列是否为空（默认按 len 判断；实现方可按语义覆写）。
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 当前 in_flight 数量。
    fn in_flight_count(&self) -> i64;

    /// 累计因容量上限被物理删除的行数：(总行数, 其中未送达行数)。
    ///
    /// 单机网关没有异地副本，被淘汰的行**不可恢复**——这个累计值是
    /// "到底丢了多少数据"的唯一凭据，因此必须能对外暴露（告警 / 前端展示 / 指标），
    /// 不能只停留在日志里。
    ///
    /// 默认 (0, 0)：内存/测试后端不做容量淘汰。
    fn evicted_stats(&self) -> (i64, i64) {
        (0, 0)
    }

    /// 历史查询：按 point_id + 时间范围 [start, end] 查询，limit 裁剪。
    fn history_query(
        &self,
        point_id: &str,
        start: i64,
        end: i64,
        limit: usize,
    ) -> GatewayResult<Vec<QueuedItem>>;

    /// 历史查询（带时间桶降采样 + 分页元信息）。
    ///
    /// `bucket_secs`：桶宽（秒），`None` 或 0 表示不降采样（返回原始行）。
    /// 默认实现退化为原始行——不支持 SQL 时间桶聚合的后端（如 MemoryStorage）
    /// 直接复用 [`QueueStorage::history_query`]，此时 `total` 只能等于返回条数，
    /// 因此不会报告截断（内存后端本身有容量上限，无长期历史）。
    fn history_query_ex(
        &self,
        point_id: &str,
        start: i64,
        end: i64,
        limit: usize,
        _bucket_secs: Option<u32>,
    ) -> GatewayResult<HistoryQueryResult> {
        let items = self.history_query(point_id, start, end, limit)?;
        let total = items.len() as i64;
        let samples = items.into_iter().map(HistorySample::from_raw).collect();
        Ok(HistoryQueryResult::new(samples, total, "raw"))
    }

    /// 定期维护（retention cleanup + WAL checkpoint + aggregation + backup）。
    /// 对于不需要维护的后端（如 MemoryStorage），此方法为 no-op。
    fn maintenance(&self) -> GatewayResult<()>;

    /// 设置历史数据保留天数（None = 不限）。v2.1 起不由 license 驱动：
    /// 启动固定 30 天，仅供运维/测试调整。
    /// 默认 no-op：内存后端无持久化历史，无需按天截断；SQLite 后端覆写以生效。
    fn set_storage_retention_days(&self, _days: Option<u32>) {}

    /// v2.0：最旧一条持久化数据的时间戳（unix 毫秒）。
    ///
    /// 用于历史保留跨度统计：实际跨度 = now - oldest_ts（仅供观测/展示，
    /// 不再作为 license 配额用量）。原始表与聚合表都需纳入，
    /// 因为超过 24h 的原始行会被压成分钟桶写入聚合表。
    /// 默认 None：内存后端无持久化历史，返回 None 表示"跨度为 0"。
    fn oldest_data_timestamp(&self) -> Option<i64> {
        None
    }

    /// 是否需要定期维护。
    fn needs_maintenance(&self) -> bool;

    /// 存储后端名称（"sqlite" / "memory"），用于日志区分。
    fn name(&self) -> &str;

    /// #20-2026：未送达行超限信号。被 `DataQueue::enqueue` 入口在每次
    /// 入队时实时读取，决定是否走 fallback 缓冲（与 `disk_ok=false` 同等路径）。
    /// 默认 false（内存后端无此概念）。SQLite 后端在 `enforce_queue_limit`
    /// 检测到 unsent/total >= `UNSENT_OVERFLOW_RATIO` 时置 true。
    fn unsent_overflow(&self) -> bool {
        false
    }
}
