//! 进程自身运行时长（与「主机开机时长」区分）
//!
//! P3-5：`/api/diagnostics/gateway` 的 uptime 原先直接取主机 uptime，实测
//! 「14 天 21 小时」而进程当次仅启动 22 分钟，直连调试时会误导「重启后观察」。
//! 主机开机时长仍由 `host::collect()` 提供，进程时长则以 bootstrap 记录的时间戳为准。

use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// 进程启动时刻（秒级 Unix 时间戳），由 bootstrap 在启动最开始写入一次
static PROCESS_START_SEC: OnceLock<u64> = OnceLock::new();

/// 记录进程启动时刻。只在 bootstrap 入口调用一次；重复调用忽略。
pub fn mark_process_start() {
    let _ = PROCESS_START_SEC.set(now_sec());
}

/// 进程已运行秒数。
///
/// 未经 bootstrap 标记时以首次读取时刻为起点（得到保守下界，而不是主机时长）。
pub fn process_uptime_secs() -> u64 {
    now_sec().saturating_sub(start_sec())
}

fn start_sec() -> u64 {
    *PROCESS_START_SEC.get_or_init(now_sec)
}

fn now_sec() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uptime_is_monotonic_and_not_host_based() {
        mark_process_start();
        let first = process_uptime_secs();
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(process_uptime_secs() >= first);
        // 标记后进程时长不应达到「主机开机数月」的量级
        assert!(first < 60, "process uptime should be seconds, got {first}");
    }
}
