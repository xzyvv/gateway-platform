//! 队列处理 task（§10）
//!
//! queue_task 通过 mpsc 通道串行处理 Enqueue / Dequeue / Ack / Release / Len / Drain / History 命令。
//! 存储后端通过 QueueStorage trait 注入，支持 SQLite（生产）和 Memory（测试）等后端。
//!
//! #2: 存储操作通过 spawn_blocking 执行，避免阻塞 tokio worker。
//! A9: enqueue 操作用 catch_unwind 包裹，避免 panic 导致 JoinError 无法区分"已部分写入"和"完全未写"。
//! Bug 81: 磁盘满保护（仅 SQLite 后端触发）：检测到 SQLITE_FULL/IOERR 后暂停入队，退避期内丢弃数据。
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use futures::FutureExt;

use tokio::sync::{mpsc, oneshot, Notify};

use crate::engine::error::{GatewayError, GatewayResult};
use crate::engine::metrics::Metrics;
use crate::engine::model::DataPoint;

use super::item::QueuedItem;

use crate::queue::storage::{HistoryQueryResult, QueueStorage, SnapshotCollapse};

/// 队列命令
pub(crate) enum QueueCommand {
    Enqueue(Vec<DataPoint>),
    Dequeue {
        limit: usize,
        reply: oneshot::Sender<Vec<QueuedItem>>,
    },
    /// 快照出队：每个点位只返回最新一行，且只取 `id <= cutoff` 的行。
    DequeueSnapshot {
        limit: usize,
        /// 周期出队上界（折叠时刻的 `MAX(id)`），用于隔离本周期新到的样本。
        cutoff: i64,
        reply: oneshot::Sender<Vec<QueuedItem>>,
    },
    /// 快照折叠：只标记不发送。返回折叠行数与本周期出队上界。
    CollapseSnapshot {
        reply: oneshot::Sender<SnapshotCollapse>,
    },
    /// 确认已发送并删除记录。
    ///
    /// 带 reply：存储层 ack 失败必须回传给调用方，否则调用方无法得知
    /// 「数据没被删掉」，也就无法据此 release 重投或告警（此前该错误只
    /// 在 actor 内 warn 一声就吞掉，MQTT 与模拟消费者两条路径都拿不到）。
    Ack {
        ids: Vec<i64>,
        reply: oneshot::Sender<GatewayResult<()>>,
    },
    /// 释放 in_flight 标志（不删除数据），让下次 dequeue 能重新取到
    Release(Vec<i64>),
    Len(oneshot::Sender<usize>),
    /// 等待队列排空（所有数据已 ack 删除）
    Drain(oneshot::Sender<()>),
    /// P2-1：带降采样与分页元信息的历史查询（N-10 起为唯一历史查询命令）。
    HistoryEx {
        point_id: String,
        start: i64,
        end: i64,
        limit: usize,
        /// 时间桶宽度（秒），None/0 = 不降采样
        bucket_secs: Option<u32>,
        reply: oneshot::Sender<GatewayResult<HistoryQueryResult>>,
    },
    /// 设置历史数据保留天数（v2.1 起与 license 无关，固定 30 天；None = 不限）
    SetStorageRetention(Option<u32>),
    /// v2.0: 查询最旧一条持久化数据的时间戳（unix 毫秒），用于历史保留跨度统计。
    /// None = 无历史数据。
    OldestTs(oneshot::Sender<Option<i64>>),
}

/// 磁盘满保护：检测到 SQLITE_FULL/IOERR 后暂停入队的时间（秒）
const DISK_FULL_BACKOFF_SECS: u64 = 30;

/// SQLite 主错误码：磁盘满。
const SQLITE_FULL: i32 = 13;
/// SQLite 主错误码：磁盘 I/O 错误。
const SQLITE_IOERR: i32 = 10;

