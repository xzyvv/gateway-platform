//! 设备监督与退避策略（§3.9 设备退避状态）
//!
//! 跟踪每个设备/点位的连续失败次数，
//! 连续失败 >3 次进入指数退避，质量码降为 Stale。
//! Q13: 扩展 last_good_ts / stale_count，支持双时间戳 + 连续重发计数。

use std::time::Duration;

use crate::engine::model::Quality;

/// 设备退避状态（§3.9）
#[derive(Debug, Clone)]
pub struct DeviceState {
    /// 连续失败次数（>3 进入退避）
    pub consecutive_failures: u32,
    /// 当前实际采集间隔（退避时放大）
    pub current_interval: Duration,
    /// 基础间隔（配置值，恢复时还原）
    pub base_interval: Duration,
    /// 上次成功值，失败时用此值上送
    pub last_good_value: Option<f64>,
    /// 上次成功采集的原始时间戳（source_ts 用），0 表示无成功历史
    pub last_good_ts: i64,
    /// 连续重发次数（每次失败 +1，成功归零）
    pub stale_count: u32,
    /// 离线次数（「正常 → 首次失败」的次数，成功不归零）。
    /// 供设备健康评分统计窗口内的掉线频次，此前该字段不存在，健康页只能用伪随机数。
    pub offline_episodes: u32,
    /// 当前离线段的起始时间戳（ms，0 = 不在离线段）。与 offline_episodes 同步置位，
    /// 供 MTBF 统计把「离线时长」落到真实时间上。
    pub offline_since_ts: i64,
    /// 已结束离线段的累计时长（ms）；进行中的离线段由 [Self::downtime_ms_at] 现算加总。
    pub total_downtime_ms: i64,
    /// 状态创建时间戳（ms）：统计窗口起点（本点位首个采集结果到达时刻）。
    pub tracking_since_ts: i64,
}

impl DeviceState {
    pub fn new(base_interval: Duration) -> Self {
        Self {
            consecutive_failures: 0,
            current_interval: base_interval,
            base_interval,
            last_good_value: None,
            last_good_ts: 0,
            stale_count: 0,
            offline_episodes: 0,
            offline_since_ts: 0,
            total_downtime_ms: 0,
            tracking_since_ts: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// 记录成功，重置退避 + 更新 last_good
    pub fn record_success(&mut self, value: f64) {
        self.consecutive_failures = 0;
        self.current_interval = self.base_interval;
        self.last_good_value = Some(value);
        let now = chrono::Utc::now().timestamp_millis();
        self.last_good_ts = now;
        self.stale_count = 0;
        // 离线段结束：把本段离线时长计入累计
        if self.offline_since_ts > 0 {
            self.total_downtime_ms = self
                .total_downtime_ms
                .saturating_add((now - self.offline_since_ts).max(0));
            self.offline_since_ts = 0;
        }
    }

    /// 记录失败，更新退避 + 累加 stale_count
    pub fn record_failure(&mut self) {
        // 从「正常」跌到失败算一次离线事件；持续失败不重复计数
        if self.consecutive_failures == 0 {
            self.offline_episodes = self.offline_episodes.saturating_add(1);
            self.offline_since_ts = chrono::Utc::now().timestamp_millis();
        }
        self.consecutive_failures += 1;
        // 修复(C-6)：`Duration * u32` 在溢出时会 panic（"overflow when multiplying
        // duration by scalar"），而这里运行在采集 task 里，panic 会直接把该组采集打挂。
        // base_interval 来自配置（point.interval 秒），配一个极端大值时
        // `base_interval * 60` 就可能溢出。改用 saturating_mul：溢出时收敛到
        // Duration 可表示的最大值（等价于"退避到天荒地老"），语义与不溢出时连续。
        self.current_interval = self.base_interval.saturating_mul(self.backoff_multiplier());
        self.stale_count = self.stale_count.saturating_add(1);
    }

    /// 退避倍率（§7.3）
    fn backoff_multiplier(&self) -> u32 {
        match self.consecutive_failures {
            0..=2 => 1,
            3..=4 => 5,
            5..=9 => 15,
            _ => 60,
        }
    }

    /// 截至 now 的累计离线时长（ms）：已结束段累计 + 进行中离线段（now - offline_since_ts）。
    /// MTBF 统计用；不改动内部状态，快照路径随时可调。
    pub fn downtime_ms_at(&self, now: i64) -> i64 {
        let ongoing = if self.offline_since_ts > 0 {
            (now - self.offline_since_ts).max(0)
        } else {
            0
        };
        self.total_downtime_ms.saturating_add(ongoing)
    }

    /// 当前质量码（成功 Good / 少量失败 Bad / 退避中 Stale）
    pub fn current_quality(&self) -> Quality {
        // Bug C2: 原实现只有 Stale/Bad 两个分支，健康点位（0 次失败）也返回 Bad，
        // 导致在线点位统计恒为 0、device_status 把所有点位显示为 bad/stale。补 Good 分支。
        match self.consecutive_failures {
            0 => Quality::Good,
            1..=2 => Quality::Bad,
            _ => Quality::Stale,
        }
    }

    /// 失败原因字符串
    pub fn current_reason(&self) -> &'static str {
        if self.consecutive_failures >= 3 {
            "device_backoff"
        } else {
            "device_timeout"
        }
    }
}

#[cfg(test)]
mod tests;
