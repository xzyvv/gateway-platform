use super::{
    dequeue_with_backpressure, is_disk_full_error, reclaim_fallback_points,
    sqlite_code_from_message,
};
use crate::engine::error::{GatewayError, GatewayResult};
use crate::engine::metrics::Metrics;
use crate::engine::model::{DataPoint, Quality};
use crate::queue::{MemoryStorage, QueueStorage, QueuedItem, FALLBACK_MAX_RETRIES};
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// r15-Q1 (fix): 错误码前缀解析必须可靠。
///
/// 关键断言：解析出的主码要与写入时一致；没有前缀时返回 None
/// （不能瞎猜，否则会把普通 DB 错误误判成磁盘满 → 误触发 30s 退避）。
#[test]
fn parses_sqlite_code_prefix() {
    assert_eq!(
        sqlite_code_from_message("sqlite_error(code=13): full"),
        Some(13)
    );
    assert_eq!(
        sqlite_code_from_message("sqlite_error(code=10): io"),
        Some(10)
    );
    // 嵌在 GatewayError::Queue 的 Display 前缀里也要能解析
    assert_eq!(
        sqlite_code_from_message("queue error: sqlite_error(code=13): database or disk is full"),
        Some(13)
    );
    // 无前缀 → None
    assert_eq!(sqlite_code_from_message("queue error: locked"), None);
    assert_eq!(sqlite_code_from_message(""), None);
    // 前缀不完整 / 数字非法 → None
    assert_eq!(sqlite_code_from_message("sqlite_error(code=): x"), None);
    assert_eq!(sqlite_code_from_message("sqlite_error(code=abc): x"), None);
    assert_eq!(sqlite_code_from_message("sqlite_error(code=13"), None);
}

/// r15-Q1 (fix): 磁盘满判定必须**优先按主码**，文案仅作兜底。
///
/// 关键断言：
/// - code=13 (SQLITE_FULL) / code=10 (SQLITE_IOERR) → 判为磁盘满
/// - 其它主码 → **不**判为磁盘满（即使文案里出现 disk full 字样，
///   说明主码是权威来源）
/// - 无前缀的历史消息 → 走文案兜底
#[test]
fn disk_full_detection_prefers_stable_code() {
    assert!(is_disk_full_error(&GatewayError::Queue(
        "sqlite_error(code=13): database or disk is full".into()
    )));
    assert!(is_disk_full_error(&GatewayError::Queue(
        "sqlite_error(code=10): disk I/O error".into()
    )));
    // 主码是权威的：主码不是 13/10 时不判为磁盘满（文案不参与）
    assert!(!is_disk_full_error(&GatewayError::Queue(
        "sqlite_error(code=5): database or disk is full".into()
    )));
    assert!(!is_disk_full_error(&GatewayError::Queue(
        "sqlite_error(code=2): no such table".into()
    )));
    // 无前缀 → 文案兜底（旧消息格式）
    assert!(is_disk_full_error(&GatewayError::Queue(
        "database or disk is full".into()
    )));
    assert!(is_disk_full_error(&GatewayError::Queue(
        "disk I/O error".into()
    )));
    assert!(!is_disk_full_error(&GatewayError::Queue(
        "no such table: points".into()
    )));
}

/// 测试替身：用开关分别注入 `dequeue_batch` / `release_in_flight` 的 DB 错误，
/// 其余方法委托给 `MemoryStorage`，保证 len / dequeue 等观测路径与生产一致。
struct FaultInjectingStorage {
    inner: MemoryStorage,
    fail_dequeue: Arc<std::sync::atomic::AtomicBool>,
    fail_release: Arc<std::sync::atomic::AtomicBool>,
}

impl FaultInjectingStorage {
    fn new() -> Self {
        Self {
            inner: MemoryStorage::new(),
            fail_dequeue: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            fail_release: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }
}

impl QueueStorage for FaultInjectingStorage {
    fn enqueue_batch(&self, points: &[DataPoint]) -> GatewayResult<usize> {
        self.inner.enqueue_batch(points)
    }
    fn dequeue_batch(&self, limit: usize) -> GatewayResult<Vec<QueuedItem>> {
        if self.fail_dequeue.load(Ordering::SeqCst) {
            return Err(GatewayError::Queue("test: dequeue DB failure".into()));
        }
        self.inner.dequeue_batch(limit)
    }
    fn ack_batch(&self, ids: &[i64]) -> GatewayResult<()> {
        self.inner.ack_batch(ids)
    }
    fn release_in_flight(&self, ids: &[i64]) -> GatewayResult<()> {
        if self.fail_release.load(Ordering::SeqCst) {
            return Err(GatewayError::Queue("test: release DB failure".into()));
        }
        self.inner.release_in_flight(ids)
    }
    fn cleanup_stale_in_flight(&self, cutoff_ts: i64) -> GatewayResult<usize> {
        self.inner.cleanup_stale_in_flight(cutoff_ts)
    }
    fn len(&self) -> usize {
        self.inner.len()
    }
    fn in_flight_count(&self) -> i64 {
        self.inner.in_flight_count()
    }
    fn history_query(
        &self,
        point_id: &str,
        start: i64,
        end: i64,
        limit: usize,
    ) -> GatewayResult<Vec<QueuedItem>> {
        self.inner.history_query(point_id, start, end, limit)
    }
    fn maintenance(&self) -> GatewayResult<()> {
        self.inner.maintenance()
    }
    fn needs_maintenance(&self) -> bool {
        false
    }
    fn name(&self) -> &str {
        "fault-injecting"
    }
}

/// #104 (fix): DB 错误必须上抛为 Err，且递增失败指标——不能再被吞成空批，
/// 否则北向看到空批以为正常，实际数据在 SQLite 里出不来且不触发熔断。
#[tokio::test]
async fn dequeue_db_error_is_reported_not_masked_as_empty_batch() {
    let raw = Arc::new(FaultInjectingStorage::new());
    raw.fail_dequeue.store(true, Ordering::SeqCst);
    let storage: Arc<dyn QueueStorage> = raw;
    let metrics = Arc::new(parking_lot::Mutex::new(Arc::new(Metrics::new())));

    let result = dequeue_with_backpressure(&storage, 10, i64::MAX, false, &metrics).await;
    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("DB 错误必须上抛，不能再伪装成空批"),
    };
    assert!(err.to_string().contains("dequeue_batch failed (DB error)"));
    assert_eq!(
        metrics
            .lock()
            .queue_persist
            .persist_failed
            .load(Ordering::Relaxed),
        1,
        "DB 错误必须递增 persist_failed"
    );
}

