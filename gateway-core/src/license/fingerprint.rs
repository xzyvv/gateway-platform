//! 机器指纹采集与分层比对（§6）
//!
//! 字段分层（与签发工具 license-issuer 共用同一套哈希函数，杜绝两侧漂移）：
//!
//! | 层 | 字段 | 来源 | 角色 |
//! |---|---|---|---|
//! | 主键 | `machine_key` | Linux `/etc/machine-id`、Windows `MachineGuid` | 极高稳定，命中即通过 |
//! | 回退1 | 板载 MAC | 物理网卡枚举（过滤蜂窝/虚拟网卡） | 中稳定 |
//! | 回退2 | 存储序列号 | eMMC CID / NVMe serial | 出厂烧写 |
//! | 回退3 | 设备序列号 | devicetree serial-number / SMBIOS board_serial | 部分板缺失，自适应 |
//!
//! 校验规则：主键命中 → 通过；否则按"实际可比对项 n"做动态阈值
//! `匹配数 ≥ ⌈n×2/3⌉`（字段缺失的板子不会因少一项反而更易被拒）。
//!
//! 隐私：所有字段仅以 sha256 哈希形态存在，原始值不落盘、不进 payload、不外传。

use std::path::Path;

/// 本机采集到的指纹（均为 sha256 hex；None = 该项缺失，自适应跳过）
#[derive(Debug, Clone, Default, PartialEq, Eq, ts_rs::TS)]
#[ts(export, export_to = "LocalFingerprints.ts")]
pub struct LocalFingerprints {
    /// 主键：machine-id / MachineGuid 哈希
    pub machine_key: Option<String>,
    /// 回退1：板载 MAC 哈希
    pub onboard_mac: Option<String>,
    /// 回退2：存储序列号哈希
    pub storage_serial: Option<String>,
    /// 回退3：设备序列号哈希
    pub device_serial: Option<String>,
}

/// 标识类字段规范化：去首尾空白 + 小写（machine-id / 序列号通用）
pub fn normalize_id(v: &str) -> String {
    v.trim().to_lowercase()
}

/// MAC 规范化：小写并去除 `:` `-` `.` 分隔符（`AA-BB-CC-DD-EE-FF` → `aabbccddeeff`）
pub fn normalize_mac(v: &str) -> String {
    v.trim()
        .to_lowercase()
        .chars()
        .filter(|c| !matches!(c, ':' | '-' | '.' | ' '))
        .collect()
}

