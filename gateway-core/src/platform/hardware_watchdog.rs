//! 硬件看门狗 + 进程级 abort 自恢复
//!
//! 与 `engine::monitor::heartbeat` 的区别：
//! - 本模块在超时后直接 `std::process::abort()`，由 systemd 拉起重启
//! - `engine::monitor::heartbeat` 只暴露 metrics + 日志，不 abort
//!
//! 软件看门狗：主循环定期调用 `kick()`，若超过 `timeout` 未 kick，
//! 监控线程会主动 abort 进程，由 systemd 拉起重启。
//!
//! 硬件看门狗（Linux）：打开 `/dev/watchdog`，定期写入心跳。
//! 进程卡死无法写入时，内核超时复位整机。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
/// S2: 看门狗 abort 前的优雅 flush 回调（如 flush SQLite/MQTT）
pub static FLUSH_CALLBACK: once_cell::sync::OnceCell<Arc<dyn Fn() + Send + Sync>> =
    once_cell::sync::OnceCell::new();

/// 注册 abort 前的 flush 回调（main 中调用）
pub fn set_flush_callback<F: Fn() + Send + Sync + 'static>(f: F) {
    let _ = FLUSH_CALLBACK.set(Arc::new(f));
}

/// 可喂狗的内部状态（Arc 共享）
struct WatchdogInner {
    last_kick: AtomicU64,
    running: AtomicBool,
    hw_fd: Mutex<Option<std::fs::File>>,
}

/// 看门狗（Clone 安全：仅共享原子状态）
#[derive(Clone)]
pub struct Watchdog {
    inner: Arc<WatchdogInner>,
    handle: Arc<std::sync::Mutex<Option<std::thread::JoinHandle<()>>>>,
}

impl Watchdog {
    /// 创建并启动看门狗
    pub fn start(timeout: Duration, hw_device: Option<&str>) -> Self {
        let last_kick = AtomicU64::new(mono_millis());
        let running = AtomicBool::new(true);

        // 硬件看门狗
        let hw_fd = if let Some(dev) = hw_device {
            #[cfg(target_os = "linux")]
            {
                match std::fs::OpenOptions::new().write(true).open(dev) {
                    Ok(mut f) => {
                        use std::io::Write;
                        tracing::info!("hardware watchdog opened: {}", dev);
                        let _ = f.write_all(b"\0");
                        Some(f)
                    }
                    Err(e) => {
                        tracing::warn!("hardware watchdog open failed ({}): {}", dev, e);
                        None
                    }
                }
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = dev;
                None
            }
        } else {
            None
        };

        let inner = Arc::new(WatchdogInner {
            last_kick,
            running,
            hw_fd: Mutex::new(hw_fd),
        });

        // 软件看门狗监控线程
        let inner_clone = inner.clone();
        let timeout_ms = timeout.as_millis() as u64;
        let handle = std::thread::Builder::new()
            .name("watchdog-sw".into())
            .spawn(move || {
                loop {
                    if !inner_clone.running.load(Ordering::Relaxed) {
                        break;
                    }
                    let elapsed =
                        mono_millis().saturating_sub(inner_clone.last_kick.load(Ordering::Relaxed));
                    if elapsed > timeout_ms {
                        tracing::error!(
                            "watchdog timeout: no kick for {}ms (>{})",
                            elapsed,
                            timeout_ms
                        );
                        // S2: abort 前先执行 flush 回调，尽量减少数据丢失
                        if let Some(cb) = FLUSH_CALLBACK.get() {
                            tracing::warn!("running flush callback before abort (3s timeout)...");
                            let cb_clone = cb.clone();
                            let (tx, rx) = std::sync::mpsc::channel();
                            let _handle = std::thread::spawn(move || {
                                cb_clone();
                                let _ = tx.send(());
                            });
                            match rx.recv_timeout(std::time::Duration::from_secs(3)) {
                                Ok(_) => tracing::info!("flush callback completed"),
                                Err(_) => tracing::error!(
                                    "flush callback timed out after 3s, forcing abort"
                                ),
                            }
                        }
                        std::process::abort();
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
            });

        let handle = match handle {
            Ok(h) => Some(h),
            Err(e) => {
                tracing::error!(
                    "failed to spawn watchdog thread: {} (software watchdog disabled)",
                    e
                );
                None
            }
        };

        let wd = Self {
            inner,
            handle: Arc::new(std::sync::Mutex::new(handle)),
        };
        // 立即喂狗一次：确保硬件看门狗在返回前已有心跳，
        // 避免 start() 之后到主循环 kick 之前若发生 panic，
        // 硬件看门狗因超时未收到心跳而硬复位整机。
        wd.kick();
        wd
    }

    /// 喂狗（通知看门狗进程仍存活）
    ///
    /// Bug r15-WD1 (fix): 写硬件 watchdog 后必须 flush——
    /// 否则数据留在进程 stdio 缓冲中，若进程被 kill 丢失末尾心跳，
    /// 硬件 watchdog 收不到 → 整机误复位。
    pub fn kick(&self) {
        self.inner.last_kick.store(mono_millis(), Ordering::Relaxed);
        if let Ok(mut guard) = self.inner.hw_fd.lock() {
            if let Some(f) = guard.as_mut() {
                use std::io::Write;
                let _ = f.write_all(b"\0");
                let _ = f.flush();
            }
        }
    }

    /// 停止看门狗（进程正常退出时调用，避免误触发）
    pub fn stop(self) {
        self.inner.running.store(false, Ordering::Relaxed);
        if let Ok(mut h) = self.handle.lock() {
            if let Some(handle) = h.take() {
                let _ = handle.join();
            }
        }
        // 关闭硬件看门狗（发送魔法字符 V 停止）
        //
        // Bug r15-WD2 (fix): 写 magic char 后必须 flush——
        // 否则进程退出时 magic char 留在 buffer，硬件 watchdog 收不到，
        // 下次启动时 watchdog 已 timeout → 整机误复位。
        #[cfg(target_os = "linux")]
        {
            if let Ok(mut guard) = self.inner.hw_fd.lock() {
                if let Some(mut f) = guard.take() {
                    use std::io::Write;
                    let _ = f.write_all(b"V");
                    let _ = f.flush();
                }
            }
        }
    }
}

/// 单调时钟毫秒（自进程启动起算）。
/// Bug C10: 原实现用 SystemTime（挂钟时间），NTP/手动校时「前跳」会让 elapsed 变成极大值，
/// 从而触发 std::process::abort() → 被 systemd 重启 → 再次校时 → 启动重启循环。
/// 改用 Instant 单调时钟，不受挂钟调整影响。
fn mono_millis() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis() as u64
}
