//! 4G 蜂窝网络管理模块
//!
//! 工业网关 4G 传输功能，支持：
//! - AT 指令控制（兼容移远 EC20/EC25、芯讯通 SIM7600 等主流模块）
//! - PPP 拨号上网（通过 pppd）
//! - 状态监控（信号强度 CSQ、运营商 COPS、IP 地址、连接状态）
//! - 链路冗余（以太网为主、4G 为备，自动切换）
//! - 断线重连（4G 掉线自动重拨）
//!
//! Bug 129: AT 指令 read_line 已用 tokio::time::timeout 包裹（防 AT 模块 hang）。
//! PPP/拨号细粒度超时（CellularConfig 的 `*_timeout_secs` 系列）：
//! - AT 指令/网络注册/拨号命令/PPP 建链/停 pppd 各阶段独立超时，均以 dial_timeout_secs 为总上限；
//! - pppd 以 tokio::process 管理，建链等待期间检测子进程提前退出并立即失败；
//! - stop_pppd 为异步非阻塞，Drop 走同步兜底 stop_pppd_force。

mod at_commands;
mod modem;

use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::broadcast;

pub use crate::config::CellularConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum CellularState {
    Init,
    NotReady,
    SimNotReady,
    Registered,
    Dialing,
    Connected,
    DialFailed,
}

#[derive(Debug, Clone, Serialize)]
pub struct CellularStatus {
    pub state: CellularState,
    pub signal_strength: u8,
    pub operator: String,
    pub ip_address: Option<String>,
    pub network_type: String,
    pub updated_at: u64,
}

