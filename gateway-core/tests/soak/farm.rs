//! 浸泡用虚拟农场装配。
//!
//! 全部使用生产代码路径（仅替换物理串口为可编程从站桩）：
//! `SoakPort` → 真实 `BusManager::spawn_with` → `ProtocolRegistry::create`
//! → 真实 `Scheduler` → `ProcessingPipeline` → `DataQueue` + `RealtimeBus`
//! → `NorthDispatcher` → 计数式北向桩。
//!
//! 与 `load/farm.rs` 的差异：
//! 1. 一条总线多个从站号（复现“同总线单表故障”）；
//! 2. 每总线暴露 `SoakControl` 供故障时间线切换行为；
//! 3. 增加真实出队消费者（`dequeue` + `ack`），否则 24h 队列单调增长会淹没判定。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gateway_core::config::{BusConfig, PointConfig};
use gateway_core::engine::metrics::Metrics;
use gateway_core::north::NorthDispatcher;
use gateway_core::pipeline::{
    ConvertEngine, EdgeProcessor, PipelineSpawnInputs, ProcessingPipeline, RealtimeBus,
};
use gateway_core::queue::DataQueue;
use gateway_core::south::bus::{BusDeps, BusManager, SerialHandle};
use gateway_core::south::protocol::{Collector, ProtocolRegistry};
use gateway_core::south::scheduler::Scheduler;
use tokio::task::JoinHandle;

use super::port::{SoakControl, SoakOpener, SoakPort};
use super::spec::SoakSpec;
use crate::chaos_harness::{real_queue_for, RecordingGpio};

/// 北向桩：在批次 / 点位计数之外，额外按质量码区分 Good / 非 Good。
///
/// 「Good 质量点数」用于交叉校验采集侧质量码是否在管道中保持一致——
/// 若北向收到的非 Good 点远多于链路未应答帧对应的点数，说明管道在污染质量码。
#[derive(Default)]
pub struct QualitySink {
    batches: AtomicU64,
    points: AtomicU64,
    good: AtomicU64,
}

impl QualitySink {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn batches(&self) -> u64 {
        self.batches.load(Ordering::Relaxed)
    }

    /// 已收到的点位数（含非 Good）。
    pub fn points(&self) -> u64 {
        self.points.load(Ordering::Relaxed)
    }

    /// 已收到的 Good 质量点数。
    pub fn good(&self) -> u64 {
        self.good.load(Ordering::Relaxed)
    }
}

#[async_trait::async_trait]
impl gateway_core::north::NorthConnector for QualitySink {
    fn name(&self) -> &str {
        "quality_sink"
    }

    async fn publish_batch(
        &self,
        batch: &[gateway_core::engine::model::DataPoint],
    ) -> gateway_core::engine::error::GatewayResult<()> {
        self.batches.fetch_add(1, Ordering::Relaxed);
        self.points.fetch_add(batch.len() as u64, Ordering::Relaxed);
        let good = batch
            .iter()
            .filter(|p| p.quality == gateway_core::engine::model::Quality::Good)
            .count() as u64;
        self.good.fetch_add(good, Ordering::Relaxed);
        Ok(())
    }

    fn health(&self) -> gateway_core::north::ConnectorHealth {
        gateway_core::north::ConnectorHealth {
            connected: true,
            last_error: None,
        }
    }

    fn is_enabled(&self) -> bool {
        true
    }
}

/// 已装配的浸泡农场。`stop()` 前须保持存活。
pub struct SoakFarm {
    pub spec: SoakSpec,
    pub metrics: Arc<Metrics>,
    pub scheduler: Arc<Scheduler>,
    pub queue: Arc<DataQueue>,
    pub sink: Arc<QualitySink>,
    /// 每总线行为控制句柄（故障注入用）
    pub controls: Vec<Arc<SoakControl>>,
    /// 已出队（=已“发往北向”）的点位数
    pub drained: Arc<AtomicU64>,
    /// sqlite 后端时保留 DB 所在 tempdir，必须活到农场析构
    _sqlite_dir: Option<tempfile::TempDir>,
    tasks: Vec<JoinHandle<()>>,
}

