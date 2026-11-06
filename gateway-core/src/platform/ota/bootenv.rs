//! bootloader 环境（u-boot env）访问层
//!
//! OTA 的 A/B 分区切换依赖四个底层动作：写 env、读 env、刷盘、重启。
//! 生产路径由真实 u-boot 工具承担（`fw_setenv` / `fw_printenv` / `sync` / `reboot`），
//! 只有 Linux 目标机才具备。
//!
//! 抽象成两个后端的因：单元测试环境（开发机 / CI runner）既没有 u-boot 工具，
//! **更不能真的重启机器**——在 CI runner 上执行真实 `reboot` 会直接终止整个作业。
//! 因此测试构建使用 `Memory` 后端：纯内存记录 env 写入 / 读取值 / 重启次数，
//! 语义与生产一致，但不触碰宿主机。
//!
//! 非 Linux 平台的生产实现仍是 no-op（与历史行为一致）：写 env 视为成功、
//! 读 env 恒为 None、`reboot` 不执行。

use crate::engine::error::GatewayResult;

// GatewayError 仅在 Linux 的真实 u-boot 实现里构造错误，其余平台用不到。
#[cfg(target_os = "linux")]
use crate::engine::error::GatewayError;

#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(test)]
use std::sync::Arc;

#[cfg(test)]
use parking_lot::Mutex;

/// bootloader 环境访问后端。
pub(super) enum BootEnv {
    /// 生产路径：调用真实 u-boot 工具（Linux 目标机）。
    // 测试构建下默认走 Memory 后端，Uboot 变体不再被构造 —— 定向豁免 dead_code。
    #[cfg_attr(test, allow(dead_code))]
    Uboot,
    /// 测试路径：内存记录，不触碰宿主机 bootloader / 不重启机器。
    #[cfg(test)]
    Memory(Arc<MemoryBootEnv>),
}

impl BootEnv {
    /// 当前构建下的默认后端。
    ///
    /// 单测构建用内存后端，保证测试永远不会读写宿主机 u-boot 环境、
    /// 更不会重启测试机；生产构建用真实 u-boot 工具。
    pub(super) fn for_current_build() -> Self {
        #[cfg(test)]
        {
            BootEnv::Memory(Arc::new(MemoryBootEnv::default()))
        }
        #[cfg(not(test))]
        {
            BootEnv::Uboot
        }
    }

    /// 写入一个 bootloader 环境变量（如 `boot_partition=b`）。
    ///
    /// 生产实现在命令缺失 / 退出码非零时返回 Err——调用方据此判定
    /// 「启动分区未真正切换」，避免设备重启后仍加载旧固件。
    pub(super) fn setenv(&self, name: &str, value: &str) -> GatewayResult<()> {
        match self {
            BootEnv::Uboot => uboot_setenv(name, value),
            #[cfg(test)]
            BootEnv::Memory(mem) => mem.setenv(name, value),
        }
    }

    /// 读取一个 bootloader 环境变量；变量不存在或读取失败返回 None。
    pub(super) fn printenv(&self, name: &str) -> Option<String> {
        match self {
            BootEnv::Uboot => uboot_printenv(name),
            #[cfg(test)]
            BootEnv::Memory(mem) => mem.get_var(name),
        }
    }

    /// 刷盘：把 env 写入持久化到介质（u-boot env 位于独立分区，page cache
    /// 丢失会导致重启后设置失效）。
    pub(super) fn sync(&self) -> GatewayResult<()> {
        match self {
            BootEnv::Uboot => uboot_sync(),
            #[cfg(test)]
            BootEnv::Memory(_) => Ok(()),
        }
    }

