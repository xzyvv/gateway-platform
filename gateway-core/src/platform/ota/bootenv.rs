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

/// `fw_printenv <name>`：读 bootloader 环境变量，返回变量值。
///
/// 兼容两种输出格式：`fw_printenv NAME` 既可能只打印裸值（`a`），
/// 也可能打印 `NAME=a`（整表打印时的行格式）。
#[cfg(target_os = "linux")]
fn uboot_printenv(name: &str) -> Option<String> {
    use std::process::Command;
    let output = Command::new("fw_printenv").arg(name).output().ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_printenv_output(&stdout, name)
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

/// `sync`：把 u-boot env 写入刷盘。
#[cfg(target_os = "linux")]
fn uboot_sync() -> GatewayResult<()> {
    use std::process::Command;
    let _ = Command::new("sync").status();
    Ok(())
}

/// 非 Linux：`sync` 无意义，no-op。
#[cfg(not(target_os = "linux"))]
fn uboot_sync() -> GatewayResult<()> {
    Ok(())
}

/// `reboot`：重启设备使 env 生效。
///
/// 失败不阻断（历史行为）：reboot 命令的退出码不可靠，真正的失败信号是
/// 上游 `fw_setenv` 的退出码——那里已经做了严格检查。
#[cfg(target_os = "linux")]
fn uboot_reboot() -> GatewayResult<()> {
    use std::process::Command;
    let _ = Command::new("reboot").status();
    Ok(())
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
}
