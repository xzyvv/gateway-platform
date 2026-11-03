//! 内置模拟消费者（SimConsumer）
//!
//! 用途：现场/开发环境没有可用 MQTT 通道时，让队列数据仍走完
//! 「出队 → 处理 → ack」完整链路——消除「队列有积压却没有消费者」的告警歧义，
//! 同时不外发任何数据（只是把数据消费掉，不写 outbox、不发网络包）。
//!
//! 与 MQTT 发布循环（`north/mqtt/publish.rs`）的语义对齐：
//! - 同样按批 dequeue → 处理 → ack；
//! - ack 失败时 release（不删数据），保证不丢；
//! - 失败路径一律 warn，不静默。
//!
//! ## 运行时可控（start / stop）
//!
//! 消费者是**运行时可启停**的：配置里的 `enabled` 只表示「启动时是否自动拉起」，
//! 运行中的开关由 API 控制（`POST /api/sim_consumer/start|stop`）。
//! 因此本模块内部持有 run 循环的句柄：
//!
//! - [`SimConsumer::start`] 幂等（已在运行返回 false），新建取消令牌并 spawn run 循环；
//! - [`SimConsumer::stop`] 幂等，发取消信号后**等待 run 循环真正退出**（上限 5s，
//!   超时则 abort 任务，保证「返回后队列不会再被消费」）——与 gatewayd 其它北向
//!   通道「先发停止信号再 await 退出」的停机序一致；
//! - [`SimConsumer::is_running`] 报告实际运行态（不是配置意图）。
//!
//! 运行参数（batch_size / interval_ms / log_every）只能在**未运行**时修改，
//! 见 [`SimConsumer::update_params`]。
//!
//! 停机：`stop()` 之后 run 循环立即退出（内部 [`CancellationToken`]）。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::config::SimConsumerConfig;
use crate::engine::error::{GatewayError, GatewayResult};
use crate::engine::metrics::Metrics;
use crate::engine::supervision::spawn_catch_unwind;
use crate::queue::{DataQueue, QueuedItem};

/// 摘要日志里展示的最近点位名样例个数。
const LOG_POINT_SAMPLE: usize = 3;

/// 等待 run 循环退出的上限。
///
/// 与 gatewayd 停机序的 `RUN_EXIT_TIMEOUT`（5s）保持一致：run 循环在收到取消
/// 信号后最多等一个轮询间隔（默认 1s）就会退出，5s 足够覆盖。超时说明循环卡在
/// 不感知取消的队列调用上，此时 [`SimConsumer::stop`] 会 abort 任务——调用方
/// 需要「stop 返回即不再消费」的确定语义。
const STOP_EXIT_TIMEOUT: Duration = Duration::from_secs(5);

/// 模拟消费者运行统计（快照）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SimConsumerStats {
    /// 已成功消费（ack）的数据条数。
    pub consumed: u64,
    /// 已成功消费的批次数。
    pub batches: u64,
    /// 最近一次消费成功的时刻（epoch 毫秒）；尚未消费过时为 None。
    pub last_consumed_at_ms: Option<u64>,
    /// 是否正在运行（实际运行态，不是配置意图）。
    pub running: bool,
    /// 本次运行的启动时刻（epoch 毫秒）；未运行时为 None。
    pub started_at_ms: Option<u64>,
    /// 本次运行已持续时长（毫秒）；未运行时为 None。
    pub uptime_ms: Option<u64>,
}

/// 一次运行的内部状态：取消令牌 + run 句柄 + 退出通知。
struct Runtime {
    /// 停机信号：`stop()` 触发后 run 循环退出。
    cancel: CancellationToken,
    /// run 循环句柄：只有第一个 `stop()` 取走并 await，其余 `stop()` 改为等退出通知。
    handle: Option<tokio::task::JoinHandle<()>>,
    /// run 循环是否已退出（配合 `notify` 避免通知丢失）。
    exited: Arc<AtomicBool>,
    /// run 循环退出通知。
    notify: Arc<tokio::sync::Notify>,
}