/// sha256 hex（复用仓库自带实现）
pub fn sha256_hex(data: &[u8]) -> String {
    crate::crypto::sha256_bytes(data)
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

/// 标识字段 → 哈希（空串视为缺失返回 None）
pub fn hash_id_field(raw: &str) -> Option<String> {
    let v = normalize_id(raw);
    (!v.is_empty()).then(|| sha256_hex(v.as_bytes()))
}

/// MAC 字段 → 哈希（空串视为缺失返回 None）
pub fn hash_mac_field(raw: &str) -> Option<String> {
    let v = normalize_mac(raw);
    (!v.is_empty()).then(|| sha256_hex(v.as_bytes()))
}

/// payload 整体指纹 `machine`：规范化字段串 `key|p0|p1|p2` 的 sha256 hex。
/// 仅随 payload 携带用于审计与远期加权计分，v1 比对逻辑不使用（见 §6 决策记录）。
pub fn compute_overall(machine_key: &str, parts: &[&str; 3]) -> String {
    let joined = format!("{}|{}|{}|{}", machine_key, parts[0], parts[1], parts[2]);
    sha256_hex(joined.as_bytes())
}

/// 分层比对（§6 校验规则）。
///
/// ① 主键命中 → 通过；
/// ② 主键不一致或缺失：对 3 个回退项，统计**两侧均非空**的可比对项 n，
///    匹配数 ≥ ⌈n×2/3⌉ → 通过；无可比对项 → 拒绝（fail-closed）。
pub fn compare(local: &LocalFingerprints, payload_key: &str, payload_parts: &[String]) -> bool {
    // ① 主键优先
    if !payload_key.is_empty() && local.machine_key.as_deref() == Some(payload_key) {
        return true;
    }
    // ② 回退动态阈值
    let local_parts = [
        &local.onboard_mac,
        &local.storage_serial,
        &local.device_serial,
    ];
    let mut n = 0usize;
    let mut matched = 0usize;
    for (i, p) in payload_parts.iter().enumerate() {
        let payload_hash = p.as_str();
        if payload_hash.is_empty() {
            continue; // 签发时该项缺失
        }
        let Some(local_hash) = local_parts.get(i).and_then(|o| o.as_deref()) else {
            continue; // 本机该项缺失，不可比对
        };
        n += 1;
        if local_hash == payload_hash {
            matched += 1;
        }
    }
    n > 0 && matched >= (2 * n).div_ceil(3)
}

// ===== 采集（平台相关）=====

/// 采集本机指纹。生产入口：Linux 走 sysfs，Windows 走注册表 MachineGuid。
pub fn collect() -> LocalFingerprints {
    #[cfg(target_os = "windows")]
    {
        collect_windows()
    }
    #[cfg(not(target_os = "windows"))]
    {
        collect_linux(Path::new("/"))
    }
}

/// 读文件 → 规范化（去空白 + 截断 NUL）。失败返回 None。
fn read_trimmed(path: &Path) -> Option<String> {
    let s = std::fs::read_to_string(path).ok()?;
    let trimmed: String = s.trim_end_matches('\0').trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

/// Linux 采集。`root` 正常为 `/`，测试可注入临时目录构造伪 sysfs。
pub fn collect_linux(root: &Path) -> LocalFingerprints {
    // 主键：/etc/machine-id（容器内为随机值，部署须从宿主机挂载，见 §6 实施期注意）
    let machine_key = read_trimmed(&root.join("etc/machine-id"))
        .or_else(|| read_trimmed(&root.join("var/lib/dbus/machine-id")))
        .as_deref()
        .and_then(hash_id_field);

    // 回退1：板载 MAC（过滤虚拟 + 蜂窝，多网卡取接口名字典序第一个，保证确定性）
    let onboard_mac = enumerate_onboard_mac(root)
        .as_deref()
        .and_then(hash_mac_field);

    // 回退2：存储序列号（eMMC CID → NVMe serial，先到先得）
    let storage_serial = read_trimmed(&root.join("sys/block/mmcblk0/device/cid"))
        .or_else(|| read_trimmed(&root.join("sys/block/nvme0n1/device/serial")))
        .as_deref()
        .and_then(hash_id_field);

    // 回退3：设备序列号（ARM devicetree → x86 SMBIOS board_serial）
    let device_serial = read_trimmed(&root.join("proc/device-tree/serial-number"))
        .or_else(|| read_trimmed(&root.join("sys/class/dmi/id/board_serial")))
        .as_deref()
        .and_then(hash_id_field);

    LocalFingerprints {
        machine_key,
        onboard_mac,
        storage_serial,
        device_serial,
    }
}

/// Windows 采集：主键 MachineGuid（注册表）。回退项 v1 不采集
/// （Windows 部署形态为桌面端/开发机，主键已足够；reg.exe 拿盘序号不可靠）。
#[cfg(target_os = "windows")]
fn collect_windows() -> LocalFingerprints {
    let machine_key = read_machine_guid().as_deref().and_then(hash_id_field);
    LocalFingerprints {
        machine_key,
        ..Default::default()
    }
}

/// 读注册表 MachineGuid（reg.exe 子进程，启动/每日各一次，非热路径）。
#[cfg(target_os = "windows")]
fn read_machine_guid() -> Option<String> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("reg")
        .args(["query", r"HKLM\SOFTWARE\Microsoft\Cryptography", "/v", "MachineGuid"])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    // 匹配 "MachineGuid    REG_SZ    <value>"
    text.lines()
        .find(|l| l.contains("MachineGuid"))
        .and_then(|l| l.split("REG_SZ").nth(1))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// 蜂窝/虚拟网卡接口名过滤（§6：蜂窝模块插拔、Docker 重建都会改变这些 MAC）。
/// 前缀匹配 wwan*/wwx*（蜂窝）、docker*/veth*/br-*/virbr*/tun*/tap*/wg*（虚拟）、
/// usb*（RNDIS/NCM 蜂窝共享网卡）；lo 无 device 挂载点自然被物理性检查排除。
fn is_excluded_interface(name: &str) -> bool {
    name == "lo"
        || [
            "docker", "veth", "virbr", "tun", "tap", "wg", "wwan", "wwx", "usb", "vmnet",
        ]
        .iter()
        .any(|p| name.starts_with(p))
        || name.starts_with("br-")
}

/// 枚举板载物理网卡 MAC：接口名过滤 + `/sys/class/net/<n>/device` 存在性
/// （虚拟网卡/lo 无对应设备节点）。多网卡取接口名字典序第一个，保证确定性。
fn enumerate_onboard_mac(root: &Path) -> Option<String> {
    let net_dir = root.join("sys/class/net");
    let mut names: Vec<String> = std::fs::read_dir(&net_dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !is_excluded_interface(n))
        // 物理性检查：有 device 节点才算板载网卡
        .filter(|n| net_dir.join(n).join("device").exists())
        .collect();
    names.sort();
    let first = names.first()?;
    read_trimmed(&net_dir.join(first).join("address"))
}

#[cfg(test)]
mod tests;
