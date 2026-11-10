//! N-1 防回归：MQTT 北向**单次投递**（不得双写 data_queue / 重复 PUBLISH）。
//!
//! 缺陷背景（v7 评估 N-1）：
//! `ProcessingPipeline` 与 `MqttClient` 共用同一个 `Arc<DataQueue>`，且
//! `MqttClient` 的 `NorthConnector::publish_batch` 又二次入队，导致同一批点
//! 在 `data_queue` 中出现 **2 行**，broker 收到 **2 条** PUBLISH。
//! `data_queue` 无唯一约束，存储层无法去重。
//!
//! 本文件锁定三条不变量：
//! 1. 一批采集 → `data_queue` **恰好 1 行**（pipeline 唯一入队点）；
//! 2. 扇出到各 connector **恰好 1 批 / 1 点**（NorthDispatcher 不重复投递）；
//! 3. MQTT 关闭（未注册 connector）时历史查询**仍有数据**
//!    （数据落库与北向发送解耦，不因北向关闭而丢历史）。
//!
//! 不依赖真实 broker：MQTT 客户端以「不可达 broker」构造（构造不失败），
//! 出队用 `DataQueue::dequeue` 模拟 `publish_loop` 的取数行为——
//! 队列里只有 1 行 ⇒ `publish_loop` 只会构造 1 条 PUBLISH 报文。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use gateway_core::config::{MqttConfig, PointConfig};
use gateway_core::engine::error::GatewayResult;
use gateway_core::engine::metrics::Metrics;
use gateway_core::engine::model::DataPoint;
use gateway_core::north::mqtt::MqttClient;
use gateway_core::north::{ConnectorHealth, NorthConnector, NorthDispatcher};
use gateway_core::pipeline::{
    CollectedBatch, ConvertEngine, EdgeProcessor, PipelineSpawnInputs, ProcessingPipeline,
    RealtimeBus,
};
use gateway_core::queue::DataQueue;

/// 记录收到的批次数 / 点位数的测试 connector。
struct CountingConnector {
    batches: AtomicUsize,
    points: AtomicUsize,
}

