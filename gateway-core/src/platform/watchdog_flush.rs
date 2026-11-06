//! P4：硬件看门狗 abort 前的状态持久化（flush 回调接线）
//!
//! ## 背景
//!
//! `platform::hardware_watchdog::set_flush_callback` 此前**零调用**——
//! `hardware_watchdog.rs:89-111` 的超时分支直接 `std::process::abort()`，
//! abort 前**不落任何状态**：内存 fallback 缓冲里的数据点、OTA 进行中标记
//! 全部随进程消失。也就是说「看门狗」这个保护机制在自己最该生效的场景
//! （系统卡死）下反而丢数据。
//!
//! ## 为什么回调体必须全同步
//!
//! 看门狗超时意味着**进程卡死**——这正是 tokio 运行时可能整体无法调度的场景。
//! 因此回调体内：
//!
//! - **不**走 `tx.send` 之类的异步通道（actor 已无法被调度 → 永久挂住）；
//! - **不**用 `block_on`（运行时卡死时同样挂住，只是把 abort 推迟 3s）；
//! - 只做**同步**文件 IO 与**同步**存储后端调用（后端自带锁，见 `QueueStorage`
//!   契约「实现方负责内部同步」）。
//!
//! 看门狗线程侧已有 3s 超时包装（`hardware_watchdog.rs:104`），即便回调卡住
//! 也会在 3s 后强制 abort，不会把「卡死」变成「挂死」。
//!
//! ## 落盘内容
//!
//! 1. **队列 fallback 缓冲** → 同步写入 SQLite（这批数据点此前只存在于内存，
//!    进程一死就永久丢失）。
//! 2. **状态快照** → `watchdog_state.json`：落盘时刻、fallback 水位、已落库条数、
//!    OTA 是否处于升级中。供重启后审计「上次是卡死 abort 还是正常退出」。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::engine::error::{GatewayError, GatewayResult};
use crate::platform::hardware_watchdog::set_flush_callback;
use crate::platform::ota::OtaManager;
use crate::queue::DataQueue;

/// 看门狗 abort 前落盘的状态快照。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchdogFlushSnapshot {
    /// 落盘时刻（Unix 毫秒）
    pub timestamp_ms: i64,
    /// flush 前 fallback 缓冲中的数据点数（= 本次可能丢掉的内存数据量）
    pub fallback_len: usize,
    /// 本次成功写入存储后端的数据点数
    pub flushed: usize,
    /// OTA 是否处于升级中（Uploading / Verifying / Writing）
    pub ota_in_progress: bool,
    /// OTA 状态名（Idle / Uploading / Verifying / Writing / PendingReboot / Failed）
    pub ota_state: String,
}

/// 安装看门狗 abort 前的 flush 回调。
///
/// 由 gatewayd 在「队列 + OTA 管理器」都已就绪后调用一次。
/// `hardware_watchdog::set_flush_callback` 内部是 `OnceCell::set`——
/// 重复安装时**先到者生效**，本进程只会有一处调用点。
///
/// 注：硬件看门狗本身仅 Linux 生效（`collection.rs` 的 `#[cfg(target_os = "linux")]`），
/// 但注册本身跨平台无副作用（非 Linux 上永不触发），故不做平台门控。
pub fn install_watchdog_flush(queue: Arc<DataQueue>, ota: Arc<OtaManager>, state_file: PathBuf) {
    set_flush_callback(move || {
        let snapshot = flush_once(&queue, &ota, &state_file);
        tracing::warn!(
            "watchdog flush done: flushed={}/{} fallback_len={} ota_in_progress={} ota_state={}",
            snapshot.flushed,
            snapshot.fallback_len,
            snapshot.fallback_len,
            snapshot.ota_in_progress,
            snapshot.ota_state
        );
    });
}

/// 执行一次 flush：同步冲刷 fallback 缓冲 + 落盘状态快照。
///
/// 拆成独立函数是为了能在单测里直接调用（不依赖看门狗线程与 abort 流程）。
///
/// 返回本次的快照内容；**任何一步失败都不 panic**——回调跑在 abort 前，
/// panic 会让整个回调被跳过（比「部分成功」更糟）。
pub fn flush_once(queue: &DataQueue, ota: &OtaManager, state_file: &Path) -> WatchdogFlushSnapshot {
    let fallback_len = queue.fallback_len();
    let flushed = match queue.flush_fallback_sync() {
        Ok(n) => n,
        Err(e) => {
            tracing::error!("watchdog flush: fallback flush failed: {}", e);
            0
        }
    };

    let ota_status = ota.status();
    let snapshot = WatchdogFlushSnapshot {
        timestamp_ms: now_millis(),
        fallback_len,
        flushed,
        ota_in_progress: ota.is_in_progress(),
        ota_state: format!("{:?}", ota_status.state),
    };

    if let Err(e) = write_snapshot(state_file, &snapshot) {
        tracing::error!("watchdog flush: write snapshot failed: {}", e);
    }
    snapshot
}