/// #104 (fix): 只有真正的 DB Err 才报错；正常空队列仍必须返回 Ok(空批)。
#[tokio::test]
async fn dequeue_empty_queue_still_returns_ok_empty_batch() {
    let storage: Arc<dyn QueueStorage> = Arc::new(FaultInjectingStorage::new());
    let metrics = Arc::new(parking_lot::Mutex::new(Arc::new(Metrics::new())));

    let items = dequeue_with_backpressure(&storage, 10, i64::MAX, false, &metrics)
        .await
        .unwrap_or_else(|e| panic!("空队列不该报错: {}", e));
    assert!(items.is_empty());
    assert_eq!(
        metrics
            .lock()
            .queue_persist
            .persist_failed
            .load(Ordering::Relaxed),
        0,
        "正常空批不得误计失败"
    );
}

/// #105 (fix): Release 的 DB 失败必须打日志 + 递增指标，让「in_flight 行卡住
/// 等 5 分钟 stale 回收」这一故障可观测。
#[tokio::test]
async fn release_db_error_increments_failure_metric() {
    let raw = Arc::new(FaultInjectingStorage::new());
    raw.fail_release.store(true, Ordering::SeqCst);
    let storage: Arc<dyn QueueStorage> = raw;
    let queue = crate::queue::DataQueue::with_storage(storage).unwrap();
    let metrics = Arc::new(Metrics::new());
    queue.set_metrics(metrics.clone());

    queue.release(vec![1, 2]).await;

    // queue_task 在独立 task 中异步处理命令，轮询等待指标落账。
    let mut observed = 0;
    for _ in 0..400 {
        observed = metrics.queue_persist.persist_failed.load(Ordering::Relaxed);
        if observed >= 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(observed, 1, "release DB 失败必须递增 persist_failed");
}

fn make_points(n: usize) -> Vec<DataPoint> {
    (0..n)
        .map(|_| DataPoint {
            point_id: "p1".into(),
            device_id: "dev1".into(),
            value: 1.0,
            raw_value: 1.0,
            raw_type: "u16".into(),
            quality: Quality::Good,
            quality_reason: "".into(),
            timestamp: DataPoint::now(),
            source_ts: DataPoint::now(),
            stale_count: 0,
            value_text: None,
        })
        .collect()
}

/// #106 (fix): fallback 冲刷发生 JoinError 时，闭包侧的点随 task 被丢弃，
/// 必须从调用方持有的 Arc 副本里把整批点重新塞回缓冲，不能永久丢点。
/// 同时验证反弹上限仍然生效——超限后整批丢弃并计数，不会无限循环。
#[test]
fn fallback_join_error_reclaims_whole_batch_and_honors_retry_limit() {
    let fallback = Arc::new(parking_lot::Mutex::new(std::collections::VecDeque::new()));
    let retry_count = Arc::new(parking_lot::Mutex::new(0u32));
    let metrics = Arc::new(parking_lot::Mutex::new(Arc::new(Metrics::new())));

    // 模拟 JoinError：闭包侧 clone 被 drop，只剩调用方 Arc 副本。
    let shared = Arc::new(make_points(3));
    let task_side = shared.clone();
    drop(task_side);
    let pts = (*shared).clone();

    let (retained, dropped) =
        reclaim_fallback_points(&pts, &fallback, &retry_count, &metrics);
    assert_eq!((retained, dropped), (3, 0));
    assert_eq!(fallback.lock().len(), 3, "整批点必须回到 fallback 缓冲");

    // 反复回退直到反弹上限：超限后整批丢弃 + 计入 persist_dropped，
    // 并重置计数给下一批机会（与 enqueue 路径同语义）。
    *retry_count.lock() = FALLBACK_MAX_RETRIES;
    let before = metrics
        .lock()
        .queue_persist
        .persist_dropped
        .load(Ordering::Relaxed);
    let (retained, dropped) =
        reclaim_fallback_points(&make_points(2), &fallback, &retry_count, &metrics);
    assert_eq!((retained, dropped), (0, 2));
    assert_eq!(
        metrics
            .lock()
            .queue_persist
            .persist_dropped
            .load(Ordering::Relaxed),
        before + 2
    );
    assert_eq!(*retry_count.lock(), 0, "达到上限后计数应重置");
}
