//! ROI-10（G-05）：从站（设备）级熔断。
//!
//! 逐点退避解决「多久再试」，本模块解决「还要不要试」：同一设备连续整轮失败
//! 达到阈值后打开熔断，冷却期内不再占用总线；冷却期满进入半开，放行一轮探测：
//! - 探测成功 → 关闭熔断，恢复正常周期；
//! - 探测仍整体失败 → 重新打开，冷却时间翻倍（封顶 [`MAX_COOLDOWN`]）。
//!
//! 判定口径是「设备本轮是否有任一成功点位」，因此单点偶发失败不会误跳闸。

use std::time::{Duration, Instant};

/// 连续整轮失败达到该次数后打开熔断。
pub(crate) const OPEN_AFTER_ROUNDS: u32 = 3;
/// 首次打开的冷却时间。
const BASE_COOLDOWN: Duration = Duration::from_secs(60);
/// 反复打开时的冷却上限。
const MAX_COOLDOWN: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BreakerEvent {
    /// 本轮由关闭/半开转为打开
    Opened { cooldown: Duration },
    /// 本轮由打开/半开恢复为关闭
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Closed,
    /// 打开至 `until`，冷却期内一律拒绝
    Open {
        until: Instant,
    },
    /// 冷却期满后放行一轮探测
    HalfOpen,
}

/// 单台设备的熔断状态机。
#[derive(Debug, Clone)]
pub(crate) struct DeviceBreaker {
    state: State,
    /// 连续整轮失败次数（成功即清零）
    failed_rounds: u32,
    /// 下次打开时的冷却时间（重复失败逐次翻倍）
    next_cooldown: Duration,
}

impl Default for DeviceBreaker {
    fn default() -> Self {
        Self {
            state: State::Closed,
            failed_rounds: 0,
            next_cooldown: BASE_COOLDOWN,
        }
    }
}

impl DeviceBreaker {
    /// 本轮是否允许该设备的点位发请求。
    ///
    /// 打开且未到冷却期 → `false`；冷却期满 → 自动转半开并放行。
    pub(crate) fn allow(&mut self, now: Instant) -> bool {
        match self.state {
            State::Closed | State::HalfOpen => true,
            State::Open { until } if now >= until => {
                self.state = State::HalfOpen;
                true
            }
            State::Open { .. } => false,
        }
    }

    /// 一轮结束后更新状态。`any_success` 表示该设备本轮至少一个点位成功。
    pub(crate) fn record_round(&mut self, any_success: bool, now: Instant) -> Option<BreakerEvent> {
        if any_success {
            let was_open = !matches!(self.state, State::Closed);
            self.state = State::Closed;
            self.failed_rounds = 0;
            self.next_cooldown = BASE_COOLDOWN;
            return was_open.then_some(BreakerEvent::Closed);
        }

        match self.state {
            // 半开探测失败：直接重新打开，冷却翻倍
            State::HalfOpen => {
                let cooldown = self.next_cooldown;
                self.next_cooldown = (self.next_cooldown * 2).min(MAX_COOLDOWN);
                self.state = State::Open {
                    until: now + cooldown,
                };
                Some(BreakerEvent::Opened { cooldown })
            }
            State::Closed => {
                self.failed_rounds = self.failed_rounds.saturating_add(1);
                if self.failed_rounds < OPEN_AFTER_ROUNDS {
                    return None;
                }
                let cooldown = self.next_cooldown;
                self.next_cooldown = (self.next_cooldown * 2).min(MAX_COOLDOWN);
                self.state = State::Open {
                    until: now + cooldown,
                };
                Some(BreakerEvent::Opened { cooldown })
            }
            // 打开期间点位不会被读，理论上不会收到结果；保守忽略
            State::Open { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_after_threshold_failed_rounds() {
        let mut b = DeviceBreaker::default();
        let t0 = Instant::now();

        assert_eq!(b.record_round(false, t0), None);
        assert_eq!(b.record_round(false, t0), None);
        assert!(b.allow(t0), "未达阈值前不得熔断");

        assert_eq!(
            b.record_round(false, t0),
            Some(BreakerEvent::Opened {
                cooldown: BASE_COOLDOWN
            })
        );
        assert!(!b.allow(t0), "打开后冷却期内必须拒绝");
        assert!(!b.allow(t0 + BASE_COOLDOWN - Duration::from_secs(1)));
    }

    #[test]
    fn half_open_probe_success_closes() {
        let mut b = DeviceBreaker::default();
        let t0 = Instant::now();
        for _ in 0..OPEN_AFTER_ROUNDS {
            b.record_round(false, t0);
        }
        let probe_at = t0 + BASE_COOLDOWN;
        assert!(b.allow(probe_at), "冷却期满必须放行探测");
        assert_eq!(b.record_round(true, probe_at), Some(BreakerEvent::Closed));
        // 关闭后立即恢复，且失败计数清零
        assert!(b.allow(probe_at));
        assert_eq!(b.record_round(false, probe_at), None);
        assert_eq!(b.record_round(false, probe_at), None);
    }

    #[test]
    fn failed_probe_reopens_with_doubled_cooldown() {
        let mut b = DeviceBreaker::default();
        let t0 = Instant::now();
        for _ in 0..OPEN_AFTER_ROUNDS {
            b.record_round(false, t0);
        }
        let probe_at = t0 + BASE_COOLDOWN;
        assert!(b.allow(probe_at));
        assert_eq!(
            b.record_round(false, probe_at),
            Some(BreakerEvent::Opened {
                cooldown: BASE_COOLDOWN * 2
            })
        );
        assert!(!b.allow(probe_at + BASE_COOLDOWN));
        assert!(b.allow(probe_at + BASE_COOLDOWN * 2));
    }

    #[test]
    fn cooldown_is_capped() {
        let mut b = DeviceBreaker::default();
        let mut now = Instant::now();
        for _ in 0..OPEN_AFTER_ROUNDS {
            b.record_round(false, now);
        }
        // 反复探测失败，冷却应逐次翻倍并封顶
        for _ in 0..10 {
            now += MAX_COOLDOWN;
            assert!(b.allow(now), "冷却期满应放行");
            if let Some(BreakerEvent::Opened { cooldown }) = b.record_round(false, now) {
                assert!(cooldown <= MAX_COOLDOWN);
            }
        }
    }

    #[test]
    fn any_success_keeps_breaker_closed() {
        let mut b = DeviceBreaker::default();
        let t0 = Instant::now();
        for _ in 0..10 {
            // 每轮都有成功点：即使同轮还有失败点，也不得熔断
            b.record_round(true, t0);
            assert!(b.allow(t0));
        }
    }
}
