//! 远程诊断通道（F5）
//!
//! 工业现场远程运维能力：
//! - 日志实时推送：通过 WebSocket tail 网关日志文件，实时推送到运维端
//! - 只读 Shell：白名单命令执行（df/free/top/ip/dmesg 等），禁止写操作
//!
//! **只读 Shell 默认关闭**：该能力受 `remote-shell` feature 门控。发行版只保留
//! 日志 tail / 归档下载这些"看"的能力，`POST /api/diagnostics/exec` 在默认构建下
//! 直接返回"功能未启用"，不发帧、不起子进程。需要现场远程排障时显式
//! `cargo build --features remote-shell` 打开（相关的白名单/截断/超时加固代码
//! 也随之一起编译进来，不会因关闭而削弱）。
//!
//! 安全设计：
//! - 所有命令走白名单，参数经过严格校验
//! - 禁止 rm/mv/dd/重定向 等写操作
//! - 超时保护（默认 10s）
//! - 白名单绕过修复：systemctl 仅允许**第一个非选项参数**为只读动词
//!   （status/is-enabled/list-units）；ip 仅允许只读对象并禁止变更动词；
//!   移除可读任意文件的 `cat`/`ls` 与可写挂载的 `mount`；禁用 `date -s` /
//!   `dmesg` 清空环形缓冲区等写选项。

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;

#[cfg(feature = "remote-shell")]
use crate::engine::error::{GatewayError, GatewayResult};

/// 日志广播通道容量
const LOG_BUFFER: usize = 1024;
/// 命令执行超时（秒）：Bug 50 修复——常量化超时并实际应用到 exec_command，
/// 防止 `ping` / `journalctl` 等无超时命令卡死诊断调用。
#[cfg(feature = "remote-shell")]
pub(super) const CMD_TIMEOUT_SECS: u64 = 10;

/// r37-DIAG1：日志 tail 单次最多读取的字节数。
///
/// 旧实现用 `read_to_end` 一次性读取全部新增内容——长时间运行的网关日志可达
/// 数百 MB，启动 / 日志轮转后会把这部分整体载入内存并逐行广播，造成内存尖峰。
/// 改为分块读取，游标按**实际消费字节**推进，剩余部分下一轮继续。
const TAIL_READ_CHUNK: usize = 256 * 1024;

/// r37-DIAG1：首次读取 / 日志轮转后最多回看的字节数。
///
/// 只 tail 最近 1 MiB，避免启动时把整份历史日志推给运维端（既无意义又昂贵）。
const TAIL_MAX_LOOKBACK: u64 = 1024 * 1024;

/// r37-DIAG2：单条诊断命令输出上限（字节）。
///
/// `journalctl`（无限量）/`du`/`dmesg` 可产出数十 MB 输出，直接经 WebSocket/HTTP
/// 回传会撑爆内存与带宽。超出部分截断并追加提示。
#[cfg(feature = "remote-shell")]
const MAX_CMD_OUTPUT_BYTES: usize = 1024 * 1024;

/// 白名单绕过修复：systemctl 变更类动词——只要在参数任意位置出现即拒绝。
///
/// systemctl 以“第一个非选项参数”作为动词，旧实现仅校验“是否含 status 等
/// 只读动词”，`systemctl restart nginx status` 即可绕过并实际执行 restart。
#[cfg(feature = "remote-shell")]
const SYSTEMCTL_FORBIDDEN_VERBS: &[&str] = &[
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
const IP_READONLY_OBJECTS: &[&str] = &["addr", "a", "route", "r", "link", "l", "li", "neigh", "n"];

/// 白名单绕过修复：ip 变更类动词——任意位置出现即拒绝
#[cfg(feature = "remote-shell")]
const IP_FORBIDDEN_VERBS: &[&str] = &[
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

/// 一次性日志快照最多回看的字节数（512 KiB）。
///
/// 与 `TAIL_MAX_LOOKBACK`（1 MiB，流式起点）分开取值：这里是"故障后立刻取证"，
/// 回看量越小越快出结果，512 KiB 约合数千行，足够覆盖一次现场故障窗口。
const SNAPSHOT_MAX_BYTES: u64 = 512 * 1024;

/// 按日期归档的日志文件条目。
///
/// 日志文件由 `tracing_appender` 以 `Rotation::DAILY` 滚动写入，
/// 文件名形如 `gateway.log.2026-10-29`（`<base>.<YYYY-MM-DD>`）。
/// 列表来自真实日志目录扫描，不做任何推算。
#[derive(Debug, Clone, serde::Serialize)]
pub struct LogArchiveInfo {
    /// `YYYY-MM-DD`；无日期后缀的当期文件取 mtime 的本地日期
    pub date: String,
    /// 文件字节数
    pub size_bytes: u64,
    /// 是否为当期正在写入的文件（即无日期后缀的那一个）
    pub current: bool,
}

/// 拆分日志路径为（目录, 文件名前缀）。
///
/// `.../gateway.log` → (`...`, `gateway.log`)，用于按 `<base>.<date>` 规则
/// 定位同目录下的滚动文件。
fn split_log_path(path: &str) -> (std::path::PathBuf, String) {
    let p = std::path::Path::new(path);
    let dir = match p.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => std::path::PathBuf::from("."),
    };
    let base = p
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "gateway.log".to_string());
    (dir, base)
}