    /// 重启设备，使 bootloader 环境变更生效。
    pub(super) fn reboot(&self) -> GatewayResult<()> {
        match self {
            BootEnv::Uboot => uboot_reboot(),
            #[cfg(test)]
            BootEnv::Memory(mem) => {
                mem.reboots.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }
    }

    /// 读取 bootloader `boot_partition` 的值 → 'a' / 'b'；非法值返回 None。
    pub(super) fn boot_partition_slot(&self) -> Option<char> {
        let raw = self.printenv("boot_partition")?;
        match raw.trim() {
            "a" => Some('a'),
            "b" => Some('b'),
            // 其他值（如 "0"/"1"）视作无效
            _ => None,
        }
    }

    /// 读取 bootloader `bootcount` 的值（u32）；不存在或非法返回 None。
    pub(super) fn bootcount(&self) -> Option<u32> {
        let raw = self.printenv("bootcount")?;
        raw.trim().parse::<u32>().ok()
    }

    /// 测试接缝：全部 env 写入记录（`(变量名, 值)`，顺序即调用顺序）。
    #[cfg(test)]
    pub(super) fn writes(&self) -> Vec<(String, String)> {
        match self {
            BootEnv::Uboot => Vec::new(),
            BootEnv::Memory(mem) => mem.writes(),
        }
    }

    /// 测试接缝：清空 env 写入记录。
    #[cfg(test)]
    pub(super) fn clear_writes(&self) {
        match self {
            BootEnv::Uboot => {}
            BootEnv::Memory(mem) => mem.clear_writes(),
        }
    }

    /// 测试接缝：预设一个 env 变量的值（模拟 bootloader 里已存在的变量）。
    #[cfg(test)]
    pub(super) fn set_var(&self, name: &str, value: &str) {
        match self {
            BootEnv::Uboot => {}
            BootEnv::Memory(mem) => mem.set_var(name, value),
        }
    }

    /// 测试接缝：`reboot` 调用次数。
    #[cfg(test)]
    pub(super) fn reboot_count(&self) -> usize {
        match self {
            BootEnv::Uboot => 0,
            BootEnv::Memory(mem) => mem.reboot_count(),
        }
    }
}

// ==================== 生产实现：真实 u-boot 工具 ====================

/// `fw_setenv <name> <value>`：写 bootloader 环境变量。
///
/// 退出码非零必须报错——env 分区只读 / 损坏时 `status()` 仍返回 Ok，
/// 继续往下走会得到「代码以为切换成功、设备重启后仍启动旧分区」的结果。
#[cfg(target_os = "linux")]
fn uboot_setenv(name: &str, value: &str) -> GatewayResult<()> {
    use std::process::Command;
    let status = Command::new("fw_setenv")
        .args([name, value])
        .status()
        .map_err(|e| GatewayError::Io(format!("fw_setenv {} spawn failed: {}", name, e)))?;
    if !status.success() {
        return Err(GatewayError::Io(format!(
            "fw_setenv {} failed: exit code {:?} (success={})",
            name,
            status.code(),
            status.success()
        )));
    }
    Ok(())
}

/// 非 Linux 无 u-boot 工具：写 env 视为成功（与历史 no-op 行为一致）。
#[cfg(not(target_os = "linux"))]
fn uboot_setenv(name: &str, value: &str) -> GatewayResult<()> {
    let _ = (name, value);
    tracing::debug!("OTA: bootloader env write skipped (not a Linux target)");
    Ok(())
}

/// `fw_printenv <name>` 的执行结果分类（**纯函数，跨平台可测**）。
///
/// 抽成纯函数的原因：`uboot_printenv` 只在 Linux 上编译，本机（Windows）
/// 无法覆盖其分支——而它恰恰决定「是否需要 A/B 回滚」这个变砖判定点。
#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PrintenvOutcome {
    /// 变量存在，值可用。
    Value(String),
    /// 变量不存在（u-boot 报 `not defined`）——这是正常情形，非故障。
    Undefined,
    /// 读取失败（工具不存在 / env 分区损坏 / 其它 IO 错误）——不可信。
    Unavailable,
}

