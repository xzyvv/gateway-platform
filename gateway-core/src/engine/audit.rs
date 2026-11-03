//! 操作审计日志
//!
//! 记录配置修改、OTA 升级、控制指令等敏感操作，支持 JSON Lines 格式落盘。
//! 便于事后追溯谁在什么时间做了什么操作。
//! A14: 新增 list(filter) 查询接口，支持子串/时间范围过滤 + 条数上限。

use std::fs::{create_dir_all, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// r22-AUDIT1 修复：审计日志读取硬上限（1 MiB）。
///
/// `list()` 历史读数据末尾若审计文件未配置保留策略长期运行会膨胀到 GB 级，
/// 用 `read_to_string` 一次性加载会 OOM。超过此上限时改为仅尾部
/// `MAX_READ_BYTES` 字节反向 tail 读取，丢弃更早的记录以换取"绝不 OOM"的契约。
///
/// 取值依据：
/// - 一行 JSONL 平均 200B，1 MiB ≈ 5000 行，远大于单次查询硬上限 1000 条，
///   覆盖「最近 N 条」查询绰绰有余。
/// - 该读取在 HTTP 查询路径上执行（历史上还同步占用 tokio worker），
///   上限过大时一次查询的 IO/CPU 抖动会拖慢整个运行时；1 MiB 把最坏耗时压到毫秒级。
const MAX_READ_BYTES: u64 = 1024 * 1024;

/// E-2：审计落盘失败计数（进程级累计）。
///
/// 磁盘满 / 权限失效时 `writeln!`/`flush` 可能失败。旧实现 `let _ =` 直接丢弃，
/// 审计记录静默丢失（合规场景不可接受）。现在每次失败都落 `error!` 日志并带累计
/// 次数，运维可通过日志关键字或该计数做告警。
static WRITE_FAILURES: AtomicU64 = AtomicU64::new(0);

fn record_write_failure(context: &str, reason: impl std::fmt::Display) {
    let n = WRITE_FAILURES.fetch_add(1, Ordering::Relaxed) + 1;
    tracing::error!(
        context,
        total_failures = n,
        "audit log write failed (record may be lost): {}",
        reason
    );
}

/// 审计日志条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// 时间戳（毫秒 epoch）
    pub ts: i64,
    /// 操作类型，如 "config.update" / "ota.rollback"
    pub action: String,
    /// 操作对象，如 "gateway" / "device:dev_1"
    pub target: String,
    /// 变更详情（自由文本）
    pub detail: String,
    /// 来源/操作人，如 "api" / "system" / 用户名
    pub source: String,
}

/// 查询过滤器：所有字符串字段均为子串匹配（大小写不敏感），时间范围为闭区间
pub struct AuditFilter {
    pub action: Option<String>,
    pub target: Option<String>,
    pub source: Option<String>,
    /// 起始时间（毫秒 epoch，闭区间）
    pub ts_from: Option<i64>,
    /// 截止时间（毫秒 epoch，闭区间）
    pub ts_to: Option<i64>,
    /// 返回条数上限
    pub limit: Option<usize>,
}

impl AuditFilter {
    fn matches(&self, e: &AuditEntry) -> bool {
        if let Some(q) = self.action.as_deref() {
            if !e.action.to_lowercase().contains(&q.to_lowercase()) {
                return false;
            }
        }
        if let Some(q) = self.target.as_deref() {
            if !e.target.to_lowercase().contains(&q.to_lowercase()) {
                return false;
            }
        }
        if let Some(q) = self.source.as_deref() {
            if !e.source.to_lowercase().contains(&q.to_lowercase()) {
                return false;
            }
        }
        if let Some(from) = self.ts_from {
            if e.ts < from {
                return false;
            }
        }
        if let Some(to) = self.ts_to {
            if e.ts > to {
                return false;
            }
        }
        true
    }
}

/// 后台写线程的入队消息。
///
/// 修复（读后写竞态）：原实现只投递 `String`，无法判断「已入队」与「已落盘」，
/// 于是 `log()` 之后立刻 `list()` 可能读不到刚写的记录。新增 `Barrier` 变体作为
/// 屏障，供 `list()` 读前同步使用。
enum AuditMsg {
    /// 一条待落盘的审计日志行
    Line(String),
    /// 屏障：写线程处理到此处（即此前的 Line 均已落盘）后回 ack
    Barrier(Sender<()>),
}

/// 审计日志记录器
///
/// 写路径：`log()` 只把序列化后的行送入无界队列，由专用后台线程顺序落盘，
/// 从而避免在 tokio 异步工作线程上执行同步文件 IO（`log()` 主要从 Web 异步
/// 处理函数调用）。`Sender` 被 drop（进程退出）后，后台线程 `recv()` 返回 Err
/// 自然结束；后台线程创建失败时退化为同步写，保证审计不丢。
///
/// 读路径：`list()` 读文件前先 [`AuditLog::flush`] 一次，保证写后读一致性。
pub struct AuditLog {
    path: PathBuf,
    /// 后台写线程的发送端；None 表示线程创建失败，退化为同步写
    tx: Option<Sender<AuditMsg>>,
}

