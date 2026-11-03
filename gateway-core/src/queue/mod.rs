//! 断点续传队列（§10）
//!
//! - SQLite WAL 模式，写入性能高，不阻塞读
//! - 单线程独占 Connection，通过 mpsc 通道串行访问（无 Mutex 死锁风险）
//! - 入队通道有界，1 万条上限，满了丢旧数据
//! - ACK 机制：MQTT QoS1 发布成功 → 删除记录；失败 → 保留重传
//! - in_flight 超时自动释放（5 分钟）, 优雅退出排空等待
//!
//! Bug 111: 所有队列子模块（history / wal / retention / limit）共享约束：
//!   - SQLite WAL 模式允许多读单写，写入路径必须串行化（mpsc 通道 / 单一 owner）
//!   - 长事务会阻塞 checkpoint（默认 1000 页），单事务 INSERT/DELETE 控制在 1000 条内
//!   - 磁盘满（< 10%）时 spawn_disk_monitor 设置 disk_ok=false，DataQueue 入队时拒绝
//!   - 入队失败 / 通道满等异常路径必须要 metric + warn 日志，避免静默丢数据

// 目录分层：
// - `core/`    队列内核（命令循环 / 容量策略 / 记录模型 / 密钥），不依赖具体存储后端
// - `storage/` 存储 SPI（QueueStorage）与实现（memory / sqlite）
// 注意: 若将来需要多连接并发读优化，可新增 pool.rs 并恢复 `mod pool;` 声明
mod core;
mod storage;

pub use self::core::QueuedItem;
pub use self::storage::MemoryStorage;
pub use self::storage::QueueStorage;
pub use self::storage::SqliteStorage;
/// P2-1：历史查询结果类型（含降采样样本与分页元信息），供 web 层直接序列化。
pub use self::storage::{HistoryQueryResult, HistorySample};

use self::core::{queue_task, QueueCommand};

use std::sync::Arc;

use tokio::sync::{mpsc, oneshot, Notify};

use crate::engine::error::{GatewayError, GatewayResult};
use crate::engine::metrics::Metrics;
use crate::engine::model::DataPoint;

/// Bug 170: 内存降级缓冲容量上限（磁盘满或 SQLite 写失败时暂存数据点）
pub(crate) const FALLBACK_CAPACITY: usize = 5000;

/// Bug 183 (N01): fallback 缓冲里同一批数据点允许的最大重试轮次。
///
/// 持久化非磁盘满错误（DB 约束/IO）会触发"写失败 → 回退 fallback → flush_fallback → 又写失败"
/// 的无限反弹，占用 CPU/IO 且无任何告警。达到本轮次上限后该批数据点被强制丢弃，
/// 触发 `queue_persist_dropped_total` 指标增长，提醒运维介入。
/// 设为 5：足以容忍短暂瞬态错误（重启、锁竞争），但能快速阻断病态反弹。
pub(crate) const FALLBACK_MAX_RETRIES: u32 = 5;

/// P0-1：队列健康判定的唯一口径对外导出。
///
/// `/health/ready`（components.queue）、`/api/health/report`（problems）与
/// `/api/watchdog/services`（queue 服务）必须全部调用 [`queue_health`]，
/// 阈值只在 [`self::core::limit`] 里定义一次，禁止各页面再硬编码数字。
pub use self::core::limit::{
    capacity_pressure_rows, queue_health, QueueHealth, BACKLOG_WARN_ROWS, BACKPRESSURE_ROWS,
    MAX_QUEUE_ROWS,
};