/// run 循环退出收尾守卫。
///
/// 为什么用 Drop 而不是把收尾写在 run 循环末尾：run 循环 panic 时末尾语句
/// 不会执行，运行态标志会永远停在 true——状态接口显示「在跑」实则已死，
/// 且 `stop()` 会一直等一个永远不来的退出通知。Drop 在展开（unwind）时
/// 同样会跑，故正常退出 / panic / 被 abort 三条路径都能保证：
/// 退出标志置位、运行态复位、等待者被唤醒。
struct ExitGuard {
    /// 消费者实例：用于复位运行态与 gauge。
    consumer: Arc<SimConsumer>,
    /// 退出标志（与 `Runtime.exited` 共享同一实例）。
    exited: Arc<AtomicBool>,
    /// 退出通知（与 `Runtime.notify` 共享同一实例）。
    notify: Arc<tokio::sync::Notify>,
}

impl Drop for ExitGuard {
    fn drop(&mut self) {
        self.exited.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
        self.consumer.on_run_finished();
    }
}

/// 内置模拟消费者。
///
/// 线程安全：内部状态用原子量 + 短临界区锁（运行参数用 `RwLock`，日志样例用
/// `Mutex`），可跨 task 共享（`Arc<SimConsumer>`）。
pub struct SimConsumer {
    queue: Arc<DataQueue>,
    /// 配置快照（`enabled` = 启动时是否自动拉起；其余三项为运行参数）。
    cfg: RwLock<SimConsumerConfig>,
    /// 可选的指标出口；未注入时**不写指标**（绝不 panic）。
    metrics: Option<Arc<Metrics>>,
    /// 当前运行态；`None` = 未运行（或已停止完毕）。
    runtime: Mutex<Option<Runtime>>,
    /// 是否正在运行（与 `runtime` 同步，供状态查询低开销读取）。
    running: AtomicBool,
    /// 本次运行的启动时刻（epoch 毫秒）；0 = 未运行。
    started_at_ms: AtomicU64,
    /// 已成功 ack 的数据条数。
    consumed: AtomicU64,
    /// 已成功 ack 的批次数。
    batches: AtomicU64,
    /// 最近一次消费成功的 epoch 毫秒；0 = 尚未消费过。
    last_consumed_at_ms: AtomicU64,
    /// 最近一批数据的点位名样例（最多 [`LOG_POINT_SAMPLE`] 个），供摘要日志展示。
    last_points: Mutex<Vec<String>>,
}