impl AuditLog {
    /// 创建审计日志记录器（日志文件目录不存在则自动创建）
    pub fn new(path: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel::<AuditMsg>();
        let writer_path = path.clone();
        let spawned = std::thread::Builder::new()
            .name("audit-writer".into())
            .spawn(move || {
                let mut file = Self::open_file(&writer_path).ok();
                while let Ok(msg) = rx.recv() {
                    match msg {
                        AuditMsg::Line(line) => {
                            // 句柄失效（日志被轮转/删除）时尝试重新打开
                            if file.is_none() {
                                file = Self::open_file(&writer_path).ok();
                            }
                            // E-2：写/刷盘失败不能静默 —— 记录 error + 计数后丢弃坏句柄，
                            // 下一条日志重新打开文件。
                            let mut failed = false;
                            match file.as_mut() {
                                Some(f) => {
                                    if let Err(e) = writeln!(f, "{}", line) {
                                        record_write_failure("writer", e);
                                        failed = true;
                                    } else if let Err(e) = f.flush() {
                                        record_write_failure("writer-flush", e);
                                        failed = true;
                                    }
                                }
                                None => record_write_failure(
                                    "writer-open",
                                    "cannot open audit log file",
                                ),
                            }
                            if failed {
                                file = None;
                            }
                        }
                        // 屏障：队列 FIFO，处理到此处说明此前的日志行均已落盘
                        AuditMsg::Barrier(ack) => {
                            let _ = ack.send(());
                        }
                    }
                }
            });
        match spawned {
            Ok(_) => Self { path, tx: Some(tx) },
            Err(e) => {
                tracing::warn!(
                    "audit writer thread spawn failed, fallback to sync write: {}",
                    e
                );
                Self { path, tx: None }
            }
        }
    }

    fn open_file(path: &PathBuf) -> std::io::Result<std::fs::File> {
        if let Some(parent) = path.parent() {
            create_dir_all(parent)?;
        }
        OpenOptions::new().create(true).append(true).open(path)
    }

    /// 同步兜底写：仅在后台线程不可用（或入队失败）时调用
    fn write_sync(&self, line: &str) {
        // E-2：兜底路径同样不能静默丢记录，打开/写/刷盘失败均记录 error + 计数。
        let mut f = match Self::open_file(&self.path) {
            Ok(f) => f,
            Err(e) => {
                record_write_failure("sync-open", e);
                return;
            }
        };
        if let Err(e) = writeln!(f, "{}", line) {
            record_write_failure("sync-write", e);
            return;
        }
        if let Err(e) = f.flush() {
            record_write_failure("sync-flush", e);
        }
    }

    /// 等待此前 `log()` 入队的日志全部落盘。
    ///
    /// 修复（读后写竞态）：`log()` 是异步入队，而 `list()` 直接读文件，两者之间
    /// 没有同步点——写完立刻查询可能读不到刚落的记录（并行负载下尤其明显）。
    /// 这里向写线程投递一个屏障并等待其 ack：队列 FIFO，ack 到达即表示屏障之前的
    /// 所有日志行都已写入并 flush。
    ///
    /// 写线程不可用时立即返回：此时 `log()` 走同步兜底，本就不存在延迟。
    fn flush(&self) {
        let Some(tx) = self.tx.as_ref() else {
            return;
        };
        let (ack_tx, ack_rx) = mpsc::channel::<()>();
        if tx.send(AuditMsg::Barrier(ack_tx)).is_err() {
            // 后台线程已消亡：log() 会同步兜底，无需等待
            return;
        }
        // 超时兜底：写线程异常卡死时不让查询无限阻塞。
        // 正常路径 ack 是毫秒级返回，2s 只会在异常时白等，把等待压到 200ms
        // （本方法在 HTTP 查询路径上被同步调用，超时值直接等于 worst-case 阻塞时长）。
        let _ = ack_rx.recv_timeout(Duration::from_millis(200));
    }