/// 是否为严格的 `YYYY-MM-DD` 日期后缀（拒绝 `../` 等路径穿越写法）
fn is_date_suffix(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// 指定日期的归档文件路径（日期非法时返回 None）
fn archive_path(path: &str, date: &str) -> Option<std::path::PathBuf> {
    if !is_date_suffix(date) {
        return None;
    }
    let (dir, base) = split_log_path(path);
    Some(dir.join(format!("{base}.{date}")))
}

/// 扫描日志目录，返回真实存在的按日归档文件列表（新日期在前）。
fn list_log_archives_blocking(path: &str) -> Vec<LogArchiveInfo> {
    let (dir, base) = split_log_path(path);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let dated_prefix = format!("{base}.");
    let mut out: Vec<LogArchiveInfo> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let (date, current) = if name == base {
            // 当期文件没有日期后缀，用修改时间推断所属日期
            let mtime = meta
                .modified()
                .ok()
                .map(chrono::DateTime::<chrono::Utc>::from);
            match mtime {
                Some(t) => (
                    t.with_timezone(&chrono::Local)
                        .format("%Y-%m-%d")
                        .to_string(),
                    true,
                ),
                None => continue,
            }
        } else if let Some(rest) = name.strip_prefix(&dated_prefix) {
            if !is_date_suffix(rest) {
                continue;
            }
            (rest.to_string(), false)
        } else {
            continue;
        };
        out.push(LogArchiveInfo {
            date,
            size_bytes: meta.len(),
            current,
        });
    }
    // 日期倒序；同日重复（当期文件与滚动文件同名日期）时优先保留当期文件
    out.sort_by(|a, b| b.date.cmp(&a.date).then(b.current.cmp(&a.current)));
    out.dedup_by(|a, b| a.date == b.date);
    out
}

/// 当期日志文件不存在时，退回到目录里最新的一个归档文件。
///
/// 滚动写入下磁盘上只有 `gateway.log.<date>`，而诊断配置的路径是
/// `gateway.log`；不退这一步会读到不存在的文件并返回空日志。
fn newest_log_file(path: &str) -> Option<std::path::PathBuf> {
    let (dir, base) = split_log_path(path);
    let dated_prefix = format!("{base}.");
    let entries = std::fs::read_dir(&dir).ok()?;
    let mut best: Option<(String, std::path::PathBuf)> = None;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(rest) = name.strip_prefix(&dated_prefix) else {
            continue;
        };
        if !is_date_suffix(rest) {
            continue;
        }
        if !entry.metadata().map(|m| m.is_file()).unwrap_or(false) {
            continue;
        }
        if best
            .as_ref()
            .map(|(d, _)| rest > d.as_str())
            .unwrap_or(true)
        {
            best = Some((rest.to_string(), entry.path()));
        }
    }
    best.map(|(_, p)| p)
}

/// 同步读取日志文件末尾 `max_lines` 行（由 `Diagnostics::tail_lines` 放进
/// `spawn_blocking` 调用，避免同步文件 IO 卡住 tokio worker）。
fn tail_lines_blocking(path: &str, max_lines: usize) -> Vec<String> {
    let direct = std::path::Path::new(path);
    let target = if direct.is_file() {
        direct.to_path_buf()
    } else {
        match newest_log_file(path) {
            Some(p) => p,
            None => return Vec::new(),
        }
    };
    read_tail(&target, max_lines)
}

