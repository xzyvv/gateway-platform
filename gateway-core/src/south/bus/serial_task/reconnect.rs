//! 串口总线后台自愈重连（串口热拔插）。
//!
//! # 为什么单独成文件
//!
//! 串口总线 task 的主循环只关心「取事务 → 执行 → 回结果」，而「端口掉了怎么办」
//! 是一组独立的关注点：单飞标志、退避档位、句柄回填、存活标志收尾。
//! 二者混在一个文件里会让主循环文件持续膨胀（行数预算门禁 C24 对本文件的
//! 上限是 745 行），故拆出为子模块，语义边界与文件边界一致。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use crate::config::{next_reconnect_backoff, BusConfig, ReconnectBackoffConfig};
use crate::engine::metrics::Metrics;
use crate::south::bus::serial::SerialHandle;
use crate::south::bus::transport::BusDeps;

/// 后台重连任务的共享上下文（字段较多，用结构体承载避免超长参数列表）。
pub(super) struct ReconnectCtx {
    pub(super) bus_config: BusConfig,
    pub(super) name: String,
    pub(super) deps: BusDeps,
    pub(super) state: Arc<StdMutex<Option<SerialHandle>>>,
    pub(super) in_flight: Arc<AtomicBool>,
    pub(super) alive: Arc<AtomicBool>,
    /// 用于累计「每总线重连次数」（统计报表 / 健康报告展示）
    pub(super) metrics: Arc<Metrics>,
}

/// 修复(审查#B3)：总线存活标志的 Drop guard。
///
/// 保证串口总线 task 的 future 在**任何**方式下结束（正常返回、panic、被
/// `JoinHandle::abort()` 取消）都会把 `bus_alive` 置为 false，从而使
/// [`ensure_background_reconnect`] 拉起的后台重连任务在下一轮循环退出，
/// 不再持续抢占串口。
pub(super) struct AliveGuard(pub(super) Arc<AtomicBool>);

impl Drop for AliveGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// 短暂持有状态桥锁（仅在同步块内使用，绝不跨 await）。锁中毒时取回内部值而非 panic：
/// 状态桥只是句柄容器，数据本身不因持锁线程 panic 而失效。
pub(super) fn lock_slot(
    state: &StdMutex<Option<SerialHandle>>,
) -> std::sync::MutexGuard<'_, Option<SerialHandle>> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 确保后台重连任务在跑（单飞）。
///
/// 任务按总线退避档位（默认 1/2/5/10/30 秒 + 抖动）反复尝试重开串口；成功后把新句柄**回填状态桥**
/// （仅在槽位为空时回填，避免覆盖主循环正在使用的句柄），下一笔事务即可正常使用，
/// 实现热插拔自愈且无需重启进程。总线存活标志被清除（主循环退出）时任务收尾，避免泄漏。
pub(super) fn ensure_background_reconnect(ctx: &ReconnectCtx) {
    // 单飞：已有后台重连在跑则直接返回
    if ctx
        .in_flight
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    let bus_config = ctx.bus_config.clone();
    let backoff_cfg = bus_config.reconnect_backoff.clone();
    let name = ctx.name.clone();
    let deps = ctx.deps.clone();
    let state = Arc::clone(&ctx.state);
    let in_flight = Arc::clone(&ctx.in_flight);
    let alive = Arc::clone(&ctx.alive);
    let metrics = Arc::clone(&ctx.metrics);
    tokio::spawn(async move {
        // 与 Modbus TCP 重连共用同一档位与抖动公式（attempt 从 1 起）。
        let mut attempt: u32 = 1;
        loop {
            if !alive.load(Ordering::Acquire) {
                break;
            }
            tracing::info!("bus {} background reconnect: attempt", name);
            match try_reopen_serial(&bus_config, &deps).await {
                Ok(new_handle) => {
                    // 修复(审查#B3)：打开过程是 await，期间总线可能已退出
                    // （alive=false）。此时不得把新句柄装回状态桥——否则该句柄
                    // 的生命周期被状态桥拖住，串口无法随总线一并释放。直接丢弃
                    // 新句柄（SerialHandle drop 即关闭串口）并收尾。
                    if !alive.load(Ordering::Acquire) {
                        drop(new_handle);
                        break;
                    }
                    let mut slot = lock_slot(&state);
                    if slot.is_none() {
                        *slot = Some(new_handle);
                        drop(slot);
                        // 句柄真正回填成功才算一次「重连」；槽位被占用时是多余重开，不计。
                        metrics.inc_bus_reconnect(&name);
                        tracing::info!("bus {} background reopen succeeded, handle restored", name);
                    } else {
                        drop(slot);
                        tracing::debug!(
                            "bus {} background reopen succeeded but slot occupied; dropping new handle",
                            name
                        );
                    }
                    break;
                }
                Err(e) => {
                    let delay = next_reconnect_backoff(
                        &backoff_cfg,
                        attempt,
                        ReconnectBackoffConfig::time_rand01(),
                    );
                    tracing::warn!(
                        "bus {} background reopen failed: {}, retrying in {:?}",
                        name,
                        e,
                        delay
                    );
                    tokio::time::sleep(delay).await;
                    attempt = attempt.saturating_add(1);
                }
            }
        }
        in_flight.store(false, Ordering::Release);
    });
}

async fn try_reopen_serial(
    bus_config: &BusConfig,
    deps: &BusDeps,
) -> Result<SerialHandle, std::io::Error> {
    let cfg = bus_config.clone();
    let opener = deps.opener.clone();
    let res = tokio::task::spawn_blocking(move || opener.open(&cfg)).await;
    match res {
        Ok(Ok(serial)) => Ok(serial),
        Ok(Err(e)) => Err(e),
        Err(join_err) => Err(std::io::Error::other(format!(
            "reopen task panicked: {}",
            join_err
        ))),
    }
}
