//! 处理管道（采集与处理解耦）
//!
//! 将采集循环中的后处理步骤（边缘过滤 → 协议转换 → 虚拟点 → 入队 → 实时广播）
//! 剥离到独立的异步任务中，通过 mpsc 通道接收采集到的原始数据点。
//!
//! 优势：
//! - 采集周期不受边缘处理耗时影响，定时更精准
//! - 处理逻辑可独立扩展（新增过滤节点、统计等不影响采集）
//! - 背压：通道满时采集侧可选择丢弃或等待

pub mod convert;
pub mod edge;
pub mod nodes;
pub mod point_cache;
pub mod quality_rules;
pub mod vformula;
pub mod virtual_point;

pub use convert::ConvertEngine;
pub use edge::EdgeProcessor;
pub use virtual_point::VirtualPointEngine;

use std::sync::Arc;

use tokio::sync::{mpsc, watch};
use tracing::Instrument;

use crate::config::PointConfig;
use crate::engine::model::DataPoint;
use crate::engine::model::Quality;
use crate::north::NorthSender;
use crate::queue::DataQueue;

/// 采集侧发送给处理管道的消息
#[derive(Debug)]
pub struct CollectedBatch {
    /// 原始采集结果（point + dp 配对，用于边缘处理时读取点位配置）
    pub items: Vec<(PointConfig, DataPoint)>,
    /// 采集周期 span，作为 process_batch 的 parent（tracing feature 开启时有效）。
    /// tracing 关闭时 Span::current() 返回 no-op Span（内部 Inner=None，零分配），
    /// clone/instrument 均零成本。字段始终存在，避免 cfg 守护所有构造点。
    pub parent_span: tracing::Span,
    /// 是否为"聚合窗口刷新产物"批（由 `Scheduler::spawn_aggregation_flush` 构造）。
    ///
    /// false：常规采集批，`process_batch` 走 `EdgeProcessor::process`。
    /// true：这些 `DataPoint` 已是聚合最终结果，`process_batch` 走
    /// `EdgeProcessor::process_flushed`（跳过聚合节点的再缓冲，避免聚合值
    /// 被重新入窗丢弃并每秒重复上报）。
    pub flushed: bool,
}

impl CollectedBatch {
    /// 构造时 capture 当前 span 作为 parent（常规采集批）
    pub fn new(items: Vec<(PointConfig, DataPoint)>) -> Self {
        Self {
            items,
            parent_span: tracing::Span::current(),
            flushed: false,
        }
    }

    /// 构造聚合窗口刷新产物批（`flushed = true`）
    pub fn flushed(items: Vec<(PointConfig, DataPoint)>) -> Self {
        Self {
            items,
            parent_span: tracing::Span::current(),
            flushed: true,
        }
    }
}

/// 处理管道
///
/// 消费 `CollectedBatch`，依次执行：
/// 1. EdgeProcessor 过滤（死区/变化上传/聚合）
/// 2. 协议转换模板
/// 3. 数据质量规则
/// 4. 虚拟点位求值
/// 5. 入队 DataQueue
/// 6. 实时数据 WebSocket 广播
///
/// 北向上报（MQTT/Webhook/Server Sinks）由 ConversionManager 统一管理，
/// 从北向专用队列（第 5 层无损通道）接收数据，不再由 Pipeline 直接持有。
pub struct ProcessingPipeline {
    edge: Arc<EdgeProcessor>,
    /// C1: 协议转换模板引擎（广播/入队之前执行 raw/linear/enum）
    convert: Arc<ConvertEngine>,
    /// 虚拟点位引擎（协议转换之后、广播/入队之前求值公式派生量）
    virtual_points: Arc<VirtualPointEngine>,
    queue: Arc<DataQueue>,
    realtime_bus: Option<Arc<RealtimeBus>>,
    /// 北向专用无损队列发送端（队列满时在此 await 施加背压）。
    /// `Mutex<Option<..>>` 支持停机时主动 drop 发送端，让 ConversionManager
    /// 消费完积压后自然退出。
    north_tx: parking_lot::Mutex<Option<NorthSender>>,
    /// 采集侧 → 处理侧 的通道
    tx: mpsc::Sender<CollectedBatch>,
    /// M2: 优雅退出信号
    shutdown_tx: watch::Sender<bool>,
    /// 1.1: 指标实例（替代全局 metrics::get()）
    metrics: Arc<crate::engine::metrics::Metrics>,
}

/// `ProcessingPipeline::spawn` 的入参集合：装配点唯一，
/// 用结构体承载以避免超长参数列表（clippy::too_many_arguments）。
pub struct PipelineSpawnInputs {
    pub edge: Arc<EdgeProcessor>,
    /// C1: 协议转换模板引擎（无配置时传 `ConvertEngine::empty()`）
    pub convert: Arc<ConvertEngine>,
    /// 虚拟点位引擎（无配置时传 `VirtualPointEngine::empty()`）
    pub virtual_points: Arc<VirtualPointEngine>,
    pub queue: Arc<DataQueue>,
    pub realtime_bus: Option<Arc<RealtimeBus>>,
    /// 北向专用无损队列发送端（`None` = 无北向 fan-out，仅广播）
    pub north_tx: Option<NorthSender>,
    /// 通道缓冲区大小，满时采集侧 send 将阻塞（背压）
    pub buffer: usize,
    pub metrics: Arc<crate::engine::metrics::Metrics>,
}

