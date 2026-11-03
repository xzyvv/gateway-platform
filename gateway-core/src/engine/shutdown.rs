//! 优雅退出信号处理

/// 等待退出信号（SIGTERM / SIGINT / Ctrl+C）
///
/// 信号注册失败时降级为监听剩余可用信号，全部失败则阻塞等待外部 kill。
///
/// **r31-SH1 (修复)**：旧版本仅 `tracing::info!` 后返回。实际工程中，进程收到信号
/// 后需要通知各子任务（sink task / metrics task 等）退出——而这些任务监听的是
/// `ShutdownToken`，不是 tokio 信号。如果 main 流程是
/// `wait_for_shutdown().await; app.run();` 模式，信号到达后 app 仍继续运行，
/// 各 task 永远收不到 shutdown 信号。
///
/// 修复：传入 `&ShutdownToken`，收到信号时 `token.shutdown()` 触发全局关停链。
/// 调用方不传则保持旧行为（仅日志），向后兼容。
pub async fn wait_for_shutdown() {
    wait_for_shutdown_with(None).await
}

/// r31-SH1: 接受可选 `ShutdownToken` 的关停等待。
/// 收到 SIGTERM/SIGINT/Ctrl+C 时记录日志并触发 token（若提供）。
pub async fn wait_for_shutdown_with(token: Option<&ShutdownToken>) {
    let trigger_token = || {
        if let Some(t) = token {
            t.shutdown();
        }
    };
    #[cfg(unix)]
    {
        let sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
        let sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt());
        match (sigterm, sigint) {
            (Ok(mut s), Ok(mut i)) => {
                tokio::select! {
                    _ = s.recv() => tracing::info!(signal = "SIGTERM", "received shutdown signal"),
                    _ = i.recv() => tracing::info!(signal = "SIGINT", "received shutdown signal"),
                }
                trigger_token();
            }
            (Ok(mut s), Err(e)) => {
                tracing::error!(
                    "SIGINT registration failed: {}, waiting for SIGTERM only",
                    e
                );
                let _ = s.recv().await;
                tracing::info!(signal = "SIGTERM", "received shutdown signal");
                trigger_token();
            }
            (Err(e), Ok(mut i)) => {
                tracing::error!(
                    "SIGTERM registration failed: {}, waiting for SIGINT only",
                    e
                );
                let _ = i.recv().await;
                tracing::info!(signal = "SIGINT", "received shutdown signal");
                trigger_token();
            }
            (Err(e1), Err(e2)) => {
                tracing::error!(
                    "signal registration failed: SIGTERM={}, SIGINT={}, fallback to Ctrl+C",
                    e1,
                    e2
                );
                let _ = tokio::signal::ctrl_c().await;
                tracing::info!(signal = "SIGINT-CtrlC", "received shutdown signal");
                trigger_token();
            }
        }
    }
    #[cfg(not(unix))]
    {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::error!(
                "Ctrl+C listen failed: {}, blocking forever (use kill to terminate)",
                e
            );
            std::future::pending::<()>().await;
        }
        tracing::info!(signal = "CtrlC", "received shutdown signal");
        trigger_token();
    }
}

use std::sync::Arc;
use tokio::sync::Notify;

/// 进程级关闭信号（轻量包装 tokio::sync::Notify）
///
/// 用于子任务（sink task / metrics task 等）监听退出信号。
/// 调用 `shutdown()` 一次，所有 `wait()` future 立即完成。
/// Clone 安全：每个 sink task 持有一份 clone，独立监听。
#[derive(Clone, Default)]
pub struct ShutdownToken {
    notify: Arc<Notify>,
    triggered: Arc<std::sync::atomic::AtomicBool>,
}

impl ShutdownToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// r31-SH4: 同步查询是否已触发关闭。
    /// 调用方模式："先检查再 wait"（如热路径中跳过 wait 调用以减少 future 调度开销），
    /// 或日志/指标中标识关停状态。
    pub fn is_triggered(&self) -> bool {
        self.triggered.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 触发关闭信号
    pub fn shutdown(&self) {
        self.triggered
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// 等待关闭信号
    pub async fn wait(&self) {
        if self.triggered.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        // Bug 159: 旧实现"先查标志 → notified().await"存在竞态——若 shutdown() 在本
        // 任务读到标志(false)之后、waiter 注册之前触发，`notify_waiters()` 既不存储
        // 许可也不唤醒尚未注册的 waiter，本任务将永久挂起（停机 drain 偶发卡死）。
        // 修复：显式 pin + `enable()` 立即注册 waiter，再复查标志，覆盖该时间窗口。
        let notified = self.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.triggered.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        notified.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// r31-SH4 (回归)：is_triggered 同步反映 shutdown() 调用结果。
    #[test]
    fn is_triggered_reflects_shutdown_state() {
        let token = ShutdownToken::new();
        assert!(!token.is_triggered(), "初始状态未触发");
        token.shutdown();
        assert!(token.is_triggered(), "shutdown 后应触发");
    }

    /// r31-SH4 补充：is_triggered 多次调用稳定（无副作用）。
    #[test]
    fn is_triggered_is_idempotent() {
        let token = ShutdownToken::new();
        token.shutdown();
        for _ in 0..10 {
            assert!(token.is_triggered());
        }
    }

    /// r31-SH1 (回归)：wait_for_shutdown_with 在传入 token 时，
    /// 收到 ctrl_c 后会触发 token。但单元测试无法模拟 tokio signal，
    /// 我们直接验证 token.shutdown() 后 wait() 立即返回——证明 token 联动
    /// 机制（wait 路径）工作正常。信号分支本身是 tokio::signal 库代码，
    /// 不在测试范围。
    #[tokio::test]
    async fn wait_returns_immediately_after_shutdown() {
        let token = ShutdownToken::new();
        let token_clone = token.clone();
        // 后台任务 wait，然后主线程 50ms 后 shutdown
        let waiter = tokio::spawn(async move {
            token_clone.wait().await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        token.shutdown();
        // 等待 waiter 完成（应立即完成）
        let res = tokio::time::timeout(std::time::Duration::from_secs(1), waiter).await;
        assert!(res.is_ok(), "waiter 必须在 shutdown 后 1s 内完成");
        assert!(token.is_triggered());
    }
}