/// 判定 `fw_printenv` 的执行结果（**纯函数**）。
///
/// - `spawn_ok`：命令是否成功启动（false = `fw_printenv` 未安装 / PATH 缺失）
/// - `success`：退出码是否为 0
/// - `stdout` / `stderr`：命令输出
///
/// 关键：`fw_printenv` 在「变量不存在」时也返回**非零退出码**，并在 stderr
/// 打印 `## Error: "bootcount" not defined`。这属于正常情形，不能当成故障。
/// 但真正需要区分的是「变量未定义」与「env 分区读取失败」——后者若被当成
/// 未定义，`bootcount` 会静默变成 `None` → `check_bootcount_rollback` 永远
/// 返回 false → **需要回滚的新分区被反复加载，设备变砖且无人察觉**。
#[cfg(any(target_os = "linux", test))]
fn classify_printenv(
    spawn_ok: bool,
    success: bool,
    stdout: &str,
    stderr: &str,
    name: &str,
) -> PrintenvOutcome {
    if !spawn_ok {
        return PrintenvOutcome::Unavailable;
    }
    if success {
        return match parse_printenv_output(stdout, name) {
            Some(v) => PrintenvOutcome::Value(v),
            None => PrintenvOutcome::Undefined,
        };
    }
    // 退出码非零：区分「变量未定义」与「读取失败」
    let combined = format!("{}{}", stdout, stderr);
    if combined.contains("not defined") || combined.contains("not found") {
        return PrintenvOutcome::Undefined;
    }
    PrintenvOutcome::Unavailable
}

/// `fw_printenv <name>`：读 bootloader 环境变量，返回变量值。
///
/// 兼容两种输出格式：`fw_printenv NAME` 既可能只打印裸值（`a`），
/// 也可能打印 `NAME=a`（整表打印时的行格式）。
#[cfg(target_os = "linux")]
fn uboot_printenv(name: &str) -> Option<String> {
    use std::process::Command;
    let output = match Command::new("fw_printenv").arg(name).output() {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!(
                "OTA: fw_printenv spawn failed ({}): {} — bootloader env is UNREADABLE; \
                 A/B rollback detection (bootcount) cannot be trusted",
                name,
                e
            );
            return None;
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    match classify_printenv(true, output.status.success(), &stdout, &stderr, name) {
        PrintenvOutcome::Value(v) => Some(v),
        PrintenvOutcome::Undefined => None,
        PrintenvOutcome::Unavailable => {
            tracing::error!(
                "OTA: fw_printenv {} failed (exit={:?}) — bootloader env read FAILED; \
                 bootcount/boot_partition are unreliable, A/B rollback detection is DISABLED",
                name,
                output.status.code()
            );
            None
        }
    }
}

/// 从 `fw_printenv` 输出里取出指定变量的值。
#[cfg(any(target_os = "linux", test))]
fn parse_printenv_output(stdout: &str, name: &str) -> Option<String> {
    let prefix = format!("{}=", name);
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix(&prefix) {
            let value = rest.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    // 裸值输出（`fw_printenv NAME` 只打印值本身）
    let trimmed = stdout.trim();
    if !trimmed.is_empty() && !trimmed.contains('\n') && !trimmed.contains('=') {
        return Some(trimmed.to_string());
    }
    None
}

/// 非 Linux 无 u-boot 工具：读 env 恒为 None。
#[cfg(not(target_os = "linux"))]
fn uboot_printenv(name: &str) -> Option<String> {
    let _ = name;
    None
}

/// `sync` 的处置决策（**纯函数，跨平台可测**）。
///
/// 抽成纯函数的原因：`uboot_sync` 只在 Linux 上编译，本机（Windows）
/// 无法覆盖其分支——而这恰恰是 OTA 回滚**变砖**的判定点。
#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncDecision {
    /// 已刷盘，继续后续流程。
    Flushed,
    /// `sync` 命令不存在：u-boot `fw_setenv` 自身已刷 env 分区，放行但留痕。
    Missing,
    /// `sync` 执行失败：env 可能未落盘 → 必须中止 reboot。
    Failed,
}

/// 判定 `sync` 的执行结果该如何处置（纯函数）。
///
/// - `spawn_ok`：`sync` 是否成功启动（false = 命令不存在 / 无法执行）
/// - `success`：`sync` 的退出码是否表示成功
#[cfg(any(target_os = "linux", test))]
fn classify_sync(spawn_ok: bool, success: bool) -> SyncDecision {
    if !spawn_ok {
        SyncDecision::Missing
    } else if success {
        SyncDecision::Flushed
    } else {
        SyncDecision::Failed
    }
}

/// `sync`：把 u-boot env 写入刷盘。
///
/// r15-OTA-SYNC (fix): 旧实现 `let _ = Command::new("sync").status();`
/// **完全忽略退出码**——`sync` 失败（磁盘 I/O 错误 / 只读根 / 进程被杀）时
/// 依然继续 `reboot()`，env 实际未落盘 → 重启后仍启动旧分区。
/// 在 A/B 回滚场景下这意味着「回滚以为成功 → 实际仍启动故障分区」→
/// 设备反复重启变砖。
///
/// 现在：`sync` 启动失败（命令不存在）→ 放行但 warn（`fw_setenv` 自身已刷
/// env 分区）；`sync` 退出码非零 → **返回 Err 中止 reboot**。
#[cfg(target_os = "linux")]
fn uboot_sync() -> GatewayResult<()> {
    use std::process::Command;
    let status = Command::new("sync").status();
    let (spawn_ok, success, code) = match &status {
        Ok(s) => (true, s.success(), s.code()),
        Err(_) => (false, false, None),
    };
    match classify_sync(spawn_ok, success) {
        SyncDecision::Flushed => Ok(()),
        SyncDecision::Missing => {
            tracing::warn!(
                "OTA: `sync` command unavailable — relying on fw_setenv's own env flush; \
                 bootloader env persistence cannot be confirmed"
            );
            Ok(())
        }
        SyncDecision::Failed => Err(GatewayError::Io(format!(
            "sync failed: exit code {:?} — bootloader env may not be persisted; \
             rebooting now would restart the OLD slot (rollback would silently fail)",
            code
        ))),
    }
}

/// 非 Linux：`sync` 无意义，no-op。
#[cfg(not(target_os = "linux"))]
fn uboot_sync() -> GatewayResult<()> {
    Ok(())
}

/// `reboot` 的执行结果分类（**纯函数，跨平台可测**）。
#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RebootOutcome {
    /// 已发起重启（或重启命令成功执行）。
    Initiated,
    /// `reboot` 命令不存在 / 无法启动——重启**没有发生**。
    SpawnFailed,
    /// `reboot` 执行了但退出码非零——重启**多半没有发生**。
    Failed,
}