/// P0-3：积压采样周期。
///
/// 30s 足够覆盖「网线被拔 → 运维收到告警」的时效要求，也避免秒级采样把
/// 采集抖动产生的瞬时峰值当成故障。
const BACKLOG_ALARM_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// 「当前是否存在北向消费者」的运行期求值探针。
///
/// 消费者（MQTT publish 循环、内置模拟消费者）都是**可运行时启停**的：
/// 启动期一次性求值出的 bool 会在运行期失真——运行期拉起模拟消费者后，
/// 积压告警仍按「无消费者」口径输出，把「有消费者但处理不足」误报成
/// 「本地历史存量」。故监控任务每轮采样时重新求值。
///
/// 用 `Arc<dyn Fn() -> bool>` 而不是直接持 north 引用：`queue` 模块不得依赖
/// `crate::north`（见 `tests/arch_deps.rs` 的分层约束），故由调用方注入
/// 中立的求值闭包，把「消费者有哪些」的知识留在上层。
pub type NorthConsumerProbe = Arc<dyn Fn() -> bool + Send + Sync>;

/// P0-3：积压告警沿判定（按健康等级比较，而非只看单一阈值）。
///
/// - `Some(new)`：等级发生变化（升级/降级/恢复），需要发事件
///   - 升级到 Backlog：warning 级告警 + 计数
///   - 升级到 Backpressure / CapacityPressure：critical 级告警 + 计数
///   - 回到 Ok：发 Info 恢复事件，不计数
/// - `None`：等级未变，不重复告警
fn backlog_alarm_edge(last: QueueHealth, now: QueueHealth) -> Option<QueueHealth> {
    (now != last).then_some(now)
}

/// Bug 170: 数据点优先级分数（越高越优先保留）
/// Good=3, Uncertain=2, Stale=1, Bad=0；stale_count 越高优先级越低
pub(crate) fn priority_score(dp: &DataPoint) -> i32 {
    use crate::engine::model::Quality;
    let base = match dp.quality {
        Quality::Good => 3,
        Quality::Uncertain => 2,
        Quality::Stale => 1,
        Quality::Bad => 0,
    };
    base * 1000 - (dp.stale_count as i32)
}

/// Bug 170: 找到缓冲区中优先级最低的条目索引
pub(crate) fn find_lowest_priority_idx(
    buf: &std::collections::VecDeque<DataPoint>,
) -> Option<usize> {
    buf.iter()
        .enumerate()
        .min_by_key(|(_, dp)| priority_score(dp))
        .map(|(idx, _)| idx)
}

/// Bug 170: 把一批数据点回退进 fallback 缓冲，超容量时按优先级淘汰最低者。
///
/// 供两处复用：`DataQueue::enqueue` 的磁盘满分支，以及队列任务写 SQLite 失败。
/// （磁盘满 / DB 约束 / IO 错误）的回退路径——后者修复了原先"非磁盘满错误被
/// warn 丢点"的静默数据丢失。
pub(crate) fn push_into_fallback(
    buf: &mut std::collections::VecDeque<DataPoint>,
    capacity: usize,
    points: Vec<DataPoint>,
    retry_count: &mut u32,
) -> (usize, usize) {
    // Bug 183 (N01): 反弹轮次达到上限后强制丢弃本批点。
    if *retry_count >= FALLBACK_MAX_RETRIES {
        let dropped = points.len();
        *retry_count = 0; // 重置，给下一批（非病态）数据点机会
        return (0, dropped);
    }
    *retry_count += 1;
    let n_in = points.len();
    for dp in points {
        buf.push_back(dp);
    }
    let mut dropped = 0usize;
    while buf.len() > capacity {
        if let Some(idx) = find_lowest_priority_idx(buf) {
            buf.remove(idx);
        } else {
            buf.pop_front();
        }
        dropped += 1;
    }
    let retained = n_in.saturating_sub(dropped);
    (retained, dropped)
}

