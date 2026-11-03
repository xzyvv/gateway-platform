//! 健康监控：磁盘空间、内存 RSS 监控

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::engine::error::{GatewayError, GatewayResult};
use crate::queue::DataQueue;

/// 磁盘压力状态（替代原全局 static DOWNSAMPLE_TRIGGERED）
///
/// 通过 Gateway struct 注入，支持多实例隔离。
///
/// ## P9：语义订正 —— 这不是"降采样开关"，而是"磁盘压力信号"
///
/// 原字段名为 `downsample_triggered`，配套注释声称"触发降采样"。但全仓检索可证：
/// 该标志**只写不读**——`set_downsample` 仅在 [`spawn_disk_monitor`] 内被调用，
/// `is_downsample_triggered` 零调用点；而磁盘紧张时真正生效的动作只有
/// `queue.set_disk_ok(false)`（入队阻断 + fallback 缓冲）。**没有任何降采样逻辑存在**。
///
/// 即：现场运维若相信"磁盘低→自动降采样"，实际得到的是"磁盘低→低优先级点被丢弃"，
/// 两者处置方式完全不同（前者等磁盘恢复，后者要补采/扩容）。
///
/// 故本结构改为承载**磁盘压力**这一真实语义：
/// - `is_pressure()`：磁盘剩余空间低于阈值（10%），入队正在被阻断；
/// - `free_pct()`：最近一次检测到的剩余百分比，供 API / 诊断页展示真实数值。
///
/// 保留 `set_downsample` / `is_downsample_triggered` 作为兼容别名已无必要
/// （唯一调用点就在本文件内），直接改名并修正文档。
pub struct DiskStatus {
    /// 磁盘压力标志：剩余空间低于阈值时为 true。
    pressure: AtomicBool,
    /// 最近一次检测到的剩余空间百分比（未检测过时为 None）。
    free_pct: std::sync::atomic::AtomicU64,
    /// free_pct 的有效性标志（区分"0%"与"未检测"）。
    free_pct_valid: AtomicBool,
}

impl DiskStatus {
    pub fn new() -> Self {
        Self {
            pressure: AtomicBool::new(false),
            free_pct: std::sync::atomic::AtomicU64::new(0),
            free_pct_valid: AtomicBool::new(false),
        }
    }

    /// 是否处于磁盘压力状态（剩余空间低于阈值，入队正在被阻断）。
    pub fn is_pressure(&self) -> bool {
        self.pressure.load(Ordering::Relaxed)
    }

    /// 最近一次检测到的剩余空间百分比；从未检测成功时为 `None`。
    ///
    /// 用 `f64::to_bits` 存原子整数，避免额外的 `Mutex`。
    pub fn free_pct(&self) -> Option<f64> {
        if !self.free_pct_valid.load(Ordering::Relaxed) {
            return None;
        }
        let bits = self.free_pct.load(Ordering::Relaxed);
        Some(f64::from_bits(bits))
    }

    /// 记录一次磁盘检测结果（剩余百分比 + 是否处于压力状态）。
    ///
    /// 返回**状态是否发生变化**，供调用方决定是否打日志（避免每 60s 重复刷同一条）。
    pub fn record(&self, free_pct: f64, pressure: bool) -> bool {
        self.free_pct.store(free_pct.to_bits(), Ordering::Relaxed);
        self.free_pct_valid.store(true, Ordering::Relaxed);
        self.pressure.swap(pressure, Ordering::Relaxed) != pressure
    }
}

impl Default for DiskStatus {
    fn default() -> Self {
        Self::new()
    }
}

/// 磁盘空间阈值：低于此百分比时拒绝入队（保护 SQLite）
const DISK_FREE_THRESHOLD_PCT: f64 = 10.0;

/// Bug 94: 默认检测路径可通过环境变量 `GATEWAY_DATA_DIR` 覆盖，
/// 避免 Windows 部署或自定义安装路径下误报。
///
/// Bug T12: 原实现无覆盖时恒返回 `/var/lib/gateway`——该路径在 Windows 上不存在，
/// `GetDiskFreeSpaceExW` 直接失败，导致磁盘监控循环永久记 warn、`disk_ok` 永不更新
/// （磁盘满保护在 Windows 下静默失效）。改为按平台给默认值：Windows 回退到系统盘
/// 根目录（`%SystemDrive%\`），非 Windows 保留 `/var/lib/gateway`。
/// 磁盘检查路径：优先 `GATEWAY_DATA_DIR`，否则按平台取默认数据分区。
///
/// 供 [`super::host`] 的指标采集复用（诊断页展示的磁盘使用率必须与监控循环同口径）。
pub fn disk_check_path() -> String {
    if let Ok(p) = std::env::var("GATEWAY_DATA_DIR") {
        return p;
    }
    #[cfg(windows)]
    {
        std::env::var("SystemDrive")
            .map(|d| format!("{}\\", d))
            .unwrap_or_else(|_| "C:\\".to_string())
    }
    #[cfg(not(windows))]
    {
        "/var/lib/gateway".to_string()
    }
}