/// 判定 `reboot` 的执行结果（**纯函数**）。
#[cfg(any(target_os = "linux", test))]
fn classify_reboot(spawn_ok: bool, success: bool) -> RebootOutcome {
    if !spawn_ok {
        RebootOutcome::SpawnFailed
    } else if success {
        RebootOutcome::Initiated
    } else {
        RebootOutcome::Failed
    }
}

/// `reboot`：重启设备使 env 生效。
///
/// r15-OTA-REBOOT (fix): 旧实现 `let _ = Command::new("reboot").status();`
/// **同时吞掉 spawn 失败与退出码**。在 systemd 管理的容器 / minimal rootfs 上
/// `reboot` 常常不在 PATH（或需要 root），此时 `perform_rollback` 以为重启成功
/// 返回 Ok，但设备**根本没有重启** → 继续运行故障分区 → 现场表现为
/// "设备重启后还是坏的"，而日志里没有任何线索。
///
/// 现在：spawn 失败或退出码非零都**上抛错误**，让调用方明确知道「重启未发生」。
/// 注意 `reboot` 成功时通常不会返回（进程被系统带下去），因此能走到
/// `status()` 返回就说明命令自身退出了。
#[cfg(target_os = "linux")]
fn uboot_reboot() -> GatewayResult<()> {
    use std::process::Command;
    let status = Command::new("reboot").status();
    let (spawn_ok, success, code) = match &status {
        Ok(s) => (true, s.success(), s.code()),
        Err(e) => {
            tracing::error!(
                "OTA: `reboot` spawn failed: {} — the device did NOT reboot; \
                 bootloader env changes (rollback) will NOT take effect until a manual reboot",
                e
            );
            return Err(GatewayError::Io(format!("reboot spawn failed: {}", e)));
        }
    };
    match classify_reboot(spawn_ok, success) {
        RebootOutcome::Initiated => Ok(()),
        RebootOutcome::SpawnFailed => Err(GatewayError::Io(
            "reboot spawn failed: device did not reboot".to_string(),
        )),
        RebootOutcome::Failed => Err(GatewayError::Io(format!(
            "reboot failed: exit code {:?} — the device did NOT reboot; \
             bootloader env changes (rollback) will NOT take effect until a manual reboot",
            code
        ))),
    }
}

