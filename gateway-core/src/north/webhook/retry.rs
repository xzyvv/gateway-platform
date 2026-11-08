//! 重试策略与相关小工具：发送结果分类、可重试判定、指数退避、
//! 重试状态、data_id 生成、flush 单飞 RAII 复位器。

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
pub(super) const REPLAY_BATCH_SIZE: i64 = 50;

/// r65-SD1：run 循环顶部 await 上一轮 detached flush 的最长等待时间。
///
/// 等待过长会让 periodic cadence 偏离 interval_ms；过短则起不到"in-flight
/// 落地"的效果。10s 约是 reqwest timeout_secs（默认 30s）的 1/3——通常一次
/// flush 在 10s 内能完成；超时则放弃等待、告警、开始新 flush。
pub(super) const PREV_FLUSH_WAIT: Duration = Duration::from_secs(10);

/// r65-SD1：run 循环退出前（停机序 break 时）等待最后一次 in-flight flush 的
/// 最长时间。比 PREV_FLUSH_WAIT 略短——停机序总宽限 80s，要留余量给后续
/// north_servers / mqtt.stop；超时不 abort（可能撕裂 SQLite 写），仅告警继续。
pub(super) const FINAL_FLUSH_WAIT: Duration = Duration::from_secs(5);

/// Bug 171: 单条消息的最大重试次数，超过则放弃（显式记 error 后移出 outbox）。
pub(super) const MAX_WEBHOOK_ATTEMPTS: u32 = 8;

/// P0：outbox 中 `protocol='http'` 的 pending 行上限（与 MQTT 的 100_000 同口径）。
///
/// webhook 此前完全没有容量上限：端点长期 5xx / 连不上时 pending 行只增不减，
/// 磁盘被耗尽、`replay_pending_batch` 的分页查询越来越慢（现场表现为「越跑越卡」）。
pub(super) const MAX_WEBHOOK_PENDING: i64 = 100_000;

/// 死信维护（刷新 gauge + 按行数上限裁剪）的周期。
///
/// 不放在每个 tick：一次全表 COUNT 在 2 万行量级约几毫秒，但没必要按秒做；
/// 60s 的可见性延迟对"人工处置"这个场景完全够用，也不会与上报节拍抢 SQLite 写锁。
pub(super) const DEAD_LETTER_MAINT_INTERVAL: Duration = Duration::from_secs(60);

/// Bug 171: webhook 单条发送结果分类。
///
/// D6：`Retriable` / `Permanent` 自带**本条消息**的失败原因，而不是让调用方
/// 事后去共享槽 `WebhookClient::last_error` 反查。那个槽的语义是「最近一次
/// 发送错误」、同时还供 `health()` 使用；靠「发送完立刻读」的隐式时序取原因，
/// 一旦同批次或并发路径插入另一次发送，死信 reason 就会写成**别的消息**的错误
/// （例如 A 实际 401，却因为 B 先返回 500 被记成 `http status 500`）。
pub(super) enum SendOutcome {
    /// 2xx：发送成功，ack 删除。
    Ack,
    /// 可重试：网络错误 / 408 / 425 / 429 / 5xx，保留并按退避重发。
    Retriable(String),
    /// 永久失败：其余 4xx / 配置类 transport 错误，重发无意义，移入死信表。
    Permanent(String),
}

/// Bug 171: 判断 HTTP 状态码是否值得重试。
///
/// 原实现把「非 2xx」一律视为可重试——端点持续返回 4xx（如 401/404）时无限重发。
pub(super) fn is_retriable_status(status: reqwest::StatusCode) -> bool {
    status.is_server_error()
        || status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status.as_u16() == 425 // Too Early（避免依赖具体 http crate 版本的常量名）
}

/// Bug 171: 指数退避时长（base * 2^(attempts-1)，上限 5 分钟）。
///
/// 边界：`base_ms` 本身超过 5 分钟时以 `base_ms` 为准（退避不得短于主循环的
/// flush 间隔，否则会退化成"立刻重试"）；`attempts = 0` 视作第 1 次。
pub(super) fn retry_backoff(base_ms: u64, attempts: u32) -> Duration {
    const MAX_BACKOFF_MS: u64 = 300_000;
    let shift = attempts.saturating_sub(1).min(20);
    let ms = base_ms.saturating_mul(1u64 << shift).min(MAX_BACKOFF_MS);
    Duration::from_millis(ms.max(base_ms))
}

/// Bug 171: 单条消息的重试状态（尝试次数 + 下次可重试时刻）。
pub(super) struct RetryState {
    pub(super) attempts: u32,
    pub(super) next_at: Instant,
}

/// Bug C17: webhook data_id 的进程内自增序号。
///
/// outbox 上有唯一索引 `idx_outbox_pending_data_id(data_id) WHERE status = 0`。
/// 原实现直接用 `timestamp_millis()` 作 data_id，同一毫秒内产生的两批数据会得到
/// 相同 data_id，第二批被 `INSERT OR IGNORE` 静默丢弃 —— 丢数据。
static WEBHOOK_DATA_ID_SEQ: AtomicI64 = AtomicI64::new(0);

/// 生成唯一的 webhook data_id：「毫秒时间戳 * 2^20 + 进程内自增序号低 20 位」。
///
/// - 同进程内：序号自增保证唯一（同一毫秒内可容纳约 100 万批，实际远达不到）。
/// - 跨重启：时间戳随时间前进，pending 行创建时间必然早于重启时刻，不会冲突。
pub(super) fn webhook_data_id() -> i64 {
    const SEQ_BITS: u32 = 20;
    let seq = WEBHOOK_DATA_ID_SEQ.fetch_add(1, Ordering::Relaxed);
    chrono::Utc::now()
        .timestamp_millis()
        .wrapping_mul(1 << SEQ_BITS)
        .wrapping_add(seq & ((1 << SEQ_BITS) - 1))
}

/// Bug C13: flush 单飞标志的 RAII 复位器。
///
/// 无论 `flush_pending` 正常返回、提前 return，还是被 `spawn_catch_unwind`
/// 捕获的 panic，`Drop` 都会把标志复位，避免一次异常后 flush 永久停摆。
pub(super) struct FlushGuard(pub(super) Arc<AtomicBool>);

impl Drop for FlushGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