impl SimConsumer {
    /// 创建模拟消费者（**不启动** run 循环）。
    ///
    /// `metrics` 可选：未接入监控的部署 / 单测传 `None`，此时本组件不写任何
    /// 指标（指标恒为 0），但功能完全正常。
    pub fn new(
        queue: Arc<DataQueue>,
        cfg: SimConsumerConfig,
        metrics: Option<Arc<Metrics>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            queue,
            cfg: RwLock::new(cfg),
            metrics,
            runtime: Mutex::new(None),
            running: AtomicBool::new(false),
            started_at_ms: AtomicU64::new(0),
            consumed: AtomicU64::new(0),
            batches: AtomicU64::new(0),
            last_consumed_at_ms: AtomicU64::new(0),
            last_points: Mutex::new(Vec::new()),
        })
    }

    /// 启动消费循环（幂等）。
    ///
    /// 返回 `true` = 本次调用真正启动了 run 循环；`false` = 已在运行（或正在
    /// 停止过程中）未重复启动。
    ///
    /// 必须在 tokio 运行时上下文里调用（内部 spawn run 循环 task）。
    pub fn start(self: &Arc<Self>) -> bool {
        let mut rt = self.runtime.lock();
        if rt.is_some() {
            // 已在运行 / 正在停止：不再叠加第二个消费者——
            // 两个消费者会分流同一批队列数据，导致静默丢数。
            return false;
        }
        let cancel = CancellationToken::new();
        let exited = Arc::new(AtomicBool::new(false));
        let notify = Arc::new(tokio::sync::Notify::new());
        let me = self.clone();
        let cancel_for_run = cancel.clone();
        let exited_for_run = exited.clone();
        let notify_for_run = notify.clone();
        // 先置运行态再 spawn：run 循环若在 spawn 后立刻退出（panic 被吸收），
        // 退出守卫会把 running 复位成 false；若顺序反过来，守卫的复位会被
        // 这里的置位覆盖，运行态永远停在 true（状态接口显示「在跑」实则已死）。
        self.running.store(true, Ordering::SeqCst);
        self.started_at_ms.store(now_millis(), Ordering::Relaxed);
        self.set_running_gauge(true);
        // 沿用 gatewayd 北向通道的统一托管方式：panic 被 catch_unwind 吸收
        // 并计入 `gateway_task_panics_total`，不会静默带走整个消费者。
        let handle = spawn_catch_unwind("sim consumer", async move {
            // 守卫在展开（panic）时同样会跑，保证退出标志一定被置位。
            let _guard = ExitGuard {
                consumer: me.clone(),
                exited: exited_for_run,
                notify: notify_for_run,
            };
            me.run_loop(cancel_for_run).await;
        });
        *rt = Some(Runtime {
            cancel,
            handle: Some(handle),
            exited,
            notify,
        });
        drop(rt);

        let cfg = self.config();
        tracing::info!(
            "sim consumer started (batch={}, interval={}ms)",
            cfg.batch_size,
            cfg.interval_ms
        );
        true
    }

    /// 停止消费循环（幂等，可重复调用）。
    ///
    /// 发取消信号后**等待 run 循环真正退出**（上限 [`STOP_EXIT_TIMEOUT`]，
    /// 超时记 warn 但不 abort）。等待是必须的：调用方（如 API 停止接口、
    /// gatewayd 停机序）需要「返回后队列不会再被消费」这一确定语义，
    /// 否则停止返回后仍可能有数据被 ack 掉。
    pub async fn stop(&self) {
        let (handle, exited, notify) = {
            let mut rt = self.runtime.lock();
            let Some(r) = rt.as_mut() else {
                // 未运行：幂等返回
                return;
            };
            r.cancel.cancel();
            (r.handle.take(), r.exited.clone(), r.notify.clone())
        };

        match handle {
            Some(mut h) => {
                // 用 select! 而不是 timeout(h)：超时后 timeout 会把句柄一并消费掉，
                // 任务被 detach 后仍在后台消费队列，与「返回即不再消费」的契约冲突。
                tokio::select! {
                    res = &mut h => match res {
                        Ok(()) => {}
                        Err(e) => {
                            tracing::warn!("sim consumer run loop join error (panicked?): {}", e);
                        }
                    },
                    _ = tokio::time::sleep(STOP_EXIT_TIMEOUT) => {
                        // 取消信号已发但循环仍未退出：通常是卡在 dequeue/ack 的
                        // 队列 actor 等待上（这两处不感知取消信号）。此时必须 abort，
                        // 否则 stop() 返回后数据仍可能被 ack 掉。abort 只会丢弃等待中的
                        // oneshot 回包，不会撕裂 SQLite 写；已出队未 ack 的行由队列的
                        // stale in_flight 清理（5 分钟）放回可出队状态。
                        tracing::warn!(
                            "sim consumer run loop still running after {:?}, aborting",
                            STOP_EXIT_TIMEOUT
                        );
                        h.abort();
                        // 等 abort 真正生效，保证调用方拿到「不再消费」的确定语义。
                        let _ = h.await;
                    }
                }
            }
            None => {
                // 另一个 stop() 已取走句柄并在等待：等退出通知即可。
                wait_exited(&exited, &notify, STOP_EXIT_TIMEOUT).await;
            }
        }

        {
            let mut rt = self.runtime.lock();
            // 只有当 runtime 仍是我们这次停掉的那一份（句柄已被取走）才复位
            // 运行态，避免把并发 start() 刚装上的新运行态误删。
            //
            // 顺序要点：**先复位运行态、再清 runtime**。反过来的话，并发的
            // start() 刚置上的 running=true 会被这里的复位覆盖，状态接口会
            // 显示「已停」但消费者其实还在消费队列数据。
            if rt.as_ref().map(|r| r.handle.is_none()).unwrap_or(false) {
                self.running.store(false, Ordering::SeqCst);
                self.started_at_ms.store(0, Ordering::Relaxed);
                self.set_running_gauge(false);
                *rt = None;
            }
        }
        tracing::info!("sim consumer stopped");
    }

    /// 是否正在运行（实际运行态，不是配置意图）。
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// 当前配置快照（`enabled` = 启动意图；其余为运行参数）。
    pub fn config(&self) -> SimConsumerConfig {
        self.cfg.read().clone()
    }

    /// 更新运行参数（batch_size / interval_ms / log_every）。
    ///
    /// **运行中不允许修改**：run 循环在每轮迭代开始时快照了 batch_size /
    /// interval_ms，运行中改会让「同一批数据用旧批量、新间隔」自相矛盾；
    /// log_every 直接决定摘要日志的节奏，与正在打印的摘要对不上。故运行中
    /// 一律返回 [`GatewayError::Config`]，由调用方先 stop 再改再 start。
    ///
    /// `enabled` 不在此更新：`enabled` 表示「启动时是否自动拉起」，属配置文件
    /// 语义（热重载按 `restart_required` 处理），运行期开关一律走 start/stop。
    pub fn update_params(
        &self,
        batch_size: Option<usize>,
        interval_ms: Option<u64>,
        log_every: Option<usize>,
    ) -> GatewayResult<()> {
        if self.is_running() {
            return Err(GatewayError::Config(
                "sim consumer 正在运行：运行参数不可修改，请先停止再修改".to_string(),
            ));
        }
        let mut cfg = self.cfg.write();
        if let Some(v) = batch_size {
            cfg.batch_size = v;
        }
        if let Some(v) = interval_ms {
            cfg.interval_ms = v;
        }
        if let Some(v) = log_every {
            cfg.log_every = v;
        }
        Ok(())
    }

    /// 当前运行统计快照。
    pub fn stats(&self) -> SimConsumerStats {
        let last = self.last_consumed_at_ms.load(Ordering::Relaxed);
        let running = self.is_running();
        let started_at_ms = self.started_at_ms.load(Ordering::Relaxed);
        let (started_at_ms, uptime_ms) = if running && started_at_ms > 0 {
            let now = now_millis();
            let uptime = now.saturating_sub(started_at_ms);
            (Some(started_at_ms), Some(uptime))
        } else {
            (None, None)
        };
        SimConsumerStats {
            consumed: self.consumed.load(Ordering::Relaxed),
            batches: self.batches.load(Ordering::Relaxed),
            last_consumed_at_ms: if last == 0 { None } else { Some(last) },
            running,
            started_at_ms,
            uptime_ms,
        }
    }

    /// run 循环：出队 → 消费 → ack，直到收到停止信号。
    async fn run_loop(self: Arc<Self>, cancel: CancellationToken) {
        loop {
            if cancel.is_cancelled() {
                break;
            }
            // 每轮重新取参数：未运行时改的参数在下一次 start 生效，
            // 运行中参数不可改（见 update_params），故这里不会读到中途变化的值。
            let cfg = self.config();
            let interval = Duration::from_millis(cfg.interval_ms.max(1));
            // batch_size = 0 会让 dequeue 恒返回空批 → 空转烧 CPU，这里兜底为 1。
            let batch_size = cfg.batch_size.max(1);
            let batch = match self.queue.dequeue(batch_size).await {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!("sim consumer dequeue failed: {}", e);
                    Vec::new()
                }
            };
            if batch.is_empty() {
                // 队列为空：等一个轮询间隔或停机信号。
                // biased 让停止信号优先于 sleep，保证停机不被 interval 拖慢。
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => break,
                    _ = tokio::time::sleep(interval) => {}
                }
                continue;
            }
            self.consume_batch(batch).await;
        }
        let stats = self.stats();
        tracing::info!(
            "sim consumer stopped (consumed={}, batches={})",
            stats.consumed,
            stats.batches
        );
    }

    /// 消费一批：模拟处理（不外发、恒视为成功）→ ack。
    ///
    /// 失败语义照 MQTT 发布循环：ack 报错时 release 这批 id，让数据回到
    /// 可出队状态，绝不因为「ack 失败」把数据丢在队列里。
    async fn consume_batch(&self, batch: Vec<QueuedItem>) {
        let ids: Vec<i64> = batch.iter().map(|i| i.id).collect();
        let n = batch.len();
        if let Err(e) = self.queue.ack(ids.clone()).await {
            tracing::warn!(
                "sim consumer ack failed ({} items), releasing for retry: {}",
                n,
                e
            );
            self.queue.release(ids).await;
            return;
        }
        self.record(n, &batch);
    }

    /// 记账 + 摘要日志（每 `log_every` 条打一条 INFO）。
    fn record(&self, n: usize, batch: &[QueuedItem]) {
        let consumed = self.consumed.fetch_add(n as u64, Ordering::Relaxed) + n as u64;
        let batches = self.batches.fetch_add(1, Ordering::Relaxed) + 1;
        self.last_consumed_at_ms
            .store(now_millis(), Ordering::Relaxed);
        {
            let mut last = self.last_points.lock();
            last.clear();
            last.extend(
                batch
                    .iter()
                    .take(LOG_POINT_SAMPLE)
                    .map(|i| i.point_id.clone()),
            );
        }
        if let Some(m) = &self.metrics {
            m.inc_sim_consumer_consumed(n as u64);
            m.inc_sim_consumer_batches(1);
        }
        let log_every = self.cfg.read().log_every as u64;
        if log_every == 0 {
            return;
        }
        // 跨过 log_every 的整数倍才打日志：一批可能跨多个整数倍，只打一条。
        if consumed / log_every > (consumed - n as u64) / log_every {
            let sample = self.last_points.lock().join(", ");
            tracing::info!(
                "sim consumer consumed {} point(s) in {} batch(es), last points: [{}]",
                consumed,
                batches,
                sample
            );
        }
    }

    /// run 循环自然退出后的收尾（把运行态标记清掉）。
    fn on_run_finished(&self) {
        self.running.store(false, Ordering::SeqCst);
        self.set_running_gauge(false);
    }

    /// 写运行状态 gauge（无 metrics 时跳过，绝不 panic）。
    fn set_running_gauge(&self, running: bool) {
        if let Some(m) = &self.metrics {
            m.set_sim_consumer_running(running);
        }
    }
}