/// 指定路径所在分区的**使用率**（%），供诊断页展示。
///
/// 与 [`check_disk_space`] 同源（statvfs / GetDiskFreeSpaceExW），只是把
/// 「剩余百分比」换算成「已用百分比」，保证诊断页与磁盘监控同口径。
pub fn disk_usage_percent(path: &str) -> GatewayResult<f64> {
    check_disk_space(path).map(|free| (100.0 - free).clamp(0.0, 100.0))
}

/// 磁盘空间监控：数据分区剩余空间低于阈值时更新 queue.disk_ok
/// 由 gatewayd 启动编排调用，故对外公开。
///
/// 同时把检测结果写入 [`DiskStatus`]（磁盘压力信号），供 `/health/ready` 的
/// `storage` 组件与 Prometheus `gateway_disk_pressure` gauge 消费——
/// 否则磁盘满（入队被阻断、低优先级点被丢弃）在监控与页面上完全不可见。
pub fn spawn_disk_monitor(
    queue: Arc<DataQueue>,
    history: Arc<DiskHistoryStore>,
    disk_status: Arc<DiskStatus>,
    metrics: Arc<crate::engine::metrics::Metrics>,
) {
    let check_path = disk_check_path();
    crate::engine::supervision::spawn_supervised("disk_monitor", move || {
        let queue = queue.clone();
        let history = history.clone();
        let disk_status = disk_status.clone();
        let metrics = metrics.clone();
        let check_path = check_path.clone();
        async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                match check_disk_space(&check_path) {
                    Ok(free_pct) => {
                        let ok = free_pct >= DISK_FREE_THRESHOLD_PCT;
                        queue.set_disk_ok(ok);
                        history.push(free_pct);
                        // 状态变化时才打日志（否则每 60s 重复刷同一条）
                        if disk_status.record(free_pct, !ok) {
                            if ok {
                                tracing::info!(
                                    "disk space recovered: {:.1}% free, enqueue unblocked",
                                    free_pct
                                );
                            } else {
                                tracing::error!(
                                    "disk space low: {:.1}% free, blocking enqueue (low-priority points dropped)",
                                    free_pct
                                );
                            }
                        }
                        metrics.set_disk_pressure(if ok { 0 } else { 1 });
                        if ok {
                            // S2: 磁盘恢复时刷新 fallback 缓冲到 SQLite
                            if let Err(e) = queue.flush_fallback().await {
                                tracing::warn!("flush fallback on disk recovery failed: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("disk space check failed: {}", e);
                    }
                }
            }
        }
    });
}

// ============================================================
// r52-MEM1: 进程 RSS 自监控
// ============================================================
//
// 模块文档承诺「磁盘空间、内存 RSS 监控」，但本轮之前只实现了磁盘。
// 这里补齐 RSS 自监控：
//   - Linux：读 /proc/self/statm（resident pages × page size）
//   - Windows：GetProcessMemoryInfo 的 WorkingSetSize
//   - 其它平台：返回 0（Prometheus 上报 0，运维侧按 "unsupported" 处理）
//
// 采集周期 30s——RSS 变化缓慢（GC / 大块分配），高频采样对运维无意义、
// 徒增 prometheus 端点开销。30s 粒度与 Garafana 1m 仪表盘窗口对齐。
//
// 与磁盘监控的区别：本监控只读 self，不影响入队路径，**不**带降采样开关
// （内存压力与磁盘压力处理策略不同：磁盘满需阻断 enqueue 保护 SQLite，
// 内存满应交给 systemd OOM / supervisor 重启，不在本进程阻断业务）。

/// r52-MEM1：启动进程 RSS 监控循环（自恢复，受 supervision 管理）。
///
/// 由 gatewayd bootstrap 在 spawn_disk_monitor 之后调用。
pub fn spawn_memory_monitor(metrics: Arc<crate::engine::metrics::Metrics>) {
    crate::engine::supervision::spawn_supervised("memory_monitor", move || {
        let metrics = metrics.clone();
        async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                match read_rss_bytes() {
                    Ok(bytes) => metrics.set_process_rss(bytes as i64),
                    Err(e) => {
                        // 采集失败：保持上次值或 0，warn 让运维侧留意。
                        // 不把 0 当作"未运行"信号——0 在 metric 端点表示"不支持/失败"。
                        tracing::warn!("process RSS read failed: {}", e);
                    }
                }
            }
        }
    });
}

