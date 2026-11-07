//! 只读诊断命令的白名单与执行策略（`remote-shell` feature 门控）。
//!
//! O-6 从 `remote_diag.rs` 拆出：原文件 999 行，命令白名单是一块自成体系的
//! 安全策略（哪些命令允许跑、参数怎么校验、输出怎么截断），与诊断管理器的
//! 生命周期管理无关。
//!
//! 依赖方向：只依赖标准库 + tokio，不依赖 `crate::engine` 的任何模块。

use crate::engine::error::{GatewayError, GatewayResult};

/// r37-DIAG2：单条诊断命令输出上限（字节）。
///
/// `journalctl`（无限量）/`du`/`dmesg` 可产出数十 MB 输出，直接经 WebSocket/HTTP
/// 回传会撑爆内存与带宽。超出部分截断并追加提示。
#[cfg(feature = "remote-shell")]
pub(super) const MAX_CMD_OUTPUT_BYTES: usize = 1024 * 1024;

/// 白名单绕过修复：systemctl 变更类动词——只要在参数任意位置出现即拒绝。
///
/// systemctl 以“第一个非选项参数”作为动词，旧实现仅校验“是否含 status 等
/// 只读动词”，`systemctl restart nginx status` 即可绕过并实际执行 restart。
#[cfg(feature = "remote-shell")]
pub(super) const SYSTEMCTL_FORBIDDEN_VERBS: &[&str] = &[
    "start",
    "stop",
    "restart",
    "reload",
    "reload-or-restart",
    "try-restart",
    "force-reload",
    "condrestart",
    "enable",
    "disable",
    "mask",
    "unmask",
    "preset",
    "preset-all",
    "set-property",
    "set-environment",
    "unset-environment",
    "import-environment",
    "edit",
    "isolate",
    "kill",
    "reset-failed",
    "daemon-reload",
    "daemon-reexec",
    "halt",
    "poweroff",
    "reboot",
    "suspend",
    "hibernate",
    "hybrid-sleep",
    "switch-root",
    "rescue",
    "emergency",
    "cancel",
    "link",
    "revert",
    "freeze",
    "thaw",
];

/// 白名单绕过修复：ip 只读对象——第一个非选项参数必须为其一
#[cfg(feature = "remote-shell")]
pub(super) const IP_READONLY_OBJECTS: &[&str] =
    &["addr", "a", "route", "r", "link", "l", "li", "neigh", "n"];

/// 白名单绕过修复：ip 变更类动词——任意位置出现即拒绝
#[cfg(feature = "remote-shell")]
pub(super) const IP_FORBIDDEN_VERBS: &[&str] = &[
    "add",
    "del",
    "delete",
    "set",
    "change",
    "replace",
    "append",
    "flush",
    "up",
    "down",
    "rename",
    "promote",
    "demote",
    "netns",
    "vrf",
    "xfrm",
    "rule",
    "maddress",
    "tunnel",
    "tuntap",
    "fou",
    "macsec",
    "mptcp",
    "tcp_metrics",
    "token",
    "nexthop",
];

/// r37-DIAG2：截断超长命令输出（按 UTF-8 字符边界安全截断）。
#[cfg(feature = "remote-shell")]
pub(super) fn truncate_command_output(s: String) -> String {
    if s.len() <= MAX_CMD_OUTPUT_BYTES {
        return s;
    }
    let mut out = s;
    let mut end = MAX_CMD_OUTPUT_BYTES;
    while end > 0 && !out.is_char_boundary(end) {
        end -= 1;
    }
    out.truncate(end);
    out.push_str(
        "\n[... output truncated: command produced more than 1 MiB; narrow the command scope ...]",
    );
    out
}

/// 解析命令字符串为 (program, args)
#[cfg(feature = "remote-shell")]
pub(super) fn parse_command(cmd: &str) -> Result<(String, Vec<String>), String> {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    if parts.is_empty() {
        return Err("empty command".into());
    }
    let program = parts[0].to_string();
    let args = parts[1..].iter().map(|s| s.to_string()).collect();
    Ok((program, args))
}

