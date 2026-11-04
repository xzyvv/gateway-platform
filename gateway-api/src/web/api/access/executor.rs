//! 计划任务调度执行器
//!
//! 后台循环：每 15s 扫描一次 `next_run_at <= now` 的启用任务并执行，
//! 执行后回写 last_run_at / run_count / last_status / last_error 并按 cron 重算
//! 下次执行时间。手动「立即执行」（run-now 接口）与定时执行共用同一条
//! [`execute_and_record`] 路径，保证语义一致。
//!
//! 幂等/安全说明：
//! - 单任务执行设 60s 超时，防止采集卡死拖垮整个调度循环；
//! - 同一 tick 内任务串行执行（网关资源有限，避免并发打爆总线）；
//! - 执行结果只回写 SQLite，失败不 panic、不影响后续任务。
//!
//! 时钟保护（无 RTC / 时钟未同步场景）：
//! cron 是墙钟语义，依赖系统时间本身正确。无电池 RTC 的板子掉电后时间常回落到
//! 1970/厂商默认值，或运行中被 OS 校时大幅调整，此时按 `Utc::now()` 判到期会
//! 在错误时刻执行任务（前跳会集中补跑、后跳会静默停摆）。因此调度循环在每 tick
//! 用 [`clock_untrust_reason`] 判定时钟可信度：
//! - 不可信期间**不触发**任何定时任务（只告警一次），手动「立即执行」不受影响；
//! - 恢复可信或检测到墙钟大幅跳变时，用 [`reanchor_all`] 按当前时刻重算全部启用
//!   任务的 next_run_at——**漏掉的窗口直接跳过，不补跑**，避免开机/校时瞬间集中执行。

use std::sync::Arc;

use gateway_core::config::{ScheduledTask, TaskRunStatus, TaskType};

use crate::web::types::AppState;

/// 单个任务执行超时
const TASK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// 调度 tick 间隔
const TICK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);

/// 时钟可信下界：2020-01-01T00:00:00Z（毫秒）。
///
/// 无电池 RTC 的工控板掉电后时间常回落到 1970 或厂商默认时刻，任何真实部署都
/// 不应早于该下界，低于它即可判定墙钟无效。
const MIN_PLAUSIBLE_MS: i64 = 1_577_836_800_000;
/// 系统时间与最近一次 NTP 校时结果的偏差上限，超过即视为时钟不可信。
/// 取 5 分钟：cron 最细粒度为分钟，超过该量级已足以让「每天 02:00」错时执行。
const MAX_NTP_OFFSET_MS: i64 = 5 * 60 * 1000;
/// 相邻 tick 间墙钟跳变阈值，超过视为时钟被 OS 校时/手动改表大幅调整。
const CLOCK_JUMP_MS: i64 = 5 * 60 * 1000;

/// 判定系统时钟不可信的原因；返回 `None` 表示可信。
///
/// `ntp_gap_ms` 仅在「最近一次 NTP 校时成功」且该结果未因墙钟跳变而失效时传入，
/// 表示 NTP 时间与本地时间之差（毫秒，正值=本地落后）。纯函数便于单测。
fn clock_untrust_reason(now_ms: i64, ntp_gap_ms: Option<i64>) -> Option<String> {
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

/// 把全部启用任务的 next_run_at 从 `now_ms` 起重算（漏掉的窗口不补跑）。
///
/// 用于时钟恢复可信或检测到大幅跳变之后：既避免后跳导致的静默停摆，也避免
/// 前跳导致一批任务在开机/校时瞬间集中执行。停用任务（next_run_at 为 NULL）不动。
fn reanchor_all(state: &AppState, now_ms: i64) {
    let tasks = match state.platform.sched_tasks.list() {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("re-anchor scheduled tasks failed to list: {}", e);
            return;
        }
    };
    let now_secs = now_ms / 1000;
    let mut changed = 0usize;
    for task in tasks.iter().filter(|t| t.enabled) {
        let Some(next) = gateway_core::support::cron::next_run_ms(&task.cron, now_secs) else {
            continue;
        };
        if Some(next) == task.next_run_at {
            continue;
        }
        match state
            .platform
            .sched_tasks
            .set_next_run_at(&task.id, Some(next))
        {
            Ok(()) => changed += 1,
            Err(e) => tracing::warn!("re-anchor task {} failed: {}", task.id, e),
        }
    }
    tracing::info!(
        "scheduled tasks re-anchored: {} task(s) rescheduled",
        changed
    );
}

