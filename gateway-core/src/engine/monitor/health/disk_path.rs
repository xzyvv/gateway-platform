//! 磁盘检查路径的推导：保证「被检查的分区」就是「数据实际落地的分区」。
//!
//! # 口径错位是怎么出问题的
//!
//! 网关的数据（队列库、审计日志、诊断日志）落在哪个卷，是由 `GATEWAY_DB_PATH`
//! （或平台默认路径）决定的；而磁盘监控检查哪个分区，最初是写死的：
//! Windows 固定看 `%SystemDrive%\`（整块系统盘），Linux 固定看 `/var/lib/gateway`。
//!
//! 两者一旦不是同一个卷，就会出现两类错误：
//!
//! - **写入卷已满却不告警**：数据盘写满 → SQLite 写失败 → 采集丢点，
//!   但监控看的是系统盘，一直显示 ok；
//! - **系统盘紧张却误阻断入队**：系统盘只剩 5% → 监控报 blocking →
//!   入队被无谓拒绝，数据盘其实还很空。
//!
//! E2E / 桌面端场景更明显：数据写在临时目录（`workDir`），
//! 检查的却是宿主机的系统盘根目录——宿主机磁盘紧张就会通过
//! `/health/ready` 503（storage degraded）打挂冒烟。
//!
//! # 推导优先级
//!
//! 1. `GATEWAY_DATA_DIR`：现场显式指定数据分区时的唯一权威来源；
//! 2. 队列库路径的父目录：数据真正落盘的位置，与被检查对象同卷；
//! 3. 平台默认：仅当队列库路径没有父目录成分时才可能触发（防御性兜底）。
//!
//! 第 2 步在生产与开发两种形态下都成立：Linux 上 `default_db_path()` 为
//! `/var/lib/gateway/data_queue.db` → 父目录 `/var/lib/gateway`（与历史默认值一致）；
//! Windows / 桌面端为 `./data_queue.db` → 父目录 `.`（进程工作目录），
//! 而该库文件正是相对工作目录打开的，故检查工作目录所在卷才是同卷口径。

use std::path::Path;

/// 磁盘检查路径：优先 `GATEWAY_DATA_DIR`，其次取队列库所在目录，
/// 最后才回退到平台默认数据分区。
///
/// 供 `super::host` 的指标采集复用（诊断页展示的磁盘使用率必须与监控循环同口径）。
pub fn disk_check_path() -> String {
    let data_dir = std::env::var("GATEWAY_DATA_DIR").ok();
    let db_path = std::env::var("GATEWAY_DB_PATH").ok();
    resolve_disk_check_path(data_dir.as_deref(), db_path.as_deref())
}

/// 纯函数形式的磁盘检查路径推导（便于单测，不触碰环境变量）。
///
/// 优先级：`GATEWAY_DATA_DIR` → 队列库路径的父目录 → 平台默认数据分区。
pub(super) fn resolve_disk_check_path(data_dir: Option<&str>, db_path: Option<&str>) -> String {
    // 1) 显式数据目录：现场指定数据分区时的唯一权威来源
    if let Some(dir) = data_dir.map(str::trim).filter(|d| !d.is_empty()) {
        return dir.to_string();
    }
    // 2) 队列库所在目录：数据真正落盘的位置，与被检查对象同卷
    let db = db_path
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| crate::shared::default_db_path());
    if let Some(dir) = Path::new(db).parent() {
        let dir = dir.to_string_lossy().to_string();
        // 相对路径（"./data_queue.db" → "."）本身即当前目录，可直接使用；
        // 无父目录（"data_queue.db" → ""）则继续回退。
        if !dir.is_empty() {
            return dir;
        }
    }
    // 3) 平台默认兜底：Linux 走 FHS 数据目录，Windows 回退系统盘根目录
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