impl ProcessingPipeline {
    /// 创建处理管道并启动后台处理任务
    ///
    /// `buffer` 为通道缓冲区大小，满时采集侧 send 将阻塞（背压）。
    pub fn spawn(inputs: PipelineSpawnInputs) -> Arc<Self> {
        let PipelineSpawnInputs {
            edge,
            convert,
            virtual_points,
            queue,
            realtime_bus,
            north_tx,
            buffer,
            metrics,
        } = inputs;
        let (tx, rx) = mpsc::channel(buffer);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let pipeline = Arc::new(Self {
            edge,
            convert,
            virtual_points,
            queue,
            realtime_bus,
            north_tx: parking_lot::Mutex::new(north_tx),
            tx,
            shutdown_tx,
            metrics,
        });

        // 启动后台处理任务
        let p = pipeline.clone();
        crate::engine::supervision::spawn_catch_unwind("pipeline_run", async move {
            p.run(rx, shutdown_rx).await;
        });

        pipeline
    }

    /// 发送一批采集数据到管道（非阻塞，失败则记录日志 + 增 counter）
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, batch), fields(n = batch.items.len()), level = "debug"))]
    pub async fn submit(&self, batch: CollectedBatch) {
        if self.tx.send(batch).await.is_err() {
            // X1：数据丢失在生产窗口里只能从 logs 看，无法给 Grafana 出 rate；
            // 增一个 counter 让 `rate(gateway_pipeline_dropped_total[5m]) > 0`
            // 直接告警。inc 必须在 inc 自身顺序之前，避免 partial-failure 计数误差。
            self.metrics
                .pipeline
                .dropped
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            tracing::error!("处理管道已关闭，数据丢失");
        } else {
            // 成功路径同步 +1，对照组；告警规则可写
            //   rate(gateway_pipeline_dropped_total[5m])
            //     / clamp_min(rate(gateway_pipeline_submitted_total[5m]) + rate(dropped[5m]), 1)
            // 给出 0..1 的 loss ratio。
            self.metrics
                .pipeline
                .submitted
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// 后台处理循环
    ///
    /// M1: micro-batching —— 10ms 窗口内合并多个采集 batch 的入队请求，
    /// 减少 SQLite INSERT 事务数。边缘过滤/实时广播仍逐批执行，
    /// 只合并最终的 `queue.enqueue()` 调用。
    async fn run(
        &self,
        mut rx: mpsc::Receiver<CollectedBatch>,
        mut shutdown_rx: watch::Receiver<bool>,
    ) {
        use std::time::Duration;

        let batch_threshold = 500; // 攒满 500 点立即 flush
        let window_ms = Duration::from_millis(10);
        let mut accum: Vec<DataPoint> = Vec::with_capacity(batch_threshold);
        let mut last_flush = std::time::Instant::now();
        // f6: micro-batch 仅在「收到新批次」时才检查是否 flush；若末尾不足
        // batch_threshold 的点之后再无新数据到达，会一直滞留在 accum 里直到进程
        // 退出（表现为最后几个点不落库/不上报）。增加按 micro-batch 窗口的定时
        // 兜底分支，强制把滞留数据入队。
        let mut flush_tick = tokio::time::interval(window_ms);
        flush_tick.tick().await; // 跳过首次立即触发

        loop {
            tokio::select! {
                // M2: 收到退出信号，flush 拋留数据后退出
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        if !accum.is_empty() {
                            self.queue.enqueue(std::mem::take(&mut accum)).await.ok();
                        }
                        tracing::info!("处理管道收到退出信号，已 flush");
                        break;
                    }
                }
                // f6: 定时兜底 flush 滞留在 accum 中的小批量数据
                _ = flush_tick.tick() => {
                    if !accum.is_empty() {
                        self.queue.enqueue(std::mem::take(&mut accum)).await.ok();
                        last_flush = std::time::Instant::now();
                    }
                }
                // 正常接收采集数据
                Some(batch) = rx.recv() => {
                    // tracing: 将采集周期的 span 作为 process_batch 的 parent，跨 mpsc channel 携带。
                    // feature off 时 Span 为 no-op，instrument() 零成本。
                    let parent_span = batch.parent_span.clone();
                    let to_upload = async { self.process_batch(batch).await }
                        .instrument(parent_span)
                        .await;
                    if !to_upload.is_empty() {
                        accum.extend(to_upload);
                        let now = std::time::Instant::now();
                        if accum.len() >= batch_threshold || now.duration_since(last_flush) >= window_ms {
                            self.queue.enqueue(std::mem::take(&mut accum)).await.ok();
                            last_flush = now;
                        }
                    }
                }
                // channel 关闭（所有 sender 被 drop）
                else => break,
            }
        }

        // 最终 flush（防止 select 分支漏掉）
        if !accum.is_empty() {
            self.queue.enqueue(std::mem::take(&mut accum)).await.ok();
        }
        tracing::info!("处理管道退出");
    }

    /// M2: 优雅退出 —— 通知 run 循环 flush 拋留数据并退出
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// 关闭北向专用队列的发送端（停机用）。
    ///
    /// drop 后 ConversionManager 消费完已积压的批次即收到 `None` 自然退出，
    /// 不会因停机信号提前截断而丢掉最终批。
    pub fn close_north_feed(&self) {
        self.north_tx.lock().take();
    }

    /// 处理一批数据，返回需要入队的数据点
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, batch), fields(n_in = batch.items.len())))]
    async fn process_batch(&self, batch: CollectedBatch) -> Vec<DataPoint> {
        let mut to_upload: Vec<DataPoint> = Vec::with_capacity(batch.items.len());

        // 1. 边缘处理（死区/变化上传/聚合过滤）
        //    flushed 批（聚合窗口刷新产物）走 process_flushed：聚合值已是最终结果，
        //    不能再经聚合节点入窗缓冲（否则当轮丢失 + 每秒重复上报）。
        for (point, dp) in &batch.items {
            // 失败数据直接上传（让平台感知设备状态）
            if dp.quality != Quality::Good {
                to_upload.push(dp.clone());
                continue;
            }
            let filtered = if batch.flushed {
                self.edge.process_flushed(point, dp.clone())
            } else {
                self.edge.process(point, dp.clone())
            };
            if let Some(filtered) = filtered {
                to_upload.push(filtered);
            }
        }

        // 2. C1: 协议转换模板（raw 直通记账 / linear 线性换算 / enum 枚举文本）。
        //    必须在广播与入队之前执行——转换后的值才是上送/落库值。
        self.convert.apply(&mut to_upload);

        // 2.1 数据质量规则（range / rate / nan）。
        //     规则快照由启动装配与 API 写接口经 `install_rules` 维护；
        //     `stale` 由调度器的连续失败/超时机制负责，此处不重复判定。
        //     无规则时 `apply` 直接返回，不改变现有行为。
        quality_rules::apply(&mut to_upload, DataPoint::now());

        // 2.6 虚拟点位求值（公式派生量）。
        //     先用本批数据刷新点位实时值缓存，再对虚拟点位求值；派生出的
        //     DataPoint 与普通点位一同广播/入队/北向上报。
        self.virtual_points.update_cache(&batch.items, &to_upload);
        let derived = self.virtual_points.evaluate();
        to_upload.extend(derived);

        // 3. 实时广播到 WebSocket（单槽覆盖，无背压内存问题）
        //    RealtimeBus 的订阅者（WS / 报警 / 最新值快照）只服务可容忍丢帧的下游。
        //    北向走第 5 层的专用无损队列，与广播并列投递同一份 Arc（零拷贝共享）。
        if !to_upload.is_empty() {
            let arc = Arc::new(to_upload.clone());
            if let Some(bus) = &self.realtime_bus {
                bus.publish(arc.clone());
            }
            // 先克隆发送端句柄再 await，避免 MutexGuard 跨 await 持有。
            let north_tx = self.north_tx.lock().clone();
            if let Some(tx) = north_tx {
                // 队列满时在此挂起 → 背压传导到采集调度（宁可降速，不丢上报）。
                // 失败仅发生在 ConversionManager 已退出（停机窗口），
                // 历史数据仍由 DataQueue 保证，不做落盘兜底。
                if let Err(e) = tx.send(arc).await {
                    tracing::warn!(
                        target: "pipeline",
                        "[hot-path] north feed closed, batch dropped: {}",
                        e
                    );
                }
            }
        }

        // M1: 入队由 run() 的 micro-batching 合并处理，这里返回数据点
        // 北向上报已由 ConversionManager 从北向专用队列消费，Pipeline 不再直接持有 sink。
        to_upload
    }

    /// 强制刷新聚合缓冲区（退出前调用）
    pub fn flush_aggregated(&self, points: &[PointConfig]) -> Vec<DataPoint> {
        self.edge.flush_aggregated(points)
    }

    /// 强制刷新所有聚合缓冲区
    pub fn flush_all(&self, points: &[PointConfig]) -> Vec<DataPoint> {
        self.edge.flush_all(points)
    }
}

// 实时广播总线契约已下沉到 `engine::bus`（消除 `engine ↔ pipeline` 循环依赖）。
// 这里保留重导出，`gateway_core::pipeline::RealtimeBus` 等既有调用点无需改动。
pub use crate::engine::bus::{BusConsumerKind, LaggedCounts, RealtimeBus};