/// 一次执行的结果
pub struct ExecResult {
    pub ok: bool,
    /// 成功时为简述（如 "read 3 points"），失败时为错误消息
    pub message: String,
}

/// 执行任务并回写记录（run-now 与后台调度共用）。
pub async fn execute_and_record(state: &Arc<AppState>, task: &ScheduledTask) -> ExecResult {
    let result = match tokio::time::timeout(TASK_TIMEOUT, execute(state, task)).await {
        Ok(r) => r,
        Err(_) => ExecResult {
            ok: false,
            message: format!("执行超时（>{}s）", TASK_TIMEOUT.as_secs()),
        },
    };
    // 按 cron 从当前时刻重算下次执行时间（停用任务不排程）
    let next = if task.enabled {
        gateway_core::support::cron::next_run_ms(&task.cron, chrono::Utc::now().timestamp())
    } else {
        None
    };
    let status = if result.ok {
        TaskRunStatus::Success
    } else {
        TaskRunStatus::Error
    };
    if let Err(e) = state.platform.sched_tasks.record_run(
        &task.id,
        status,
        if result.ok {
            None
        } else {
            Some(result.message.as_str())
        },
        next,
    ) {
        tracing::warn!("scheduled task record_run failed: {}", e);
    }
    result
}

/// 执行任务本体（不写记录，供 execute_and_record 包装）
async fn execute(state: &Arc<AppState>, task: &ScheduledTask) -> ExecResult {
    match task.task_type {
        TaskType::ReadPoint => execute_read_point(state, task).await,
        TaskType::ReadDevice => execute_read_device(state, task).await,
        TaskType::BackupConfig => execute_backup_config(state),
        TaskType::SyncNtp => execute_sync_ntp(state).await,
    }
}

fn missing_target() -> ExecResult {
    ExecResult {
        ok: false,
        message: "任务缺少目标 ID".to_string(),
    }
}

async fn execute_read_point(state: &Arc<AppState>, task: &ScheduledTask) -> ExecResult {
    let Some(target) = task.target.as_deref() else {
        return missing_target();
    };
    let config = state.config.load_full().as_ref().clone();
    let Some(point) = config.points.iter().find(|p| p.id == target).cloned() else {
        return ExecResult {
            ok: false,
            message: format!("点位 {} 不存在", target),
        };
    };
    let collectors = state.south.scheduler.collectors();
    let Some(collector) = collectors.get(&point.bus) else {
        return ExecResult {
            ok: false,
            message: format!("总线 {} 无采集器", point.bus),
        };
    };
    match collector.read(&point).await {
        Ok(dp) => ExecResult {
            ok: true,
            message: format!("点位 {} = {}", target, dp.value),
        },
        Err(e) => ExecResult {
            ok: false,
            message: format!("读点位 {} 失败: {}", target, e),
        },
    }
}

async fn execute_read_device(state: &Arc<AppState>, task: &ScheduledTask) -> ExecResult {
    let Some(target) = task.target.as_deref() else {
        return missing_target();
    };
    let config = state.config.load_full().as_ref().clone();
    // 与 /api/devices/:id/points 同口径：优先按 device_id 匹配，回退按 bus 名
    let points: Vec<_> = config
        .points
        .iter()
        .filter(|p| p.device_id.as_deref() == Some(target) || p.bus == target)
        .cloned()
        .collect();
    if points.is_empty() {
        return ExecResult {
            ok: false,
            message: format!("设备 {} 下没有点位", target),
        };
    }
    let collectors = state.south.scheduler.collectors();
    let mut ok_count = 0usize;
    let mut err_msgs: Vec<String> = Vec::new();
    for point in &points {
        match collectors.get(&point.bus) {
            Some(collector) => match collector.read(point).await {
                Ok(_) => ok_count += 1,
                Err(e) => err_msgs.push(format!("{}: {}", point.id, e)),
            },
            None => err_msgs.push(format!("{}: 总线 {} 无采集器", point.id, point.bus)),
        }
    }
    if err_msgs.is_empty() {
        ExecResult {
            ok: true,
            message: format!("读取 {} 个点位全部成功", ok_count),
        }
    } else {
        ExecResult {
            ok: ok_count > 0,
            message: format!(
                "成功 {}/{}，失败: {}",
                ok_count,
                points.len(),
                err_msgs.join("; ")
            ),
        }
    }
}