/// 非 Linux 不支持 A/B 回滚重启：no-op 并留痕。
#[cfg(not(target_os = "linux"))]
fn uboot_reboot() -> GatewayResult<()> {
    tracing::warn!("A/B rollback not supported on non-Linux");
    Ok(())
}

// ==================== 测试实现：内存 bootloader 环境 ====================

/// 内存版 bootloader 环境：记录 env 写入、env 当前值与重启次数。
#[cfg(test)]
#[derive(Default)]
pub(super) struct MemoryBootEnv {
    /// env 变量当前值（供 `printenv` 读取）。
    vars: Mutex<HashMap<String, String>>,
    /// 全部 env 写入记录，顺序即调用顺序。
    writes: Mutex<Vec<(String, String)>>,
    /// `reboot` 调用次数（断言「回滚确实触发了重启」）。
    reboots: AtomicUsize,
}

#[cfg(test)]
impl MemoryBootEnv {
    /// 写 env：更新当前值并记录写入历史。
    fn setenv(&self, name: &str, value: &str) -> GatewayResult<()> {
        self.vars.lock().insert(name.to_string(), value.to_string());
        self.writes
            .lock()
            .push((name.to_string(), value.to_string()));
        Ok(())
    }

    /// 读 env 当前值。
    fn get_var(&self, name: &str) -> Option<String> {
        self.vars.lock().get(name).cloned()
    }

    /// 预设一个 env 变量的值（模拟 bootloader 里已存在的变量）。
    fn set_var(&self, name: &str, value: &str) {
        self.vars.lock().insert(name.to_string(), value.to_string());
    }

    /// 全部 env 写入记录（`(变量名, 值)`，顺序即调用顺序）。
    fn writes(&self) -> Vec<(String, String)> {
        self.writes.lock().clone()
    }

    /// 清空写入记录（用于「某操作之前的历史写入不参与断言」的场景）。
    fn clear_writes(&self) {
        self.writes.lock().clear();
    }

