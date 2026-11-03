//! P1 端到端：虚拟点位派生量真实参与「入队 + 北向上报」。
//!
//! 单元测试（`pipeline::virtual_point::tests`）只覆盖求值器本身；
//! 本文件锁定**接线契约**——派生值不是「算出来就扔」：
//! 1. 派生 `DataPoint` 与普通点位一同进入 `data_queue`（可查历史）；
//! 2. 派生值随 RealtimeBus 扇出到北向 connector（订阅者能收到）；
//! 3. 派生值的 `point_id` = 虚拟点位 ID，`device_id` 取自虚拟点位配置；
//! 4. 输入不变时不重复产出（上报节流，避免刷屏）。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use gateway_core::config::{PointConfig, VirtualPoint};
use gateway_core::engine::error::GatewayResult;
use gateway_core::engine::metrics::Metrics;
use gateway_core::engine::model::DataPoint;
use gateway_core::north::{ConnectorHealth, ConversionManager, NorthConnector};
use gateway_core::pipeline::{
    CollectedBatch, ConvertEngine, EdgeProcessor, PipelineSpawnInputs, ProcessingPipeline,
    RealtimeBus, VirtualPointEngine,
};
use gateway_core::queue::DataQueue;

/// 记录收到的点位 id 集合的测试 connector。
struct RecordingConnector {
    batches: AtomicUsize,
    points: Mutex<Vec<String>>,
}

/// 简易互斥（测试内自备，避免与 core 内部锁类型耦合）。
type Mutex<T> = std::sync::Mutex<T>;

impl RecordingConnector {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            batches: AtomicUsize::new(0),
            points: Mutex::new(Vec::new()),
        })
    }

    fn point_ids(&self) -> Vec<String> {
        self.points.lock().expect("lock").clone()
    }
}

#[async_trait]
impl NorthConnector for RecordingConnector {
    fn name(&self) -> &str {
        "recording"
    }
    async fn publish_batch(&self, batch: &[DataPoint]) -> GatewayResult<()> {
        self.batches.fetch_add(1, Ordering::Relaxed);
        let mut pts = self.points.lock().expect("lock");
        pts.extend(batch.iter().map(|d| d.point_id.clone()));
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

fn virtual_point(id: &str, formula: &str) -> VirtualPoint {
    VirtualPoint {
        id: id.to_string(),
        name: id.to_string(),
        device_id: Some("d1".to_string()),
        formula: formula.to_string(),
        unit: "W".to_string(),
        decimals: 2,
        enabled: true,
        created_at: 0,
    }
}

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

async fn wait_subscriber(bus: &Arc<RealtimeBus>) {
    for _ in 0..200 {
        if bus.subscriber_count() > 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("realtime bus 无订阅者：ConversionManager 未启动");
}

async fn wait_points(connector: &Arc<RecordingConnector>, expected: usize) -> Vec<String> {
    for _ in 0..200 {
        let ids = connector.point_ids();
        if ids.len() >= expected {
            return ids;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    connector.point_ids()
}

/// 派生量端到端：采集 2 个点位 → 虚拟点位求值 → 入队 + 北向扇出。
#[tokio::test]
async fn derived_virtual_point_reaches_queue_and_northbound() {
    let metrics = Arc::new(Metrics::new());
    let queue = Arc::new(DataQueue::new_memory().expect("内存队列初始化失败"));
    let bus = RealtimeBus::new(64);

    let connector = RecordingConnector::new();
    let cm = ConversionManager::new(bus.clone());
    cm.register_connector(connector.clone() as Arc<dyn NorthConnector>);
    let cm_handle = tokio::spawn(async move { cm.run().await });
    wait_subscriber(&bus).await;

    // 虚拟点位：总功率 = U * I
    let engine = VirtualPointEngine::new(vec![virtual_point("vp_power", "{u} * {i}")]);
    let pipeline = ProcessingPipeline::spawn(PipelineSpawnInputs {
        edge: Arc::new(EdgeProcessor::new()),
        convert: ConvertEngine::empty(),
        virtual_points: engine.clone(),
        queue: queue.clone(),
        realtime_bus: Some(bus.clone()),
        buffer: 16,
        metrics: metrics.clone(),
    });

    pipeline
        .submit(CollectedBatch::new(vec![
            (
                point_config("u"),
                DataPoint::good("u", "d1", 220.0, 220.0, "f32"),
            ),
            (
                point_config("i"),
                DataPoint::good("i", "d1", 10.0, 10.0, "f32"),
            ),
        ]))
        .await;

    // 契约 1：3 个点进入队列（2 采集 + 1 派生）
    let len = wait_queue_len(&queue, 3).await;
    assert_eq!(len, 3, "派生虚拟点位应与普通点位一同入队，实际 {} 行", len);

    // 契约 2：北向 connector 收到派生点位
    let ids = wait_points(&connector, 3).await;
    assert!(
        ids.iter().any(|id| id == "vp_power"),
        "北向应收到派生点位 vp_power，实际收到 {:?}",
        ids
    );

    // 契约 3：派生点的身份正确
    let states = engine.states();
    let st = states.get("vp_power").expect("vp_power 状态");
    assert_eq!(st.value, Some(2200.0), "220 * 10 = 2200");
    assert!(st.error.is_none(), "求值不应报错: {:?}", st.error);

    // 历史里能查到派生点位
    let now = DataPoint::now();
    let history = queue
        .history_ex("vp_power", now - 60_000, now + 60_000, 100, None)
        .await
        .expect("history 查询失败");
    assert_eq!(history.items.len(), 1, "派生点位应落历史库（可被平台查询）");
    assert_eq!(history.items[0].value, 2200.0);
    assert_eq!(
        history.items[0].raw_type.as_deref(),
        Some("virtual"),
        "派生点在历史库中应带 virtual 类型标记"
    );

    pipeline.shutdown();
    cm_handle.abort();
}

/// 上报节流：输入不变时派生值不重复产出（避免同一数值反复刷屏）。
#[tokio::test]
async fn derived_value_is_not_re_emitted_when_input_unchanged() {
    let metrics = Arc::new(Metrics::new());
    let queue = Arc::new(DataQueue::new_memory().expect("内存队列初始化失败"));
    let engine = VirtualPointEngine::new(vec![virtual_point("vp_power", "{u} * {i}")]);
    let pipeline = ProcessingPipeline::spawn(PipelineSpawnInputs {
        edge: Arc::new(EdgeProcessor::new()),
        convert: ConvertEngine::empty(),
        virtual_points: engine.clone(),
        queue: queue.clone(),
        realtime_bus: None,
        buffer: 16,
        metrics: metrics.clone(),
    });

    let make_batch = || {
        CollectedBatch::new(vec![
            (
                point_config("u"),
                DataPoint::good("u", "d1", 220.0, 220.0, "f32"),
            ),
            (
                point_config("i"),
                DataPoint::good("i", "d1", 10.0, 10.0, "f32"),
            ),
        ])
    };

    pipeline.submit(make_batch()).await;
    // 首轮：2 采集 + 1 派生
    assert_eq!(wait_queue_len(&queue, 3).await, 3);

    // 第二轮输入完全相同 → 只入队 2 个采集点，派生值不再产出
    pipeline.submit(make_batch()).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let len = queue.len().await;
    assert_eq!(
        len, 5,
        "输入未变时派生值不应重复上报（期望 3 + 2 = 5），实际 {}",
        len
    );

    pipeline.shutdown();
}
