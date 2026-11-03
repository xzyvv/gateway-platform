//! 后台任务监管基础设施
//!
//! 提供 panic 自动重启的 `spawn_supervised`，
//! 替代裸 `tokio::spawn`，保证后台任务持续运行。

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::FutureExt;
use once_cell::sync::OnceCell;

use crate::engine::metrics::Metrics;

/// 基础设施层指标实例（bootstrap 启动时通过 `init()` 注入一次）
///
/// supervision 的 `spawn_supervised` / `spawn_catch_unwind` 被 ~40 处调用，
/// 逐个传参不现实。采用 OnceCell 一次初始化模式（与 heartbeat_monitor 的
/// WATCHDOG 一致），bootstrap 阶段设置，运行期只读。
static METRICS: OnceCell<Arc<Metrics>> = OnceCell::new();

/// 初始化基础设施层指标实例（bootstrap 启动时调用一次）
pub fn init(metrics: Arc<Metrics>) {
    let _ = METRICS.set(metrics);
}

/// 获取注入的指标实例，未初始化时返回临时空实例（适用于单元测试）
fn metrics() -> Arc<Metrics> {
    METRICS
        .get()
        .cloned()
        .unwrap_or_else(|| Arc::new(Metrics::new()))
}

/// S-2：后台任务重启退避上限（秒）
const MAX_BACKOFF_SECS: u64 = 30;

/// S-2：稳定运行时长阈值——运行超过此阈值后发生的 panic/错误视为「新故障」，
/// 重启退避重置为 1s。避免长跑稳定的任务因偶发一次崩溃而继承历史累积的最大退避
/// （旧实现 `backoff` 从 1 单调倍增至 30 且成功运行后从不归零）。
const BACKOFF_RESET_AFTER: Duration = Duration::from_secs(60);

/// S-2：本次运行是否已稳定存活足够久（久到随后的崩溃应视为新故障）
fn is_healthy_uptime(ran_for: Duration) -> bool {
    ran_for >= BACKOFF_RESET_AFTER
}

/// r34-SV2: 退避 jitter——把固定的 base backoff 加 ±25% 抖动，避免多 task
/// 同时 panic 后同步退避雪崩。jitter 来源 `SystemTime::now().subsec_nanos()`，
/// 无需外部 rand 依赖。
pub(crate) fn jittered_backoff_secs(base_secs: u64) -> u64 {
    if base_secs == 0 {
        return 0;
    }
    let lo = base_secs.saturating_mul(3) / 4;
    let hi = base_secs.saturating_sub(lo);
    let jitter = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0)
        % (hi + 1);
    lo + jitter
}

/// 启动一个受监管的后台任务：panic 后自动重启（指数退避 1s→2s→…→30s）。
///
/// S-1：统一后台任务监管。原本只有 scheduler 的采集任务有 `supervision_loop` 保护，
/// 其余 12 个文件中的 `tokio::spawn` 裸任务一旦 panic 会导致该功能模块静默死亡
/// （甚至波及 runtime）。调用此函数替代裸 `tokio::spawn`，保证后台任务持续运行。
///
/// 语义：
/// - `make_fut()` 返回的 future 正常结束 → 监管循环退出（任务自然完成）
/// - future panic → 记录 error 日志 + 退避后重新调用 `make_fut()` 重启
pub fn spawn_supervised<F, Fut>(name: impl Into<String>, make_fut: F) -> tokio::task::JoinHandle<()>
where
    F: Fn() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let name = name.into();
    tokio::spawn(async move {
        let mut backoff: u64 = 1;
        loop {
            let run_started = Instant::now();
            let res = std::panic::AssertUnwindSafe(make_fut())
                .catch_unwind()
                .await;
            match res {
                Ok(()) => {
                    tracing::debug!("supervised task {} exited normally", name);
                    return;
                }
                Err(panic_info) => {
                    // S-2: 长期稳定运行后偶发一次 panic，不应继承历史累积的最大退避
                    if is_healthy_uptime(run_started.elapsed()) {
                        backoff = 1;
                    }
                    let msg = if let Some(s) = panic_info.downcast_ref::<&str>() {
                        s.to_string()
                    } else if let Some(s) = panic_info.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "unknown panic".to_string()
                    };
                    tracing::error!(
                        "supervised task {} panicked: {}, restarting in {}s",
                        name,
                        msg,
                        backoff
                    );
                    metrics().inc_task_panic(&name);
                    // r34-SV2: 退避加 jitter，防止多 task 同步重试雪崩
                    let sleep_secs = jittered_backoff_secs(backoff);
                    tokio::time::sleep(Duration::from_secs(sleep_secs)).await;
                    backoff = backoff.saturating_mul(2).min(MAX_BACKOFF_SECS);
                }
            }
        }
    })
}