/// 读取指定文件的末尾 `max_lines` 行
fn read_tail(path: &std::path::Path, max_lines: usize) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if len == 0 {
        return Vec::new();
    }
    let start = len.saturating_sub(SNAPSHOT_MAX_BYTES);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::new();
    if (&mut file)
        .take(SNAPSHOT_MAX_BYTES)
        .read_to_end(&mut buf)
        .is_err()
    {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<&str> = text.lines().collect();
    // 从文件中间起读时首行可能是半行，丢掉以免导出的日志里出现残缺记录
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    let skip = lines.len().saturating_sub(max_lines);
    lines.into_iter().skip(skip).map(str::to_string).collect()
}

/// 诊断管理器
pub struct Diagnostics {
    /// 日志广播发送端
    log_tx: broadcast::Sender<String>,
    /// 日志文件路径
    log_path: String,
}

impl Diagnostics {
    /// 创建诊断管理器并启动日志 tail 任务
    pub fn new(log_path: &str) -> Arc<Self> {
        let (log_tx, _rx) = broadcast::channel::<String>(LOG_BUFFER);
        let this = Arc::new(Self {
            log_tx,
            log_path: log_path.to_string(),
        });

        // 启动日志 tail 后台任务
        let this_clone = this.clone();
        crate::engine::supervision::spawn_supervised("log_tail", move || {
            let this_clone = this_clone.clone();
            async move {
                this_clone.tail_loop().await;
            }
        });

        this
    }

    /// 订阅日志流
    pub fn subscribe_logs(&self) -> broadcast::Receiver<String> {
        self.log_tx.subscribe()
    }

    /// 读取日志文件末尾 `max_lines` 行，供诊断包一次性取证。
    ///
    /// 与 [`Self::tail_loop`] 的流式追加互补：订阅方只能拿到订阅之后的新行，
    /// 而现场排障往往是在故障**之后**才来取日志，需要"此刻文件里有什么"。
    /// 文件不存在 / 无权限 / IO 出错时返回空列表——不因为日志缺失让整次导出失败。
    pub async fn tail_lines(&self, max_lines: usize) -> Vec<String> {
        if max_lines == 0 {
            return Vec::new();
        }
        let path = self.log_path.clone();
        tokio::task::spawn_blocking(move || tail_lines_blocking(&path, max_lines))
            .await
            .unwrap_or_default()
    }

    /// 列出日志目录下真实存在的按日归档文件（新日期在前）。
    ///
    /// 供「日志归档」页面取列表：此前前端只能在 localStorage 里造 7 条空记录，
    /// 现场排障时无从确认网关到底留了哪几天的日志。
    pub async fn log_archives(&self) -> Vec<LogArchiveInfo> {
        let path = self.log_path.clone();
        tokio::task::spawn_blocking(move || list_log_archives_blocking(&path))
            .await
            .unwrap_or_default()
    }

    /// 读取某个日期归档文件的末尾 `max_lines` 行。
    ///
    /// 日期非法（非 `YYYY-MM-DD`）或文件不存在时返回空列表，调用方按「该日无日志」提示。
    pub async fn tail_lines_of_date(&self, date: &str, max_lines: usize) -> Vec<String> {
        if max_lines == 0 {
            return Vec::new();
        }
        let Some(target) = archive_path(&self.log_path, date) else {
            return Vec::new();
        };
        tokio::task::spawn_blocking(move || read_tail(&target, max_lines))
            .await
            .unwrap_or_default()
    }