impl SoakFarm {
    pub async fn build(spec: SoakSpec) -> Self {
        let metrics = Arc::new(Metrics::new());
        // A1：后端可切换（`SOAK_STORAGE=sqlite`）——生产部署的是 SQLite 后端，
        // 长跑必须覆盖真实持久化路径（WAL / 容量淘汰 / retention / 计数器对账）。
        let (queue, sqlite_dir) = real_queue_for(spec.storage).await;
        let realtime_bus = RealtimeBus::new(8192);
        // 第 5 层：北向走专用无损队列（容量按批，8192 批足以吸收长跑抖动）
        let (north_tx, north_rx) = gateway_core::north::NorthFeed::channel(8192);
        let pipeline = ProcessingPipeline::spawn(PipelineSpawnInputs {
            edge: Arc::new(EdgeProcessor::new()),
            convert: ConvertEngine::empty(),
            virtual_points: gateway_core::pipeline::VirtualPointEngine::empty(),
            queue: queue.clone(),
            realtime_bus: Some(realtime_bus.clone()),
            north_tx: Some(north_tx),
            buffer: 8192,
            metrics: metrics.clone(),
        });

        let registry = ProtocolRegistry::new();
        let mut collectors: HashMap<String, Arc<dyn Collector>> = HashMap::new();
        let mut points: Vec<PointConfig> = Vec::with_capacity(spec.actual_points());
        let mut controls: Vec<Arc<SoakControl>> = Vec::with_capacity(spec.buses);
        let points_per_slave = spec.points_per_slave;

        for i in 0..spec.buses {
            let bus_name = format!("soakbus{i}");
            let ctl = Arc::new(SoakControl::new());
            let port = SoakPort::new(spec.baud, ctl.clone());
            let serial = Some(SerialHandle::from_stream(port));
            let deps = BusDeps {
                opener: Arc::new(SoakOpener::new(spec.baud, ctl.clone())),
                gpio: Arc::new(RecordingGpio::new()),
            };
            let bus_cfg = BusConfig {
                port_name: bus_name.clone(),
                protocol: "modbus".to_string(),
                timeout_ms: spec.bus_timeout_ms,
                retry_count: Some(0),
                retry_delay_ms: Some(20),
                // 与 load farm 一致：485 方向脚；桩不消费 GPIO，仅走真实路径
                de_pin: Some(4),
                channel_capacity: Some(128),
                ..Default::default()
            };
            let bm = Arc::new(BusManager::spawn_with(
                &bus_cfg,
                serial,
                metrics.clone(),
                deps,
            ));
            let collector = registry
                .create("modbus", &bus_name, bm, metrics.clone())
                .expect("modbus collector must be registered");
            collectors.insert(bus_name.clone(), collector);
            controls.push(ctl);

            for s in 0..spec.slaves_per_bus {
                // 从站号从 1 开始（0 为广播地址，自检帧排除）
                let slave = (s + 1) as u8;
                for j in 0..points_per_slave {
                    points.push(PointConfig {
                        id: format!("{bus_name}_s{slave}_p{j}"),
                        name: Some(format!("{bus_name}_s{slave}_p{j}")),
                        bus: bus_name.clone(),
                        protocol: "modbus".to_string(),
                        address: j.to_string(),
                        address_num: Some(j as u16),
                        slave_id: Some(slave),
                        function_code: Some(3),
                        register_count: Some(1),
                        interval: spec.interval_secs,
                        unit: Some("u16".to_string()),
                        ..Default::default()
                    });
                }
            }
        }

        let scheduler = Arc::new(Scheduler::new(
            collectors,
            points,
            pipeline,
            metrics.clone(),
        ));

        // 北向：质量计数桩，验证“采集 → 管道 → 北向专用队列 → 北向”整链打通
        let sink = QualitySink::new();
        let cm = NorthDispatcher::new(north_rx);
        cm.register_connector(sink.clone() as Arc<dyn gateway_core::north::NorthConnector>);
        let cm_handle = tokio::spawn(async move { cm.run().await });

        // 队列消费者：模拟北向取数，避免 24h 队列无界增长
        let drained = Arc::new(AtomicU64::new(0));
        let drain_handle = {
            let queue = queue.clone();
            let drained = drained.clone();
            tokio::spawn(async move {
                loop {
                    match queue.dequeue(500).await {
                        Ok(items) if items.is_empty() => {
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                        Ok(items) => {
                            let ids: Vec<i64> = items.iter().map(|i| i.id).collect();
                            let n = ids.len() as u64;
                            if queue.ack(ids).await.is_ok() {
                                drained.fetch_add(n, Ordering::Relaxed);
                            }
                        }
                        Err(_) => {
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                    }
                }
            })
        };

        Self {
            spec,
            metrics,
            scheduler,
            queue,
            sink,
            controls,
            drained,
            _sqlite_dir: sqlite_dir,
            tasks: vec![cm_handle, drain_handle],
        }
    }

    pub fn start(&self) {
        self.scheduler.start();
    }

    pub async fn stop(self) {
        self.scheduler.stop().await;
        for t in self.tasks {
            t.abort();
        }
    }

    /// 每从站累计 (从站号, 请求数, 应答数)，按总线分组。
    pub fn slave_snapshot(&self) -> Vec<Vec<(u8, u64, u64)>> {
        self.controls.iter().map(|c| c.snapshot()).collect()
    }
}
