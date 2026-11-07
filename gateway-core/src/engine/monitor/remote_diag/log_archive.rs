//! 日志按日归档：文件发现、按日期定位、内容读取。
//!
//! O-6 从 `remote_diag.rs` 拆出：原文件 999 行，把「归档文件的定位与列出」
//! 与「诊断管理器本体」混在一起，两类关注点互相淹没。
//!
//! 依赖方向：只依赖标准库，不依赖 `crate::engine` 的任何模块。

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
pub(super) fn is_date_suffix(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// 指定日期的归档文件路径（日期非法时返回 None）
pub(super) fn archive_path(path: &str, date: &str) -> Option<std::path::PathBuf> {
    if !is_date_suffix(date) {
        return None;
    }
    let (dir, base) = split_log_path(path);
    Some(dir.join(format!("{base}.{date}")))
}

/// 扫描日志目录，返回真实存在的按日归档文件列表（新日期在前）。
pub(super) fn list_log_archives_blocking(path: &str) -> Vec<LogArchiveInfo> {
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
pub(super) fn newest_log_file(path: &str) -> Option<std::path::PathBuf> {
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