/// r52-MEM1：读取当前进程 RSS 字节数。
///
/// 跨平台实现：Linux 走 /proc/self/statm，Windows 走 GetProcessMemoryInfo。
/// 不支持的平台返回 0（与 prometheus 端点的 "0 = unsupported" 约定对齐）。
pub fn read_rss_bytes() -> GatewayResult<u64> {
    #[cfg(target_os = "linux")]
    {
        use std::fs;
        // /proc/self/statm 内容（单行）：
        //   size resident shared text lib data dt  (单位：页)
        // 我们要 resident（第 2 列）× page size = RSS 字节数。
        let content = fs::read_to_string("/proc/self/statm")
            .map_err(|e| GatewayError::Io(format!("read /proc/self/statm: {}", e)))?;
        let mut it = content.split_ascii_whitespace();
        let _size = it.next();
        let resident_pages: u64 = it
            .next()
            .ok_or_else(|| GatewayError::Io("statm 缺 resident 字段".into()))?
            .parse()
            .map_err(|e| GatewayError::Io(format!("statm resident 解析失败: {}", e)))?;
        // sysconf(_SC_PAGESIZE) 在 Linux 上是 4 KiB（x86_64/aarch64 一致）；
        // libc::sysconf 返回 i64，错误时返 -1。用 PAGE_SIZE fallback 兜底。
        let page_size: u64 = unsafe {
            let p = libc::sysconf(libc::_SC_PAGESIZE);
            if p > 0 {
                p as u64
            } else {
                4096
            }
        };
        Ok(resident_pages.saturating_mul(page_size))
    }
    #[cfg(windows)]
    {
        use std::mem::MaybeUninit;
        #[link(name = "kernel32")]
        extern "system" {
            fn GetProcessMemoryInfo(
                process: *mut std::ffi::c_void,
                mem_counters: *mut PROCESS_MEMORY_COUNTERS,
                cb: u32,
            ) -> i32;
            fn GetCurrentProcess() -> *mut std::ffi::c_void;
        }
        #[repr(C)]
        #[derive(Default)]
        struct PROCESS_MEMORY_COUNTERS {
            cb: u32,
            page_fault_count: u32,
            peak_working_set_size: usize,
            working_set_size: usize, // 当前 RSS（Working Set ≈ 物理驻留）
            quota_peak_paged_pool_usage: usize,
            quota_paged_pool_usage: usize,
            quota_peak_non_paged_pool_usage: usize,
            quota_non_paged_pool_usage: usize,
            pagefile_usage: usize,
            peak_pagefile_usage: usize,
        }
        let mut info = MaybeUninit::<PROCESS_MEMORY_COUNTERS>::zeroed();
        // SAFETY:
        // - info 是 MaybeUninit 零值，GetProcessMemoryInfo 仅在成功时写入
        //   （cb 字段由我们预填 0；GetProcessMemoryInfo 内部不会读它来判大小，
        //   但按惯例应填 sizeof，让我们在调用前手填）。
        // - GetCurrentProcess 返回伪句柄（-1），进程内有效。
        // - info.assume_init() 在 GetProcessMemoryInfo 成功后安全（其余字段
        //   同样在成功后由系统填零或有效值）。
        let ok = unsafe {
            let p = info.as_mut_ptr();
            (*p).cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
            GetProcessMemoryInfo(GetCurrentProcess(), p, (*p).cb)
        };
        if ok == 0 {
            return Err(GatewayError::Io(format!(
                "GetProcessMemoryInfo failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        let info = unsafe { info.assume_init() };
        Ok(info.working_set_size as u64)
    }
    #[cfg(all(not(target_os = "linux"), not(windows)))]
    {
        let _ = ();
        Ok(0)
    }
}

/// 检查指定目录所在分区的剩余空间百分比
///
/// `as u64` 转换在 32 位平台（如 armv7）是必要的：statvfs 的 f_frsize 类型跟随
/// c_ulong，在 32 位上仍是 32 位无符号数。64 位平台上 clippy 会判其为「同类型
/// 转换」，故在此局部放行该 lint，保留跨宽度可编译性。
#[allow(clippy::unnecessary_cast)]
fn check_disk_space(path: &str) -> GatewayResult<f64> {
    #[cfg(unix)]
    {
        use std::mem::MaybeUninit;
        let mut stat: MaybeUninit<libc::statvfs> = MaybeUninit::uninit();
        let c_path = std::ffi::CString::new(path)?;
        // SAFETY:
        // - `c_path` 是 CString::new(path) 的结果，含 NUL 终止符，指针有效。
        // - `stat` 是 MaybeUninit<libc::statvfs>，as_mut_ptr() 指向其内存，
        //   statvfs 成功返回 0 后 assume_init() 安全（POD 类型，无 Drop）。
        // - 调用线程独占 stat 内存，无并发访问。
        unsafe {
            if libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) != 0 {
                return Err(GatewayError::Io(format!(
                    "statvfs failed: {}",
                    std::io::Error::last_os_error()
                )));
            }
            let stat = stat.assume_init();
            let total = stat.f_blocks as u64 * stat.f_frsize as u64;
            let free = stat.f_bavail as u64 * stat.f_frsize as u64;
            if total == 0 {
                return Ok(100.0);
            }
            Ok((free as f64 / total as f64) * 100.0)
        }
    }
    #[cfg(windows)]
    {
        // #13 修复：Windows 下用 GetDiskFreeSpaceExW 获取真实剩余空间，
        // 替代原固定返回 50% 的占位实现。
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;

        #[link(name = "kernel32")]
        extern "system" {
            fn GetDiskFreeSpaceExW(
                lp_directory_name: *const u16,
                free_bytes_available: *mut u64,
                total_number_of_bytes: *mut u64,
                total_number_of_free_bytes: *mut u64,
            ) -> i32;
        }

        let wide: Vec<u16> = OsStr::new(path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut free_available: u64 = 0;
        let mut total_bytes: u64 = 0;
        let mut total_free: u64 = 0;

        // SAFETY:
        // - `wide` 是 OsStr::encode_wide().chain(once(0)) 的 Vec<u16>，
        //   含 NUL 终止符，as_ptr() 在调用期间有效（wide 活到函数结束）。
        // - 三个输出指针指向栈上 u64 局部变量（已初始化为 0），
        //   GetDiskFreeSpaceExW 仅在成功时写入，对齐和大小匹配。
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free_available,
                &mut total_bytes,
                &mut total_free,
            )
        };
        if ok == 0 {
            return Err(GatewayError::Io(format!(
                "GetDiskFreeSpaceExW failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        if total_bytes == 0 {
            return Ok(100.0);
        }
        Ok((free_available as f64 / total_bytes as f64) * 100.0)
    }
    #[cfg(all(not(unix), not(windows)))]
    {
        let _ = path;
        Ok(50.0)
    }
}
// ============================================================
// D10: 磁盘健康趋势历史采样 + SQLite 持久化
// ============================================================

/// 磁盘采样点
#[derive(Debug, Clone, serde::Serialize)]
pub struct DiskSample {
    /// 采样时间戳（毫秒 epoch）
    pub ts: i64,
    /// 剩余空间百分比
    pub free_pct: f64,
}

/// 磁盘历史环形缓冲（保留最近 max 条采样）
///
/// D10: 双写内存 + SQLite，重启后从 DB 恢复历史数据。
/// 内存侧用 VecDeque 保证读取性能，DB 侧保证重启不丢数据。
pub struct DiskHistoryStore {
    samples: parking_lot::Mutex<std::collections::VecDeque<DiskSample>>,
    max: usize,
    db_path: PathBuf,
}

impl DiskHistoryStore {
    /// 创建磁盘历史存储并从 SQLite 恢复历史数据
    pub fn new(max: usize, db_path: &str) -> GatewayResult<Self> {
        let max = max.max(10);
        let db_path = PathBuf::from(db_path);
        let store = Self {
            samples: parking_lot::Mutex::new(std::collections::VecDeque::with_capacity(max)),
            max,
            db_path,
        };
        store.init_db()?;
        store.load_from_db()?;
        Ok(store)
    }

    /// 打开本库连接并设置共享库所需 PRAGMA。
    ///
    /// r39-VER1：disk_history 与 config_versions / uplink_test_history 共用
    /// default_version_db 库文件，三处各持独立连接并发写。busy_timeout 是
    /// per-connection 设置，每次 open 都必须重设，否则并发写遇 SQLITE_BUSY
    /// 会立即失败（默认 0ms）→ 磁盘采样偶发丢失。
    fn open_db(&self) -> GatewayResult<rusqlite::Connection> {
        let conn = rusqlite::Connection::open(&self.db_path)
            .map_err(|e| GatewayError::Queue(format!("open disk_history db failed: {}", e)))?;
        conn.busy_timeout(std::time::Duration::from_millis(5000))
            .map_err(|e| GatewayError::Queue(format!("set busy_timeout failed: {}", e)))?;
        Ok(conn)
    }

    /// 建表（IF NOT EXISTS）
    fn init_db(&self) -> GatewayResult<()> {
        let conn = self.open_db()?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")
            .map_err(|e| GatewayError::Queue(format!("set WAL failed: {}", e)))?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS disk_history (
                ts INTEGER PRIMARY KEY,
                free_pct REAL NOT NULL
            )",
            [],
        )
        .map_err(|e| GatewayError::Queue(format!("create disk_history table failed: {}", e)))?;
        Ok(())
    }

    /// 从 DB 加载最近 max 条采样到内存（按 ts 升序）
    fn load_from_db(&self) -> GatewayResult<()> {
        let conn = self.open_db()?;
        let mut stmt = conn
            .prepare(&format!(
                "SELECT ts, free_pct FROM disk_history ORDER BY ts DESC LIMIT {}",
                self.max
            ))
            .map_err(|e| {
                GatewayError::Queue(format!("prepare disk_history query failed: {}", e))
            })?;
        let rows: Vec<DiskSample> = stmt
            .query_map([], |row| {
                Ok(DiskSample {
                    ts: row.get(0)?,
                    free_pct: row.get(1)?,
                })
            })
            .map_err(|e| GatewayError::Queue(format!("query disk_history failed: {}", e)))?
            .filter_map(|r| r.ok())
            .collect();
        let mut guard = self.samples.lock();
        guard.clear();
        // rows 是按 ts DESC 取的，反转后为升序
        for s in rows.into_iter().rev() {
            guard.push_back(s);
        }
        Ok(())
    }

    /// 追加一条采样（超过容量时丢弃最旧的，同时写入 SQLite）
    ///
    /// Bug 92: 当前在调用线程同步写 SQLite。盘监视循环（60s 一次）调用频率极低，
    /// 同步开销可接受；如未来接入更高频指标采集，应改为 spawn_blocking 或后台
    /// writer 任务，避免阻塞业务 tokio 调度。
    pub fn push(&self, free_pct: f64) {
        let sample = DiskSample {
            ts: chrono::Utc::now().timestamp_millis(),
            free_pct,
        };
        {
            let mut guard = self.samples.lock();
            if guard.len() >= self.max {
                guard.pop_front();
            }
            guard.push_back(sample.clone());
        }
        // 持久化到 SQLite；失败仅记录日志，不影响内存采样
        if let Err(e) = self.persist_sample(&sample) {
            tracing::warn!("persist disk sample to db failed: {}", e);
        }
    }

    /// 写入单条采样到 DB 并裁剪旧数据
    fn persist_sample(&self, sample: &DiskSample) -> GatewayResult<()> {
        let conn = self.open_db()?;
        conn.execute(
            "INSERT OR REPLACE INTO disk_history (ts, free_pct) VALUES (?1, ?2)",
            rusqlite::params![sample.ts, sample.free_pct],
        )
        .map_err(|e| GatewayError::Queue(format!("insert disk_history failed: {}", e)))?;
        // Bug 90/91: 原嵌套子查询 `WHERE ts NOT IN (SELECT ... LIMIT N)`
        // 在 SQLite 上虽然 ts 是 PRIMARY KEY 可走索引，但仍要做完整子查询。
        // 改为基于"裁剪阈值 ts"的两步策略：先取最旧保留条目的 ts，再删除早于它的行。
        // 优势：单层 WHERE 范围扫描，max 较小时 O(1)；原方案每次 O(N)。
        // P2-d: 原实现 `let _ = conn.execute(..)` 静默吞掉裁剪失败——历史表会
        // 持续膨胀而不报警。裁剪失败不应让采样写入失败（样本已写入），但必须留痕。
        if let Err(e) = conn.execute(
            &format!(
                "DELETE FROM disk_history WHERE ts < (
                    SELECT ts FROM disk_history ORDER BY ts DESC LIMIT 1 OFFSET {}
                )",
                self.max.saturating_sub(1)
            ),
            [],
        ) {
            tracing::warn!("disk_history retention cleanup failed: {}", e);
        }
        Ok(())
    }

    /// 读取全部历史快照（按时间升序）
    pub fn list(&self) -> Vec<DiskSample> {
        self.samples.lock().iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests;