impl Default for CellularStatus {
    fn default() -> Self {
        Self {
            state: CellularState::Init,
            signal_strength: 99,
            operator: String::new(),
            ip_address: None,
            network_type: String::new(),
            updated_at: now_millis(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum LinkState {
    EthernetUp,
    CellularUp,
    BothUp,
    Down,
}

pub struct CellularManager {
    config: CellularConfig,
    status: Mutex<CellularStatus>,
    state_tx: broadcast::Sender<CellularState>,
    link_state: Mutex<LinkState>,
    /// pppd 子进程句柄（tokio::process），用于建链存活检测与优雅停止。
    pppd_child: Mutex<Option<tokio::process::Child>>,
    /// A6 修复：取消令牌，让 stop() 真正能停掉 dial_loop / monitor_loop / failover_loop，
    /// 避免进程重启时 task 仍在飞造成下次 spawn 时 pppd 重叠等竞态。
    ///
    /// 用 Mutex 包裹以支持 [`CellularManager::restart`]：stop() 会取消当前令牌，
    /// 自愈 ResetNetwork 需要 stop→start 复用同一实例，必须先换上全新令牌。
    cancel_token: Mutex<tokio_util::sync::CancellationToken>,
    /// A6 修复：持有的后台 task handle，用于 stop 时主动 abort 兜底
    /// （cancel_token 未及时响应时的最后防线）。
    task_handles: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl CellularManager {
    pub fn new(config: CellularConfig) -> Arc<Self> {
        let (state_tx, _) = broadcast::channel(16);
        Arc::new(Self {
            config,
            status: Mutex::new(CellularStatus::default()),
            state_tx,
            link_state: Mutex::new(LinkState::Down),
            pppd_child: Mutex::new(None),
            // A6 修复：独立的 cancel token，让每个 CellularManager 实例的 task
            // 都能被定向取消（避免与全局 runtime shutdown 冲突）
            cancel_token: Mutex::new(tokio_util::sync::CancellationToken::new()),
            task_handles: Mutex::new(Vec::new()),
        })
    }

    pub fn status(&self) -> CellularStatus {
        self.status.lock().clone()
    }
    pub fn link_state(&self) -> LinkState {
        *self.link_state.lock()
    }
    pub fn subscribe_state(&self) -> broadcast::Receiver<CellularState> {
        self.state_tx.subscribe()
    }

    pub fn start(self: Arc<Self>) {
        if !self.config.enabled {
            tracing::info!("cellular module disabled");
            return;
        }
        // A5 修复：Windows / 非 Linux 系统没有 pppd 可执行，spawn 会立刻 ENOENT；
        // 原代码在此 panic（spawn 之前 std::process::Command 不会 panic，但后续
        // 调 modem.rs::start_pppd 在 Windows 上找不到 pppd.exe 会 runtime panic）。
        // 把 dial_loop 整体门控 cfg!(target_os = "linux")，非 Linux 保留 monitor
        // (link 状态上报) 与 failover (上层选路) 能力，但不下发拨号动作。
        let cancel = self.cancel_token.lock().clone();
        let mut handles = self.task_handles.lock();

        if cfg!(target_os = "linux") {
            let mgr = self.clone();
            let cancel_dial = cancel.clone();
            handles.push(crate::engine::supervision::spawn_supervised(
                "cellular_dial",
                move || {
                    let mgr = mgr.clone();
                    let cancel = cancel_dial.clone();
                    async move {
                        tokio::select! {
                            _ = cancel.cancelled() => {
                                tracing::info!("cellular_dial cancelled, exiting");
                            }
                            _ = mgr.dial_loop() => {}
                        }
                    }
                },
            ));
        } else {
            tracing::info!(
                "cellular: 当前平台不支持 pppd ({}), dial_loop 未启动；monitor/failover 仍运行",
                std::env::consts::OS
            );
        }

        // monitor / failover 跨平台
        let mgr = self.clone();
        let cancel_mon = cancel.clone();
        handles.push(crate::engine::supervision::spawn_supervised(
            "cellular_monitor",
            move || {
                let mgr = mgr.clone();
                let cancel = cancel_mon.clone();
                async move {
                    tokio::select! {
                        _ = cancel.cancelled() => {
                            tracing::info!("cellular_monitor cancelled, exiting");
                        }
                        _ = mgr.monitor_loop() => {}
                    }
                }
            },
        ));

        if self.config.failover_enabled {
            let mgr = self.clone();
            let cancel_fo = cancel.clone();
            handles.push(crate::engine::supervision::spawn_supervised(
                "cellular_failover",
                move || {
                    let mgr = mgr.clone();
                    let cancel = cancel_fo.clone();
                    async move {
                        tokio::select! {
                            _ = cancel.cancelled() => {
                                tracing::info!("cellular_failover cancelled, exiting");
                            }
                            _ = mgr.failover_loop() => {}
                        }
                    }
                },
            ));
        }
        tracing::info!(
            "cellular manager started (apn={}, platform={})",
            self.config.apn,
            std::env::consts::OS
        );
    }

    /// 优雅停机：触发 cancel + abort 后台 task + 停 pppd。
    ///
    /// A6 修复：原实现是 no-op，dial_loop 持续 spawn pppd，重启时上一次 4G
    /// 链路仍在后台挂载，导致下一次启动时 ip route/addr 状态混乱。
    /// 现在改为：
    ///   1. cancel_token.cancel() 通知所有 select! 退出
    ///   2. 给每个 task 500ms 自愿退出时间
    ///   3. 超时未退出的 task 强制 abort
    ///   4. stop_pppd() 兜底杀残留 pppd 进程
    ///   5. state 转 NotReady 让上层选路感知
    pub async fn stop(&self) {
        tracing::info!("cellular manager stopping (cancelling tasks)");
        self.cancel_token.lock().cancel();
        // 给在飞 task 500ms 自愿退出时间
        let graceful_ms = 500u64;
        let handles: Vec<tokio::task::JoinHandle<()>> = {
            let mut g = self.task_handles.lock();
            std::mem::take(&mut *g)
        };
        for mut h in handles {
            match tokio::time::timeout(std::time::Duration::from_millis(graceful_ms), &mut h).await
            {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::warn!("cellular task join error: {}", e),
                Err(_) => {
                    tracing::warn!(
                        "cellular task did not exit within {}ms, aborting",
                        graceful_ms
                    );
                    h.abort();
                }
            }
        }
        // 兜底停 pppd
        self.stop_pppd().await;
        // 状态转 NotReady，让上层 failover / 上报感知
        self.set_state(crate::platform::cellular::CellularState::NotReady);
        tracing::info!("cellular manager stopped");
    }

    /// 重启蜂窝模块（自愈动作 ResetNetwork）。
    ///
    /// stop() 会取消当前取消令牌并停掉在飞 task / pppd；随后必须换上一枚**全新**令牌
    /// 再 start()，否则 start 派生的 task 一进入 `select!` 就会看到已取消令牌而立即退出，
    /// 等于"重启无效"。stop → 换新令牌 → start 保证拨号 / 监控 / 选路真正重新拉起。
    pub async fn restart(self: Arc<Self>) {
        tracing::info!("cellular manager restarting (self-heal ResetNetwork)");
        self.stop().await;
        *self.cancel_token.lock() = tokio_util::sync::CancellationToken::new();
        self.clone().start();
    }

    fn set_state(&self, state: CellularState) {
        {
            let mut s = self.status.lock();
            s.state = state;
            s.updated_at = now_millis();
        }
        let _ = self.state_tx.send(state);
    }
}

impl Drop for CellularManager {
    fn drop(&mut self) {
        // Drop 无法 await，走同步兜底（直接 SIGKILL + 清理接口）
        self.stop_pppd_force();
    }
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