pub struct DataQueue {
    tx: mpsc::Sender<QueueCommand>,
    _handle: tokio::task::JoinHandle<()>,
    /// 磁盘是否可写（由 health_monitor 更新，磁盘满时阻止入队保护SQLite）
    disk_ok: Arc<std::sync::atomic::AtomicBool>,
    /// S2: 磁盘满降级内存环形缓冲    
    ///
    /// Bug 170: 改为 `Arc` 共享给 queue_task——队列任务写 SQLite 失败（磁盘满、    
    /// 非磁盘满、DB 错误）时，由任务侧把整批点回退进该缓冲，避免静默丢数据。
    fallback_buffer: Arc<parking_lot::Mutex<std::collections::VecDeque<DataPoint>>>,
    /// Bug 183 (N01): fallback 缓冲里的数据点累积失败轮次，
    /// 达到 FALLBACK_MAX_RETRIES 后强制丢弃，避免非磁盘满 DB 错误无限反弹。    
    /// 占位计数；任务侧已用本地 Arc 持有独立计数器，
    /// 上层 self.fallback_retry_count 主要供测试和上层 enqueue 失败时    
    /// 反馈当前反弹轮次。
    #[allow(dead_code)]
    fallback_retry_count: Arc<parking_lot::Mutex<u32>>,
    /// Bug 183 (N01): 共享给 queue_task 写失败路径，用于累计指标。    
    ///
    /// Bug T6: 改为 `Arc<Mutex<Arc<Metrics>>>` 句柄，与 queue_task 共享同一实例——    
    /// 原实现 DataQueue 与 queue_task 各持有一份独立占位 Metrics，bootstrap 阶段
    /// 即使调用 `set_metrics` 也无法影响任务侧，导致任务写失败 / fallback 丢弃计数
    /// 永远写入无人读取的对象，静默丢数不可观测。
    metrics: Arc<parking_lot::Mutex<Arc<crate::engine::metrics::Metrics>>>,
    /// #20-2026：存储后端句柄（trait 对象）。仅用于 enqueue 入口读    
    /// `storage.unsent_overflow()`，与 queue_task 持有的句柄共享同一份状态。    
    /// 不存这份无法在 enqueue 时做"未送达比例超限 → fallback 缓冲"决策。
    storage: Arc<dyn QueueStorage>,
    /// fallback 告警沿标记：false→true 的「进入沿」才 WARN 一次，
    /// 稳态重复入队降为 DEBUG，避免北向无消费者（unsent 比例恒超限）时
    /// 每次 enqueue 都刷屏 "disk full"（实际一条没丢，文案还误导）。
    /// 退出 fallback（磁盘恢复 / unsent 回落）时复位为 false。
    fallback_warned: std::sync::atomic::AtomicBool,
}

impl DataQueue {
    /// 设置磁盘是否可写（由 health_monitor 调用，磁盘满时阻止 enqueue）。
    pub fn set_disk_ok(&self, ok: bool) {
        self.disk_ok.store(ok, std::sync::atomic::Ordering::Relaxed);
    }

    /// S2: fallback 缓冲区当前条目数
    pub fn fallback_len(&self) -> usize {
        self.fallback_buffer.lock().len()
    }

