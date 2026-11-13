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
        // 缓冲区非空时一定能找到淘汰对象；找不到说明缓冲已被抽空，
        // 此时必须退出：旧写法用 `unwrap_or(0)` 兜底，会拿一个无效下标去
        // `VecDeque::remove`——对空队列而言 remove 是空操作，len 永不下降，
        // 循环原地打转直到进程被拖死（现场表现：采集线程占满 CPU、队列不再上报）。
        let Some(idx) = find_lowest_priority_idx(buf) else {
            break;
        };
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::model::DataPoint;
    use crate::engine::model::Quality;

    /// 构造一个指定质量与 stale 次数的数据点。
    fn point(quality: Quality, stale_count: u32) -> DataPoint {
        let now = DataPoint::now();
        DataPoint {
            point_id: "p1".into(),
            device_id: "dev1".into(),
            value: 1.0,
            raw_value: 1.0,
            raw_type: "u16".into(),
            quality,
            quality_reason: "".into(),
            timestamp: now,
            source_ts: now,
            stale_count,
            value_text: None,
        }
    }

    /// 优先级分数的**具体数值**是契约的一部分：质量每降一级扣 1000 分，
    /// stale_count 每多 1 再扣 1 分。
    ///
    /// 这条断言盯住 `base * 1000 - stale_count` 里的乘号与减号：
    /// 只比较相对顺序抓不到 —— 乘号改成加号后（Good=1003、Bad=1000）
    /// 顺序照样成立，但「坏点分数为负、可以被整体垫底」这一语义已经变了。
    #[test]
    fn priority_score_has_stable_absolute_values() {
        assert_eq!(priority_score(&point(Quality::Good, 0)), 3000);
        assert_eq!(priority_score(&point(Quality::Uncertain, 0)), 2000);
        assert_eq!(priority_score(&point(Quality::Stale, 0)), 1000);
        assert_eq!(priority_score(&point(Quality::Bad, 0)), 0);
        // stale_count 越多，分数越低（且可以降到 0 以下）
        assert_eq!(priority_score(&point(Quality::Good, 7)), 2993);
        assert_eq!(priority_score(&point(Quality::Bad, 3)), -3);
    }

    /// 淘汰时应挑出分数最低的那一个（坏点先走、好点后走）。
    #[test]
    fn find_lowest_priority_idx_picks_worst_point() {
        let mut buf = std::collections::VecDeque::new();
        buf.push_back(point(Quality::Good, 0));
        buf.push_back(point(Quality::Bad, 0));
        buf.push_back(point(Quality::Uncertain, 0));
        assert_eq!(find_lowest_priority_idx(&buf), Some(1));
    }

    /// 超容量时只淘汰到**恰好等于容量**为止，并且返回值的语义是
    /// `(本批留下数, 本批被淘汰数)`。
    ///
    /// 这条断言盯住三处：
    /// - `while buf.len() > capacity` 的边界（改成 >= 会多淘汰一个）；
    /// - `new_evicted += 1` 的累加（改成 *= 1 在初值为 0 时等价，掩盖了淘汰数）；
    /// - `n_in - new_evicted` 的减法（改成加法会让「留下数」凭空翻倍）。
    #[test]
    fn fallback_evicts_exactly_one_when_over_capacity() {
        let mut buf = std::collections::VecDeque::new();
        let mut retry = 0u32;
        let pts = vec![
            point(Quality::Bad, 0),
            point(Quality::Bad, 0),
            point(Quality::Good, 0),
        ];
        let (retained, dropped) = push_into_fallback(&mut buf, 2, pts, &mut retry);
        assert_eq!(dropped, 1, "容量 2 装 3 个，只该淘汰 1 个");
        assert_eq!(retained, 2, "留下数 = 批内总数 - 本批被淘汰数");
        assert_eq!(buf.len(), 2, "淘汰后长度应恰好等于容量");
    }

    /// 淘汰的是**旧的低优先级点**时，本批新点应全部留下。
    /// 这条断言盯住「区间下标统计」那段：`new_start` 的递减与
    /// `new_evicted` 的累加一旦被改写，日志里的 retained 会误报为 0
    /// （运维会以为新数据全丢了）。
    #[test]
    fn fallback_keeps_new_points_when_old_worst_is_evicted() {
        let mut buf = std::collections::VecDeque::new();
        let mut retry = 0u32;

        let (retained, dropped) =
            push_into_fallback(&mut buf, 2, vec![point(Quality::Bad, 0)], &mut retry);
        assert_eq!((retained, dropped), (1, 0), "未超容量，不该淘汰");

        let (retained, dropped) = push_into_fallback(
            &mut buf,
            2,
            vec![point(Quality::Good, 0), point(Quality::Good, 0)],
            &mut retry,
        );
        assert_eq!(dropped, 1, "被淘汰的是旧的坏点");
        assert_eq!(retained, 2, "本批两个新点都应留下");
        assert_eq!(buf.len(), 2);
    }

    /// 反弹轮次达到上限后强制丢弃本批点，并重置计数（给下一批机会）。
    #[test]
    fn fallback_drops_batch_after_max_retries() {
        let mut buf = std::collections::VecDeque::new();
        let mut retry = FALLBACK_MAX_RETRIES;
        let (retained, dropped) =
            push_into_fallback(&mut buf, 10, vec![point(Quality::Good, 0)], &mut retry);
        assert_eq!((retained, dropped), (0, 1), "达到上限应整批丢弃");
        assert_eq!(retry, 0, "计数应重置，给下一批机会");
        assert!(buf.is_empty(), "被丢弃的点不应进缓冲");
    }
}