/// 判断底层存储错误是否为磁盘满相关（SQLITE_FULL=13, SQLITE_IOERR=10）
///
/// r15-Q1 (fix): 旧实现只做**字符串匹配**（`contains("database or disk is full")`
/// 等）。根因是 `From<rusqlite::Error>` 把结构化错误码压扁成了自由文本——一旦
/// rusqlite / SQLite 升级或措辞变化，判定即失效 → 磁盘满时走「普通错误」路径，
/// 失去 30s 退避保护 → 持续重试写库，刷日志 + 加重 IO 劣化。
///
/// 现在优先按 `sqlite_error(code=N)` 前缀里的**主错误码**判定（稳定），
/// 文案匹配仅作兜底（兼容未经过 `From` 转换的错误）。
fn is_disk_full_error(e: &GatewayError) -> bool {
    if let Some(code) = sqlite_code_from_message(&e.to_string()) {
        return code == SQLITE_FULL || code == SQLITE_IOERR;
    }
    // 兜底：文案匹配（未带 code 前缀的历史消息）
    let msg = e.to_string();
    msg.contains("database or disk is full")
        || msg.contains("disk I/O error")
        || msg.contains("SQLITE_FULL")
        || msg.contains("SQLITE_IOERR")
}

/// 从错误消息的 `sqlite_error(code=N)` 前缀里解析 SQLite 主错误码。
///
/// 纯函数：消息不含该前缀时返回 None。
fn sqlite_code_from_message(msg: &str) -> Option<i32> {
    let start = msg.find("sqlite_error(code=")? + "sqlite_error(code=".len();
    let rest = &msg[start..];
    let end = rest.find(')')?;
    rest[..end].parse::<i32>().ok()
}

/// 修复(审查#B6)：Dequeue 的反压 / 陈旧 in_flight 清理逻辑从 match 分支中抽离，
/// 原分支用 `continue` 跳过本轮循环，但 `continue` 无法跨越 catch_unwind 用的
/// async 块边界，故改为返回值语义（返回空批 = 本次拒绝 dequeue）。
async fn dequeue_with_backpressure(
    storage: &Arc<dyn QueueStorage>,
    limit: usize,
    cutoff: i64,
    snapshot: bool,
) -> Vec<QueuedItem> {
    let cur_in_flight = storage.in_flight_count();
    if cur_in_flight >= super::limit::MAX_IN_FLIGHT {
        // 释放陈旧 in_flight（5 分钟未确认），避免 broker 长时间不可达，
        // 永久卡死 dequeue
        let cutoff_ts = DataPoint::now() - 5 * 60 * 1000;
        let storage_clone = storage.clone();
        // E-4：区分「0 条陈旧行」与「DB 清理失败」。旧实现 `unwrap_or(0)` 把 DB 错误
        // 当成 0 条释放，随后走到下方「无陈旧行 → broker 背压」分支，真实故障被误诊。
        let released = match tokio::task::spawn_blocking(move || {
            storage_clone.cleanup_stale_in_flight(cutoff_ts)
        })
        .await
        {
            Ok(Ok(n)) => n,
            Ok(Err(e)) => {
                tracing::error!(
                    "cleanup_stale_in_flight failed (in_flight {} >= limit {}): {}",
                    cur_in_flight,
                    super::limit::MAX_IN_FLIGHT,
                    e
                );
                return Vec::new();
            }
            Err(e) => {
                tracing::error!("cleanup_stale_in_flight join failed: {}", e);
                return Vec::new();
            }
        };
        if released > 0 {
            tracing::warn!(
                "in_flight {} >= hard limit {}, released {} stale in_flight rows to recover",
                cur_in_flight,
                super::limit::MAX_IN_FLIGHT,
                released
            );
            if storage.in_flight_count() >= super::limit::MAX_IN_FLIGHT {
                if snapshot {
                    collapse_quietly(storage).await;
                }
                return Vec::new();
            }
        } else {
            tracing::warn!(
                "in_flight {} >= hard limit {}, refusing dequeue (no stale rows to release; broker backpressure)",
                cur_in_flight,
                super::limit::MAX_IN_FLIGHT
            );
            if snapshot {
                // 反压期间不发数据，但仍把积压收敛到「每点位最新一行」，
                // 避免旧样本持续占磁盘、把队列水位推到淘汰阈值。
                collapse_quietly(storage).await;
            }
            return Vec::new();
        }
    }
    let storage_clone = storage.clone();
    // r15-Q2 (fix): 旧实现 `dequeue_batch(limit).unwrap_or_default()` 把 DB 错误
    // 伪装成「队列为空」——上层拿到空批，无法区分「确实没有数据」与「DB 查询失败」
    // → 表现为**北向静默无数据**（采集正常、MQTT 收不到东西、日志无异常）。
    // 现在错误必须上抛。
    match tokio::task::spawn_blocking(move || {
        if snapshot {
            storage_clone.dequeue_snapshot_batch(limit, cutoff)
        } else {
            storage_clone.dequeue_batch(limit)
        }
    })
    .await
    {
        Ok(Ok(items)) => items,
        Ok(Err(e)) => {
            tracing::error!(
                "dequeue_batch failed (DB error): {} — the batch is NOT empty, it is UNREADABLE; \
                 the northbound channel will see no data for this round",
                e
            );
            Vec::new()
        }
        Err(e) => {
            tracing::error!("dequeue spawn_blocking failed: {}", e);
            Vec::new()
        }
    }
}