/// 原子写快照文件（先写 tmp 再 rename，避免半截文件）。
fn write_snapshot(path: &Path, snapshot: &WatchdogFlushSnapshot) -> GatewayResult<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| {
                GatewayError::Config(format!(
                    "create watchdog state dir {} failed: {}",
                    parent.display(),
                    e
                ))
            })?;
        }
    }
    let json = serde_json::to_string_pretty(snapshot)
        .map_err(|e| GatewayError::Config(format!("serialize watchdog snapshot failed: {}", e)))?;
    let tmp = path.with_extension("json.tmp");
    // 权限：创建即 0600（Unix）。
    //
    // 快照内容本身不含凭据（仅时间戳 / 队列水位 / OTA 状态），泄露后果有限，
    // 但它记录的是「上次是卡死 abort 还是正常退出」——现场排障依据，
    // 不应让同机其它用户可读可改。`std::fs::write` 走默认 0666&~umask
    // （典型 0644），且 `File::create` 先宽后紧的写法会留下 chmod 竞态窗口，
    // 故直接用 `OpenOptions::mode` 在创建时收紧。
    {
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp).map_err(|e| {
            GatewayError::Config(format!("write watchdog snapshot tmp failed: {}", e))
        })?;
        f.write_all(json.as_bytes()).map_err(|e| {
            GatewayError::Config(format!("write watchdog snapshot tmp failed: {}", e))
        })?;
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        GatewayError::Config(format!(
            "rename watchdog snapshot {} failed: {}",
            path.display(),
            e
        ))
    })
}

/// Unix 毫秒时间戳（失败退化为 0，不 panic）。
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::model::{DataPoint, Quality};

    fn make_point(id: &str, value: f64) -> DataPoint {
        let now = DataPoint::now();
        DataPoint {
            point_id: id.into(),
            device_id: "dev1".into(),
            value,
            raw_value: value,
            raw_type: "u16".into(),
            quality: Quality::Good,
            quality_reason: "".into(),
            timestamp: now,
            source_ts: now,
            stale_count: 0,
            value_text: None,
        }
    }

    fn temp_state_file(tag: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("should succeed");
        let path = dir.path().join(tag);
        (dir, path)
    }

    fn test_ota() -> Arc<OtaManager> {
        let dir = tempfile::tempdir().expect("should succeed");
        let staging = dir.path().to_string_lossy().to_string();
        // 目录需活到 manager 使用结束：泄漏 TempDir，避免测试中途被清理。
        std::mem::forget(dir);
        OtaManager::new(
            crate::platform::ota::AbPartitionConfig::default(),
            &staging,
            None,
            Arc::new(crate::engine::metrics::Metrics::new()),
        )
    }

    /// P4 契约：abort 前必须把内存 fallback 缓冲里的数据点**真正写进存储**，
    /// 否则看门狗一触发这批数据就永久丢失。
    #[tokio::test]
    async fn flush_once_persists_fallback_points() {
        let queue = DataQueue::new_memory().expect("should succeed");
        let (_dir, state_file) = temp_state_file("watchdog_state.json");

        // 磁盘满 → 数据点进内存 fallback 缓冲
        queue.set_disk_ok(false);
        queue
            .enqueue(vec![make_point("p1", 1.0), make_point("p2", 2.0)])
            .await
            .expect("should succeed");
        assert_eq!(queue.fallback_len(), 2);
        assert_eq!(queue.len().await, 0);

        let ota = test_ota();
        let snapshot = flush_once(&queue, &ota, &state_file);

        assert_eq!(snapshot.fallback_len, 2);
        assert_eq!(snapshot.flushed, 2);
        // fallback 已清空，数据进了存储
        assert_eq!(queue.fallback_len(), 0);
        assert_eq!(queue.len().await, 2);
        // 状态快照已落盘
        let text = std::fs::read_to_string(&state_file).expect("should succeed");
        assert!(
            text.contains("\"flushed\": 2"),
            "快照应记录 flushed=2，实际:\n{}",
            text
        );
        assert!(
            text.contains("\"ota_in_progress\": false"),
            "快照应记录 ota_in_progress=false，实际:\n{}",
            text
        );
    }

    /// P4 契约：没有待落库数据时 flush 必须是**空操作**（不写多余文件内容变化、
    /// flushed=0），不能凭空造数据。
    #[tokio::test]
    async fn flush_once_is_noop_when_fallback_empty() {
        let queue = DataQueue::new_memory().expect("should succeed");
        let (_dir, state_file) = temp_state_file("watchdog_state.json");
        let ota = test_ota();

        let snapshot = flush_once(&queue, &ota, &state_file);
        assert_eq!(snapshot.fallback_len, 0);
        assert_eq!(snapshot.flushed, 0);
        assert_eq!(queue.len().await, 0);
        assert!(state_file.exists());
    }

    /// P4 契约：快照文件是**完整 JSON**（原子写，不会留下半截文件）。
    #[tokio::test]
    async fn flush_once_writes_parseable_json() {
        let queue = DataQueue::new_memory().expect("should succeed");
        let (_dir, state_file) = temp_state_file("watchdog_state.json");
        let ota = test_ota();

        flush_once(&queue, &ota, &state_file);
        let text = std::fs::read_to_string(&state_file).expect("should succeed");
        let parsed: WatchdogFlushSnapshot =
            serde_json::from_str(&text).expect("should be valid json");
        assert!(parsed.timestamp_ms > 0);
        assert_eq!(parsed.ota_state, "Idle");
        // 原子写：tmp 文件不应残留
        let tmp = state_file.with_extension("json.tmp");
        assert!(!tmp.exists(), "原子写后不应残留 tmp 文件");
    }

    /// 权限契约：快照文件创建即 0600，不给同机其它用户留「可读可改」窗口。
    ///
    /// 该文件是现场排障依据（判断上次是否卡死 abort），默认 0644 会被任意
    /// 本地用户读取甚至篡改。断言用 `cfg(unix)` 门控——Windows 无 POSIX 权限位。
    #[cfg(unix)]
    #[tokio::test]
    async fn flush_once_snapshot_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let queue = DataQueue::new_memory().expect("should succeed");
        let (_dir, state_file) = temp_state_file("watchdog_state.json");
        let ota = test_ota();

        flush_once(&queue, &ota, &state_file);

        let mode = std::fs::metadata(&state_file)
            .expect("should succeed")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "看门狗快照必须为 0600，实际为 {:o}", mode);
    }
}
