//! 寄存器扫描后台任务

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

use crate::web::api::protocol_tools::DebugLease;
use crate::web::debug_bus::MAX_SCAN_DURATION;
use crate::web::services::WordProbe;

use super::judge::{area_for, judge_cell, judge_word_probe, read_frame};
use super::ScanResult;

/// 扫描审计回调：(action, bus, detail)
pub(super) type ScanAudit = Arc<dyn Fn(&'static str, &str, &str) + Send + Sync>;

/// 启动真实寄存器扫描后台任务。
///
/// 任务持有租约逐地址探测，增量更新 `entry.result`；遇总线掉线 → status=failed；
/// DELETE `/register-scan/:id` 置 cancel → 下一地址前停止 → status=aborted；正常结束 → done。
/// 必须 `let lease = lease;` 整体移入，避免 Rust 2021 disjoint capture 把
/// `_guard` 提前 drop（p0-2 #28 同根因）。
#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_register_scan(
    audit: ScanAudit,
    bus_name: String,
    lease: DebugLease,
    entry: Arc<Mutex<ScanResult>>,
    cancel: Arc<AtomicBool>,
    slave_id: i32,
    reg_type: String,
    start_addr: i32,
    end_addr: i32,
    timeout_ms: u64,
    interval_ms: u64,
) {
    tokio::spawn(async move {
        // 租约整体使用：`session` 字段内已绑定互斥守卫，捕获它即连守卫一起进任务
        let protocol = lease.bus.protocol.to_ascii_lowercase();
        // mc/fins 自建 TCP 连接，不走总线裸帧 exchange；其余协议沿用 Modbus RTU 帧探测
        let area = area_for(&protocol, &reg_type);
        let probe = area.map(|_| WordProbe::new(&lease.bus, timeout_ms));
        let started = std::time::Instant::now();
        let total = end_addr - start_addr + 1;
        let mut responded: i32 = 0;
        let mut offline = false;
        let mut aborted = false;

        for (idx, addr) in (start_addr..=end_addr).enumerate() {
            if started.elapsed() >= MAX_SCAN_DURATION {
                aborted = true;
                break;
            }
            if idx > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(interval_ms)).await;
            }
            // /stop 中止：下一地址开始前停止（契约 §6.3）
            if cancel.load(Ordering::Relaxed) {
                aborted = true;
                break;
            }

            let (cell, is_offline) = match (area, probe.as_ref()) {
                (Some(area), Some(probe)) => match probe.probe(slave_id, area, addr).await {
                    Some(outcome) => judge_word_probe(&protocol, addr, outcome),
                    // 构建未启用该协议 feature：如实报不可探测，不能伪造 responded=true
                    None => judge_word_probe(
                        &protocol,
                        addr,
                        Err("该构建未启用对应协议 feature".into()),
                    ),
                },
                _ => {
                    let outcome = lease
                        .session
                        .exchange(&read_frame(slave_id, &reg_type, addr), timeout_ms)
                        .await;
                    judge_cell(slave_id, &reg_type, addr, outcome)
                }
            };
            let cell_responded = cell.responded;
            {
                let mut r = entry.lock();
                r.cells.push(cell);
                if cell_responded {
                    r.responded += 1;
                }
                responded = r.responded;
                r.progress = ((r.cells.len() as i64 * 100) / total as i64) as i32;
            }
            if is_offline {
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
        {
            let mut r = entry.lock();
            r.status = final_status.into();
            r.progress = 100;
        }
        let action = if final_status == "aborted" {
            "debug.register_scan.abort"
        } else {
            "debug.register_scan"
        };
        audit(
            action,
            &bus_name,
            &format!(
                "slave={} reg_type={} range={}-{} responded={}/{} status={} duration_ms={}",
                slave_id,
                reg_type,
                start_addr,
                end_addr,
                responded,
                total,
                final_status,
                duration_ms
            ),
        );
    });
}
