//! 日志尾部读取与 tail 游标推进。
//!
//! O-6 从 `remote_diag.rs` 拆出：原文件 999 行，日志 tail 的「怎么读」与
//! 「谁在读」混在一起。
//!
//! 依赖方向：只依赖标准库 + 同级的 `log_archive`（找当日日志文件）。

use super::log_archive::newest_log_file;
use super::TAIL_MAX_LOOKBACK;

/// 一次性日志快照最多回看的字节数（512 KiB）。
///
/// 与 `TAIL_MAX_LOOKBACK`（1 MiB，流式起点）分开取值：这里是"故障后立刻取证"，
/// 回看量越小越快出结果，512 KiB 约合数千行，足够覆盖一次现场故障窗口。
const SNAPSHOT_MAX_BYTES: u64 = 512 * 1024;

/// 同步读取日志文件末尾 `max_lines` 行（由 `Diagnostics::tail_lines` 放进
/// `spawn_blocking` 调用，避免同步文件 IO 卡住 tokio worker）。
pub(super) fn tail_lines_blocking(path: &str, max_lines: usize) -> Vec<String> {
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
pub(super) fn read_tail(path: &std::path::Path, max_lines: usize) -> Vec<String> {
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

/// r37-DIAG1：日志 tail 游标推进前，计算本次宜消费的字节数。
///
/// 只消费到**最后一个换行**为止，剩余（半行）留给下一轮与后续内容拼接，
/// 避免把半行日志拆成两条广播；整块无换行（单行超长 > 单次读取上限）时
/// 整块消费，保证游标一定前进、不死循环。
pub(super) fn tail_consumable(buf: &[u8]) -> usize {
    match buf.iter().rposition(|&b| b == b'\n') {
        Some(i) => i + 1,
        None => buf.len(),
    }
}

/// r37-DIAG1：计算本次 tail 的起始偏移。
///
/// 首次读取 / 日志轮转后 `last_pos` 为 0，此时若文件很大，只回看末尾
/// `TAIL_MAX_LOOKBACK` 字节，避免启动时把整份历史日志载入内存并广播给运维端。
pub(super) fn tail_resume_offset(last_pos: u64, file_len: u64) -> u64 {
    if last_pos == 0 && file_len > TAIL_MAX_LOOKBACK {
        file_len - TAIL_MAX_LOOKBACK
    } else {
        last_pos
    }
}
