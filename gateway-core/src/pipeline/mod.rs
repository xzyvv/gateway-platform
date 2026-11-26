//! 处理管道（采集与处理解耦）
//!
//! 将采集循环中的后处理步骤（边缘过滤 → 协议转换 → 质量规则 → 虚拟点 → 实时广播 → 入队）
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
/// 5. 实时数据 WebSocket 广播（并投递北向无损队列）
/// 6. 入队 DataQueue（由 `run()` 的 micro-batching 合并后落库）
///
/// 注意 5 / 6 的先后：**先广播、后入队**。广播在 `process_batch` 内逐批发出
/// （订阅者只服务可容忍丢帧的下游），入队在 `run()` 里攒批后统一落库；
/// 两者并列投递同一份数据（零拷贝共享），不存在「入队成功才算广播成功」的依赖。
///
/// 北向上报（MQTT/Webhook/Server Sinks）由 NorthDispatcher 统一管理，
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
    /// `Mutex<Option<..>>` 支持停机时主动 drop 发送端，让 NorthDispatcher
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

    /// 入队失败必须可观测。旧实现四处直接 `.ok()`，队列 actor 退出后所有入队
    /// 都失败却没有任何信号，现场只表现为「采集正常、日志无异常、北向无数据」。
    /// `persist_failed` 与 queue 任务侧共用同一 counter（queue 的 Err 路径本身不计数，
    /// 不会重复统计）。
    async fn enqueue_checked(&self, points: Vec<DataPoint>) {
        let count = points.len();
        if let Err(e) = self.queue.enqueue(points).await {
            record_enqueue_failure(&self.metrics, count, &e);
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
                                 // DD-16：点位实时值缓存按 TTL 定期淘汰，防止点位下线后缓存单调增长。
        let mut prune_tick = tokio::time::interval(Duration::from_secs(60));
        prune_tick.tick().await; // 跳过首次立即触发

        loop {
            tokio::select! {
                // M2: 收到退出信号，flush 拋留数据后退出
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        if !accum.is_empty() {
                            self.enqueue_checked(std::mem::take(&mut accum)).await;
                        }
                        tracing::info!("处理管道收到退出信号，已 flush");
                        break;
                    }
                }
                // f6: 定时兜底 flush 滞留在 accum 中的小批量数据
                _ = flush_tick.tick() => {
                    if !accum.is_empty() {
                        self.enqueue_checked(std::mem::take(&mut accum)).await;
                        last_flush = std::time::Instant::now();
                    }
                }
                // DD-16：按 TTL 真正淘汰已过期的点位实时值缓存
                _ = prune_tick.tick() => {
                    self.virtual_points.prune_cache(DataPoint::now());
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
                            self.enqueue_checked(std::mem::take(&mut accum)).await;
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
            self.enqueue_checked(std::mem::take(&mut accum)).await;
        }
        tracing::info!("处理管道退出");
    }

    /// M2: 优雅退出 —— 通知 run 循环 flush 拋留数据并退出
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// 关闭北向专用队列的发送端（停机用）。
    ///
    /// drop 后 NorthDispatcher 消费完已积压的批次即收到 `None` 自然退出，
    /// 不会因停机信号提前截断而丢掉最终批。
    pub fn close_north_feed(&self) {
        self.north_tx.lock().take();
    }

    /// 北向队列发送端的可观测句柄（Web 层读积压/背压用）。
    ///
    /// 返回克隆而非借用，避免把内部锁的生命周期暴露给调用方；
    /// 发送端克隆共享同一队列与计数器，读取开销只有几个原子 load。
    pub fn north_sender(&self) -> Option<crate::north::NorthSender> {
        self.north_tx.lock().clone()
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
                // 失败仅发生在 NorthDispatcher 已退出（停机窗口），
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
        // 北向上报已由 NorthDispatcher 从北向专用队列消费，Pipeline 不再直接持有 sink。
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

    /// 清理已下线点位的管道内部状态（DD-16）
    ///
    /// 由调度器在点位热重载提交后调用：`active` 是重载后的全部点位 ID，
    /// 死区/变化上传基线、质量边沿按它淘汰；虚拟点实时值缓存按 TTL 淘汰。
    pub fn prune_state(&self, active: &std::collections::HashSet<String>) {
        self.edge.prune(active);
        self.virtual_points.prune_cache(DataPoint::now());
    }
}

// 实时广播总线契约已下沉到 `engine::bus`（消除 `engine ↔ pipeline` 循环依赖）。
// 这里保留重导出，`gateway_core::pipeline::RealtimeBus` 等既有调用点无需改动。
pub use crate::engine::bus::{BusConsumerKind, LaggedCounts, RealtimeBus};

/// 记录一次入队失败：错误日志 + Prometheus counter。
/// 独立成函数便于单测直接验证指标，无需构造完整 Pipeline / actor。
fn record_enqueue_failure(
    metrics: &crate::engine::metrics::Metrics,
    count: usize,
    err: impl std::fmt::Display,
) {
    metrics
        .queue_persist
        .persist_failed
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    tracing::error!(
        "pipeline enqueue failed: {} data points not persisted: {}",
        count,
        err
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConvertRuleConfig, QualityRule, VirtualPoint};
    use crate::engine::metrics::Metrics;
    use crate::engine::model::Quality;
    use crate::north::NorthFeed;
    use crate::pipeline::edge::PipelineNode;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    #[test]
    fn enqueue_failure_increments_persist_failed_counter() {
        let metrics = Metrics::default();
        assert_eq!(
            metrics.queue_persist.persist_failed.load(Ordering::Relaxed),
            0
        );

        record_enqueue_failure(&metrics, 42, "queue actor exited");
        record_enqueue_failure(&metrics, 7, "queue actor exited");

        assert_eq!(
            metrics.queue_persist.persist_failed.load(Ordering::Relaxed),
            2
        );
    }

    /// 四条入队路径（退出 flush / 定时兜底 / 攒批 / 最终 flush）都必须走
    /// `enqueue_checked`，任何一条退回 `.ok()` 都会重新引入静默丢数。
    #[test]
    fn every_enqueue_path_is_checked() {
        let src = include_str!("mod.rs");
        // 字面量必须拆分：include_str! 会把本测试的源码也读进去，
        // 完整字串出现在断言里会让自己命中自己。
        let swallowed = ".enqueue(std::mem::take(&mut accum)).await".to_owned() + ".ok()";
        assert!(!src.contains(&swallowed), "入队结果不得再用 .ok() 吞掉");
        let checked = "self.enqueue_checked(std::mem::take(&mut accum))".to_owned() + ".await;";
        assert_eq!(
            src.matches(&checked).count(),
            4,
            "退出/定时/攒批/最终四条 flush 路径都必须记录失败"
        );
    }

    // ===== ROI-27：数据管道不变量 I-52 / I-35 =====

    fn point(id: &str, deadband: f64) -> PointConfig {
        PointConfig {
            id: id.to_string(),
            interval: 1,
            deadband,
            ..PointConfig::default()
        }
    }

    fn linear_rule(point_id: &str) -> ConvertRuleConfig {
        ConvertRuleConfig {
            id: "cv".into(),
            name: "cv".into(),
            source_proto: "modbus".into(),
            source_point_id: point_id.into(),
            source_point_name: String::new(),
            target_proto: "mqtt".into(),
            target_tag: String::new(),
            transform: "linear".into(),
            scale: Some(10.0),
            offset: Some(0.0),
            enum_map: None,
            enabled: true,
            created_at_ms: 0,
        }
    }

    fn virtual_point(id: &str, formula: &str) -> VirtualPoint {
        VirtualPoint {
            id: id.to_string(),
            name: id.to_string(),
            device_id: Some("dev".into()),
            formula: formula.into(),
            unit: String::new(),
            decimals: 2,
            enabled: true,
            created_at: 0,
        }
    }

    /// 装配一条真实处理管道，返回管道、队列与北向接收端。
    fn spawn_pipeline(
        convert: Vec<ConvertRuleConfig>,
        vps: Vec<VirtualPoint>,
    ) -> (
        Arc<ProcessingPipeline>,
        Arc<DataQueue>,
        mpsc::Receiver<Arc<Vec<DataPoint>>>,
    ) {
        spawn_pipeline_with_edge(EdgeProcessor::new(), convert, vps)
    }

    /// 同上，但允许自定义边缘节点序列（用于验证「派生点不经过边缘阶段」）。
    fn spawn_pipeline_with_edge(
        edge: EdgeProcessor,
        convert: Vec<ConvertRuleConfig>,
        vps: Vec<VirtualPoint>,
    ) -> (
        Arc<ProcessingPipeline>,
        Arc<DataQueue>,
        mpsc::Receiver<Arc<Vec<DataPoint>>>,
    ) {
        let queue = Arc::new(DataQueue::new_memory().expect("memory queue"));
        let (north_tx, north_rx) = NorthFeed::channel(16);
        let pipeline = ProcessingPipeline::spawn(PipelineSpawnInputs {
            edge: Arc::new(edge),
            convert: ConvertEngine::new(convert),
            virtual_points: VirtualPointEngine::new(vps),
            queue: queue.clone(),
            realtime_bus: None,
            north_tx: Some(north_tx),
            buffer: 16,
            metrics: Arc::new(Metrics::default()),
        });
        (pipeline, queue, north_rx)
    }

    /// I-52：处理顺序必须是「边缘过滤 → 协议转换 → 虚拟点」。
    ///
    /// 用死区 0.5 + 线性 ×10 构造顺序敏感场景：
    /// - 正确顺序：原始差 0.2 被死区过滤 → 只上送 1 个点，值为 100；
    /// - 反过来先换算再过滤：差变成 2.0，两个点都会上送。
    /// 同时虚拟点 `{p1} + 1` 必须看到换算后的 100（得到 101），证明转换先于虚拟点。
    // 测试内跨 await 持锁是有意为之：串行化进程级全局规则快照；
    // 用例走 current_thread 运行时，future 无需 Send，不存在死锁/跨线程问题。
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn stage_order_is_edge_then_convert_then_virtual() {
        // 与 I-52 质量规则用例串行：规则是进程级全局态，避免互相覆盖。
        let _guard = quality_rules::TEST_LOCK.lock();
        quality_rules::install_rules(Vec::new());
        let (pipeline, queue, mut north_rx) = spawn_pipeline(
            vec![linear_rule("p1")],
            vec![virtual_point("p2", "{p1} + 1")],
        );

        let items = vec![
            (
                point("p1", 0.5),
                DataPoint::good("p1", "dev", 10.0, 10.0, "f32"),
            ),
            (
                point("p1", 0.5),
                DataPoint::good("p1", "dev", 10.2, 10.2, "f32"),
            ),
        ];
        pipeline.submit(CollectedBatch::new(items)).await;

        let batch = tokio::time::timeout(Duration::from_secs(2), north_rx.recv())
            .await
            .expect("管道必须在超时前产出批次")
            .expect("北向通道不应关闭");
        assert_eq!(
            batch.len(),
            2,
            "应为 1 个物理点 + 1 个虚拟点，实际 {batch:?}"
        );

        let p1 = batch.iter().find(|d| d.point_id == "p1").expect("缺 p1");
        assert!(
            (p1.value - 100.0).abs() < 1e-9,
            "p1 必须已线性换算为 100，实际 {}",
            p1.value
        );
        let p2 = batch.iter().find(|d| d.point_id == "p2").expect("缺 p2");
        assert!(
            (p2.value - 101.0).abs() < 1e-9,
            "虚拟点必须基于换算后的值求值（101），实际 {}",
            p2.value
        );

        // 队列最终也应收到这两条（micro-batch 有 10ms 窗口，等一下）
        for _ in 0..100 {
            if queue.len().await >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(queue.len().await >= 2, "物理点与虚拟点都必须入队");
    }

    /// I-35：采集失败点（Bad + last_good 值）必须原样进入上报/入队路径，
    /// 不能因为质量不为 Good 被边缘层过滤掉。
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn failed_points_reach_queue() {
        let _guard = quality_rules::TEST_LOCK.lock();
        quality_rules::install_rules(Vec::new());
        let (pipeline, queue, _north_rx) = spawn_pipeline(Vec::new(), Vec::new());
        let failed = DataPoint::failed(
            "p1",
            "dev",
            Some(12.5),
            12345,
            Quality::Bad,
            "collect_failed:TIMEOUT",
            3,
        );
        pipeline
            .submit(CollectedBatch::new(vec![(point("p1", 0.0), failed)]))
            .await;

        for _ in 0..100 {
            if queue.len().await >= 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(
            queue.len().await,
            1,
            "失败点必须入队，否则平台感知不到设备异常"
        );
    }

    /// 构造一条「取值超过上限即降级为 Bad」的质量规则。
    fn range_max_rule(max: f64) -> QualityRule {
        QualityRule {
            id: "qr_map".into(),
            name: "map-range".into(),
            condition: "range".into(),
            min: None,
            max: Some(max),
            max_rate: None,
            stale_seconds: None,
            result_quality: "bad".into(),
            enabled: true,
            created_at: 0,
            updated_at: 0,
        }
    }

    /// I-52：完整阶段顺序必须是「convert → 质量规则 → 虚拟点」。
    ///
    /// 构造顺序敏感场景：p1 原始值 1.0，线性 ×10 换算为 10.0，质量规则上限 5。
    /// - 若质量规则跑在转换之后：看到 10.0 > 5 → 降级 Bad；
    /// - 若跑在转换之前：看到原始 1.0 ≤ 5 → 不命中，点位仍是 Good。
    /// 再由虚拟点 `{p1} + 1` 验证：p1 已被降级 → 不进实时值缓存 → 虚拟点必须报错。
    /// 一次断言同时锁住「转换先于质量规则」和「质量规则先于虚拟点」。
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn stage_order_convert_then_quality_rules_then_virtual() {
        let _guard = quality_rules::TEST_LOCK.lock();
        quality_rules::install_rules(vec![range_max_rule(5.0)]);

        let (pipeline, _queue, mut north_rx) = spawn_pipeline(
            vec![linear_rule("p1")],
            vec![virtual_point("vp1", "{p1} + 1")],
        );

        pipeline
            .submit(CollectedBatch::new(vec![(
                point("p1", 0.0),
                DataPoint::good("p1", "dev", 1.0, 1.0, "f32"),
            )]))
            .await;

        let batch = tokio::time::timeout(Duration::from_secs(2), north_rx.recv())
            .await
            .expect("管道必须在超时前产出批次")
            .expect("北向通道不应关闭");

        let p1 = batch.iter().find(|d| d.point_id == "p1").expect("缺 p1");
        assert!(
            (p1.value - 10.0).abs() < 1e-9,
            "转换必须先于质量规则，p1 值应为 10，实际 {}",
            p1.value
        );
        assert_eq!(
            p1.quality,
            Quality::Bad,
            "质量规则必须看到换算后的 10（>5）并降级，实际 {:?}",
            p1.quality
        );
        assert_eq!(p1.quality_reason, "quality_rule:map-range");

        let vp = batch
            .iter()
            .find(|d| d.point_id == "vp1")
            .expect("虚拟点必须上报错误点");
        assert_eq!(
            vp.quality,
            Quality::Uncertain,
            "虚拟点求值必须晚于质量规则，降级点不得进入实时值缓存"
        );
        assert_eq!(vp.quality_reason, "virtual_point:ERROR");
        assert_eq!(vp.value, 0.0);

        quality_rules::install_rules(Vec::new());
    }

    /// 丢弃所有派生点（`raw_type == "virtual"`）的边缘节点。
    struct DropVirtualNode;

    impl PipelineNode for DropVirtualNode {
        fn name(&self) -> &'static str {
            "drop_virtual"
        }

        fn process(&self, _point: &PointConfig, dp: DataPoint) -> Option<DataPoint> {
            if dp.raw_type == "virtual" {
                None
            } else {
                Some(dp)
            }
        }
    }

    /// I-32：虚拟派生点不经过边缘阶段（死区/变化/自定义节点），
    /// 且在派生时带上自洽的时间戳与清零的陈旧计数。
    ///
    /// 用一个「丢弃所有 virtual 点」的节点接管边缘阶段：若派生点真的走了边缘，
    /// 它会被丢掉；实际必须仍能上报，证明它是在边缘之后追加的。
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn derived_point_bypasses_edge_stage() {
        let _guard = quality_rules::TEST_LOCK.lock();
        quality_rules::install_rules(Vec::new());

        let queue = Arc::new(DataQueue::new_memory().expect("memory queue"));
        let (north_tx, mut north_rx) = NorthFeed::channel(16);
        let pipeline = ProcessingPipeline::spawn(PipelineSpawnInputs {
            edge: Arc::new(EdgeProcessor::with_nodes(vec![Box::new(DropVirtualNode)])),
            convert: ConvertEngine::empty(),
            virtual_points: VirtualPointEngine::new(vec![virtual_point("vp1", "{p1} + 1")]),
            queue: queue.clone(),
            realtime_bus: None,
            north_tx: Some(north_tx),
            buffer: 16,
            metrics: Arc::new(Metrics::default()),
        });

        pipeline
            .submit(CollectedBatch::new(vec![(
                point("p1", 0.0),
                DataPoint::good("p1", "dev", 10.0, 10.0, "f32"),
            )]))
            .await;

        let batch = tokio::time::timeout(Duration::from_secs(2), north_rx.recv())
            .await
            .expect("管道必须在超时前产出批次")
            .expect("北向通道不应关闭");

        let vp = batch
            .iter()
            .find(|d| d.point_id == "vp1")
            .expect("派生点必须绕过边缘阶段直接上报");
        assert_eq!(vp.value, 11.0);
        assert_eq!(vp.quality, Quality::Good);
        assert_eq!(vp.raw_type, "virtual");
        assert_eq!(
            vp.timestamp, vp.source_ts,
            "派生点 timestamp 必须等于 source_ts"
        );
        assert!(vp.timestamp > 0);
        assert_eq!(vp.stale_count, 0, "派生点不得携带陈旧计数");
    }
}
