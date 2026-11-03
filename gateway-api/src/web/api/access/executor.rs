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

use std::sync::Arc;

use gateway_core::config::{ScheduledTask, TaskRunStatus, TaskType};

use crate::web::types::AppState;

/// 单个任务执行超时
const TASK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// 调度 tick 间隔
const TICK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);

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
    loop {
        let now = chrono::Utc::now().timestamp_millis();
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
        tokio::time::sleep(TICK_INTERVAL).await;
    }
}