/// 启动一个带 panic 捕获的后台任务：panic 后记录日志并退出（不重启）。
///
/// 用于捕获变量不可 Clone（如 `EventLoop`、`TcpListener`、`mpsc::Receiver`）
/// 而无法用 `spawn_supervised` 的场景。panic 后优雅退出比静默死亡好。
pub fn spawn_catch_unwind<Fut>(name: impl Into<String>, fut: Fut) -> tokio::task::JoinHandle<()>
where
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let name = name.into();
    tokio::spawn(async move {
        let res = std::panic::AssertUnwindSafe(fut).catch_unwind().await;
        if let Err(panic_info) = res {
            let msg = if let Some(s) = panic_info.downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = panic_info.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic".to_string()
            };
            tracing::error!("task {} panicked: {}", name, msg);
            metrics().inc_task_panic(&name);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{
        is_healthy_uptime, jittered_backoff_secs, spawn_catch_unwind, spawn_supervised,
        BACKOFF_RESET_AFTER, MAX_BACKOFF_SECS,
    };
    use std::time::Duration;

    /// r64-SD1 契约：`spawn_catch_unwind` 返回的 JoinHandle 必须可被 await，
    /// 且无论 future 正常完成还是 panic（被 catch_unwind 吸收）都会 resolve。
    ///
    /// graceful_shutdown 依赖此契约在 `stop()` 之后 await 北向 run 循环退出——
    /// 若句柄在 panic 路径下不 resolve（例如未来改动让 panic 逃逸成 JoinError
    /// 之外的挂起），停机序会在 timeout 前白等 5s。此处锁死该行为。
    #[tokio::test]
    async fn r64_spawn_catch_unwind_handle_resolves_on_completion_and_panic() {
        // 正常完成 → 句柄立即 resolve
        let h = spawn_catch_unwind("r64_normal", async {});
        assert!(
            tokio::time::timeout(Duration::from_secs(2), h)
                .await
                .is_ok(),
            "r64: future 正常完成时句柄必须 resolve"
        );

        // panic → catch_unwind 吸收，句柄仍 resolve（Ok(())，非 JoinError）
        let h2 = spawn_catch_unwind("r64_panic", async {
            panic!("r64 injected panic");
        });
        let res = tokio::time::timeout(Duration::from_secs(2), h2).await;
        assert!(
            res.is_ok(),
            "r64: panic 被 catch_unwind 吸收后句柄仍必须 resolve，否则停机序会白等"
        );
        assert!(
            res.unwrap().is_ok(),
            "r64: panic 经 catch_unwind 后任务应正常结束（Ok），不应是 JoinError"
        );
    }

    /// S-2 契约：退避重置阈值须在合理区间（太短易受抖动影响，太长等同不重置）
    #[test]
    fn backoff_reset_after_in_reasonable_range() {
        assert!(
            BACKOFF_RESET_AFTER >= Duration::from_secs(5)
                && BACKOFF_RESET_AFTER <= Duration::from_secs(300),
            "BACKOFF_RESET_AFTER out of range: {:?}",
            BACKOFF_RESET_AFTER
        );
    }

    /// S-2 契约：退避上限有界，避免重启延迟无限放大
    // 契约守门：断言对象就是常量取值范围本身，常量断言属预期，不简化
    #[allow(clippy::assertions_on_constants)]
    #[test]
    fn max_backoff_is_bounded() {
        assert!(
            MAX_BACKOFF_SECS >= 10 && MAX_BACKOFF_SECS <= 60,
            "MAX_BACKOFF_SECS out of range: {}",
            MAX_BACKOFF_SECS
        );
    }

    /// S-2 契约：稳定存活 ≥ 阈值 → 视为新故障（应重置退避）；否则不重置
    #[test]
    fn healthy_uptime_predicate() {
        assert!(is_healthy_uptime(Duration::from_secs(120)));
        assert!(is_healthy_uptime(BACKOFF_RESET_AFTER));
        assert!(!is_healthy_uptime(Duration::from_secs(1)));
        assert!(!is_healthy_uptime(
            BACKOFF_RESET_AFTER - Duration::from_secs(1)
        ));
    }

    /// 指数升级封顶验证（模拟多次短命崩溃后的退避序列）
    #[test]
    fn backoff_escalation_caps_at_max() {
        let mut b: u64 = 1;
        for _ in 0..12 {
            b = b.saturating_mul(2).min(MAX_BACKOFF_SECS);
        }
        assert_eq!(b, MAX_BACKOFF_SECS);
    }

    /// P2 契约：`spawn_supervised` 必须能从 panic 自动重启。
    ///
    /// 这是 gatewayd 中「Web 控制面启动失败即 panic → 由监管循环按退避重启」的前提
    /// （见 gatewayd/src/bootstrap/helpers/assemble.rs 的 `start_web_server`）。
    /// 若监管循环误把 panic 也当作正常结束退出，控制面就会永久失效。
    #[tokio::test]
    async fn spawn_supervised_restarts_after_panic() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc as StdArc;

        let attempts = StdArc::new(AtomicU32::new(0));
        let counter = attempts.clone();
        let handle = spawn_supervised("test_panic_restart", move || {
            let counter = counter.clone();
            async move {
                // 首次必定 panic，其后正常结束
                if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                    panic!("injected first-run failure");
                }
            }
        });

        // 首次 panic 后按退避 1s 重启，最多轮询 3s
        for _ in 0..60 {
            if attempts.load(Ordering::SeqCst) >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        handle.abort();
        assert!(
            attempts.load(Ordering::SeqCst) >= 2,
            "panic 后未重启：factory 仅被调用 {} 次",
            attempts.load(Ordering::SeqCst)
        );
    }

    /// r34-SV2 (回归)：jittered_backoff_secs 输出落在 [base*0.75, base*1.25] 区间内。
    /// base=0 短路 → 0；base=8 落在 [6, 10]；base=4 落在 [3, 5]。
    #[test]
    fn jittered_backoff_within_25_percent_range() {
        // base=0 → 0（disable 退避）
        assert_eq!(jittered_backoff_secs(0), 0);
        // base=8 区间 [6, 10]（5+5+0+1=11 mod 区间）
        for _ in 0..20 {
            let v = jittered_backoff_secs(8);
            assert!((6..=10).contains(&v), "jitter(8)={} 应在 [6, 10] 区间", v);
        }
        // base=4 区间 [3, 5]
        for _ in 0..20 {
            let v = jittered_backoff_secs(4);
            assert!((3..=5).contains(&v), "jitter(4)={} 应在 [3, 5]", v);
        }
        // base=1 区间 [0..=1]（0..=1 即 floor/ceil）
        for _ in 0..20 {
            let v = jittered_backoff_secs(1);
            assert!((0..=1).contains(&v), "jitter(1)={} 应在 [0, 1]", v);
        }
    }
}