fn execute_backup_config(state: &Arc<AppState>) -> ExecResult {
    let config = state.config.load_full().as_ref().clone();
    match state
        .config
        .version_store
        .save_version(&config, "scheduled backup")
    {
        Ok(id) => ExecResult {
            ok: true,
            message: format!("配置已备份为版本 v{}", id),
        },
        Err(e) => ExecResult {
            ok: false,
            message: format!("配置备份失败: {}", e),
        },
    }
}

async fn execute_sync_ntp(state: &Arc<AppState>) -> ExecResult {
    match state.platform.ntp.sync_now().await {
        Ok(()) => ExecResult {
            ok: true,
            message: "NTP 校时成功".to_string(),
        },
        Err(e) => ExecResult {
            ok: false,
            message: format!("NTP 校时失败: {}", e),
        },
    }
}

/// 调度主循环：由 gatewayd bootstrap 通过 spawn_supervised 启动。
///
/// 退出语义：本循环自身不退出；panic 由 supervisor 捕获并按退避策略重启。
pub async fn run_loop(state: Arc<AppState>) {
    // 启动后等一个 tick 再扫：给采集器/NTP 等子系统留出初始化时间，
    // 避免开机瞬间任务因依赖未就绪而打出误导性失败记录。
    tokio::time::sleep(TICK_INTERVAL).await;

    // 时钟门控状态（仅进程内；重启后重新判定，不持久化）
    let mut last_now: Option<i64> = None;
    // 已据以判定的 NTP 校时时间戳（毫秒）
    let mut seen_ntp_sync: Option<i64> = None;
    // 墙钟大幅跳变后，旧的 NTP 偏差不再代表当前误差，需等下一次成功校时才重新采信
    let mut ntp_gap_valid = true;
    // 当前不可信原因（Some = 已暂停并已告警过）
    let mut paused: Option<String> = None;

    loop {
        let now = chrono::Utc::now().timestamp_millis();

        // 新的校时结果到达 → 旧的「偏差失效」标记复位
        let ntp = state.platform.ntp.status();
        if ntp.last_sync != seen_ntp_sync {
            seen_ntp_sync = ntp.last_sync;
            ntp_gap_valid = true;
        }

        // 相邻 tick 墙钟跳变（OS 校时 / 手动改表），阈值取 5 分钟
        let jumped = last_now.is_some_and(|prev| (now - prev).abs() > CLOCK_JUMP_MS);
        if jumped {
            ntp_gap_valid = false;
        }

        // 只有最近一次校时成功（source == "NTP"）且结果未失效时，才用偏差判可信度
        let ntp_gap = if ntp_gap_valid && ntp.source == "NTP" && ntp.last_sync.is_some() {
            Some(ntp.offset_ms)
        } else {
            None
        };

        match clock_untrust_reason(now, ntp_gap) {
            Some(reason) => {
                // 不可信期间不触发任何定时任务，原因变化时才再告警一次
                if paused.as_deref() != Some(reason.as_str()) {
                    tracing::warn!(
                        "system clock not trustworthy ({}); scheduled tasks paused until the clock is corrected",
                        reason
                    );
                }
                paused = Some(reason);
            }
            None => {
                if let Some(prev) = paused.take() {
                    tracing::info!(
                        "system clock restored (was: {}); re-anchoring scheduled tasks to current time",
                        prev
                    );
                    reanchor_all(&state, now);
                } else if jumped && last_now.is_some() {
                    // 正常运行中时钟被大幅校正：跳过漏跑窗口，避免一次性补跑
                    tracing::warn!(
                        "wall clock jumped by {}s; re-anchoring scheduled tasks (missed windows are not back-filled)",
                        (now - last_now.unwrap_or(now)) / 1000
                    );
                    reanchor_all(&state, now);
                }

                match state.platform.sched_tasks.due_tasks(now) {
                    Ok(due) => {
                        for task in due {
                            let result = execute_and_record(&state, &task).await;
                            if !result.ok {
                                tracing::warn!(
                                    "scheduled task {} ({}) failed: {}",
                                    task.id,
                                    task.name,
                                    result.message
                                );
                            } else {
                                tracing::info!(
                                    "scheduled task {} ({}) ok: {}",
                                    task.id,
                                    task.name,
                                    result.message
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("scheduled tasks due scan failed: {}", e);
                    }
                }
            }
        }

        last_now = Some(now);
        tokio::time::sleep(TICK_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-09 附近（远晚于可信下界）
    const T2026: i64 = 1_790_000_000_000;

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
}
