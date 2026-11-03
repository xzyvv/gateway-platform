//! 监督循环 + 内部动作枚举
//!
//! 从 scheduler/mod.rs 拆分：负责 task 的 panic 重启 + handle_results 的锁内决策枚举。

use std::panic;
use std::time::Duration;

use futures::FutureExt;
use tokio::sync::watch;

use crate::engine::model::DataPoint;
use crate::engine::model::Quality;

pub enum PointAction {
    Ok(DataPoint),
    Err {
        quality: Quality,
        last_good: Option<f64>,
        err_msg: String,
        point_id: String,
        consecutive_failures: u32,
        last_good_ts: i64,
        stale_count: u32,
    },
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(interval_fn, task_fn), fields(name = %name), level = "debug"))]
pub(super) async fn supervision_loop<Fut, G, IF>(
    name: String,
    mut shutdown: watch::Receiver<bool>,
    // 修复(审查#B1)：本组独立的停止信号。热重载需精确停掉「某一组」，
    // 全局 shutdown 无法区分，故此处额外监听本组信号；两者任一置 true（或
    // 发送端被 drop → changed() 返回 Err）即优雅退出。
    mut group_stop: watch::Receiver<bool>,
    mut interval_fn: IF,
    mut task_fn: G,
) where
    Fut: std::future::Future<Output = ()> + Send,
    // F-11：把本轮 interval 传给任务闭包，便于按真实周期判定「耗时超周期」，
    // 时序快照无需再重复推算间隔；闭包忽略入参即可保持旧行为。
    G: FnMut(Duration) -> Fut + Send + 'static,
    IF: FnMut() -> Duration + Send + 'static,
{
    // W2: 注册看门狗心跳，超时为采集间隔的 3 倍（至少 30s）
    let hb_name = format!("scheduler:{}", name);
    let heartbeat = crate::engine::monitor::heartbeat::get()
        .register(&hb_name, 30_000)
        .await;

    // 固定速率调度：next_tick 按 interval 递增，与任务耗时无关，避免长期漂移
    let mut next_tick = tokio::time::Instant::now();
    // Bug 74: panic 退避计数器——任务 panic 后指数退避重启，避免 bug 反复触发刷屏
    // Bug 110: 退避基数 5s，指数翻倍后实际由 match 分支钳制（避免长时间死循环）；
    // 计数器仅跟踪同次调度循环的 panic，进程重启时归零（普通 u64 局部变量）。
    let mut panic_backoff_secs: u64 = 5;
    let mut panic_count: u32 = 0;
    loop {
        // W2: 心跳，表示循环正常运行
        heartbeat.kick();
        // 读取当前间隔（可能因退避而放大）—— 包裹 catch_unwind 防止 interval_fn panic
        let interval =
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(&mut interval_fn)) {
                Ok(d) => d,
                Err(_) => {
                    tracing::error!("task {} interval_fn panicked, using default 1s", name);
                    Duration::from_secs(1)
                }
            };
        // 推进到下一个 tick（固定速率：不随任务耗时漂移）
        next_tick += interval;
        let now = tokio::time::Instant::now();
        let sleep_dur = match next_tick.checked_duration_since(now) {
            Some(d) => d,
            None => {
                // 修复(审查#A3)：任务耗时超过 interval 时 next_tick 会被越抛越远，
                // 原 `unwrap_or(Duration::ZERO)` 让 sleep_dur 恒为 0 → 该组背靠背
                // 连续占用总线，同一总线上的其他组被饿死（现场表现为整条总线只有
                // 一个组在上报）。落后时丢弃已错过的 tick、把基准拉回当前时刻：
                // 固定速率 + 不追补，宁可少采一次也不突发补采。
                next_tick = now;
                Duration::ZERO
            }
        };
        tokio::select! {
            res = shutdown.changed() => {
                // Sender drop 或 set(true) → 退出
                if res.is_err() || *shutdown.borrow() {
                    tracing::info!("task {} shutting down", name);
                    return;
                }
                continue;
            }
            // 修复(审查#B1)：本组独立停止信号（热重载精确停组）
            res = group_stop.changed() => {
                if res.is_err() || *group_stop.borrow() {
                    tracing::info!("task {} stopping (group reload)", name);
                    return;
                }
                continue;
            }
            _ = tokio::time::sleep(sleep_dur) => {}
        }

        // 执行业务逻辑，捕获 panic
        let result = std::panic::AssertUnwindSafe(task_fn(interval))
            .catch_unwind()
            .await;

        match result {
            Ok(_) => {
                // Bug 74: 成功一次即重置 panic 计数器与退避，恢复正常调度节奏
                if panic_backoff_secs != 5 || panic_count != 0 {
                    panic_backoff_secs = 5;
                    panic_count = 0;
                }
            }
            Err(panic_info) => {
                let msg = if let Some(s) = panic_info.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = panic_info.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic".to_string()
                };
                // Bug D3: 原实现 panic 分支恒 sleep 5s，panic_backoff_secs/panic_count
                // 从未参与退避计算（panic_count 甚至从不自增），注释声称的指数退避
                // 实为死代码 → 任务反复 panic 时只会每 5s 重启并刷日志。
                // 现按 5s 起、每次 panic 翻倍（上限 300s），成功一次即复位。
                panic_count = panic_count.saturating_add(1);
                let backoff = panic_backoff_secs.min(300);
                tracing::error!(
                    "task {} panicked (#{}): {}, restarting in {}s",
                    name,
                    panic_count,
                    msg,
                    backoff
                );
                tokio::select! {
                    res = shutdown.changed() => {
                        if res.is_err() || *shutdown.borrow() { return; }
                    }
                    // 修复(审查#B1)：panic 退避等待期间同样响应本组停止信号
                    res = group_stop.changed() => {
                        if res.is_err() || *group_stop.borrow() { return; }
                    }
                    _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}
                }
                panic_backoff_secs = panic_backoff_secs.saturating_mul(2).min(300);
            }
        }
    }
}
