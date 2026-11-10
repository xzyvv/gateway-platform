//! 系统时钟可信度判定（无 RTC / 时钟未同步场景）。
//!
//! cron 是**墙钟**语义，依赖系统时间本身正确。无电池 RTC 的工控板掉电后时间常
//! 回落到 1970 或厂商默认时刻；运行中 OS 校时也会让时间突然跳变。此时若直接拿
//! `Utc::now()` 判到期，会出现两种故障：
//! - **时间回退**：任务的 `next_run_at` 落在"将来"，静默停摆；
//! - **时间前跳**：一批任务的 `next_run_at` 同时成为过去，在错误时刻集中补跑。
//!
//! 本模块提供可复用的判定与状态机：[`clock_untrust_reason`] 判单次可信度，
//! [`ClockGate`] 负责跨 tick 的状态迁移（暂停 / 恢复 / 跳变重排）。
//!
//! 设计约束：**不做 I/O、不读系统时间**——由调用方把当前墙钟和 NTP 观测传入，
//! 因此可以完整单测；调度器与 API 共用同一份判据，避免各写一套阈值。

/// 时钟可信下界：2020-01-01T00:00:00Z（毫秒）。
///
/// 无电池 RTC 的工控板掉电后时间常回落到 1970 或厂商默认时刻，任何真实部署都
/// 不应早于该下界，低于它即可判定墙钟无效。
pub const MIN_PLAUSIBLE_MS: i64 = 1_577_836_800_000;
/// 系统时间与最近一次 NTP 校时结果的偏差上限，超过即视为时钟不可信。
/// 取 5 分钟：cron 最细粒度为分钟，超过该量级已足以让「每天 02:00」错时执行。
pub const MAX_NTP_OFFSET_MS: i64 = 5 * 60 * 1000;
/// 相邻 tick 间墙钟跳变阈值，超过视为时钟被 OS 校时/手动改表大幅调整。
pub const CLOCK_JUMP_MS: i64 = 5 * 60 * 1000;

/// 判定系统时钟不可信的原因；返回 `None` 表示可信。
///
/// `ntp_gap_ms` 仅在「最近一次 NTP 校时成功」且该结果未因墙钟跳变而失效时传入，
/// 表示 NTP 时间与本地时间之差（毫秒，正值=本地落后）。
pub fn clock_untrust_reason(now_ms: i64, ntp_gap_ms: Option<i64>) -> Option<String> {
    if now_ms < MIN_PLAUSIBLE_MS {
        return Some("系统时间早于 2020-01-01（疑似无 RTC 掉电回退）".to_string());
    }
    match ntp_gap_ms {
        Some(gap) if gap.abs() > MAX_NTP_OFFSET_MS => {
            Some(format!("系统时间与 NTP 偏差 {} 秒", gap / 1000))
        }
        _ => None,
    }
}

/// 计算「此刻」的时钟可信度，返回不可信原因（`None` = 可信）。
///
/// 用于 API / 自检等**只读**场景：只看当前墙钟与最近一次 NTP 偏差，不考虑跨 tick
/// 的墙钟跳变。调度器用 [`ClockGate`]，额外处理跳变导致的偏差失效。
pub fn evaluate_clock(now_ms: i64, ntp: NtpObservation) -> Option<String> {
    let gap = if ntp.synced && ntp.last_sync_ms.is_some() {
        Some(ntp.offset_ms)
    } else {
        None
    };
    clock_untrust_reason(now_ms, gap)
}

/// 单次 tick 观察到的校时状态（与具体 `NtpStatus` 解耦，便于纯逻辑单测与复用）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NtpObservation {
    /// 最近一次成功校时时间戳（毫秒）；`None` = 从未成功
    pub last_sync_ms: Option<i64>,
    /// 最近一次校时是否成功且来自 NTP；`false` = 未同步/失败
    pub synced: bool,
    /// NTP 时间与本地时间之差（毫秒，正值 = 本地落后）
    pub offset_ms: i64,
}