    /// 日志 tail 循环：定期读取日志文件新增内容并广播
    ///
    /// r37-DIAG1 修复（三处）：
    /// 1. 单次读取上限 `TAIL_READ_CHUNK`——旧实现 `read_to_end` 会把全部新增内容
    ///    一次性载入内存，长时间运行（数百 MB 日志）时造成内存尖峰；
    /// 2. 首次读取 / 轮转后最多回看 `TAIL_MAX_LOOKBACK`——避免启动时载入整份历史日志；
    /// 3. 游标按**实际消费字节**推进（旧实现用读取前的 `file_len` 赋值：若读取期间
    ///    文件继续增长，多出的字节已被广播却未计入游标，下一轮会重复广播），
    ///    且只推进到最后一个换行处，避免把半行日志拆成两条。
    async fn tail_loop(&self) {
        use std::fs::File;
        use std::io::{Read, Seek, SeekFrom};

        let mut last_pos: u64 = 0;

        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;

            let mut file = match File::open(&self.log_path) {
                Ok(f) => f,
                Err(_) => continue, // 文件不存在时跳过
            };

            // 获取当前文件大小
            let metadata = match file.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let file_len = metadata.len();

            // 文件被截断（日志轮转），从头开始
            if file_len < last_pos {
                last_pos = 0;
            }

            if file_len <= last_pos {
                continue;
            }

            // r37-DIAG1(2)：首次 / 轮转后避免回看整份历史日志
            last_pos = tail_resume_offset(last_pos, file_len);

            if file.seek(SeekFrom::Start(last_pos)).is_err() {
                continue;
            }
            // r37-DIAG1(1)：分块读取，内存占用有上界
            let mut buf = Vec::new();
            if file
                .take(TAIL_READ_CHUNK as u64)
                .read_to_end(&mut buf)
                .is_err()
            {
                continue;
            }
            if buf.is_empty() {
                continue;
            }

            // r37-DIAG1(3)：只消费到最后一个换行为止，游标按实际消费字节推进
            let consumed = tail_consumable(&buf);
            let text = String::from_utf8_lossy(&buf[..consumed]);
            for line in text.lines() {
                if !line.is_empty() {
                    let _ = self.log_tx.send(line.to_string());
                }
            }
            last_pos += consumed as u64;
        }
    }

    /// 执行只读诊断命令（白名单）
    ///
    /// 返回命令输出（stdout+stderr）。非白名单命令返回错误。
    /// Bug 50: 常量化超时，避免 `ping` / `journalctl` 等无超时命令永久阻塞。
    /// Bug 165: 原实现用 `spawn_blocking` 包裹同步 `Command::output()`——
    /// 同步子进程调用无法取消，`timeout` 只是"放弃等待"，被占用的 blocking
    /// 线程仍在跑；反复超时会耗尽 blocking 线程池，连带队列的 SQLite 写
    /// （同样走 spawn_blocking）一起卡死。改用 `tokio::process::Command` +
    /// `kill_on_drop(true)`：超时使 `wait_with_output()` 的 future 被 drop，
    /// child 随之 drop 从而真正 kill 子进程，且等待不再占用 blocking 线程。
    #[cfg(feature = "remote-shell")]
    pub async fn exec_command(&self, cmd: &str) -> GatewayResult<String> {
        let (program, args) = parse_command(cmd)?;
        validate_whitelist(&program, &args)?;

        let child = tokio::process::Command::new(&program)
            .args(&args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| GatewayError::Io(format!("command spawn failed: {}", e)))?;

        let output = match tokio::time::timeout(
            Duration::from_secs(CMD_TIMEOUT_SECS),
            child.wait_with_output(),
        )
        .await
        {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => return Err(GatewayError::Io(format!("command failed: {}", e))),
            // 超时分支：future 被 drop → child 被 drop → kill_on_drop 终止子进程
            Err(_elapsed) => {
                return Err(GatewayError::Io(format!(
                    "command timed out after {}s",
                    CMD_TIMEOUT_SECS
                )));
            }
        };

        let mut result = String::from_utf8_lossy(&output.stdout).to_string();
        if !output.stderr.is_empty() {
            result.push_str(&String::from_utf8_lossy(&output.stderr));
        }
        // r37-DIAG2：输出上限保护，避免无限量命令输出撑爆诊断通道
        Ok(truncate_command_output(result))
    }
}

/// r37-DIAG1：日志 tail 游标推进前，计算本次宜消费的字节数。
///
/// 只消费到**最后一个换行**为止，剩余（半行）留给下一轮与后续内容拼接，
/// 避免把半行日志拆成两条广播；整块无换行（单行超长 > 单次读取上限）时
/// 整块消费，保证游标一定前进、不死循环。
fn tail_consumable(buf: &[u8]) -> usize {
    match buf.iter().rposition(|&b| b == b'\n') {
        Some(i) => i + 1,
        None => buf.len(),
    }
}

