//! 队列容量配置（队列表行数硬上限）
//!
//! 该值决定存储层何时开始淘汰最旧数据，以及容量压力线的位置
//! （`capacity_pressure_rows` = 上限 × 90%）。背压线（10 万行）与积压线
//! （1 万行）是恒定阈值，不随本配置漂移——它们是「上行是否健康」的判据，
//! 而本配置是「磁盘还能放多少」的预算。
//!
//! 运行期语义：存储层在构造时取一次（决定淘汰线），健康判定每次请求现取
//! （`queue_health` 的 `max_rows` 入参），因此改配置后水位判定立即生效，
//! 淘汰线在重启后生效。

use serde::{Deserialize, Serialize};

/// 队列表行数硬上限默认值：100 万行（与配置化前的硬编码一致）。
///
/// 类型用 `usize` 而非 `i64`：行数不可能为负，且 `usize` 在 ts-rs 里映射为
/// TS `number`；若用 `i64` 会生成 `bigint`，前端 `JSON.stringify` 配置时会抛错。
pub fn default_max_rows() -> usize {
    crate::queue::DEFAULT_MAX_QUEUE_ROWS as usize
}

/// 队列容量分节。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, ts_rs::TS)]
#[ts(export)]
pub struct QueueConfig {
    /// 队列表行数硬上限：超过后按 FIFO 淘汰最旧的非在途记录
    #[serde(default = "default_max_rows")]
    pub max_rows: usize,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            max_rows: default_max_rows(),
        }
    }
}

impl QueueConfig {
    /// 语义校验：范围 + 与既有水位阈值的顺序（实现见
    /// [`crate::queue::max_rows_errors`]，阈值定义集中在队列内核）。
    pub fn semantic_errors(&self) -> Vec<String> {
        crate::queue::max_rows_errors(self.max_rows as i64)
    }
}