/// 时钟门控要求调度循环执行的动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClockAction {
    /// 时钟可信：正常扫描到期任务
    RunTasks,
    /// 时钟可信但刚恢复/发生大幅跳变：先重排再扫描
    ReanchorThenRun(ReanchorReason),
    /// 时钟不可信：暂停定时任务；`announce` 仅在原因变化时给出，避免每 tick 重复告警
    Pause { announce: Option<String> },
}

/// 需要重排计划任务的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReanchorReason {
    /// 从不可信恢复为可信（携带恢复前的暂停原因）
    Restored { was: String },
    /// 正常运行时墙钟大幅跳变（携带跳变毫秒数，正 = 前跳）
    Jumped { delta_ms: i64 },
}

/// 时钟可信度状态机（纯逻辑，不做 I/O）。
///
/// 每 tick 调用 [`ClockGate::step`]，由返回值决定「暂停 / 正常扫描 / 先重排再扫描」。
/// 关键不变量：
/// - 只有「最近一次校时成功 + 偏差未失效」才用 NTP 偏差判可信度；
/// - 墙钟大幅跳变会让旧偏差失效，等下一次成功校时才重新采信；
/// - 不可信期间只在原因变化时告警，避免刷屏。
#[derive(Debug)]
pub struct ClockGate {
    /// 上一个 tick 的墙钟时间（毫秒）
    last_now_ms: Option<i64>,
    /// 已据以判定的 NTP 校时时间戳（毫秒）
    seen_ntp_sync: Option<i64>,
    /// 旧的 NTP 偏差是否仍代表当前误差（墙钟跳变后置 false）
    ntp_gap_valid: bool,
    /// 当前不可信原因（`Some` = 处于暂停；也用于判定是否需要再次告警）
    paused: Option<String>,
}

impl Default for ClockGate {
    fn default() -> Self {
        Self::new()
    }
}

impl ClockGate {
    pub fn new() -> Self {
        Self {
            last_now_ms: None,
            seen_ntp_sync: None,
            ntp_gap_valid: true,
            paused: None,
        }
    }