/// r37-DIAG1：计算本次 tail 的起始偏移。
///
/// 首次读取 / 日志轮转后 `last_pos` 为 0，此时若文件很大，只回看末尾
/// `TAIL_MAX_LOOKBACK` 字节，避免启动时把整份历史日志载入内存并广播给运维端。
fn tail_resume_offset(last_pos: u64, file_len: u64) -> u64 {
    if last_pos == 0 && file_len > TAIL_MAX_LOOKBACK {
        file_len - TAIL_MAX_LOOKBACK
    } else {
        last_pos
    }
}

/// r37-DIAG2：截断超长命令输出（按 UTF-8 字符边界安全截断）。
#[cfg(feature = "remote-shell")]
fn truncate_command_output(s: String) -> String {
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
fn parse_command(cmd: &str) -> Result<(String, Vec<String>), String> {
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
fn validate_whitelist(program: &str, args: &[String]) -> GatewayResult<()> {
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

#[cfg(test)]
mod tests {
    use super::{
        archive_path, is_date_suffix, list_log_archives_blocking, newest_log_file, tail_consumable,
        tail_resume_offset, TAIL_MAX_LOOKBACK,
    };
    #[cfg(feature = "remote-shell")]
    use super::{parse_command, truncate_command_output, validate_whitelist, MAX_CMD_OUTPUT_BYTES};

    #[cfg(feature = "remote-shell")]
    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    // ===== 白名单绕过修复：systemctl 首参语义 =====

    /// 漏洞复现：旧实现“任一参数 == status 即放行”，`systemctl restart nginx status`
    /// 会通过校验，而 systemctl 实际执行的是首参 restart
    #[cfg(feature = "remote-shell")]
    #[test]
    fn whitelist_rejects_systemctl_restart_with_status_arg() {
        assert!(
            validate_whitelist("systemctl", &args(&["restart", "nginx", "status"])).is_err(),
            "must reject systemctl restart even if a later arg is 'status'"
        );
    }

    #[cfg(feature = "remote-shell")]
    #[test]
    fn whitelist_allows_systemctl_readonly_verbs() {
        assert!(validate_whitelist("systemctl", &args(&["status", "nginx"])).is_ok());
        assert!(validate_whitelist("systemctl", &args(&["is-enabled", "nginx"])).is_ok());
        assert!(validate_whitelist("systemctl", &args(&["list-units"])).is_ok());
        // 选项在动词之前也要正确识别“第一个非选项参数”
        assert!(validate_whitelist("systemctl", &args(&["--no-pager", "status", "nginx"])).is_ok());
    }

    #[cfg(feature = "remote-shell")]
    #[test]
    fn whitelist_rejects_systemctl_without_readonly_verb_first() {
        assert!(validate_whitelist("systemctl", &args(&[])).is_err());
        assert!(validate_whitelist("systemctl", &args(&["restart", "nginx"])).is_err());
        assert!(validate_whitelist("systemctl", &args(&["enable", "nginx"])).is_err());
        assert!(validate_whitelist("systemctl", &args(&["nginx", "status"])).is_err());
    }

    // ===== 白名单绕过修复：ip 只读约束 =====

    #[cfg(feature = "remote-shell")]
    #[test]
    fn whitelist_allows_ip_readonly() {
        assert!(validate_whitelist("ip", &args(&["addr", "show"])).is_ok());
        assert!(validate_whitelist("ip", &args(&["-br", "a"])).is_ok());
        assert!(validate_whitelist("ip", &args(&["route"])).is_ok());
        assert!(validate_whitelist("ip", &args(&["neigh", "show"])).is_ok());
    }

    #[cfg(feature = "remote-shell")]
    #[test]
    fn whitelist_rejects_ip_write_operations() {
        assert!(validate_whitelist("ip", &args(&["link", "set", "eth0", "up"])).is_err());
        assert!(
            validate_whitelist("ip", &args(&["addr", "add", "10.0.0.1/24", "dev", "eth0"]))
                .is_err()
        );
        assert!(validate_whitelist("ip", &args(&["route", "del", "10.0.0.0/24"])).is_err());
        assert!(validate_whitelist("ip", &args(&["link", "down", "eth0"])).is_err());
    }

    // ===== 白名单绕过修复：通用读文件 / 挂载命令已移除 =====

    #[cfg(feature = "remote-shell")]
    #[test]
    fn whitelist_rejects_arbitrary_file_read_and_mount() {
        assert!(validate_whitelist("cat", &args(&["/etc/shadow"])).is_err());
        assert!(validate_whitelist("ls", &args(&["/root"])).is_err());
        assert!(validate_whitelist("mount", &args(&["/dev/sda1", "/mnt"])).is_err());
        assert!(validate_whitelist("rm", &args(&["-rf", "/"])).is_err());
    }

    // ===== Bug 150: 白名单绕过——basename 通过但执行完整路径 =====

    /// "/tmp/evil/df" 的 basename 是白名单内的 "df"，旧实现会放行并执行该路径；
    /// 现要求可执行文件必须位于可信目录内（此处 /tmp 与不存在路径均被拒绝）。
    #[cfg(feature = "remote-shell")]
    #[test]
    fn whitelist_rejects_full_path_outside_allowed_dirs() {
        assert!(validate_whitelist("/tmp/evil/df", &args(&[])).is_err());
        assert!(validate_whitelist("/usr/bin/../tmp/evil/df", &args(&[])).is_err());
        // 相对路径同样不能逃逸可信目录
        assert!(validate_whitelist("./evil/df", &args(&[])).is_err());
    }

    // ===== 同类写选项加固：date / dmesg =====

    #[cfg(feature = "remote-shell")]
    #[test]
    fn whitelist_rejects_date_set_and_dmesg_clear() {
        assert!(validate_whitelist("date", &args(&["-s", "2020-01-01"])).is_err());
        assert!(validate_whitelist("date", &args(&["--set=2020-01-01"])).is_err());
        assert!(validate_whitelist("date", &args(&["+%Y-%m-%d"])).is_ok());

        assert!(validate_whitelist("dmesg", &args(&["-c"])).is_err());
        assert!(validate_whitelist("dmesg", &args(&["--clear"])).is_err());
        assert!(validate_whitelist("dmesg", &args(&["--follow"])).is_err());
        assert!(validate_whitelist("dmesg", &args(&["-T"])).is_ok());
    }

    // ===== 危险字符仍被拦截 =====

    #[cfg(feature = "remote-shell")]
    #[test]
    fn whitelist_rejects_shell_metacharacters() {
        assert!(validate_whitelist("df", &args(&["|", "sh"])).is_err());
        assert!(validate_whitelist("df", &args(&[";", "rm"])).is_err());
        assert!(validate_whitelist("df", &args(&["$(id)"])).is_err());
    }

    // ===== parse_command 基础行为 =====

    #[cfg(feature = "remote-shell")]
    #[test]
    fn parse_command_splits_program_and_args() {
        let (p, a) = parse_command("df -h").unwrap();
        assert_eq!(p, "df");
        assert_eq!(a, vec!["-h".to_string()]);
        assert!(parse_command("   ").is_err());
    }

    // ===== r37-DIAG1：日志 tail 游标推进 =====

    /// 契约：只消费到最后一个换行为止，半行留给下一轮（不得拆行广播）
    #[test]
    fn tail_consumable_stops_at_last_newline() {
        assert_eq!(
            tail_consumable(b"a\nb\nc"),
            4,
            "只消费到 'b\\n'，'c' 留给下一轮"
        );
        assert_eq!(tail_consumable(b"a\nb\n"), 4);
        // 无换行（单行超长）时整块消费，保证游标前进不死循环
        assert_eq!(tail_consumable(b"no-newline-here"), 15);
        assert_eq!(tail_consumable(b""), 0);
    }

    /// 契约：首次/轮转后（last_pos=0）大文件只回看末尾 1 MiB，小文件从头读
    #[test]
    fn tail_resume_offset_caps_initial_lookback() {
        // 已有游标：原样返回（增量 tail 不受回看上限制约）
        assert_eq!(tail_resume_offset(4096, 10 * TAIL_MAX_LOOKBACK), 4096);
        // 首次读取大文件：只回看末尾 1 MiB
        assert_eq!(
            tail_resume_offset(0, 10 * TAIL_MAX_LOOKBACK),
            10 * TAIL_MAX_LOOKBACK - TAIL_MAX_LOOKBACK
        );
        // 首次读取小文件：从头读
        assert_eq!(tail_resume_offset(0, 1024), 0);
        assert_eq!(tail_resume_offset(0, TAIL_MAX_LOOKBACK), 0);
    }

    // ===== r37-DIAG2：命令输出上限 =====

    /// 契约：超长输出被截断到上限并追加提示；短输出原样返回
    #[cfg(feature = "remote-shell")]
    #[test]
    fn command_output_is_bounded() {
        let short = "df output".to_string();
        assert_eq!(truncate_command_output(short.clone()), short);

        let big = "x".repeat(MAX_CMD_OUTPUT_BYTES + 5_000);
        let out = truncate_command_output(big);
        assert!(out.len() < MAX_CMD_OUTPUT_BYTES + 200, "截断后应有界");
        assert!(out.starts_with(&"x".repeat(16)));
        assert!(out.contains("output truncated"), "应追加截断提示");
    }

    /// 契约：多字节 UTF-8 下按字符边界截断，不得产生非法 UTF-8（此处仅要求不 panic 且可解码）
    #[cfg(feature = "remote-shell")]
    #[test]
    fn command_output_truncation_is_utf8_safe() {
        // 构造恰好跨上限的中文字符串（每字 3 字节），确保截断点落在字符中间
        let unit = "中".repeat((MAX_CMD_OUTPUT_BYTES / 3) + 10);
        let out = truncate_command_output(unit);
        assert!(out.contains("output truncated"));
        // 上界内的部分必须仍是合法 UTF-8（String 保证，这里验证前缀未截断出乱码）
        assert!(out.starts_with("中中中"));
    }

    // ===== 日志归档：日期后缀校验与目录扫描 =====

    #[test]
    fn date_suffix_accepts_only_strict_yyyy_mm_dd() {
        assert!(is_date_suffix("2026-10-30"));
        // 非法日期写法、路径穿越、长度不符一律拒绝
        assert!(!is_date_suffix("2026-10-3"));
        assert!(!is_date_suffix("../etc"));
        assert!(!is_date_suffix("2026/10/30"));
        assert!(!is_date_suffix("2026-10-30.log"));
        assert!(!is_date_suffix(""));
    }

    #[test]
    fn archive_path_rejects_traversal() {
        // 合法日期拼出同目录归档文件
        let p = archive_path("/var/log/gateway/gateway.log", "2026-10-30")
            .expect("valid date must produce path");
        assert_eq!(
            p.to_string_lossy().replace('\\', "/"),
            "/var/log/gateway/gateway.log.2026-10-30"
        );
        // 路径穿越写法直接拒绝，不允许拼出目录外的路径
        assert!(archive_path("/var/log/gateway/gateway.log", "../../etc/passwd").is_none());
    }

    #[test]
    fn list_log_archives_reads_real_directory() {
        let dir = make_temp_dir("archives");
        let base = dir.join("gateway.log");
        std::fs::write(dir.join("gateway.log.2020-01-01"), b"old\n").unwrap();
        std::fs::write(dir.join("gateway.log.2020-01-02"), b"newer\n").unwrap();
        std::fs::write(&base, b"current\n").unwrap();
        // 干扰项：不符合 `<base>.<date>` 命名的文件必须被忽略
        std::fs::write(dir.join("other.log"), b"x").unwrap();
        std::fs::write(dir.join("gateway.log.2020-01-02.bak"), b"x").unwrap();

        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let list = list_log_archives_blocking(&base.to_string_lossy());
        let dates: Vec<&str> = list.iter().map(|a| a.date.as_str()).collect();
        // 当期文件按修改时间推断为今天（必然晚于历史归档），并按日期倒序排在最前
        assert_eq!(dates, vec![today.as_str(), "2020-01-02", "2020-01-01"]);
        assert!(list[0].current, "当日文件应标记为当期");
        assert!(!list[1].current);
        assert_eq!(list[1].size_bytes, 6);

        // 当期文件缺失时，尾日志回退到最新的滚动文件
        std::fs::remove_file(&base).unwrap();
        let newest = newest_log_file(&base.to_string_lossy()).expect("dated file must be found");
        assert!(newest.to_string_lossy().ends_with("2020-01-02"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 创建独立临时目录（测试并行时互不干扰）
    fn make_temp_dir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("gateway-diag-{tag}-{nanos}"));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }
}