    /// `reboot` 调用次数。
    fn reboot_count(&self) -> usize {
        self.reboots.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `fw_printenv` 输出解析：兼容行格式（`NAME=value`）与裸值格式。
    #[test]
    fn parses_printenv_output_formats() {
        // 行格式：整表打印时的一行
        assert_eq!(
            parse_printenv_output("boot_partition=b\nbootcount=0\n", "boot_partition"),
            Some("b".to_string())
        );
        // 裸值格式：`fw_printenv NAME` 只打印值本身
        assert_eq!(
            parse_printenv_output("b\n", "boot_partition"),
            Some("b".to_string())
        );
        // 变量不存在 → None
        assert_eq!(
            parse_printenv_output("bootcount=0\n", "boot_partition"),
            None
        );
        // 其他变量名不误匹配（r31-OTA8：避免 `cat_a=...` 被判为 slot_a）
        assert_eq!(parse_printenv_output("cat_a=xyz\n", "boot_partition"), None);
    }

    /// 内存后端：写 env 后可读回；`bootcount` 解析为 u32。
    #[test]
    fn memory_backend_reads_back_written_env() {
        let env = MemoryBootEnv::default();
        env.setenv("boot_partition", "b").expect("setenv");
        assert_eq!(env.get_var("boot_partition"), Some("b".to_string()));
        assert_eq!(env.get_var("nope"), None);
        assert_eq!(env.writes().len(), 1);
        env.clear_writes();
        assert!(env.writes().is_empty());
    }

    /// r15-OTA-SYNC: `sync` 三种结果的处置判定。
    ///
    /// 关键断言：`sync` 退出码非零必须判为 `Failed`——这决定 OTA 回滚
    /// 是否中止 reboot（旧实现忽略退出码 → 回滚静默失效 → 设备变砖）。
    #[test]
    fn sync_outcomes_are_classified() {
        assert_eq!(classify_sync(true, true), SyncDecision::Flushed);
        assert_eq!(classify_sync(true, false), SyncDecision::Failed);
        assert_eq!(classify_sync(false, false), SyncDecision::Missing);
    }

    /// r15-OTA5: `fw_printenv` 结果必须区分「变量未定义」与「读取失败」。
    ///
    /// 关键断言：`not defined`（u-boot 的正常未定义提示）判为 `Undefined`，
    /// 其它失败一律判为 `Unavailable`——若把「读取失败」误判成「未定义」，
    /// `bootcount` 会静默变成 None → A/B 回滚判定彻底失效 → 变砖。
    #[test]
    fn printenv_undefined_is_distinguished_from_unavailable() {
        // spawn 失败（fw_printenv 未安装 / PATH 缺失）→ Unavailable
        assert_eq!(
            classify_printenv(false, false, "", "", "bootcount"),
            PrintenvOutcome::Unavailable
        );
        // spawn 失败但退出码为 true（防御性：spawn_ok 优先）→ Unavailable
        assert_eq!(
            classify_printenv(false, true, "1", "", "bootcount"),
            PrintenvOutcome::Unavailable
        );
        // 正常：变量存在（裸值格式）→ Value
        assert_eq!(
            classify_printenv(true, true, "3\n", "", "bootcount"),
            PrintenvOutcome::Value("3".to_string())
        );
        // 正常：变量存在（行格式）→ Value
        assert_eq!(
            classify_printenv(true, true, "bootcount=0\n", "", "bootcount"),
            PrintenvOutcome::Value("0".to_string())
        );
        // 退出码 0 但输出里没有该变量 → Undefined（等价于「未定义」）
        assert_eq!(
            classify_printenv(true, true, "other=1\n", "", "bootcount"),
            PrintenvOutcome::Undefined
        );
        // 退出码非零 + u-boot 的 "not defined" → Undefined（正常情形）
        assert_eq!(
            classify_printenv(
                true,
                false,
                "",
                "## Error: \"bootcount\" not defined",
                "bootcount"
            ),
            PrintenvOutcome::Undefined
        );
        // 退出码非零 + "not found" → Undefined
        assert_eq!(
            classify_printenv(true, false, "", "variable not found", "bootcount"),
            PrintenvOutcome::Undefined
        );
        // 退出码非零 + 其它错误（env 分区读取失败）→ Unavailable
        assert_eq!(
            classify_printenv(
                true,
                false,
                "",
                "## Error: unable to read environment",
                "bootcount"
            ),
            PrintenvOutcome::Unavailable
        );
    }

    /// r15-OTA6: `reboot` 结果必须区分「没重启」与「已重启」。
    ///
    /// 关键断言：spawn 失败与退出码非零都**不能**判为 `Initiated`——
    /// 否则 `perform_rollback` 返回 Ok 但设备根本没重启 → 继续跑故障分区。
    #[test]
    fn reboot_outcomes_are_classified() {
        assert_eq!(classify_reboot(false, false), RebootOutcome::SpawnFailed);
        assert_eq!(classify_reboot(false, true), RebootOutcome::SpawnFailed);
        assert_eq!(classify_reboot(true, true), RebootOutcome::Initiated);
        assert_eq!(classify_reboot(true, false), RebootOutcome::Failed);
    }
}
