//! 参数探测后台任务
//!
//! 逐项发帧并增量写回 `steps`/`progress`；总线掉线 → failed，`/stop` → aborted，
//! 正常结束 → done。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

use crate::web::api::protocol_tools::DebugLease;
use crate::web::debug_bus::{BusErrorKind, MAX_SCAN_DURATION};

use super::judge::{
    judge_byteorder, judge_connectivity, judge_fc, judge_latency, judge_slave_id, read_one_frame,
    read_two_frame, StepOut,
};
use super::now_ms;
use super::AutoDetectResult;

/// 探测项总数（progress 计算用）
const STEP_COUNT: i32 = 5;

/// 探测审计回调：(action, device, detail)
pub(super) type DetectAudit = Arc<dyn Fn(&'static str, &str, &str) + Send + Sync>;

/// 启动真实参数探测后台任务。
///
/// 逐项发帧并增量写回 `steps`/`progress`；总线掉线 → failed，`/stop` → aborted，
/// 正常结束 → done。租约必须整体移入（`session` 内含互斥守卫），
/// 否则 Rust 2021 disjoint capture 会把 `_guard` 提前 drop（p0-2 #28 同根因）。
#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_auto_detect(
    audit: DetectAudit,
    device_id: String,
    device_name: String,
    bus_name: String,
    lease: DebugLease,
    entry: Arc<Mutex<AutoDetectResult>>,
    cancel: Arc<AtomicBool>,
    slave_id: u8,
    timeout_ms: u64,
    interval_ms: u64,
) {
    tokio::spawn(async move {
        // 租约整体使用：`session` 字段内已绑定互斥守卫，捕获它即连守卫一起进任务
        let lease = lease;
        let started = std::time::Instant::now();
        let mut offline = false;
        let mut aborted = false;
        // 第 1 项是否收到应答（第 2 项复用，不另发帧）
        let mut online = false;

        for i in 0..STEP_COUNT {
            if started.elapsed() >= MAX_SCAN_DURATION {
                aborted = true;
                break;
            }
            if i > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(interval_ms)).await;
            }
            if cancel.load(Ordering::Relaxed) {
                aborted = true;
                break;
            }
            // 进入该项：先标 testing，让前端轮询看到"探测中"
            {
                let mut r = entry.lock();
                r.steps[i as usize].status = "testing".into();
                r.progress = (i * 100) / STEP_COUNT;
            }

            let out = match i {
                0 => {
                    let out = judge_connectivity(
                        lease
                            .session
                            .exchange(&read_one_frame(slave_id, 0x03), timeout_ms)
                            .await,
                    );
                    online = out.status == "success";
                    out
                }
                1 => judge_slave_id(slave_id, online),
                2 => {
                    let out03 = lease
                        .session
                        .exchange(&read_one_frame(slave_id, 0x03), timeout_ms)
                        .await;
                    if matches!(&out03, Err(BusErrorKind::BusOffline)) {
                        // 总线已掉线，不再发第二帧
                        judge_fc(out03, Err(BusErrorKind::BusOffline))
                    } else {
                        tokio::time::sleep(std::time::Duration::from_millis(interval_ms)).await;
                        let out04 = lease
                            .session
                            .exchange(&read_one_frame(slave_id, 0x04), timeout_ms)
                            .await;
                        judge_fc(out03, out04)
                    }
                }
                3 => {
                    // 连续 3 帧测耗时；遇总线掉线立即停止
                    let mut outcomes = Vec::with_capacity(3);
                    for k in 0..3 {
                        if k > 0 {
                            tokio::time::sleep(std::time::Duration::from_millis(interval_ms)).await;
                        }
                        let outcome = lease
                            .session
                            .exchange(&read_one_frame(slave_id, 0x03), timeout_ms)
                            .await;
                        let is_offline = matches!(&outcome, Err(BusErrorKind::BusOffline));
                        outcomes.push(outcome);
                        if is_offline {
                            break;
                        }
                    }
                    judge_latency(outcomes)
                }
                4 => judge_byteorder(
                    lease
                        .session
                        .exchange(&read_two_frame(slave_id), timeout_ms)
                        .await,
                ),
                _ => StepOut::fail("未知探测项"),
            };

            {
                let mut r = entry.lock();
                r.steps[i as usize].status = out.status.into();
                r.steps[i as usize].value = out.value;
                r.steps[i as usize].detail = out.detail;
                r.progress = ((i + 1) * 100) / STEP_COUNT;
            }
            if out.offline {
                offline = true;
                break;
            }
        }

        let duration_ms = started.elapsed().as_millis() as i64;
        let final_status = if offline {
            "failed"
        } else if aborted {
            "aborted"
        } else {
            "done"
        };
        let ok_steps = {
            let r = entry.lock();
            r.steps.iter().filter(|s| s.status == "success").count()
        };
        {
            let mut r = entry.lock();
            r.status = final_status.into();
            r.progress = 100;
            r.end_time_ms = Some(now_ms());
        }
        let action = if final_status == "aborted" {
            "debug.auto_detect.abort"
        } else {
            "debug.auto_detect"
        };
        audit(
            action,
            &device_id,
            &format!(
                "device={} bus={} slave={} ok={}/{} status={} duration_ms={}",
                device_name, bus_name, slave_id, ok_steps, STEP_COUNT, final_status, duration_ms
            ),
        );
    });
}
