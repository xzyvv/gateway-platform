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
//!
//! # 设备句柄全局唯一（EBUSY 修复）
//!
//! `/dev/watchdog` 在绝大多数 watchdog 驱动下是**独占设备**：第一个
//! `open()` 成功后，第二个 `open()` 直接返回 `EBUSY`。
//! 历史上 OTA 写入路径（`ota/apply.rs` 的 `kick_platform_watchdog`）为了
//! "喂狗"会独立再 open 一次设备——在 Linux 上**必然拿不到 fd**，写失败还被
//! 静默吞掉，结果是：OTA 长写入期间硬件看门狗**实际未被喂**，写满超时后
//! 整机被 watchdog 复位（升级变砖）。
//!
//! 现在设备句柄只有一份（模块级全局），主循环与 OTA 复用同一个 fd，
//! 从根上消除 EBUSY。
//!
//! # 写失败必须可见
//!
//! 喂狗失败 = 硬件看门狗已失效（fd 失效 / 权限回收 / 驱动异常），
//! 此时进程卡死将**无法**被硬件复位——这是最难排查的现场事故类型。
//! 历史上 `kick()` / `stop()` / OTA 喂狗三处都把写错误整个吞掉，
//! 日志里没有任何线索。现在每次失败计数 + `error!` 日志。

use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// S2: 看门狗 abort 前的优雅 flush 回调（如 flush SQLite/MQTT）
pub static FLUSH_CALLBACK: once_cell::sync::OnceCell<Arc<dyn Fn() + Send + Sync>> =
    once_cell::sync::OnceCell::new();

/// 注册 abort 前的 flush 回调（main 中调用）
pub fn set_flush_callback<F: Fn() + Send + Sync + 'static>(f: F) {
    let _ = FLUSH_CALLBACK.set(Arc::new(f));
}

/// 进程级硬件看门狗设备句柄（全局唯一）。
///
/// 见模块文档「设备句柄全局唯一」：独占设备只能被 open 一次，
/// 所有喂狗路径（主循环 / OTA）必须共用这一个 fd。
static HW_FD: OnceLock<Mutex<Option<std::fs::File>>> = OnceLock::new();

/// 全局硬件看门狗设备槽（非 Linux 恒为 `None`）。
fn hw_fd_slot() -> &'static Mutex<Option<std::fs::File>> {
    HW_FD.get_or_init(|| Mutex::new(None))
}

/// 进程级看门狗内部状态（供 OTA 等其它路径读取错误计数）。
static ACTIVE_INNER: OnceLock<Arc<WatchdogInner>> = OnceLock::new();

/// 硬件看门狗写操作的结果分类（**纯函数，跨平台可测**）。
///
/// 抽成独立类型的原因：`/dev/watchdog` 只存在于 Linux 目标机，本机
/// （Windows）无法构造真实的写失败场景。若把判定逻辑内联在 `kick()` /
/// `stop()` 里，这些分支将**永久零覆盖**——而它们恰恰是现场事故的
/// 唯一线索来源。抽出后可在任何平台直接单测。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HwWriteOutcome {
    /// 写入成功（已 flush）。
    Ok,
    /// 写入失败：硬件看门狗已失效，需计入错误并告警。
    Failed,
    /// 没有硬件看门狗设备（非 Linux 或打开失败）：无需处理。
    NoDevice,
}

/// 对硬件看门狗设备执行一次写入并 flush，返回结果分类。
///
/// `bytes` 为写入内容：心跳用 `\0`，解除 armed 用 magic char `'V'`。
/// 写失败**不**向上返回错误——喂狗失败不能阻断主流程，但必须被记录。
fn hw_write(fd: Option<&mut std::fs::File>, bytes: &[u8]) -> HwWriteOutcome {
    match fd {
        None => HwWriteOutcome::NoDevice,
        Some(f) => match f.write_all(bytes).and_then(|_| f.flush()) {
            Ok(()) => HwWriteOutcome::Ok,
            Err(_) => HwWriteOutcome::Failed,
        },
    }
}