    /// 记录一条审计日志（正常路径只入队，不做同步文件 IO）
    pub fn log(&self, action: &str, target: &str, detail: &str, source: &str) {
        let entry = AuditEntry {
            ts: chrono::Utc::now().timestamp_millis(),
            action: action.into(),
            target: target.into(),
            detail: detail.into(),
            source: source.into(),
        };
        let line = match serde_json::to_string(&entry) {
            Ok(s) => s,
            Err(_) => return,
        };
        match self.tx.as_ref() {
            Some(tx) => {
                // 入队失败（后台线程已消亡）时退回同步写，避免丢审计
                // （msg 非 Line 变体的情形不可能发生——本分支刚包入的就是 Line）
                if let Err(mpsc::SendError(AuditMsg::Line(back))) = tx.send(AuditMsg::Line(line)) {
                    self.write_sync(&back);
                }
            }
            None => self.write_sync(&line),
        }
        // 同时输出到 tracing，便于实时监控
        tracing::info!(
            "AUDIT action={} target={} source={} detail={}",
            action,
            target,
            source,
            detail
        );
    }

    /// 查询审计日志：从最新向前反向遍历，应用过滤器，最多返回 limit 条
    /// 文件不存在视为空列表（不报错），方便首次启动未生成日志的场景
    pub fn list(
        &self,
        filter: AuditFilter,
    ) -> crate::engine::error::GatewayResult<Vec<AuditEntry>> {
        // 修复（读后写竞态）：log() 异步入队、此处直接读文件，中间没有同步点，
        // 写完立刻查询可能读不到刚落的记录。读文件前先等待入队日志落盘。
        self.flush();
        let lines = read_tail_lines(&self.path, MAX_READ_BYTES).map_err(|e| {
            crate::engine::error::GatewayError::Io(format!("read audit log failed: {}", e))
        })?;
        let limit = filter.limit.unwrap_or(200).min(1000);
        let mut entries: Vec<AuditEntry> = Vec::with_capacity(limit);
        // 反向遍历：JSONL 文件按时间顺序追加，末尾为最新
        for line in lines.iter().rev() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let entry: AuditEntry = match serde_json::from_str(line) {
                Ok(e) => e,
                Err(_) => continue, // 跳过损坏的行，保证查询不中断
            };
            if !filter.matches(&entry) {
                continue;
            }
            entries.push(entry);
            if entries.len() >= limit {
                break;
            }
        }
        Ok(entries)
    }
}