/// S07: 启动一个可重启的后台任务（针对「future 自身可重建」的场景）。
///
/// 与 `spawn_supervised` 的差异：
/// - `spawn_supervised` 接受 `Fn() -> Fut`（闭包本身被多次调用），要求 future 内部状态
///   可 Clone（如 `Arc<Metrics>` + 简单 channel）。`bus_task` / `mqtt_event_loop` / OPC UA
///   session 这类持有不可 Clone 资源（`mpsc::Receiver`、`EventLoop`、`TcpStream`）的 task
///   改用 `spawn_supervised` 需要把内部状态改成 `Arc<Mutex<...>>`，改动面大、易引入新 bug。
/// - `spawn_with_retry` 接受 `FnMut() -> Fut` + 业务侧的"重启前 sleep"策略——业务代码
///   自己负责"重建"future（重新 connect / 重新 subscribe / 重新构造 receiver），helper 只
///   负责"future 完成后按策略再调一次 factory"。
///
/// 适用场景举例：
/// - 一个连接型 task：future 完成后短暂 sleep → 重新发起 connect → 新 future 入参
/// - 一个批处理 worker：future 完成后短暂 sleep → 重新订阅消息源 → 新 future
///
/// 不适用：future 内部持有不可重建的资源（如 EventLoop）——这种情况继续用
/// `spawn_catch_unwind`，panic 后让运维感知 + 重启 gatewayd。
///
/// 与 panic 的关系：本函数**不**捕获 panic（不强制 `catch_unwind`）。需要 panic 重启
/// 仍请用 `spawn_supervised`（业务侧闭包可 Clone 的情况）或 `spawn_catch_unwind` +
/// 外层 `loop`（业务侧 future 不可 Clone 但可重建的情况）。
///
/// 退避策略由调用方控制（`backoff_ms` 为初始 backoff 毫秒值），helper 在 future 完成
/// 后 sleep 该毫秒再重启；持续运行 `n` 毫秒以上时退避重置为初始值（防止"长时间稳定
/// 后偶发完成"继承历史大 backoff）。
pub fn spawn_with_retry<F, Fut>(
    name: impl Into<String>,
    initial_backoff_ms: u64,
    max_backoff_ms: u64,
    reset_after_ms: u64,
    mut make_fut: F,
) -> tokio::task::JoinHandle<()>
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let name = name.into();
    tokio::spawn(async move {
        let mut backoff_ms = initial_backoff_ms;
        loop {
            let run_started = Instant::now();
            make_fut().await;
            let ran_for_ms = run_started.elapsed().as_millis() as u64;
            if ran_for_ms >= reset_after_ms {
                backoff_ms = initial_backoff_ms;
            }
            tracing::info!(
                "retryable task {} exited, restarting in {}ms (ran_for={}ms)",
                name,
                backoff_ms,
                ran_for_ms
            );
            // r34-SV4: 退避加 jitter（毫秒版，与 jittered_backoff_secs 一致）
            let sleep_ms = if backoff_ms == 0 {
                0
            } else {
                let lo = backoff_ms.saturating_mul(3) / 4;
                let hi = backoff_ms.saturating_sub(lo);
                let jitter = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.subsec_nanos() as u64)
                    .unwrap_or(0)
                    % (hi + 1);
                lo + jitter
            };
            tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
            backoff_ms = backoff_ms.saturating_mul(2).min(max_backoff_ms);
        }
    })
}