/// 可喂狗的内部状态（Arc 共享）
struct WatchdogInner {
    last_kick: AtomicU64,
    running: AtomicBool,
    /// 硬件看门狗写失败累计次数。
    ///
    /// 写狗失败 = 硬件看门狗已失效（fd 失效 / 权限回收 / 驱动异常），
    /// 此时进程卡死将**无法**被硬件复位——这是最难排查的现场事故类型，
    /// 此前 `kick()`/`stop()` 把写错误整个吞掉，日志里没有任何线索。
    /// 现在每次失败计数 + 明确 error 日志。
    hw_write_errors: AtomicU64,
    /// 软件看门狗监控线程句柄（`stop()` 时 join）。
    handle: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl WatchdogInner {
    /// 停止软件看门狗线程 + 解除硬件看门狗 armed 状态。
    ///
    /// 与 `Watchdog::stop` 同语义，但可由全局 `ACTIVE_INNER` 直接调用
    /// （优雅停机路径拿不到具体实例时使用）。
    fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
        if let Ok(mut h) = self.handle.lock() {
            if let Some(handle) = h.take() {
                let _ = handle.join();
            }
        }
        // 关闭硬件看门狗（发送魔法字符 V 停止）
        //
        // 写 magic char 后必须 flush——否则进程退出时 magic char 留在 buffer，
        // 硬件 watchdog 收不到，下次启动时 watchdog 已 timeout → 整机误复位。
        let outcome = {
            let mut guard = match hw_fd_slot().lock() {
                Ok(g) => g,
                Err(e) => e.into_inner(),
            };
            // take 出 fd：写入 'V' 后设备不再 armed，且后续 kick() 不会
            // 再次写入心跳把设备重新 arm 起来；句柄随之关闭。
            let mut fd = guard.take();
            hw_write(fd.as_mut(), b"V")
        };
        match outcome {
            HwWriteOutcome::Ok => tracing::info!("hardware watchdog disarmed (magic 'V' written)"),
            HwWriteOutcome::Failed => tracing::error!(
                "hardware watchdog disarm FAILED (magic 'V' not delivered): \
                 the device stays armed — the board may reset on the next power cycle"
            ),
            HwWriteOutcome::NoDevice => {
                tracing::debug!("no hardware watchdog device: disarm skipped")
            }
        }
    }

    /// 记一次硬件写失败（计数 + 告警）。
    fn record_hw_write_failure(&self, ctx: &str) {
        let n = self.hw_write_errors.fetch_add(1, Ordering::Relaxed) + 1;
        tracing::error!(
            "hardware watchdog write failed ({}), total failures = {} — \
             the hardware watchdog is NOT being fed; a hung process will NOT be reset by hardware",
            ctx,
            n
        );
    }
}

/// 看门狗（Clone 安全：仅共享原子状态）
#[derive(Clone)]
pub struct Watchdog {
    inner: Arc<WatchdogInner>,
}

impl Watchdog {
    /// 创建并启动看门狗
    pub fn start(timeout: Duration, hw_device: Option<&str>) -> Self {
        let inner = Arc::new(WatchdogInner {
            last_kick: AtomicU64::new(mono_millis()),
            running: AtomicBool::new(true),
            hw_write_errors: AtomicU64::new(0),
            handle: Mutex::new(None),
        });
        let _ = ACTIVE_INNER.set(inner.clone());

        // 硬件看门狗：独占设备，只能 open 一次，句柄存全局。
        let hw_fd = hw_device.and_then(|dev| {
            #[cfg(target_os = "linux")]
            {
                match std::fs::OpenOptions::new().write(true).open(dev) {
                    Ok(mut f) => {
                        tracing::info!("hardware watchdog opened: {}", dev);
                        // 初次心跳：写失败必须留痕（写不进 = 设备已失效）。
                        if hw_write(Some(&mut f), b"\0") == HwWriteOutcome::Failed {
                            inner.record_hw_write_failure("initial heartbeat");
                        }
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
        });
        {
            let mut guard = match hw_fd_slot().lock() {
                Ok(g) => g,
                Err(e) => e.into_inner(),
            };
            *guard = hw_fd;
        }

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
                                Err(_) => {
                                    tracing::error!(
                                        "flush callback timed out after 3s, forcing abort"
                                    )
                                }
                            }
                        }
                        std::process::abort();
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
            });

        *inner.handle.lock().unwrap_or_else(|e| e.into_inner()) = handle.ok();

        let wd = Self { inner };
        // 立即喂狗一次：确保硬件看门狗在返回前已有心跳，
        // 避免 start() 之后到主循环 kick 之前若发生 panic，
        // 硬件看门狗因超时未收到心跳而硬复位整机。
        wd.kick();
        wd
    }

    /// 喂狗（通知看门狗进程仍存活）
    ///
    /// 写硬件 watchdog 后必须 flush——否则数据留在进程 stdio 缓冲中，
    /// 若进程被 kill 丢失末尾心跳，硬件 watchdog 收不到 → 整机误复位。
    pub fn kick(&self) {
        self.inner.last_kick.store(mono_millis(), Ordering::Relaxed);
        let outcome = {
            let mut guard = match hw_fd_slot().lock() {
                Ok(g) => g,
                Err(_) => {
                    tracing::error!("hardware watchdog fd slot poisoned: kick skipped");
                    return;
                }
            };
            hw_write(guard.as_mut(), b"\0")
        };
        if outcome == HwWriteOutcome::Failed {
            self.inner.record_hw_write_failure("kick");
        }
    }

    /// 软件看门狗是否仍在运行（喂狗循环据此退出）。
    pub fn is_running(&self) -> bool {
        self.inner.running.load(Ordering::Relaxed)
    }

    /// 硬件看门狗写失败累计次数（诊断用）。
    pub fn hw_write_errors(&self) -> u64 {
        self.inner.hw_write_errors.load(Ordering::Relaxed)
    }

    /// 停止看门狗（进程正常退出时调用，避免误触发）
    pub fn stop(&self) {
        self.inner.stop();
    }
}