    /// 推进一个 tick，返回调度循环应执行的动作。
    pub fn step(&mut self, now_ms: i64, ntp: NtpObservation) -> ClockAction {
        // 新的校时结果到达 → 旧的「偏差失效」标记复位
        if ntp.last_sync_ms != self.seen_ntp_sync {
            self.seen_ntp_sync = ntp.last_sync_ms;
            self.ntp_gap_valid = true;
        }

        // 相邻 tick 墙钟跳变（OS 校时 / 手动改表）
        let jumped = self
            .last_now_ms
            .is_some_and(|prev| (now_ms - prev).abs() > CLOCK_JUMP_MS);
        if jumped {
            self.ntp_gap_valid = false;
        }

        let ntp_gap = if self.ntp_gap_valid && ntp.synced && ntp.last_sync_ms.is_some() {
            Some(ntp.offset_ms)
        } else {
            None
        };

        let action = match clock_untrust_reason(now_ms, ntp_gap) {
            Some(reason) => {
                let announce = if self.paused.as_deref() != Some(reason.as_str()) {
                    Some(reason.clone())
                } else {
                    None
                };
                self.paused = Some(reason);
                ClockAction::Pause { announce }
            }
            None => {
                let reason = if let Some(was) = self.paused.take() {
                    Some(ReanchorReason::Restored { was })
                } else if jumped {
                    Some(ReanchorReason::Jumped {
                        delta_ms: now_ms - self.last_now_ms.unwrap_or(now_ms),
                    })
                } else {
                    None
                };
                match reason {
                    Some(r) => ClockAction::ReanchorThenRun(r),
                    None => ClockAction::RunTasks,
                }
            }
        };

        self.last_now_ms = Some(now_ms);
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-09 附近（远晚于可信下界）
    const T2026: i64 = 1_790_000_000_000;

    fn ntp_synced(last_sync_ms: i64, offset_ms: i64) -> NtpObservation {
        NtpObservation {
            last_sync_ms: Some(last_sync_ms),
            synced: true,
            offset_ms,
        }
    }

    fn ntp_unsynced() -> NtpObservation {
        NtpObservation::default()
    }

    #[test]
    fn epoch_time_is_untrusted() {
        // 1970 / 2001 / 下界前一毫秒都应判不可信
        assert!(clock_untrust_reason(0, None).is_some());
        assert!(clock_untrust_reason(1_000_000_000_000, None).is_some());
        assert!(clock_untrust_reason(MIN_PLAUSIBLE_MS - 1, None).is_some());
    }

    #[test]
    fn plausible_time_without_ntp_is_trusted() {
        // 无 NTP 的离线现场：只要系统时间合理就不阻塞计划任务
        assert!(clock_untrust_reason(MIN_PLAUSIBLE_MS, None).is_none());
        assert!(clock_untrust_reason(T2026, None).is_none());
    }

    #[test]
    fn large_ntp_gap_is_untrusted() {
        let r = clock_untrust_reason(T2026, Some(MAX_NTP_OFFSET_MS + 1)).unwrap();
        assert!(r.contains("NTP"), "reason should mention NTP: {r}");
        // 本地超前 NTP（负偏差）同样不可信
        assert!(clock_untrust_reason(T2026, Some(-(MAX_NTP_OFFSET_MS + 1))).is_some());
    }

    #[test]
    fn small_ntp_gap_is_trusted() {
        // 阈值内（含边界）视为可信
        assert!(clock_untrust_reason(T2026, Some(0)).is_none());
        assert!(clock_untrust_reason(T2026, Some(30_000)).is_none());
        assert!(clock_untrust_reason(T2026, Some(MAX_NTP_OFFSET_MS)).is_none());
    }

    // ── 状态机迁移测试：覆盖「暂停 → 恢复 → 重排」与前后跳变 ──

    #[test]
    fn first_tick_plausible_runs_tasks() {
        let mut gate = ClockGate::new();
        assert_eq!(gate.step(T2026, ntp_unsynced()), ClockAction::RunTasks);
    }

    #[test]
    fn first_tick_with_epoch_time_pauses() {
        let mut gate = ClockGate::new();
        match gate.step(0, ntp_unsynced()) {
            ClockAction::Pause { announce } => {
                assert!(announce.unwrap().contains("2020-01-01"));
            }
            other => panic!("expected Pause, got {other:?}"),
        }
    }

    #[test]
    fn persistent_untrusted_announces_only_once() {
        let mut gate = ClockGate::new();
        // 首次不可信要告警
        let first = gate.step(0, ntp_unsynced());
        assert!(matches!(first, ClockAction::Pause { announce: Some(_) }));
        // 原因不变的后续 tick 不再重复告警
        let second = gate.step(0, ntp_unsynced());
        assert_eq!(second, ClockAction::Pause { announce: None });
    }

    #[test]
    fn clock_restored_reanchors_with_previous_reason() {
        let mut gate = ClockGate::new();
        assert!(matches!(
            gate.step(0, ntp_unsynced()),
            ClockAction::Pause { announce: Some(_) }
        ));
        // 校时把时间拉到 2026：转为「先重排再扫描」，并带回暂停原因
        match gate.step(T2026, ntp_unsynced()) {
            ClockAction::ReanchorThenRun(ReanchorReason::Restored { was }) => {
                assert!(was.contains("2020-01-01"));
            }
            other => panic!("expected Restored re-anchor, got {other:?}"),
        }
        // 恢复之后回归正常，不再重排
        assert_eq!(
            gate.step(T2026 + 15_000, ntp_unsynced()),
            ClockAction::RunTasks
        );
    }

    #[test]
    fn forward_jump_reanchors() {
        let mut gate = ClockGate::new();
        assert_eq!(gate.step(T2026, ntp_unsynced()), ClockAction::RunTasks);
        // 前跳 1 小时：否则一批任务会在瞬间集中补跑
        match gate.step(T2026 + 3_600_000, ntp_unsynced()) {
            ClockAction::ReanchorThenRun(ReanchorReason::Jumped { delta_ms }) => {
                assert_eq!(delta_ms, 3_600_000);
            }
            other => panic!("expected Jumped re-anchor, got {other:?}"),
        }
    }

    #[test]
    fn backward_jump_reanchors() {
        let mut gate = ClockGate::new();
        assert_eq!(gate.step(T2026, ntp_unsynced()), ClockAction::RunTasks);
        // 后跳 1 小时：否则任务会静默停摆到原时间之后
        match gate.step(T2026 - 3_600_000, ntp_unsynced()) {
            ClockAction::ReanchorThenRun(ReanchorReason::Jumped { delta_ms }) => {
                assert_eq!(delta_ms, -3_600_000);
            }
            other => panic!("expected Jumped re-anchor, got {other:?}"),
        }
    }

    #[test]
    fn small_jitter_does_not_reanchor() {
        let mut gate = ClockGate::new();
        assert_eq!(gate.step(T2026, ntp_unsynced()), ClockAction::RunTasks);
        // 一个 tick 的正常抖动（远小于阈值）不应触发重排
        assert_eq!(
            gate.step(T2026 + 20_000, ntp_unsynced()),
            ClockAction::RunTasks
        );
        assert_eq!(
            gate.step(T2026 + 15_000, ntp_unsynced()),
            ClockAction::RunTasks
        );
    }

    #[test]
    fn large_ntp_gap_pauses_tasks() {
        let mut gate = ClockGate::new();
        let obs = ntp_synced(T2026, MAX_NTP_OFFSET_MS + 1_000);
        match gate.step(T2026, obs) {
            ClockAction::Pause { announce } => {
                assert!(announce.unwrap().contains("NTP"));
            }
            other => panic!("expected Pause, got {other:?}"),
        }
    }

    #[test]
    fn jump_invalidates_stale_ntp_gap() {
        let mut gate = ClockGate::new();
        // 校时发现本地落后 1 小时 → 暂停
        let obs = ntp_synced(T2026, 3_600_000);
        assert!(matches!(
            gate.step(T2026, obs),
            ClockAction::Pause { announce: Some(_) }
        ));
        // 随后 OS 把墙钟校正了 1 小时（旧偏差不再代表当前误差）→ 恢复并重排
        match gate.step(T2026 + 3_600_000, obs) {
            ClockAction::ReanchorThenRun(ReanchorReason::Restored { .. }) => {}
            other => panic!("stale NTP gap should be invalidated by jump, got {other:?}"),
        }
    }

    #[test]
    fn fresh_ntp_sync_reenables_gap_check() {
        let mut gate = ClockGate::new();
        let obs = ntp_synced(T2026, 3_600_000);
        assert!(matches!(
            gate.step(T2026, obs),
            ClockAction::Pause { announce: Some(_) }
        ));
        // 墙钟跳变让旧偏差失效 → 暂时恢复
        match gate.step(T2026 + 3_600_000, obs) {
            ClockAction::ReanchorThenRun(_) => {}
            other => panic!("expected re-anchor, got {other:?}"),
        }
        // 新的校时结果（last_sync 变化）重新采信偏差：仍差 1 小时 → 再次暂停
        let fresh = ntp_synced(T2026 + 3_600_000, 3_600_000);
        assert!(matches!(
            gate.step(T2026 + 3_600_015, fresh),
            ClockAction::Pause { announce: Some(_) }
        ));
    }

    #[test]
    fn offline_without_ntp_still_runs() {
        // 无 NTP 的离线现场：时间合理就照常调度，不能因「从未同步」而永久暂停
        let mut gate = ClockGate::new();
        assert_eq!(gate.step(T2026, ntp_unsynced()), ClockAction::RunTasks);
        assert_eq!(
            gate.step(T2026 + 15_000, ntp_unsynced()),
            ClockAction::RunTasks
        );
    }

    #[test]
    fn evaluate_clock_matches_gate_verdict() {
        // 只读判据（API/自检）应与状态机的稳定态结论一致
        assert!(evaluate_clock(T2026, ntp_unsynced()).is_none());
        assert!(evaluate_clock(0, ntp_unsynced()).is_some());
        assert!(evaluate_clock(T2026, ntp_synced(T2026, MAX_NTP_OFFSET_MS + 1)).is_some());
        // 未同步时忽略 offset，避免用「上次失败校时」的陈旧值误判
        assert!(evaluate_clock(
            T2026,
            NtpObservation {
                synced: false,
                offset_ms: 3_600_000,
                last_sync_ms: Some(T2026)
            }
        )
        .is_none());
    }
}