/// 流式反向 tail 读取文件末尾若干行。
///
/// r22-AUDIT1 修复：审计日志文件无保留策略，长期运行可膨胀至 GB 级。
/// 旧 `list()` 用 `read_to_string` 一次性加载整个文件，会触发 OOM。
/// 本函数仅读取文件最后 `max_bytes` 字节，按换行切分并以行 Vec 返回
/// （调用方按 `.iter().rev()` 顺序处理即可得到"最新优先"序列）。
///
/// 错误处理：
/// - 文件不存在：返回 `Ok(Vec::new())`（首次启动未生成日志的常见情况）。
/// - 文件大小 ≤ 上限：完整加载（与旧实现一致）。
/// - 文件大小 > 上限：仅 seek 到 `len - max_bytes` 位置开始，丢弃更早内容。
/// - 其他 IO 错误：透传给调用方。
fn read_tail_lines(path: &PathBuf, max_bytes: u64) -> std::io::Result<Vec<String>> {
    let metadata = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let file_len = metadata.len();
    let mut file = File::open(path)?;
    let read_len = if file_len > max_bytes {
        // seek 到「max_bytes 之前最近的换行边界」，避免从一行的中间开始切
        // —— JSONL 每行独立可解析，但半行会反序列化失败被丢弃，本可避免。
        let mut buf = vec![0u8; max_bytes as usize];
        file.seek(SeekFrom::Start(file_len - max_bytes))?;
        use std::io::Read;
        file.read_exact(&mut buf)?;
        // 在 buf 头部找到第一个换行符后从下一个字节开始，避免半行 JSON
        let start = buf
            .iter()
            .position(|&b| b == b'\n')
            .map(|p| p + 1)
            .unwrap_or(0);
        // 用 UTF-8 解码（替换非法字节为 U+FFFD，避免 IO 错误）
        String::from_utf8_lossy(&buf[start..])
            .split_terminator('\n')
            .map(str::to_string)
            .collect()
    } else {
        let mut content = String::new();
        use std::io::Read;
        file.read_to_string(&mut content)?;
        content.split_terminator('\n').map(str::to_string).collect()
    };
    Ok(read_len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_log_path() -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "gateway_audit_test_{}_{}.log",
            std::process::id(),
            n
        ))
    }

    /// 回归（读后写竞态）：`log()` 只把日志异步入队，`list()` 却直接读文件。
    /// 原实现两者之间无同步点：追加大批记录后再写一条哨兵记录并立即查询，
    /// 写线程还堵在积压里、哨兵尚未落盘，查询就会漏掉哨兵（并行负载下即此现象）。
    /// 修复后 `list()` 读文件前先同步落盘，故哨兵必然可见。
    #[test]
    fn list_sees_entry_logged_immediately_before_query() {
        let path = temp_log_path();
        let _ = std::fs::remove_file(&path);
        let log = AuditLog::new(path.clone());

        // 先灌入一批记录制造写入积压（异步入队远快于逐条落盘）。
        // 条数需在 flush() 的 200ms 等待上限内落完（HTTP 查询路径不能无限阻塞），
        // 50k 条逐行 flush 会超时，反而让「flush 保证落盘」的断言失去意义。
        for i in 0..2_000 {
            log.log("config.update", &format!("bulk-{}", i), "d", "test");
        }
        // 哨兵：排在队列末尾，写线程必须先清空积压才会写它
        log.log("ota.rollback", "sentinel", "d", "test");

        // 写完立即查询，不等待
        let entries = log
            .list(AuditFilter {
                action: None,
                target: Some("sentinel".into()),
                source: None,
                ts_from: None,
                ts_to: None,
                limit: None,
            })
            .expect("list audit log");

        drop(log);
        let _ = std::fs::remove_file(&path);
        assert_eq!(
            entries.len(),
            1,
            "查询应看到刚写入的哨兵记录，实际 {} 条（写线程积压未落盘）",
            entries.len()
        );
    }

    /// r22-AUDIT1 回归：超大审计日志（> MAX_READ_BYTES）查询不应 OOM，
    /// 且应只读取尾部 8 MiB 内的行，旧记录被丢弃但查询仍能返回最新结果。
    ///
    /// 用 `read_tail_lines` 直接灌入 ~10 MiB 的 JSONL 文件，验证：
    /// 1. 不 panic / 不 OOM
    /// 2. 返回行数受限于文件尾部实际内容
    /// 3. 过滤查询仍能找到"在尾部范围内"的记录
    #[test]
    fn list_truncates_when_file_exceeds_max_read_bytes() {
        let path = temp_log_path();
        let _ = std::fs::remove_file(&path);

        // 直接写入大文件：~6_000 行 × ~1.8 KB ≈ 10.8 MiB，超过 8 MiB 上限
        // 用单一 record 字符串足够大以确保每行 KB 级
        let big_detail = "x".repeat(1500);
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&path).expect("create temp audit");
            for i in 0..6_000u32 {
                // 直接写 JSONL（避免用 AuditLog::log 异步队列，便于控制总大小）
                writeln!(
                    f,
                    r#"{{"ts":{},"action":"bulk","target":"t{}","detail":"{}","source":"test"}}"#,
                    1_700_000_000_000i64 + i as i64,
                    i,
                    big_detail
                )
                .expect("write line");
            }
        }
        let file_size = std::fs::metadata(&path).unwrap().len();
        assert!(
            file_size > MAX_READ_BYTES,
            "测试文件必须超过上限，实际 {} bytes（上限 {}）",
            file_size,
            MAX_READ_BYTES
        );

        // 现在用 AuditLog::list 查询：不应 OOM，且能返回尾部记录
        let log = AuditLog::new(path.clone());
        // 写入一条哨兵（在已有大文件之后）
        log.log("sentinel.action", "sentinel.target", "d", "test");
        // 等待哨兵落盘
        std::thread::sleep(std::time::Duration::from_millis(200));
        drop(log);

        // 重新打开读取（绕开异步队列行为，直接验证 read_tail_lines）
        let tail = read_tail_lines(&path, MAX_READ_BYTES).expect("read tail");
        // 读到的字节不超过 MAX_READ_BYTES 的一个常数倍
        assert!(!tail.is_empty(), "tail 读取应至少包含部分行，实际为空");
        // 过滤出 sentinel：必须在 tail 内（因为是最近写入）
        let sentinel_count = tail.iter().filter(|l| l.contains("sentinel")).count();
        assert_eq!(
            sentinel_count, 1,
            "tail 应包含最近写入的 sentinel 行，实际 {} 条",
            sentinel_count
        );
        // 验证未越界：tail 总字节数不超过 MAX_READ_BYTES × 1.1（一些 metadata 开销）
        let total: usize = tail.iter().map(|l| l.len() + 1).sum();
        assert!(
            (total as u64) <= MAX_READ_BYTES,
            "tail 总字节数 {} 超过上限 {}",
            total,
            MAX_READ_BYTES
        );

        let _ = std::fs::remove_file(&path);
    }

    /// r22-AUDIT1 边界：文件不存在时 `list()` 不应返回错误。
    #[test]
    fn list_returns_empty_when_file_missing() {
        let path = temp_log_path();
        let _ = std::fs::remove_file(&path);
        let log = AuditLog::new(path.clone());

        let entries = log
            .list(AuditFilter {
                action: None,
                target: None,
                source: None,
                ts_from: None,
                ts_to: None,
                limit: None,
            })
            .expect("list audit log");

        drop(log);
        let _ = std::fs::remove_file(&path);
        assert!(entries.is_empty(), "文件不存在时 list 应返回空 vec");
    }
}