/// 停止进程级看门狗（优雅停机路径调用）。
///
/// 供 `gatewayd` 的优雅停机流程调用：正常退出前必须解除硬件看门狗的
/// armed 状态，否则下次上电 / 超时后整机会被误复位。
///
/// 历史问题：本函数等价物（`Watchdog::stop`）**全仓零调用**——
/// Linux 优雅停机从不写 magic char 'V'，设备保持 armed。
pub fn stop_platform_watchdog() {
    match ACTIVE_INNER.get() {
        Some(inner) => {
            tracing::info!("stopping platform watchdog (graceful shutdown)");
            inner.stop();
        }
        None => tracing::debug!("platform watchdog not started: nothing to stop"),
    }
}

/// 长耗时路径（OTA 写入等）复用主看门狗的 fd 喂狗。
///
/// **不要**在这里自己 `open("/dev/watchdog")`：独占设备第二次 open 必然
/// `EBUSY`，写失败被吞掉后表现为"硬件喂狗在 OTA 期间静默失效"。
pub fn kick_platform_watchdog() {
    let outcome = {
        let mut guard = match hw_fd_slot().lock() {
            Ok(g) => g,
            Err(_) => {
                tracing::error!("hardware watchdog fd slot poisoned: OTA kick skipped");
                return;
            }
        };
        hw_write(guard.as_mut(), b"\0")
    };
    match outcome {
        HwWriteOutcome::Ok => tracing::debug!("OTA: hardware watchdog kicked"),
        HwWriteOutcome::Failed => {
            if let Some(inner) = ACTIVE_INNER.get() {
                inner.record_hw_write_failure("ota kick");
            } else {
                tracing::error!(
                    "OTA: hardware watchdog write failed — the hardware watchdog is NOT being fed \
                     during the OTA write; a hung write will NOT be reset by hardware"
                );
            }
        }
        HwWriteOutcome::NoDevice => {
            tracing::debug!("OTA: no hardware watchdog device, kick skipped")
        }
    }
}

/// 单调时钟毫秒（自进程启动起算）。
///
/// 不用 SystemTime（挂钟时间）：NTP/手动校时「前跳」会让 elapsed 变成极大值，
/// 从而触发 std::process::abort() → 被 systemd 重启 → 再次校时 → 启动重启循环。
/// 改用 Instant 单调时钟，不受挂钟调整影响。
fn mono_millis() -> u64 {
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 写失败分支可测：只读句柄写入必然失败（跨平台成立）。
    #[test]
    fn hw_write_reports_failure_on_readonly_handle() {
        let dir = std::env::temp_dir().join("gw-hw-wd-test");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("ro.bin");
        std::fs::write(&path, b"x").expect("write file");
        let mut ro = std::fs::File::open(&path).expect("open read-only");
        assert_eq!(hw_write(Some(&mut ro), b"\0"), HwWriteOutcome::Failed);
        let _ = std::fs::remove_file(&path);
    }

    /// 写成功分支可测：可写句柄写入并 flush 成功。
    #[test]
    fn hw_write_reports_ok_on_writable_handle() {
        let dir = std::env::temp_dir().join("gw-hw-wd-test");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("rw.bin");
        let mut rw = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .expect("open writable");
        assert_eq!(hw_write(Some(&mut rw), b"\0"), HwWriteOutcome::Ok);
        let _ = std::fs::remove_file(&path);
    }

    /// 无设备分支：无 fd 时不写、不报错。
    #[test]
    fn hw_write_reports_no_device_without_fd() {
        assert_eq!(hw_write(None, b"\0"), HwWriteOutcome::NoDevice);
    }

    /// 写失败必须被计入错误计数（现场事故的唯一线索来源）。
    #[test]
    fn hw_write_failures_are_counted() {
        let inner = WatchdogInner {
            last_kick: AtomicU64::new(0),
            running: AtomicBool::new(true),
            hw_write_errors: AtomicU64::new(0),
            handle: Mutex::new(None),
        };
        assert_eq!(inner.hw_write_errors.load(Ordering::Relaxed), 0);
        inner.record_hw_write_failure("test");
        assert_eq!(inner.hw_write_errors.load(Ordering::Relaxed), 1);
        inner.record_hw_write_failure("test");
        assert_eq!(inner.hw_write_errors.load(Ordering::Relaxed), 2);
    }

    /// 停止后 `is_running()` 为 false（喂狗循环据此退出）。
    #[test]
    fn stop_clears_running_flag() {
        let wd = Watchdog::start(Duration::from_secs(60), None);
        assert!(wd.is_running());
        wd.stop();
        assert!(!wd.is_running());
    }
}
