//! 队列侧的纯函数辅助：积压告警沿判定、优先级打分、fallback 缓冲回退。
//!
//! 从 `queue/mod.rs` 抽出：这些函数不依赖 `DataQueue` 的内部状态，单独成模块
//! 后 mod.rs 只保留类型与主体逻辑，也便于按函数粒度过测试。

use super::*;

/// P0-3：积压告警沿判定（按健康等级比较，而非只看单一阈值）。
///
/// - `Some(new)`：等级发生变化（升级/降级/恢复），需要发事件
///   - 升级到 Backlog：warning 级告警 + 计数
///   - 升级到 Backpressure / CapacityPressure：critical 级告警 + 计数
///   - 回到 Ok：发 Info 恢复事件，不计数
/// - `None`：等级未变，不重复告警
pub(super) fn backlog_alarm_edge(last: QueueHealth, now: QueueHealth) -> Option<QueueHealth> {
    (now != last).then_some(now)
}

/// Bug 170: 数据点优先级分数（越高越优先保留）
/// Good=3, Uncertain=2, Stale=1, Bad=0；stale_count 越高优先级越低
pub(crate) fn priority_score(dp: &DataPoint) -> i32 {
    use crate::engine::model::Quality;
    let base = match dp.quality {
        Quality::Good => 3,
        Quality::Uncertain => 2,
        Quality::Stale => 1,
        Quality::Bad => 0,
    };
    base * 1000 - (dp.stale_count as i32)
}

/// Bug 170: 找到缓冲区中优先级最低的条目索引
pub(crate) fn find_lowest_priority_idx(
    buf: &std::collections::VecDeque<DataPoint>,
) -> Option<usize> {
    buf.iter()
        .enumerate()
        .min_by_key(|(_, dp)| priority_score(dp))
        .map(|(idx, _)| idx)
}

/// Bug 170: 把一批数据点回退进 fallback 缓冲，超容量时按优先级淘汰最低者。
///
/// 供两处复用：`DataQueue::enqueue` 的磁盘满分支，以及队列任务写 SQLite 失败。
/// （磁盘满 / DB 约束 / IO 错误）的回退路径——后者修复了原先"非磁盘满错误被
/// warn 丢点"的静默数据丢失。
pub(crate) fn push_into_fallback(
    buf: &mut std::collections::VecDeque<DataPoint>,
    capacity: usize,
    points: Vec<DataPoint>,
    retry_count: &mut u32,
) -> (usize, usize) {
    // Bug 183 (N01): 反弹轮次达到上限后强制丢弃本批点。
    if *retry_count >= FALLBACK_MAX_RETRIES {
        let dropped = points.len();
        *retry_count = 0; // 重置，给下一批（非病态）数据点机会
        return (0, dropped);
    }
    *retry_count += 1;
    let n_in = points.len();
    let before = buf.len();
    for dp in points {
        buf.push_back(dp);
    }
    // 新点占用的下标区间起点。淘汰不一定是新点（按优先级可能踢掉更旧的条目），
    // 因此要按区间位置统计「本批真正留下来的条数」，否则日志里的 retained
    // 会在淘汰旧点时被误报为 0（运维会以为新数据全丢了）。
    let mut new_start = before;
    let mut new_evicted = 0usize;
    let mut dropped = 0usize;
    while buf.len() > capacity {
        let idx = find_lowest_priority_idx(buf).unwrap_or(0);
        if idx >= new_start {
            new_evicted += 1;
        } else {
            new_start = new_start.saturating_sub(1);
        }
        buf.remove(idx);
        dropped += 1;
    }
    (n_in - new_evicted, dropped)
}