/// 反压时做一次快照折叠：不发数据，只把积压收敛到「每点位最新一行」。
/// 失败只记日志，不影响反压返回（反压本身是正常路径）。
async fn collapse_quietly(storage: &Arc<dyn QueueStorage>) {
    let storage = storage.clone();
    match tokio::task::spawn_blocking(move || storage.collapse_snapshot()).await {
        Ok(Ok(r)) if r.superseded == 0 => {}
        Ok(Ok(r)) => tracing::debug!(
            "snapshot collapse under backpressure: {} rows",
            r.superseded
        ),
        Ok(Err(e)) => tracing::warn!("snapshot collapse failed under backpressure: {}", e),
        Err(e) => tracing::warn!("snapshot collapse join failed: {}", e),
    }
}

/// 队列处理 task
///
/// 通过 `Arc<dyn QueueStorage>` 注入存储后端，支持 SQLite / Memory 等。
/// 所有存储操作通过 spawn_blocking 执行，避免阻塞 tokio worker。
///
/// Bug 170: `fallback` 与 `DataQueue` 共享同一降级缓冲——SQLite 写失败（磁盘满或
/// 非磁盘满的 DB 错误）时，把整批数据点回退进该缓冲，避免静默丢数据。
pub(crate) async fn queue_task(
    storage: Arc<dyn QueueStorage>,
    mut rx: mpsc::Receiver<QueueCommand>,
    notify: Arc<Notify>,
    fallback: Arc<parking_lot::Mutex<std::collections::VecDeque<DataPoint>>>,
    // Bug 183 (N01): 反弹轮次累计 + 写失败指标注入。
    fallback_retry_count: Arc<parking_lot::Mutex<u32>>,
    // Bug T6: 与 DataQueue 共享同一 `Arc<Mutex<Arc<Metrics>>>` 句柄。
    // `set_metrics` 注入真实指标后本任务计数同步生效（原为独立占位对象，
    // 写失败 / 丢弃计数永远写进无人读取的 Metrics）。
    metrics: Arc<parking_lot::Mutex<Arc<Metrics>>>,
) -> GatewayResult<()> {
    // 磁盘满保护标志：检测到磁盘满后暂停入队，避免 SQLite 损坏（仅 SQLite 后端触发）
    let disk_full = Arc::new(AtomicBool::new(false));
    let disk_full_retry_at = Arc::new(std::sync::atomic::AtomicU64::new(0));

    // 定期维护定时器
    let mut cleanup_interval = tokio::time::interval(std::time::Duration::from_secs(30));
    // 等待排空的调用方（优雅退出时）
    let mut pending_drain: Option<oneshot::Sender<()>> = None;

    // W3: 队列循环看门狗心跳（60s 超时）
    let queue_hb = crate::engine::monitor::heartbeat::get()
        .register("queue", 60_000)
        .await;

    loop {
        // W3: 心跳
        queue_hb.kick();
        tokio::select! {
                    cmd = rx.recv() => {
                        let Some(cmd) = cmd else { break; };
        // 修复(审查#B6)：单条命令处理逻辑用 catch_unwind 包裹，
        // queue_task 由 spawn_catch_unwind 启动且不会重启，此前任一命令
                        // panic 都会 drop 掉 future 并丢弃 mpsc::Receiver，导致队列静默停摆。
        let handled = std::panic::AssertUnwindSafe(async {
                            match cmd {
                                QueueCommand::Enqueue(points) => {
        // Bug 81: 磁盘满时跳过入队，避免反复触发写入失败和日志洪水。
        // disk_full_retry_at 退避期内直接丢弃数据；
        // retention cleanup 释放空间后，下次成功入队会自动清除
        // disk_full 标志恢复正常写入。
        // Bug 2.3 (2026-Q1): 内层不再 30s drop 数据。
        // 上层 DataQueue::enqueue 已经在 disk_ok=false 时走 fallback buffer。
        // 本任务只负责中转，disk_full 仅用作可观测事件。
        let _ = disk_full.load(Ordering::Relaxed);
                                    let _ = disk_full_retry_at.load(Ordering::Relaxed);
                                    let n_points = points.len();
                                    // #2: 存储操作移出 tokio worker 线程，避免阻塞 channel 处理
                                    let storage_clone = storage.clone();
                                    // r15-Q3 (fix): points 用 `Arc` 共享，闭包只拿一份 clone。
        // 旧实现把 `Vec<DataPoint>` **move** 进闭包：闭包 panic / task 被取消时
        // `spawn_blocking` 返回 `Err(JoinError)`，points 已被闭包消费 →
        // `Err(e) => tracing::error!(...)` 分支**无法回退到 fallback 缓冲** →
        // 该批数据静默丢失（与 `Ok(Err((pts, e)))` 分支行为不一致）。
        // 现在调用方持有 Arc 副本，JoinError 分支也能把整批点回退进缓冲。
        let points_shared = std::sync::Arc::new(points);
        let points_for_task = points_shared.clone();
                                    // A9: 用 catch_unwind 包裹存储操作，避免 panic
        // 触发 spawn_blocking JoinError 后上层只能 log，无法区分"已部分写入"和"完全未写"，
        // 误判为失败重试可能引入重复数据，误判为成功则丢数据。改为捕获 panic 后
        // 转为正常错误返回，由上层 disk_full / 失败告警路径统一处理。
        //
                                    // Bug 170: 失败时把整批 points 一并带回（`Err((points, e))`），
                                    // 供下方回退到 fallback 缓冲——原实现 points 已被 move 进闭包，
                                    // 非磁盘满错误只能 warn 后丢弃，造成静默丢点。
        let sql_result = tokio::task::spawn_blocking(
                                        move || -> Result<usize, (Vec<DataPoint>, GatewayError)> {
                                            let inner_result = std::panic::catch_unwind(
                                                std::panic::AssertUnwindSafe(|| storage_clone.enqueue_batch(&points_for_task))
                                            );
                                            match inner_result {
                                                Ok(Ok(n)) => Ok(n),
                                                // 失败：把整批点原样带回调用方，便于回退缓冲
                                                Ok(Err(e)) => Err(((*points_for_task).clone(), e)),
                                                Err(p) => {
                                                    let msg = if let Some(s) = p.downcast_ref::<&str>() {
                                                        (*s).to_string()
                                                    } else if let Some(s) = p.downcast_ref::<String>() {
                                                        s.clone()
                                                    } else {
                                                        "<unknown panic>".to_string()
                                                    };
                                                    Err(((*points_for_task).clone(), GatewayError::Queue(format!("enqueue panic: {}", msg))))
                                                }
                                            }
                                        },
                                    ).await;
                                    match sql_result {
                                        Ok(Ok(_)) => {
                                            // 入队成功，清除磁盘满标志
                                            if disk_full.load(Ordering::Relaxed) {
                                                disk_full.store(false, Ordering::Relaxed);
                                                tracing::info!("disk recovered, resuming enqueue");
                                            }
                                            // Bug 183 (N01): 写成功后重置反弹计数。
                                            *fallback_retry_count.lock() = 0;
                                        }
                                        Ok(Err((pts, e))) => {
                                            // Bug 183 (N01): 写失败先 +1 指标，再决定如何回退。
                metrics
                                                .lock()
                                                .queue_persist.persist_failed
                                                .fetch_add(1, Ordering::Relaxed);
                                            if is_disk_full_error(&e) {
                                                disk_full.store(true, Ordering::Relaxed);
                                                let retry_at = std::time::SystemTime::now()
                                                    .duration_since(std::time::UNIX_EPOCH)
                                                    .map(|d| d.as_secs() + DISK_FULL_BACKOFF_SECS)
                                                    .unwrap_or(0);
                                                disk_full_retry_at.store(retry_at, Ordering::Relaxed);
                                                tracing::error!("disk full detected, pausing enqueue for {}s: {}", DISK_FULL_BACKOFF_SECS, e);
                                            } else {
                                                tracing::warn!("enqueue batch failed: {}", e);
                                            }
                                            // Bug 170: 无论磁盘满还是其它 DB 错误，都把整批点回退进降级缓冲，
        // 待下次 enqueue 成功时由 DataQueue::flush_fallback 重投，
        // 不再静默丢点。缓冲已满时按优先级淘汰最低者。
        //
        // Bug 183 (N01): 反弹轮次由 push_into_fallback 内部维护：
        //   - 达到 FALLBACK_MAX_RETRIES 时本批被强制丢弃，记 dropped 指标
                                            //   - 磁盘满路径使用哨兵计数 0（与上层 DataQueue::enqueue 行为一致）
        //     但 task 侧无法识别"磁盘满 vs 其他错误"，因此非磁盘满错误
        //     会真的消耗反弹计数。
        let mut cnt = fallback_retry_count.lock();
                                            let (retained, dropped) = {
                                                let mut buf = fallback.lock();
                                                crate::queue::push_into_fallback(
                                                    &mut buf,
                                                    crate::queue::FALLBACK_CAPACITY,
                                                    pts,
                                                    &mut cnt,
                                                )
                                            };
                                            if dropped > 0 {
                                                metrics
                                                    .lock()
                                                    .queue_persist.persist_dropped
                                                    .fetch_add(dropped as u64, Ordering::Relaxed);
                                                // Bug 183 (N01): 若反弹到上限后被丢弃，标记一次反弹事件。
                if *cnt == 0 {
                                                    metrics
                                                        .lock()
                                                        .queue_persist.fallback_bounce
                                                        .fetch_add(1, Ordering::Relaxed);
                                                }
                                                tracing::error!(
                                                    "enqueue failed: {} points ({} retained, {} dropped, bounce={})",
                                                    n_points,
                                                    retained,
                                                    dropped,
                                                    *cnt
                                                );
                                            } else {
                                                tracing::info!(
                                                    "enqueue failed: {} points reclaimed into fallback (retained={}, bounce={})",
                                                    n_points,
                                                    retained,
                                                    *cnt
                                                );
                                            }
                                        }
                                        Err(e) => {
                                            // r15-Q3 (fix): JoinError 分支原先只打一行日志就丢点。
                                            // 现在用调用方持有的 Arc 副本把整批点回退进降级缓冲，
                                            // 与 `Ok(Err((pts, e)))` 分支行为对齐 —— 不再静默丢点。
                                            let pts = (*points_shared).clone();
                                            metrics
                                                .lock()
                                                .queue_persist.persist_failed
                                                .fetch_add(1, Ordering::Relaxed);
                                            tracing::error!(
                                                "enqueue spawn_blocking join failed: {} — {} points reclaimed into fallback",
                                                e,
                                                pts.len()
                                            );
                                            let mut cnt = fallback_retry_count.lock();
                                            let (retained, dropped) = {
                                                let mut buf = fallback.lock();
                                                crate::queue::push_into_fallback(
                                                    &mut buf,
                                                    crate::queue::FALLBACK_CAPACITY,
                                                    pts,
                                                    &mut cnt,
                                                )
                                            };
                                            if dropped > 0 {
                                                metrics
                                                    .lock()
                                                    .queue_persist.persist_dropped
                                                    .fetch_add(dropped as u64, Ordering::Relaxed);
                                                tracing::error!(
                                                    "enqueue join failed: {} points ({} retained, {} dropped, bounce={})",
                                                    n_points,
                                                    retained,
                                                    dropped,
                                                    *cnt
                                                );
                                            }
                                        }
                                    }
                                }
                                QueueCommand::Dequeue { limit, reply } => {
        // A2: in_flight 硬上限检查——超限时拒绝 dequeue，反压采集层，
        // 避免 broker 长时间不 ACK 导致 in_flight 无限增长撑爆存储。
        // #5 修复：硬上限分支主动跑一次 stale in_flight 清理
                                    // （cutoff 与 storage.maintenance 内部一致：5 分钟前），见
                                    // dequeue_with_backpressure。
        let items = dequeue_with_backpressure(&storage, limit, i64::MAX, false).await;
                                    if !items.is_empty() {
                                        let n = storage.in_flight_count();
                                        if n > 5000 {
                                            tracing::warn!("in_flight count {} exceeds 5000, broker may be slow", n);
                                        }
                                    }
                                    let _ = reply.send(items);
                                }
                                QueueCommand::CollapseSnapshot { reply } => {
                                    let storage_clone = storage.clone();
                                    let result = match tokio::task::spawn_blocking(move || {
                                        storage_clone.collapse_snapshot()
                                    })
                                    .await
                                    {
                                        Ok(Ok(r)) => r,
                                        Ok(Err(e)) => {
                                            tracing::warn!("snapshot collapse failed: {}", e);
                                            SnapshotCollapse::unrestricted()
                                        }
                                        Err(e) => {
                                            tracing::warn!("snapshot collapse join failed: {}", e);
                                            SnapshotCollapse::unrestricted()
                                        }
                                    };
                                    let _ = reply.send(result);
                                }
                                QueueCommand::DequeueSnapshot {
                                    limit,
                                    cutoff,
                                    reply,
                                } => {
                                    // 与普通出队共享反压 / 陈旧 in_flight 清理逻辑，
                                    // 仅出队语句不同（按点位取最新一行 + 周期 cutoff）。
                                    let items =
                                        dequeue_with_backpressure(&storage, limit, cutoff, true)
                                            .await;
                                    if !items.is_empty() {
                                        let n = storage.in_flight_count();
                                        if n > 5000 {
                                            tracing::warn!("in_flight count {} exceeds 5000, broker may be slow", n);
                                        }
                                    }
                                    let _ = reply.send(items);
                                }
                                QueueCommand::Ack { ids, reply } => {
                                    // #2: ack_batch 走 spawn_blocking
                                    let storage_clone = storage.clone();
                                    let ack_result = tokio::task::spawn_blocking(move || {
                                        storage_clone.ack_batch(&ids)
                                    }).await;
                                    // 错误回传给调用方：ack 失败意味着数据并未被删除，
                                    // 调用方需要知道才能 release 重投 / 告警。
                                    let ack_outcome: GatewayResult<()> = match ack_result {
                                        Ok(Ok(())) => Ok(()),
                                        Ok(Err(e)) => {
                                            tracing::warn!("ack batch failed: {}", e);
                                            Err(GatewayError::Queue(format!("ack batch failed: {}", e)))
                                        }
                                        Err(e) => {
                                            tracing::error!("ack spawn_blocking failed: {}", e);
                                            Err(GatewayError::Queue(format!(
                                                "ack spawn_blocking failed: {}", e
                                            )))
                                        }
                                    };
                                    let _ = reply.send(ack_outcome);
                                    // 队列已空且有等待排空的调用方：通知
                                    if pending_drain.is_some() && storage.is_empty() {
                                        if let Some(tx) = pending_drain.take() {
                                            let _ = tx.send(());
                                        }
                                        notify.notify_waiters();
                                    }
                                }
                                QueueCommand::Release(ids) => {
                                    // #2: release_in_flight 走 spawn_blocking
                                    let storage_clone = storage.clone();
                                    let _ = tokio::task::spawn_blocking(move || {
                                        storage_clone.release_in_flight(&ids)
                                    }).await;
                                }
                                QueueCommand::Len(reply) => {
                                    let len = storage.len();
                                    let _ = reply.send(len);
                                }
                                QueueCommand::Drain(reply) => {
                                    if storage.is_empty() {
                                        let _ = reply.send(());
                                    } else {
                                        pending_drain = Some(reply);
                                    }
                                }
                                QueueCommand::HistoryEx {
                                    point_id,
                                    start,
                                    end,
                                    limit,
                                    bucket_secs,
                                    reply,
                                } => {
                                    // P2-1：降采样查询同样走 spawn_blocking；
                                    // 与 History 不同，这里的错误必须回传给调用方
                                    // （由 web 层转 500），不能吞成空结果。
                                    let storage_clone = storage.clone();
                                    let result = match tokio::task::spawn_blocking(move || {
                                        storage_clone.history_query_ex(
                                            &point_id,
                                            start,
                                            end,
                                            limit,
                                            bucket_secs,
                                        )
                                    })
                                    .await
                                    {
                                        Ok(result) => result,
                                        Err(e) => Err(GatewayError::Queue(format!(
                                            "history_ex spawn_blocking failed: {}",
                                            e
                                        ))),
                                    };
                                    let _ = reply.send(result);
                                }
                                QueueCommand::SetStorageRetention(days) => {
                                    // 仅更新内存槽位（锁内赋值，无 IO），无需 spawn_blocking；
                                    // 实际截断由 maintenance 周期按新的保留天数执行
                                    storage.set_storage_retention_days(days);
                                }
                                QueueCommand::OldestTs(reply) => {
                                    // 真实 DB 查询（SQLite MIN 聚合）走 spawn_blocking，
                                    // 与 HistoryEx 同范式，避免阻塞 tokio worker。
                                    let storage_clone = storage.clone();
                                    let result = tokio::task::spawn_blocking(move || {
                                        storage_clone.oldest_data_timestamp()
                                    })
                                    .await;
                                    let ts = match result {
                                        Ok(v) => v,
                                        Err(e) => {
                                            tracing::warn!("oldest_data_timestamp join failed: {}", e);
                                            None
                                        }
                                    };
                                    let _ = reply.send(ts);
                                }
                            }
                        })
                        .catch_unwind()
                        .await;
                        if let Err(p) = handled {
                            let msg = if let Some(s) = p.downcast_ref::<&str>() {
                                (*s).to_string()
                            } else if let Some(s) = p.downcast_ref::<String>() {
                                s.clone()
                            } else {
                                "<unknown panic>".to_string()
                            };
                            // 修复(审查#B6)：单条命令 panic 不终止 actor，仅丢弃本条命令
                            metrics.lock().inc_task_panic("queue_task");
                            tracing::error!("queue_task command handler panicked, command dropped: {}", msg);
                        }
                    }
                    _ = cleanup_interval.tick() => {
                        // 定期维护（retention + WAL checkpoint + aggregation 等）
                        if storage.needs_maintenance() {
                            let storage_clone = storage.clone();
                            // P3: 维护结果不再静默丢弃——此前 maintenance 内部失败
        // （WAL checkpoint / 定时备份 / retention 清理 / 分钟聚合）
        // 、spawn_blocking 的 JoinError 都无任何日志，问题不可见。
        let join_result = tokio::task::spawn_blocking(move ||
            {
                                storage_clone.maintenance()
                            }).await;
                            match join_result {
                                Ok(Ok(())) => {}
                                Ok(Err(e)) => tracing::warn!("queue maintenance failed: {}", e),
                                Err(e) => tracing::error!("maintenance spawn_blocking failed: {}", e),
                            }
                        }
        // #3 修复（数据丢失）：fallback 缓冲只在「下次 enqueue」时被上层
        // flush——采集停止 / 进程静默期间缓冲的 5000 点在重启时全部丢失。
        // 每 30s tick 主动尝试把 fallback 缓冲 drain 出来落盘。
        // 失败时按原路径回退（用 fallback_retry_count 走反弹语义）。
        // 避免与上层 enqueue 路径行为漂移。
                if !fallback.lock().is_empty()
            {
        // 取一份快照（drain 出来的 items 已被 move 进闭包；失败时
        // 由 push_into_fallback 推回——与上层 flush_fallback 行为一致）。
        let items: Vec<DataPoint> = fallback.lock().drain(..).collect();
                            let n = items.len();
                            let storage_clone = storage.clone();
                            let fallback_clone = fallback.clone();
                            let retry_count_clone = fallback_retry_count.clone();
                            let metrics_clone = metrics.clone();
                            let sql_result = tokio::task::spawn_blocking(
                                move || -> Result<usize, (Vec<DataPoint>, GatewayError)> {
                                    let inner = std::panic::catch_unwind(
                                        std::panic::AssertUnwindSafe(|| storage_clone.enqueue_batch(&items))
                                    );
                                    match inner {
                                        Ok(Ok(n)) => Ok(n),
                                        Ok(Err(e)) => Err((items, e)),
                                        Err(p) => {
                                            let msg = p.downcast_ref::<&str>()
                                                .map(|s| (*s).to_string())
                                                .or_else(|| p.downcast_ref::<String>().cloned())
                                                .unwrap_or_else(|| "<unknown panic>".to_string());
                                            Err((items, GatewayError::Queue(format!("fallback flush panic: {}", msg))))
                                        }
                                    }
                                },
                            ).await;
                            match sql_result {
                                Ok(Ok(_)) => {
                                    // 写成功后重置反弹计数
                                    *retry_count_clone.lock() = 0;
                                    tracing::info!(
                                        "fallback flush by tick: {} points persisted to SQLite",
                                        n
                                    );
                                }
                                Ok(Err((pts, e))) => {
                                    metrics_clone
                                        .lock()
                                        .queue_persist.persist_failed
                                        .fetch_add(1, Ordering::Relaxed);
                                    if is_disk_full_error(&e) {
                                        tracing::warn!("fallback flush: disk full, will retry: {}", e);
                                    } else {
                                        tracing::warn!("fallback flush failed: {}", e);
                                    }
                                    // 写失败：回退到 fallback 缓冲。复用 push_into_fallback
                                    // 走反弹计数与优先级淘汰。
        let mut cnt = retry_count_clone.lock();
                                    let (retained, dropped) = {
                                        let mut buf = fallback_clone.lock();
                                        crate::queue::push_into_fallback(
                                            &mut buf,
                                            crate::queue::FALLBACK_CAPACITY,
                                            pts,
                                            &mut cnt,
                                        )
                                    };
                                    if dropped > 0 {
                                        metrics_clone
                                            .lock()
                                            .queue_persist.persist_dropped
                                            .fetch_add(dropped as u64, Ordering::Relaxed);
                                        if *cnt == 0 {
                                            metrics_clone
                                                .lock()
                                                .queue_persist.fallback_bounce
                                                .fetch_add(1, Ordering::Relaxed);
                                        }
                                    }
                                    tracing::info!(
                                        "fallback flush: {} points reclaimed to buffer (retained={}, dropped={}, bounce={})",
                                        n, retained, dropped, *cnt
                                    );
                                }
                                Err(e) => tracing::error!("fallback flush spawn_blocking join failed: {}", e),
                            }
                        }
                        // 等待排空的调用方检查
                if pending_drain.is_some() && storage.is_empty()
            {
                            if let Some(tx) = pending_drain.take() {
                                let _ = tx.send(());
                            }
                        }
                    }
                }
    }

    Ok(())
}

#[cfg(test)]
mod tests;