#[cfg(test)]
mod s07_tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    /// S07: factory 在 future 完成后被反复调用——验证 retry 行为
    #[tokio::test]
    async fn spawn_with_retry_invokes_factory_again_after_completion() {
        let counter = Arc::new(AtomicU32::new(0));
        let counter_for_task = counter.clone();
        let handle = spawn_with_retry(
            "test_retry",
            10,   // 10ms initial backoff（测试加速）
            100,  // 100ms cap
            1000, // 1s reset threshold
            move || {
                let counter = counter_for_task.clone();
                async move {
                    counter.fetch_add(1, Ordering::Relaxed);
                    // future 立即结束 → 触发 retry
                }
            },
        );
        // 等 100ms：至少调用 3 次（10ms + 20ms + 40ms + ... backoff 序列）
        tokio::time::sleep(Duration::from_millis(120)).await;
        handle.abort();
        let n = counter.load(Ordering::Relaxed);
        assert!(n >= 2, "expected >= 2 invocations, got {}", n);
    }

    /// S07: 长期稳定运行后 future 退出 → 退避应重置为初始值
    #[test]
    fn backoff_reset_predicate_matches_supervision_module() {
        // 与 BACKOFF_RESET_AFTER 一致：稳定 >= 阈值 → reset
        let reset_threshold = Duration::from_secs(60);
        assert!(is_healthy_uptime(reset_threshold));
        assert!(is_healthy_uptime(Duration::from_secs(120)));
        assert!(!is_healthy_uptime(Duration::from_millis(500)));
    }

    /// S07: 退避序列封顶——多次倍增后停在 max_backoff_ms
    #[test]
    fn backoff_escalation_caps_at_max_for_retry_helper() {
        let mut b: u64 = 50;
        let max = 1000u64;
        for _ in 0..10 {
            b = b.saturating_mul(2).min(max);
        }
        assert_eq!(b, max);
    }
}