impl CountingConnector {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            batches: AtomicUsize::new(0),
            points: AtomicUsize::new(0),
        })
    }

    /// 已收到的批次数。
    fn batches(&self) -> usize {
        self.batches.load(Ordering::Relaxed)
    }

    /// 已收到的点位数。
    fn points(&self) -> usize {
        self.points.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl NorthConnector for CountingConnector {
    fn name(&self) -> &str {
        "counting"
    }
    async fn publish_batch(&self, batch: &[DataPoint]) -> GatewayResult<()> {
        self.batches.fetch_add(1, Ordering::Relaxed);
        self.points.fetch_add(batch.len(), Ordering::Relaxed);
        Ok(())
    }
    fn health(&self) -> ConnectorHealth {
        ConnectorHealth {
            connected: true,
            last_error: None,
        }
    }
    fn is_enabled(&self) -> bool {
        true
    }
}

/// 构造一个测试点位（默认无聚合 / 无死区 / 不判变化 → 数据点原样通过边缘处理）。
fn point_config(id: &str) -> PointConfig {
    PointConfig {
        id: id.to_string(),
        bus: "bus0".to_string(),
        protocol: "modbus".to_string(),
        address: "1".to_string(),
        interval: 1,
        ..Default::default()
    }
}

/// 轮询等待队列长度达到期望值（pipeline 有 10ms micro-batch 窗口）。
async fn wait_queue_len(queue: &Arc<DataQueue>, expected: usize) -> usize {
    for _ in 0..200 {
        let len = queue.len().await;
        if len == expected {
            return len;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    queue.len().await
}

/// 等待 n 个批次到达（背压队列投递是异步的）。
async fn wait_batches(connector: &CountingConnector, expected: usize) {
    for _ in 0..200 {
        if connector.batches() >= expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!(
        "北向 connector 未在超时内收到 {} 批（实际 {}）",
        expected,
        connector.batches()
    );
}

/// 一批采集 → data_queue 恰好 1 行 → 各 connector 恰好收 1 批 1 点。
#[tokio::test]
async fn n1_one_collected_batch_is_delivered_exactly_once() {
    let metrics = Arc::new(Metrics::new());
    let queue = Arc::new(DataQueue::new_memory().expect("内存队列初始化失败"));
    let bus = RealtimeBus::new(64);

    // 北向侧：MQTT（不可达 broker，构造不失败）+ 一个计数 connector
    // 第 5 层：北向走专用无损队列
    let (north_tx, north_rx) = gateway_core::north::NorthFeed::channel(64);
    let mqtt_cfg = MqttConfig {
        enabled: true,
        broker: "tcp://127.0.0.1:1".to_string(),
        ..MqttConfig::default()
    };
    let mqtt = Arc::new(
        MqttClient::new(mqtt_cfg, queue.clone(), None, None, metrics.clone())
            .expect("MqttClient 构造失败"),
    );
    let counter = CountingConnector::new();

    let cm = NorthDispatcher::new(north_rx);
    cm.register_connector(mqtt.clone() as Arc<dyn NorthConnector>);
    cm.register_connector(counter.clone() as Arc<dyn NorthConnector>);
    let cm_handle = tokio::spawn(async move { cm.run().await });

    // 采集侧：pipeline 与 MQTT 共用同一队列（与 bootstrap 装配一致）
    let pipeline = ProcessingPipeline::spawn(PipelineSpawnInputs {
        edge: Arc::new(EdgeProcessor::new()),
        convert: ConvertEngine::empty(),
        virtual_points: gateway_core::pipeline::VirtualPointEngine::empty(),
        queue: queue.clone(),
        realtime_bus: Some(bus.clone()),
        north_tx: Some(north_tx),
        buffer: 16,
        metrics: metrics.clone(),
    });

    pipeline
        .submit(CollectedBatch::new(vec![(
            point_config("p1"),
            DataPoint::good("p1", "d1", 1.0, 1.0, "u16"),
        )]))
        .await;

    // 不变量 1：data_queue 恰好 1 行（N-1 修复前为 2 行）
    let len = wait_queue_len(&queue, 1).await;
    assert_eq!(
        len, 1,
        "一批采集在 data_queue 中应恰好落 1 行；出现多行说明北向 SPI 二次入队（N-1 回归）"
    );

    // 等待扇出完成
    wait_batches(&counter, 1).await;

    // 不变量 2：每个 connector 恰好收 1 批 / 1 点
    assert_eq!(
        counter.batches(),
        1,
        "connector 应恰好收到 1 批（重复投递说明扇出与入队路径重叠）"
    );
    assert_eq!(counter.points(), 1, "connector 应恰好收到 1 个点位");

    // 不变量 3（隐含）：队列里只有 1 行 ⇒ publish_loop 只构造 1 条 PUBLISH
    let dequeued = queue.dequeue(100).await.expect("dequeue 失败");
    assert_eq!(
        dequeued.len(),
        1,
        "publish_loop 一次 dequeue 应只取到 1 行（>1 说明存在重复入队）"
    );
    assert_eq!(dequeued[0].point_id, "p1");

    pipeline.shutdown();
    cm_handle.abort();
}

/// MQTT 关闭（未注册北向 connector）时，历史查询仍有数据。
#[tokio::test]
async fn n1_history_survives_when_mqtt_disabled() {
    let metrics = Arc::new(Metrics::new());
    let queue = Arc::new(DataQueue::new_memory().expect("内存队列初始化失败"));
    let bus = RealtimeBus::new(64);

    // 不注册任何 connector —— 模拟「MQTT 未启用」的现场
    let pipeline = ProcessingPipeline::spawn(PipelineSpawnInputs {
        edge: Arc::new(EdgeProcessor::new()),
        convert: ConvertEngine::empty(),
        virtual_points: gateway_core::pipeline::VirtualPointEngine::empty(),
        queue: queue.clone(),
        realtime_bus: Some(bus.clone()),
        north_tx: None,
        buffer: 16,
        metrics: metrics.clone(),
    });

    pipeline
        .submit(CollectedBatch::new(vec![(
            point_config("p2"),
            DataPoint::good("p2", "d1", 2.0, 2.0, "u16"),
        )]))
        .await;

    assert_eq!(wait_queue_len(&queue, 1).await, 1);

    let now = DataPoint::now();
    let history = queue
        .history_ex("p2", now - 60_000, now + 60_000, 100, None)
        .await
        .expect("history 查询失败");
    assert_eq!(
        history.items.len(),
        1,
        "MQTT 关闭时历史数据仍须落库（数据落库不得依赖北向通道）"
    );

    pipeline.shutdown();
}