/// 等待 run 循环退出（配合「退出标志 + Notify」，避免通知先于等待到达时丢失）。
///
/// 超时只返回、不 abort：调用方（停机序）继续推进即可。
async fn wait_exited(exited: &AtomicBool, notify: &tokio::sync::Notify, timeout: Duration) {
    let waiter = async {
        loop {
            if exited.load(Ordering::SeqCst) {
                return;
            }
            let notified = notify.notified();
            tokio::pin!(notified);
            // 先注册再复查：覆盖「退出发生在注册之前」的窗口。
            notified.as_mut().enable();
            if exited.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    };
    if tokio::time::timeout(timeout, waiter).await.is_err() {
        tracing::warn!(
            "sim consumer run loop still running after {:?}, continuing",
            timeout
        );
    }
}

/// 当前 epoch 毫秒（失败时退化为 0，不影响主流程）。
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::engine::model::{DataPoint, Quality};
    use crate::queue::{MemoryStorage, QueueStorage};
    use std::sync::atomic::AtomicUsize;

    fn make_point(id: &str, value: f64) -> DataPoint {
        let now = DataPoint::now();
        DataPoint {
            point_id: id.into(),
            device_id: "dev1".into(),
            value,
            raw_value: value,
            raw_type: "u16".into(),
            quality: Quality::Good,
            quality_reason: "".into(),
            timestamp: now,
            source_ts: now,
            stale_count: 0,
            value_text: None,
        }
    }

    fn test_cfg() -> SimConsumerConfig {
        SimConsumerConfig {
            enabled: true,
            batch_size: 10,
            interval_ms: 10,
            log_every: 100,
        }
    }

    /// 等待队列被消费干净（最多 5s）。
    async fn wait_empty(queue: &Arc<DataQueue>) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while queue.len().await > 0 && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// 契约：消费者启动后应把队列里预置的数据全部消费掉（len 归零），
    /// 且 stats.consumed 与预置条数一致。
    #[tokio::test]
    async fn consumes_all_queued_points() {
        let queue = Arc::new(DataQueue::new_memory().unwrap());
        let points: Vec<DataPoint> = (0..5)
            .map(|i| make_point(&format!("p{i}"), i as f64))
            .collect();
        queue.enqueue(points).await.unwrap();
        assert_eq!(queue.len().await, 5);

        let consumer = SimConsumer::new(queue.clone(), test_cfg(), None);
        assert!(consumer.start(), "首次 start 应返回 true");

        wait_empty(&queue).await;
        consumer.stop().await;

        assert_eq!(queue.len().await, 0, "队列应被消费干净");
        let stats = consumer.stats();
        assert_eq!(stats.consumed, 5, "消费条数应与预置条数一致");
        assert_eq!(stats.batches, 1, "5 条（batch_size=10）应只产生 1 个批次");
        assert!(stats.last_consumed_at_ms.is_some(), "应记录最近消费时刻");
    }

    /// 契约：start 幂等——已在运行时重复 start 返回 false，且不会叠加第二个
    /// 消费者（两个消费者会分流同一批队列数据，导致静默丢数）。
    #[tokio::test]
    async fn start_is_idempotent() {
        let queue = Arc::new(DataQueue::new_memory().unwrap());
        let consumer = SimConsumer::new(queue.clone(), test_cfg(), None);

        assert!(!consumer.is_running(), "初始不应处于运行态");
        assert!(consumer.start(), "首次 start 应返回 true");
        assert!(consumer.is_running(), "start 后应为运行态");
        assert!(!consumer.start(), "重复 start 应返回 false（幂等）");
        assert!(consumer.is_running(), "重复 start 后仍应为运行态");

        consumer.stop().await;
        assert!(!consumer.is_running(), "stop 后应回到非运行态");
        // 停止后允许再次启动
        assert!(consumer.start(), "停止后应可再次启动");
        consumer.stop().await;
    }

    /// 契约：stop 幂等——未运行时调用不报错；且 stop 返回后 run 循环已退出，
    /// 之后新入队的数据不会被消费。
    #[tokio::test]
    async fn stop_is_idempotent_and_stops_consuming() {
        let queue = Arc::new(DataQueue::new_memory().unwrap());
        let consumer = SimConsumer::new(queue.clone(), test_cfg(), None);

        // 未运行直接 stop：幂等，不 panic
        consumer.stop().await;
        assert!(!consumer.is_running());

        consumer.start();
        consumer.stop().await;
        // 重复 stop：幂等
        consumer.stop().await;
        assert!(!consumer.is_running(), "stop 后不得处于运行态");

        // 停止后再入队：数据应原封不动留在队列里。
        queue.enqueue(vec![make_point("p1", 1.0)]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(queue.len().await, 1, "停止后不得再消费队列数据");
        assert_eq!(consumer.stats().consumed, 0, "停止后消费计数不得增长");
    }

    /// 契约：stats 的 running / started_at_ms / uptime_ms 必须反映真实运行态。
    #[tokio::test]
    async fn stats_reports_running_state() {
        let queue = Arc::new(DataQueue::new_memory().unwrap());
        let consumer = SimConsumer::new(queue.clone(), test_cfg(), None);

        let s = consumer.stats();
        assert!(!s.running, "未运行时 running 应为 false");
        assert!(
            s.started_at_ms.is_none(),
            "未运行时 started_at_ms 应为 None"
        );
        assert!(s.uptime_ms.is_none(), "未运行时 uptime_ms 应为 None");

        consumer.start();
        tokio::time::sleep(Duration::from_millis(30)).await;
        let s = consumer.stats();
        assert!(s.running, "运行中 running 应为 true");
        assert!(s.started_at_ms.is_some(), "运行中应有启动时刻");
        assert!(
            s.uptime_ms.unwrap() >= 20,
            "uptime 应反映已运行时长, got {:?}",
            s.uptime_ms
        );

        consumer.stop().await;
        let s = consumer.stats();
        assert!(!s.running, "停止后 running 应为 false");
        assert!(s.started_at_ms.is_none(), "停止后 started_at_ms 应为 None");
        assert!(s.uptime_ms.is_none(), "停止后 uptime_ms 应为 None");
    }

    /// 契约：运行参数只能在未运行时修改——运行中修改必须返回错误（不静默忽略），
    /// 停止后修改成功并在下次 start 生效。
    #[tokio::test]
    async fn params_cannot_change_while_running() {
        let queue = Arc::new(DataQueue::new_memory().unwrap());
        let consumer = SimConsumer::new(queue.clone(), test_cfg(), None);

        // 未运行：允许修改
        consumer
            .update_params(Some(5), Some(20), Some(7))
            .expect("未运行时应允许修改参数");
        let cfg = consumer.config();
        assert_eq!(cfg.batch_size, 5);
        assert_eq!(cfg.interval_ms, 20);
        assert_eq!(cfg.log_every, 7);

        consumer.start();
        let err = consumer
            .update_params(Some(9), None, None)
            .expect_err("运行中修改参数必须返回错误");
        assert!(
            err.to_string().contains("正在运行"),
            "错误信息应说明运行中不可改, got {}",
            err
        );
        // 参数未被改动
        assert_eq!(consumer.config().batch_size, 5);

        consumer.stop().await;
        consumer
            .update_params(Some(9), None, None)
            .expect("停止后应可修改参数");
        assert_eq!(consumer.config().batch_size, 9);
    }

    /// 契约：ack 失败时数据不得丢失——存储层 ack 失败时该行仍留在队列里，
    /// 且必须走过 release 分支（in_flight 被清掉，数据回到可出队状态），
    /// 否则这批数据会被永久卡在 in_flight 上直到 stale 清理。
    #[tokio::test]
    async fn ack_failure_releases_batch_for_retry() {
        let storage = Arc::new(FailingAckStorage::new());
        let queue = Arc::new(DataQueue::with_storage(storage.clone()).unwrap());
        queue.enqueue(vec![make_point("p1", 1.0)]).await.unwrap();

        let consumer = SimConsumer::new(queue.clone(), test_cfg(), None);
        consumer.start();

        // 等消费者至少尝试一轮（ack 必失败 → 走 release 分支）。
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while storage.release_calls.load(Ordering::SeqCst) == 0
            && std::time::Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        consumer.stop().await;

        assert!(
            storage.release_calls.load(Ordering::SeqCst) > 0,
            "ack 失败后必须调用 release 把数据放回可出队状态"
        );
        assert_eq!(
            queue.len().await,
            1,
            "ack 失败时数据必须留在队列里（不得被删除）"
        );
        assert_eq!(consumer.stats().consumed, 0, "ack 失败不得计入消费条数");
    }

    /// 契约：注入 metrics 时计数随消费累加；未注入 metrics 时不得 panic。
    #[tokio::test]
    async fn metrics_are_updated_when_injected() {
        let metrics = Arc::new(Metrics::new());
        let queue = Arc::new(DataQueue::new_memory().unwrap());
        queue.enqueue(vec![make_point("p1", 1.0)]).await.unwrap();

        let consumer = SimConsumer::new(queue.clone(), test_cfg(), Some(metrics.clone()));
        consumer.start();
        wait_empty(&queue).await;
        consumer.stop().await;

        assert_eq!(
            metrics.sim_consumer.consumed_total.load(Ordering::Relaxed),
            1,
            "消费条数应写入 metrics"
        );
        assert_eq!(
            metrics.sim_consumer.batches_total.load(Ordering::Relaxed),
            1,
            "消费批次数应写入 metrics"
        );
        assert_eq!(
            metrics.sim_consumer.running.load(Ordering::Relaxed),
            0,
            "停止后运行状态 gauge 应为 0"
        );
    }

    /// 测试用存储后端：`ack_batch` 恒失败，其余行为委托给内存后端。
    ///
    /// 额外记录 `release_in_flight` 的调用次数——用于断言「ack 失败 → release
    /// 重投」分支真实可达（否则数据会卡在 in_flight 上）。
    struct FailingAckStorage {
        inner: MemoryStorage,
        release_calls: AtomicUsize,
    }

    impl FailingAckStorage {
        fn new() -> Self {
            Self {
                inner: MemoryStorage::new(),
                release_calls: AtomicUsize::new(0),
            }
        }
    }

    impl QueueStorage for FailingAckStorage {
        fn enqueue_batch(&self, points: &[DataPoint]) -> GatewayResult<usize> {
            self.inner.enqueue_batch(points)
        }
        fn dequeue_batch(&self, limit: usize) -> GatewayResult<Vec<QueuedItem>> {
            self.inner.dequeue_batch(limit)
        }
        fn ack_batch(&self, _ids: &[i64]) -> GatewayResult<()> {
            Err(GatewayError::Queue("test: ack failed".into()))
        }
        fn release_in_flight(&self, ids: &[i64]) -> GatewayResult<()> {
            self.release_calls.fetch_add(1, Ordering::SeqCst);
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
            self.inner.needs_maintenance()
        }
        fn name(&self) -> &str {
            "failing-ack"
        }
    }
}
