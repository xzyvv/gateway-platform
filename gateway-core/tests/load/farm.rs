//! 虚拟设备农场 + 配置矩阵生成（T2a）。
//!
//! 逐总线装配：节流串口桩 → 真实 `BusManager::spawn_with` →
//! `ProtocolRegistry::create` → 收集 `Arc<dyn Collector>`；再由真实
//! `ProcessingPipeline` + `Scheduler` 组成运行态。全程仅使用 `pub` API，
//! 不改动任何生产代码。

use std::collections::HashMap;
use std::sync::Arc;

use gateway_core::config::{BusConfig, PointConfig};
use gateway_core::engine::metrics::Metrics;
use gateway_core::pipeline::{
    ConvertEngine, EdgeProcessor, PipelineSpawnInputs, ProcessingPipeline, RealtimeBus,
};
use gateway_core::queue::DataQueue;
use gateway_core::south::bus::{BusDeps, BusManager, SerialHandle};
use gateway_core::south::protocol::{Collector, ProtocolRegistry};
use gateway_core::south::scheduler::Scheduler;

use super::chaos_harness::{real_queue_for, MockOpener, RecordingGpio, StorageBackend};
use super::throttle::ThrottledModbusPort;

/// 农场规模。默认 4 总线 × 75 点/总线 = 300 点，4800 baud。
#[derive(Clone, Debug)]
pub struct FarmSpec {
    pub buses: usize,
    pub points_per_bus: usize,
    pub baud: u32,
    /// 采集周期（秒，>=1）
    pub interval_secs: u64,
    pub slave_id: u8,
    /// 每总线开头静默的事务次数（L3 恢复场景用；正常为 0）
    pub silent_first: usize,
    /// 队列存储后端（默认 memory；`LOAD_STORAGE=sqlite` 切到生产同款后端）
    pub storage: StorageBackend,
}

impl FarmSpec {
    pub fn from_env() -> Self {
        Self {
            buses: env_usize("LOAD_BUSES", 4),
            points_per_bus: env_usize("LOAD_POINTS_PER_BUS", 75),
            baud: env_usize("LOAD_BAUD", 4800) as u32,
            interval_secs: env_usize("LOAD_INTERVAL_SECS", 1) as u64,
            slave_id: 1,
            silent_first: 0,
            storage: StorageBackend::from_env("LOAD_STORAGE"),
        }
    }

    pub fn total_points(&self) -> usize {
        self.buses * self.points_per_bus
    }
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// 已装配的虚拟农场。`stop()` 前须保持存活。
pub struct Farm {
    pub spec: FarmSpec,
    pub metrics: Arc<Metrics>,
    pub scheduler: Arc<Scheduler>,
    pub queue: Arc<DataQueue>,
    pub realtime_bus: Arc<RealtimeBus>,
    /// 第 5 层：北向专用队列接收端，由调用方交给 ConversionManager。
    pub north_rx:
        Option<tokio::sync::mpsc::Receiver<Arc<Vec<gateway_core::engine::model::DataPoint>>>>,
    /// 每总线串口桩（可断言事务数）
    pub ports: Vec<ThrottledModbusPort>,
    /// sqlite 后端时保留 DB 所在 tempdir，必须活到农场析构
    _sqlite_dir: Option<tempfile::TempDir>,
}

impl Farm {
    pub async fn build(spec: FarmSpec) -> Self {
        let metrics = Arc::new(Metrics::new());
        // A1：后端可切换（`LOAD_STORAGE=sqlite`），覆盖生产真实持久化路径。
        let (queue, sqlite_dir) = real_queue_for(spec.storage).await;
        let realtime_bus = RealtimeBus::new(4096);
        // 第 5 层：北向走专用无损队列
        let (north_tx, north_rx) = gateway_core::north::NorthFeed::channel(4096);
        let pipeline = ProcessingPipeline::spawn(PipelineSpawnInputs {
            edge: Arc::new(EdgeProcessor::new()),
            convert: ConvertEngine::empty(),
            virtual_points: gateway_core::pipeline::VirtualPointEngine::empty(),
            queue: queue.clone(),
            realtime_bus: Some(realtime_bus.clone()),
            north_tx: Some(north_tx),
            buffer: 4096,
            metrics: metrics.clone(),
        });

        let registry = ProtocolRegistry::new();
        let mut collectors: HashMap<String, Arc<dyn Collector>> = HashMap::new();
        let mut points: Vec<PointConfig> = Vec::with_capacity(spec.total_points());
        let mut ports: Vec<ThrottledModbusPort> = Vec::with_capacity(spec.buses);

        for i in 0..spec.buses {
            let bus_name = format!("loadbus{i}");
            let port = ThrottledModbusPort::new(spec.baud, spec.slave_id, spec.silent_first);
            let serial = Some(SerialHandle::from_stream(port.clone()));
            let deps = BusDeps {
                opener: Arc::new(MockOpener::new()),
                gpio: Arc::new(RecordingGpio::new()),
            };
            let bus_cfg = BusConfig {
                port_name: bus_name.clone(),
                protocol: "modbus".to_string(),
                // 超时须大于最坏批量应答线路耗时（4800 baud 下 254B ≈ 530ms）
                timeout_ms: 2000,
                retry_count: Some(0),
                retry_delay_ms: Some(20),
                de_pin: Some(4),
                channel_capacity: Some(64),
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
            ports.push(port);

            for j in 0..spec.points_per_bus {
                points.push(PointConfig {
                    id: format!("{bus_name}_p{j}"),
                    name: Some(format!("{bus_name}_p{j}")),
                    bus: bus_name.clone(),
                    protocol: "modbus".to_string(),
                    address: j.to_string(),
                    address_num: Some(j as u16),
                    slave_id: Some(spec.slave_id),
                    function_code: Some(3),
                    register_count: Some(1),
                    interval: spec.interval_secs.max(1),
                    unit: Some("u16".to_string()),
                    ..Default::default()
                });
            }
        }

        let scheduler = Arc::new(Scheduler::new(
            collectors,
            points,
            pipeline,
            metrics.clone(),
        ));

        Self {
            spec,
            metrics,
            scheduler,
            queue,
            realtime_bus,
            north_rx: Some(north_rx),
            ports,
            _sqlite_dir: sqlite_dir,
        }
    }

    pub fn start(&self) {
        self.scheduler.start();
    }

    pub async fn stop(&self) {
        self.scheduler.stop().await;
    }

    pub fn total_points(&self) -> usize {
        self.spec.total_points()
    }
}