    /// 使用指定存储后端创建并启动队列。
    pub fn with_storage(storage: Arc<dyn QueueStorage>) -> GatewayResult<Self> {
        let (tx, rx) = mpsc::channel::<QueueCommand>(10000);
        let drain_notify = Arc::new(Notify::new());
        let notify_clone = drain_notify.clone();

        // Bug 170: fallback 缓冲改为 Arc 共享——队列任务在 SQLite 写失败时也要用它回退数据
        let fallback_buffer = Arc::new(parking_lot::Mutex::new(
            std::collections::VecDeque::with_capacity(FALLBACK_CAPACITY),
        ));
        let fallback_for_task = fallback_buffer.clone();
        // Bug 183 (N01): fallback 共享给 queue_task，用于累计重试计数。
        let fallback_retry_for_task = Arc::new(parking_lot::Mutex::new(0u32));
        // Bug T6: metrics 句柄在 DataQueue 与 queue_task 之间共享同一份
        // `Arc<Mutex<Arc<Metrics>>>`——`set_metrics` 注入真实指标后两侧同时生效，
        // 任务侧写失败 / 丢弃计数即可被 /api/metrics 观测。
        let metrics_handle = Arc::new(parking_lot::Mutex::new(Arc::new(
            crate::engine::metrics::Metrics::new(),
        )));
        let metrics_for_task = metrics_handle.clone();
        // #20-2026：DataQueue 自身保留一份 storage 句柄；
        // enqueue 入口需要读 storage.unsent_overflow() 判断降级路径。
        // queue_task 也持有一份（独立 task 上下文）。
        let storage_for_self = storage.clone();

        let handle = crate::engine::supervision::spawn_catch_unwind("queue_task", async move {
            if let Err(e) = queue_task(
                storage,
                rx,
                notify_clone,
                fallback_for_task,
                fallback_retry_for_task,
                metrics_for_task,
            )
            .await
            {
                tracing::error!("data queue task error: {}", e);
            }
        });

        Ok(Self {
            tx,
            _handle: handle,
            disk_ok: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            fallback_buffer,
            // Bug 183 (N01): 占位计数；任务侧已用本地 Arc 持有独立计数器，
            fallback_retry_count: Arc::new(parking_lot::Mutex::new(0)),
            // Bug 183 (N01): 占位 metrics 句柄；由 set_metrics 在 bootstrap 阶段替换。
            // Bug T6: 与 queue_task 共享同一句柄，替换后任务侧同步生效。
            metrics: metrics_handle,
            // #20-2026：storage 句柄用于 enqueue 入口读 unsent_overflow
            storage: storage_for_self,
            fallback_warned: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Bug 183 (N01): 注入真实 metrics，使 queue_task 写失败路径与上层 enqueue
    /// 共享同一指标对象。调用方在 bootstrap 阶段拿到 metrics 后必须调用一次。    
    ///
    /// Bug T6: 句柄与 queue_task 共享，本方法替换后任务侧下一次计数即写入真实
    /// Metrics（此前无调用点，任务侧计数落入占位对象，静默丢数不可观测）。
    pub fn set_metrics(&self, metrics: Arc<crate::engine::metrics::Metrics>) {
        *self.metrics.lock() = metrics;
    }

    /// 创建 SQLite 后端队列（生产环境，持久化）
    pub fn new(db_path: &str, metrics: Arc<Metrics>) -> GatewayResult<Self> {
        let storage: Arc<dyn QueueStorage> =
            Arc::new(SqliteStorage::new(db_path, metrics.clone())?);
        let queue = Self::with_storage(storage)?;
        // Bug T6: 把真实 metrics 注入共享句柄，保证 queue_task 写失败 / 丢弃计数
        // 与存储层上报到同一 Metrics 实例（原 `set_metrics` 无调用点）。
        queue.set_metrics(metrics);
        Ok(queue)
    }

    /// 创建内存后端队列（测试 / 轻量场景，无持久化）
    pub fn new_memory() -> GatewayResult<Self> {
        let storage: Arc<dyn QueueStorage> = Arc::new(MemoryStorage::new());
        Self::with_storage(storage)
    }

    /// v2.0 Step 5: 设置历史数据保留天数（license `storage_days` 配额；None = 不限）。    
    ///
    /// 由 bootstrap 的配额对账任务周期调用；发送命令给队列 actor 更新存储后端
    /// 的保留天数槽位，实际 FIFO 截断由 maintenance 周期执行。内存后端为 no-op。    
    /// 配额同步是尽力而为（advisory）：队列任务已退出时静默忽略。
    pub async fn set_storage_retention_days(&self, days: Option<u32>) {
        let _ = self.tx.send(QueueCommand::SetStorageRetention(days)).await;
    }

    /// 批量入队
    ///
    /// S2: 磁盘满时不再直接拒绝，而是存入内存环形缓冲 + 优先级丢弃策略。    
    /// 磁盘恢复后自动 flush 回 SQLite。    
    ///
    /// tracing: 入队 span（n=points.len()），tracing feature off 时 cfg_attr 剥离为零成本。
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, points), fields(n = points.len())))]
    pub async fn enqueue(&self, points: Vec<DataPoint>) -> GatewayResult<()> {
        if points.is_empty() {
            return Ok(());
        }
        // #20-2026：unsent_overflow（未送达行比例超限）等价于 disk_ok=false 的
        // 降级路径——P0-3 修复后未送达行被保护，broker 长期不可用时可能撑爆
        // 内存/磁盘。两种降级共用 fallback 缓冲与告警机制，避免策略漂移。
        let disk_ok = self.disk_ok.load(std::sync::atomic::Ordering::Relaxed);
        let unsent_overflow = self.storage.unsent_overflow();
        if !disk_ok || unsent_overflow {
            let reason = if !disk_ok {
                "disk full"
            } else {
                "unsent overflow"
            };
            // P3：原实现在此处向 EventBus 发布 Critical 告警事件，但 EventBus
            // 生产侧零订阅者，事件投递后即被丢弃——真正生效的告警是紧随其后的
            // tracing::warn!（含 reason / 缓冲量 / 丢弃数）。故此处不再重复发布。
            // Bug 170: 复用 push_into_fallback（超容量按优先级淘汰）——与队列任务
            // 写失败回退共用同一实现，避免两处淘汰策略漂移。
            // Bug 183 (N01): 磁盘满 + unsent overflow 都是「系统级」事件，不应触发反弹上限。
            // 传 `&mut 0u32` 等价于"本路径不消耗反弹计数"——disk_ok 恢复后
            // unsent_overflow 恢复后 flush_fallback 自然把数据落盘。
            let (retained, dropped_by_cap) = {
                let mut buf = self.fallback_buffer.lock();
                let mut no_bounce = 0u32; // 哨兵
                push_into_fallback(&mut buf, FALLBACK_CAPACITY, points, &mut no_bounce)
            };
            let buffered = self.fallback_buffer.lock().len();
            // Bug 183 (N01): 容量超限被踢出的点计入 dropped 指标。
            if dropped_by_cap > 0 {
                self.metrics
                    .lock()
                    .queue_persist
                    .persist_dropped
                    .fetch_add(dropped_by_cap as u64, std::sync::atomic::Ordering::Relaxed);
                // 真丢数据必须每次 WARN，不容降级
                tracing::warn!(
                    "queue fallback dropping data (reason={}): {} points in memory buffer (cap={}, retained={}, dropped={})",
                    reason,
                    buffered,
                    FALLBACK_CAPACITY,
                    retained,
                    dropped_by_cap
                );
            } else if !self
                .fallback_warned
                .swap(true, std::sync::atomic::Ordering::Relaxed)
            {
                // 仅在「进入 fallback 的沿」WARN 一次：北向无消费者时 unsent 比例
                // 恒超限，每次 enqueue 都 WARN 会每秒刷屏；且 dropped=0 一条没丢，
                // 原文案 "disk full" 属于误导（不是磁盘满，是没有消费者）。
                tracing::warn!(
                    "queue fallback active (reason={}, no consumer drained, nothing lost): {} points buffered (cap={})",
                    reason,
                    buffered,
                    FALLBACK_CAPACITY
                );
            } else {
                // 稳态重复：无丢失、已告警过，降为 DEBUG 留痕即可
                tracing::debug!(
                    "queue fallback steady state (reason={}, nothing lost): {} points buffered (cap={})",
                    reason,
                    buffered,
                    FALLBACK_CAPACITY
                );
            }
            return Ok(());
        }
        // 磁盘正常：先 flush fallback 缓冲
        // 已退出 fallback：复位告警沿标记，下次进入时重新 WARN 一次
        self.fallback_warned
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.flush_fallback().await?;
        self.tx
            .send(QueueCommand::Enqueue(points))
            .await
            .map_err(|e| GatewayError::Queue(format!("enqueue failed: {}", e)))
    }

    /// S2: 从 fallback 缓冲数据刷新到 SQLite 队列
    ///
    /// #1 修复（数据丢失）：原实现先 `drain` 缓冲再 `tx.send`，send 失败
    /// 时（actor 已退出 / 通道满）`items` 被 drop，整批点永久丢失。    
    /// 改为：drain 后尝试 send；失败时把整批**推回缓冲头部**（复用    
    /// `push_into_fallback` 的优先级淘汰语义，超容仍按策略踢最低优先级）。    
    /// 配合 Bug 183 (N01) 反弹计数：本路径不消耗反弹轮次（哨兵 0u32），
    /// 留给下次再试；enqueue() 失败回退的同源反弹由任务侧计数。
    pub async fn flush_fallback(&self) -> GatewayResult<()> {
        let items: Vec<DataPoint> = {
            let mut buf = self.fallback_buffer.lock();
            if buf.is_empty() {
                return Ok(());
            }
            buf.drain(..).collect()
        };
        let n = items.len();
        tracing::info!("flushing {} points from fallback to SQLite", n);
        // #1：send 失败后 items 已被包成 QueueCommand 移交进 mpsc；
        // 拿不回来——所以先 clone 一份 Vec<DataPoint> 作为兜底副本。
        // 兜底只在失败路径使用，正常路径 clone 是浪费但 n 较小（≤5000 个点），
        // 远低于磁盘 IO 开销。
        let backup = items.clone();
        if let Err(e) = self.tx.send(QueueCommand::Enqueue(items)).await {
            // 推回缓冲（用兜底副本）。若超容，按优先级淘汰
            // （与 enqueue 失败回退同源实现，避免策略漂移）。
            let mut buf = self.fallback_buffer.lock();
            let mut no_bounce = 0u32; // flush 自身不消耗反弹计数
            let (_retained, dropped) =
                push_into_fallback(&mut buf, FALLBACK_CAPACITY, backup, &mut no_bounce);
            if dropped > 0 {
                self.metrics
                    .lock()
                    .queue_persist
                    .persist_dropped
                    .fetch_add(dropped as u64, std::sync::atomic::Ordering::Relaxed);
            }
            tracing::warn!(
                "flush fallback send failed, restored {} points to buffer (dropped by priority={}): {}",
                n - dropped,
                dropped,
                e
            );
            return Err(GatewayError::Queue(format!(
                "flush fallback send failed ({} points restored to buffer): {}",
                n, e
            )));
        }
        Ok(())
    }

    /// P4：**同步**冲刷 fallback 缓冲到存储后端（**不经 actor 通道**）。
    ///
    /// 供硬件看门狗 abort 前使用。看门狗触发的正是「进程卡死」场景，此时
    /// queue actor 可能已无法被调度——走 `tx.send` 的异步 [`Self::flush_fallback`]
    /// 会永久挂住（看门狗的 3s 超时最终仍会 abort，数据照样丢）。
    ///
    /// 本方法在调用线程内直接调用存储后端（后端自带锁，见 `QueueStorage`
    /// 契约「实现方负责内部同步」），因此即使 tokio 运行时卡死也能把内存中的
    /// 待落库数据写进 SQLite。
    ///
    /// 写失败时按与 `flush_fallback` 相同的语义把数据点推回缓冲头部，
    /// 保证「冲刷失败 ≠ 丢数据」。
    pub fn flush_fallback_sync(&self) -> GatewayResult<usize> {
        let items: Vec<DataPoint> = {
            let mut buf = self.fallback_buffer.lock();
            if buf.is_empty() {
                return Ok(0);
            }
            buf.drain(..).collect()
        };
        let n = items.len();
        match self.storage.enqueue_batch(&items) {
            Ok(inserted) => {
                if inserted < n {
                    // 部分坏点被跳过（逐行 INSERT 失败）——与普通入队路径同一语义。
                    tracing::warn!(
                        "sync flush fallback: inserted {}/{} points (bad points skipped)",
                        inserted,
                        n
                    );
                }
                Ok(inserted)
            }
            Err(e) => {
                let mut buf = self.fallback_buffer.lock();
                let mut no_bounce = 0u32; // 哨兵：flush 自身不消耗反弹计数
                let (_retained, dropped) =
                    push_into_fallback(&mut buf, FALLBACK_CAPACITY, items, &mut no_bounce);
                if dropped > 0 {
                    self.metrics
                        .lock()
                        .queue_persist
                        .persist_dropped
                        .fetch_add(dropped as u64, std::sync::atomic::Ordering::Relaxed);
                }
                tracing::warn!(
                    "sync flush fallback failed, restored {} points to buffer (dropped by priority={}): {}",
                    n - dropped,
                    dropped,
                    e
                );
                Err(e)
            }
        }
    }

    /// 出队（按 ID 升序，保证时序）
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(limit = limit)))]
    pub async fn dequeue(&self, limit: usize) -> GatewayResult<Vec<QueuedItem>> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(QueueCommand::Dequeue { limit, reply })
            .await
            .map_err(|e| GatewayError::Queue(format!("dequeue send failed: {}", e)))?;
        rx.await
            .map_err(|e| GatewayError::Queue(format!("dequeue recv failed: {}", e)))
    }

    /// 确认已发送，删除记录
    ///
    /// 存储层 ack 失败会返回 Err（数据其实没被删除）：调用方据此决定
    /// 是否 release 重投并上报告警，不再出现「ack 失败被静默吞掉」。
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(n = ids.len())))]
    pub async fn ack(&self, ids: Vec<i64>) -> GatewayResult<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(QueueCommand::Ack { ids, reply })
            .await
            .map_err(|e| GatewayError::Queue(format!("ack failed: {}", e)))?;
        rx.await
            .map_err(|e| GatewayError::Queue(format!("ack reply failed: {}", e)))?
    }

    /// 释放 in_flight 标志（不删除数据），用于 PubAck 超时/失败后让数据可被重新 dequeue
    pub async fn release(&self, ids: Vec<i64>) {
        if ids.is_empty() {
            return;
        }
        if let Err(e) = self.tx.send(QueueCommand::Release(ids)).await {
            tracing::warn!("release in_flight send failed: {}", e);
        }
    }

    /// 队列长度
    /// 队列是否为空（仅在调用方需要语义化判断时使用；长度本身请用 `len()`）
    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }

    pub async fn len(&self) -> usize {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(QueueCommand::Len(reply)).await.is_err() {
            return 0;
        }
        rx.await.unwrap_or(0)
    }

    /// P0-3：启动「队列健康 → 告警」联动后台任务（**无论有没有北向消费者都启动**）。
    ///
    /// 旧实现有两个致命缺陷（对应测试报告 P0-3「14.9 万条积压零告警，告警链路
    /// 在直连场景整体失明」）：
    /// 1. `bootstrap` 只在 `mqtt.is_some()` 时才 spawn 本任务 → 网线直连调试
    ///    （未配 MQTT）时根本没有采样者，`today_alerts` 恒为 0；
    /// 2. 判定只看 `len >= 100_000`，把「有消费者时的上行故障」与「无消费者时
    ///    的正常本地历史存量」混为一谈（P0-1 的三套口径冲突）。
    ///
    /// 现改为调用 [`queue_health`]：等级从 Ok 升级即计数 + 发事件（Backlog 用
    /// warn，Backpressure / CapacityPressure 用 critical），回到 Ok 发 Info 恢复
    /// 事件；等级不变不重复告警，避免积压持续期间刷屏。
    ///
    /// `has_north_consumer`：运行期求值的「是否存在会 dequeue/ack 队列的北向
    /// 消费者」探针（见 [`NorthConsumerProbe`]）。每轮采样都重新求值，
    /// 因此运行期启停消费者（模拟消费者）后告警口径立即跟随变化。
    pub fn spawn_backlog_alarm_monitor(self: &Arc<Self>, has_north_consumer: NorthConsumerProbe) {
        let queue = self.clone();
        crate::engine::supervision::spawn_supervised("queue_backlog_alarm", move || {
            let queue = queue.clone();
            let has_north_consumer = has_north_consumer.clone();
            async move {
                let mut last = QueueHealth::Ok;
                loop {
                    tokio::time::sleep(BACKLOG_ALARM_INTERVAL).await;
                    let len = queue.len().await;
                    // 运行期求值：消费者可随时启停，判定必须每轮刷新
                    let has_consumer = has_north_consumer();
                    let now = queue_health(len as i64, has_consumer);
                    let Some(now) = backlog_alarm_edge(last, now) else {
                        continue;
                    };
                    last = now;
                    if now == QueueHealth::Ok {
                        tracing::info!("queue health recovered: {} messages pending", len);
                        continue;
                    }
                    // 进入（或升级到）异常等级：计数 + 告警日志，无一例外。
                    queue.metrics.lock().inc_alarm_trigger();
                    // P3：原实现在此发布 Alert/QueueBackpressure 事件到 EventBus，
                    // 但该总线生产侧零订阅者，事件投递即被丢弃；实际生效的告警
                    // 渠道是 tracing::warn!（内容与原事件 message 完全一致），
                    // 外加 /health/ready 的 components.queue 与 watchdog 判定。
                    let message = match now {
                        QueueHealth::Backlog if has_consumer => format!(
                            "队列积压 {} 条（≥ {} 预警阈值），上行通道处理不足，请检查 MQTT 连接",
                            len, BACKLOG_WARN_ROWS
                        ),
                        QueueHealth::Backlog => format!(
                            "队列存量 {} 条（≥ {} 预警阈值）：未配置北向消费者，队列作为本地历史库按 storage_days 保留",
                            len, BACKLOG_WARN_ROWS
                        ),
                        QueueHealth::Backpressure => format!(
                            "队列积压 {} 条，超过背压阈值 {} 条（北向通道不可达或处理不足）",
                            len, BACKPRESSURE_ROWS
                        ),
                        _ => format!(
                            "队列 {} 条，已逼近硬上限 {} 条：存储层开始淘汰最旧数据，请检查上行或降低采集频率",
                            len,
                            MAX_QUEUE_ROWS
                        ),
                    };
                    tracing::warn!(
                        "queue health degraded: level={} len={} has_consumer={} msg={}",
                        now.label(),
                        len,
                        has_consumer,
                        message
                    );
                }
            }
        });
    }

    /// 等待队列完全排空（用于优雅退出）
    ///
    /// 阻塞直到所有数据被 ack 删除。调用方应先停止采集入队。
    pub async fn drain_and_wait(&self) {
        loop {
            let (reply, rx) = oneshot::channel();
            if self.tx.send(QueueCommand::Drain(reply)).await.is_err() {
                return;
            }
            if rx.await.is_ok() {
                return; // 队列已空
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    /// 历史数据查询（带降采样与分页元信息，P2-1）。
    ///
    /// 唯一的公开历史查询入口（N-10：旧的原始行查询入口 `history()` 已下线，
    /// 避免两条路径口径不一致）：
    /// - `bucket_secs`：时间桶宽度（秒），`None`/0 = 不降采样
    /// - 返回 `total`/`truncated`/`has_more`，前端可据此提示"已截断"
    /// - DB 错误向上抛出（不会把故障伪装成空结果）
    pub async fn history_ex(
        &self,
        point_id: &str,
        start: i64,
        end: i64,
        limit: usize,
        bucket_secs: Option<u32>,
    ) -> GatewayResult<HistoryQueryResult> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(QueueCommand::HistoryEx {
                point_id: point_id.to_string(),
                start,
                end,
                limit,
                bucket_secs,
                reply,
            })
            .await
            .map_err(|e| GatewayError::Queue(format!("history_ex send failed: {}", e)))?;
        rx.await
            .map_err(|e| GatewayError::Queue(format!("history_ex recv failed: {}", e)))?
    }
}

#[cfg(test)]
mod tests;
