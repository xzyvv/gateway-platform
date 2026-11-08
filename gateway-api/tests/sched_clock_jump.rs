//! 计划任务调度：NTP 校时导致的墙钟**回拨 / 前跳**行为测试。
//!
//! 无电池 RTC 的板子掉电后时间会回落，运行中也会被 NTP 大幅校正。cron 是墙钟语义，
//! 若不做重锚定：
//! - **回拨**：`next_run_at` 停留在「按旧时间算出的未来时刻」，任务静默停摆数小时；
//! - **前跳**：一批早已错过的窗口同时变成「到期」，开机/校时瞬间集中补跑。
//!
//! 调度循环的处置是 [`reanchor_all`]：按当前时刻重算全部启用任务的 next_run_at，
//! **漏掉的窗口直接跳过、不补跑**。这些语义此前零测试覆盖，本文件把它们钉死。

mod common;

use std::sync::Arc;

use gateway_api::web::api::access::executor::reanchor_all;
use gateway_api::web::AppState;
use gateway_core::config::{ScheduledTask, ScheduledTaskStore, TaskType};

/// 用例统一按 UTC+8 墙钟构造 epoch，避免结果随开发机 / CI 时区漂移。
const CST: i32 = 8 * 3600;

/// 构造「UTC+8 墙钟 y-mo-d h:mi:s」对应的 epoch 秒
fn cst(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> i64 {
    chrono::NaiveDate::from_ymd_opt(y, mo, d)
        .and_then(|d| d.and_hms_opt(h, mi, s))
        .expect("valid date")
        .and_utc()
        .timestamp()
        - CST as i64
}

/// 把设备时区钉在 UTC+8：`cron` 的墙钟字段解释与运行环境无关。
///
/// `resolve_timezone` 进程内只初始化一次，因此必须在任何 cron 求值之前设置。
fn pin_timezone() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::env::set_var(gateway_core::support::cron::TZ_ENV, "+08:00");
    });
}

fn store(state: &Arc<AppState>) -> &ScheduledTaskStore {
    &state.platform.sched_tasks
}

fn new_task(cron: &str, enabled: bool, next_run_at: Option<i64>) -> ScheduledTask {
    ScheduledTask {
        id: String::new(),
        name: "clock jump case".into(),
        // 用 backup_config：本文件只验证排程，不触发采集/读点等依赖 mock 的行为
        task_type: TaskType::BackupConfig,
        target: None,
        cron: cron.into(),
        enabled,
        last_run_at: None,
        next_run_at,
        run_count: 0,
        last_status: None,
        last_error: None,
        created_at: 0,
    }
}

fn create(state: &Arc<AppState>, task: ScheduledTask) -> ScheduledTask {
    store(state).create(task).expect("create scheduled task")
}

fn next_of(state: &Arc<AppState>, id: &str) -> Option<i64> {
    store(state)
        .get(id)
        .expect("get scheduled task")
        .expect("task should exist")
        .next_run_at
}

/// 回拨（时间倒退）：不得停留在「旧时间算出的未来锚点」而停摆。
///
/// 场景：墙钟曾走到 12:00，每天 08:00 的任务锚在「次日 08:00」；随后时钟被回拨到
/// 06:00。此时旧锚点离现在 26 小时——当天 08:00 的窗口会被整段跳过。
#[tokio::test]
async fn clock_rollback_reanchors_instead_of_stalling() {
    pin_timezone();
    let state = common::mock_state::mock_app_state();

    let now_ms = cst(2026, 10, 25, 6, 0, 0) * 1000;
    let stale = cst(2026, 10, 26, 8, 0, 0) * 1000; // 回拨后距 now 26 小时
    let task = create(&state, new_task("0 8 * * *", true, Some(stale)));
    assert_eq!(
        next_of(&state, &task.id),
        Some(stale),
        "前置：锚点是回拨前的未来时刻"
    );

    reanchor_all(&state, now_ms);

    assert_eq!(
        next_of(&state, &task.id),
        Some(cst(2026, 10, 25, 8, 0, 0) * 1000),
        "回拨后应重锚到当天 08:00，而不是保持旧时间算出的次日 08:00"
    );
    assert!(
        store(&state).due_tasks(now_ms).expect("due").is_empty(),
        "重锚定后的任务不应在回拨时刻立即到期"
    );
}

/// 前跳（时间越过窗口）：漏掉的窗口必须跳过，不得集中补跑。
#[tokio::test]
async fn forward_jump_skips_missed_window_instead_of_backfilling() {
    pin_timezone();
    let state = common::mock_state::mock_app_state();

    // 墙钟从 06:00 前跳到 09:00，08:00 的窗口已经错过
    let now_ms = cst(2026, 10, 25, 9, 0, 0) * 1000;
    let missed = cst(2026, 10, 25, 8, 0, 0) * 1000;
    let task = create(&state, new_task("0 8 * * *", true, Some(missed)));

    // 前置：不做重锚定时，这个错过的窗口会被当成「到期」——正是要避免的补跑
    assert_eq!(
        store(&state).due_tasks(now_ms).expect("due").len(),
        1,
        "前置：前跳后错过的窗口在旧实现里会进入补跑队列"
    );

    reanchor_all(&state, now_ms);

    assert_eq!(
        next_of(&state, &task.id),
        Some(cst(2026, 10, 26, 8, 0, 0) * 1000),
        "前跳后应直接排到下一个 08:00（次日），不保留已错过的窗口"
    );
    assert!(
        store(&state).due_tasks(now_ms).expect("due").is_empty(),
        "漏掉的窗口不得补跑"
    );
}

/// 停用任务不参与重锚定（next_run_at 保持为空）。
#[tokio::test]
async fn disabled_tasks_are_not_reanchored() {
    pin_timezone();
    let state = common::mock_state::mock_app_state();

    let task = create(&state, new_task("0 8 * * *", false, None));
    reanchor_all(&state, cst(2026, 10, 25, 6, 0, 0) * 1000);

    assert_eq!(next_of(&state, &task.id), None, "停用任务不应被重新排程");
}

/// 重锚定幂等；无法解析的 cron 保持原样（不 panic、不误改）。
#[tokio::test]
async fn reanchor_is_idempotent_and_skips_bad_cron() {
    pin_timezone();
    let state = common::mock_state::mock_app_state();

    let now_ms = cst(2026, 10, 25, 6, 0, 0) * 1000;
    let good = create(
        &state,
        new_task("0 8 * * *", true, Some(cst(2026, 10, 26, 8, 0, 0) * 1000)),
    );
    let bad = create(&state, new_task("not a cron", true, Some(12_345)));

    reanchor_all(&state, now_ms);
    let first = next_of(&state, &good.id);
    assert_eq!(first, Some(cst(2026, 10, 25, 8, 0, 0) * 1000));

    // 同一时刻再重锚定一次：结果必须不变（否则每次 tick 都会扰动排程）
    reanchor_all(&state, now_ms);
    assert_eq!(
        next_of(&state, &good.id),
        first,
        "同一 now 下重锚定必须幂等"
    );
    assert_eq!(
        next_of(&state, &bad.id),
        Some(12_345),
        "无法解析的 cron 应保持原锚点，交由创建/更新接口拦截"
    );
}