/// 白名单校验：仅允许只读诊断命令
#[cfg(feature = "remote-shell")]
pub(super) fn validate_whitelist(program: &str, args: &[String]) -> GatewayResult<()> {
    // 禁止的危险字符（管道、重定向、命令替换）
    for arg in args {
        if arg.contains('|')
            || arg.contains('>')
            || arg.contains('<')
            || arg.contains(';')
            || arg.contains('&')
            || arg.contains('`')
            || arg.contains('$')
            || arg.contains('*')
        {
            return Err(GatewayError::Config(format!(
                "forbidden character in argument: {}",
                arg
            )));
        }
    }

    // 允许的命令白名单
    //
    // 白名单绕过修复：移除 `cat` / `ls` / `mount`。
    // - `cat` 可读取任意文件（/etc/shadow、私钥等），是主要信息泄露通道；
    // - `ls` 可枚举任意目录（信息泄露）；
    // - `mount` 会修改挂载表并能挂载任意设备，属写操作。
    // 远程诊断面向状态/健康观测（df/free/uptime/top/ps/dmesg/netstat/ss/lsof/
    // iostat/vmstat/du/journalctl/systemctl）；若运维确需浏览文件，应另行提供
    // 带路径囚笼的受限读接口，而非放开通用 cat/ls。
    const ALLOWED: &[&str] = &[
        "df",
        "free",
        "uptime",
        "top",
        "ps",
        "dmesg",
        "ip",
        "uname",
        "date",
        "ping",
        "netstat",
        "ss",
        "lsof",
        "iostat",
        "vmstat",
        "du",
        "journalctl",
        "systemctl",
        "gpio",
        "modbus",
        "serial",
    ];

    let base = program.rsplit('/').next().unwrap_or(program);
    if !ALLOWED.contains(&base) {
        return Err(GatewayError::Config(format!(
            "command not in whitelist: {}",
            base
        )));
    }

    // Bug 150: 修复白名单绕过——旧实现仅按 basename 校验
    // （program.rsplit('/').next()），但 exec_command 却以完整路径
    // Command::new(program) 执行，因此 "/tmp/evil/df" 能借 basename "df"
    // 通过校验并实际运行任意被投放的二进制。
    // 现对含路径分隔符的命令做 canonicalize（解析 symlink、"." / ".."），
    // 并要求最终可执行文件必须位于可信可执行目录内，从来源上限定执行目标。
    if program.contains('/') {
        const ALLOWED_DIRS: &[&str] = &[
            "/usr/bin",
            "/bin",
            "/usr/sbin",
            "/sbin",
            "/usr/local/bin",
            "/usr/local/sbin",
        ];
        let canon = std::fs::canonicalize(program).map_err(|e| {
            GatewayError::Config(format!("cannot resolve command path '{}': {}", program, e))
        })?;
        let dir = canon.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let dir_str = dir.to_string_lossy();
        if !ALLOWED_DIRS.iter().any(|d| dir_str.as_ref() == *d) {
            return Err(GatewayError::Config(format!(
                "command path '{}' is outside allowed executable directories",
                program
            )));
        }
    }

    // 特定命令的额外限制
    match base {
        "systemctl" => {
            // 白名单绕过修复：仅允许“第一个非选项参数”为只读动词。
            // 旧实现只要求“任一参数 == status”，`systemctl restart nginx status`
            // 会通过校验（systemctl 取首参为动词 → 实际执行 restart）。
            const READONLY_VERBS: &[&str] = &["status", "is-enabled", "list-units"];
            match args.iter().find(|a| !a.starts_with('-')) {
                Some(v) if READONLY_VERBS.contains(&v.as_str()) => {}
                _ => {
                    return Err(GatewayError::Config(
                        "systemctl only allows a read-only verb \
                         (status/is-enabled/list-units) as the first argument"
                            .into(),
                    ))
                }
            }
            // 双保险：任意位置出现变更类动词一律拒绝
            if let Some(bad) = args
                .iter()
                .find(|a| SYSTEMCTL_FORBIDDEN_VERBS.contains(&a.as_str()))
            {
                return Err(GatewayError::Config(format!(
                    "systemctl forbidden verb: {}",
                    bad
                )));
            }
        }
        "ip" => {
            // 白名单绕过修复：仅允许只读对象（addr/route/link/neigh），
            // 且禁止变更类动词（add/del/set/up/down/flush...），避免
            // `ip addr add` / `ip link set` / `ip route del` 改网络配置。
            match args.iter().find(|a| !a.starts_with('-')) {
                Some(o) if IP_READONLY_OBJECTS.contains(&o.as_str()) => {}
                _ => {
                    return Err(GatewayError::Config(
                        "ip only allows read-only objects (addr/route/link/neigh)".into(),
                    ))
                }
            }
            if let Some(bad) = args.iter().find(|a| IP_FORBIDDEN_VERBS.contains(&a.as_str())) {
                return Err(GatewayError::Config(format!(
                    "ip write operation forbidden: {}",
                    bad
                )));
            }
        }
        "date" => {
            // 白名单绕过修复：禁止设置系统时间（date -s / --set*）
            if args.iter().any(|a| a == "-s" || a.starts_with("--set")) {
                return Err(GatewayError::Config(
                    "date -s/--set (set system clock) is forbidden".into(),
                ));
            }
        }
        "journalctl" => {
            // 禁止 --vacuum-time/--rotate/--flush/--sync 等写操作
            if args.iter().any(|a| {
                a.starts_with("--vacuum") || a == "--rotate" || a == "--flush" || a == "--sync"
            }) {
                return Err(GatewayError::Config(
                    "journalctl write operations forbidden".into(),
                ));
            }
        }
        "dmesg"
            // 禁止 -w/--follow（持续跟踪）与 -c/-C/--clear（清空内核环形缓冲区）
            if args.iter().any(|a| {
                a == "-w"
                    || a == "--follow"
                    || a == "-c"
                    || a == "-C"
                    || a == "--clear"
                    || a == "--read-clear"
            }) => {
                return Err(GatewayError::Config(
                    "dmesg -w/--follow/clear forbidden".into(),
                ));
            }
        _ => {}
    }

    Ok(())
}
