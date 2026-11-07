use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;

#[cfg(feature = "remote-shell")]
use crate::engine::error::{GatewayError, GatewayResult};

mod log_archive;
mod tail;

#[cfg(feature = "remote-shell")]
mod shell;

pub use log_archive::LogArchiveInfo;

use log_archive::{archive_path, list_log_archives_blocking};
use tail::{read_tail, tail_consumable, tail_lines_blocking, tail_resume_offset};

#[cfg(feature = "remote-shell")]
use shell::{parse_command, truncate_command_output, validate_whitelist};

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

#[cfg(test)]
mod tests;
