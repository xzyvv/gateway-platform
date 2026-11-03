//! NTP 同步（§11 启动流程第 3 步）
//!
//! 基于 rsntp 纯 Rust NTP 客户端，失败时用 RTC 后备。

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::Notify;

use crate::config::NtpConfig;
use crate::engine::error::{GatewayError, GatewayResult};

/// NTP 同步状态
#[derive(Debug, Clone, Serialize)]
pub struct NtpStatus {
    pub last_sync: Option<i64>,
    pub status: String,
    pub source: String,
    /// NTP 时间与本地系统时间差（毫秒，NTP - local）
    /// 正值=本地时钟落后，负值=本地时钟超前
    /// 用于判定漂移是否过大（如 |offset_ms| > 500ms 视为漂移过大）
    #[serde(default)]
    pub offset_ms: i64,
}

/// NTP 管理器
pub struct NtpManager {
    /// 配置（热更新：支持在线修改 NTP 服务器和同步间隔）
    config: parking_lot::Mutex<NtpConfig>,
    status: Mutex<NtpStatus>,
    shutdown: Arc<Notify>,
}

impl NtpManager {
    pub fn new(config: NtpConfig) -> Self {
        Self {
            config: parking_lot::Mutex::new(config),
            status: Mutex::new(NtpStatus {
                last_sync: None,
                status: "idle".to_string(),
                source: "RTC".to_string(),
                offset_ms: 0,
            }),
            shutdown: Arc::new(Notify::new()),
        }
    }

    /// 立即同步一次
    pub async fn sync_now(&self) -> GatewayResult<()> {
        let server = self.config.lock().server.clone();
        let result = sync_ntp(&server).await;
        let mut status = self.status.lock();
        match result {
            Ok(ts) => {
                // 计算本地系统时钟与 NTP 时间的偏移（毫秒）
                // offset = NTP_ts - local_ts；正值=本地落后，负值=本地超前
                let local_ms = chrono::Utc::now().timestamp_millis();
                let offset = ts - local_ms;
                status.last_sync = Some(ts);
                status.offset_ms = offset;
                status.status = "synced".to_string();
                status.source = "NTP".to_string();
                tracing::info!(
                    "ntp synced: {}, offset_ms={} ({}ms {})",
                    ts,
                    offset,
                    offset.abs(),
                    if offset >= 0 { "behind" } else { "ahead" }
                );
                Ok(())
            }
            Err(e) => {
                status.status = format!("error: {}", e);
                status.source = "RTC".to_string();
                tracing::warn!(
                    "ntp sync failed: {}, falling back to RTC (system clock continues, drift unbounded)",
                    e
                );
                // M1-8 + M2-1: fallback 不返回 Err，避免启动流程中断
                // M2-1: 告警 metric（metrics 模块暂无 ntp_fallback 字段，仅日志）
                tracing::error!(
                    target: "ntp_fallback",
                    ntp_server = % self .config. lock ().server,
                    "NTP sync failed, falling back to RTC; clock drift unbounded"
                );
                Ok(())
            }
        }
    }

    /// 启动后台同步循环
    ///
    /// 每次循环重新读取 config.sync_interval，因此在线修改同步间隔可立即生效。
    pub fn start(self: &Arc<Self>) {
        let self_arc = self.clone();
        drop(crate::engine::supervision::spawn_supervised(
            "ntp_sync",
            move || {
                let self_arc = self_arc.clone();
                async move {
                    loop {
                        let interval = Duration::from_secs(self_arc.config.lock().sync_interval);
                        tokio::select! {
                            _ = self_arc.shutdown.notified() => break,
                            _ = tokio::time::sleep(interval) => {
                                let _ = self_arc.sync_now().await;
                            }
                        }
                    }
                }
            },
        ));
    }

    /// 获取状态
    pub fn status(&self) -> NtpStatus {
        self.status.lock().clone()
    }

    /// 在线更新 NTP 配置（服务器地址 / 同步间隔）
    ///
    /// 同步间隔在下一次循环迭代时生效；服务器地址在下一次 sync_now 时生效。
    pub fn update_config(&self, new_config: NtpConfig) {
        let old = self.config.lock().clone();
        tracing::info!(
            "ntp config updated: server {} -> {}, interval {} -> {}",
            old.server,
            new_config.server,
            old.sync_interval,
            new_config.sync_interval
        );
        *self.config.lock() = new_config;
    }

    /// 停止
    pub fn stop(&self) {
        // Bug T10: 原用 `notify_waiters()`——若无 waiter 正挂起（同步循环尚未跑到
        // `notified().await`），通知会被直接丢弃，导致 stop() 后循环永不退出（与
        // Bug 159 同类竞态）。`notify_one()` 会在无 waiter 时暂存一个 permit，
        // 下次 `notified().await` 立即完成，消除竞态。
        self.shutdown.notify_one();
    }
}

/// 同步 NTP，返回 UTC 毫秒时间戳
async fn sync_ntp(server: &str) -> GatewayResult<i64> {
    let server = server.to_string();
    let result = tokio::task::spawn_blocking(move || {
        // rsntp 3.0: 使用 SntpClient
        let client = rsntp::SntpClient::new();
        client.synchronize(&server)
    })
    .await
    .map_err(|e| GatewayError::Io(format!("ntp task join error: {}", e)))?
    .map_err(|e| GatewayError::Io(format!("ntp sync error: {}", e)))?;

    let datetime = result
        .datetime()
        .into_chrono_datetime()
        .map_err(|e| GatewayError::Io(format!("ntp returned no datetime: {}", e)))?;
    Ok(datetime.timestamp_millis())
}
