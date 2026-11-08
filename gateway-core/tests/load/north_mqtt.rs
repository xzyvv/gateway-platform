//! 真 MQTT broker 北向（决策 1A）。
//!
//! 装配与生产同构，零新增基建（broker 桩复用 `chaos/harness.rs`）：
//! - 真 `MqttBroker`（MQTT 3.1.1 桩）承接 PUBLISH 并回 PubAck；
//! - 真 `MqttClient` 与 `ProcessingPipeline` **共用同一个 `Arc<DataQueue>`**，
//!   由 `publish_loop` 出队 → 构建 payload → 发布，收到 PubAck 才 `queue.ack`；
//! - 仍然跑 `ConversionManager` 并注册该客户端，消费 `NorthFeed` 专用队列，
//!   避免队列积满 4096 批后反压 pipeline（与生产 bootstrap 一致）。
//!
//! 注：`MqttClient` 的 `NorthConnector::publish_batch` 是幂等空实现（数据已在
//! pipeline 层入队），因此 `ConversionManager` 只起「消费北向队列」的作用，
//! 不会二次入队造成双发。
//!
//! 证据口径：以 broker **实收** PUBLISH 条数 / 实收点数（解析 payload）为准，
//! 而不是桩内部计数器——这才证明数据真的走到了真 broker。

use std::sync::Arc;
use std::time::Duration;

use gateway_core::engine::metrics::Metrics;
use gateway_core::engine::model::DataPoint;
use gateway_core::north::mqtt::MqttClient;
use gateway_core::north::{ConversionManager, NorthConnector};
use gateway_core::queue::DataQueue;

use super::chaos_harness::{make_mqtt_config, MqttBroker};

/// 北向 topic（与生产同形态）。
const LOAD_TOPIC: &str = "gateway/load";

pub struct MqttNorth {
    broker: MqttBroker,
    client: Arc<MqttClient>,
    /// 消费北向专用队列的转换管理器任务（停机时 abort）
    cm_handle: tokio::task::JoinHandle<()>,
}

impl MqttNorth {
    /// 启动真 broker 桩 + 真 MQTT 客户端（与 pipeline 共用 `queue`）。
    pub async fn start(
        queue: Arc<DataQueue>,
        metrics: Arc<Metrics>,
        north_rx: tokio::sync::mpsc::Receiver<Arc<Vec<DataPoint>>>,
    ) -> Self {
        let broker = MqttBroker::start().await.expect("mqtt broker 桩启动失败");
        let config = make_mqtt_config(&broker.addr, LOAD_TOPIC);
        let client = Arc::new(
            MqttClient::new(config, queue, None, None, metrics).expect("mqtt client 构建失败"),
        );
        client.start();

        // 与生产一致：ConversionManager 负责把北向专用队列排空
        // （publish_batch 为空实现，不会重复入队）。
        let cm = ConversionManager::new(north_rx);
        cm.register_connector(client.clone() as Arc<dyn NorthConnector>);
        let cm_handle = tokio::spawn(async move { cm.run().await });

        Self {
            broker,
            client,
            cm_handle,
        }
    }

    /// broker 实收 PUBLISH 条数。
    pub fn batches(&self) -> u64 {
        self.broker.publish_count() as u64
    }

    /// broker 实收点数（逐条 payload 解析后累加）。
    pub fn points(&self) -> u64 {
        self.broker
            .received
            .lock()
            .iter()
            .filter_map(|(_, payload, _, _)| decode_point_count(payload))
            .sum()
    }

    /// 等 broker 实收至少 n 条 PUBLISH。
    pub async fn wait_for_publishes(&self, n: usize, timeout: Duration) -> bool {
        self.broker.wait_for_publishes(n, timeout).await
    }

    /// 停机：先停客户端（等后台任务退出），再收掉转换管理器任务。
    pub async fn stop(&self) {
        self.client.stop().await;
        self.cm_handle.abort();
    }
}

/// 解析单条上行 payload 的点位数。
///
/// payload 首字节是压缩标志（本测试 `compression=None`，恒为 0x00），其后为
/// JSON 数组 `[{ts, device_id, points:[{point_id, ...}]}]`。压缩/解析失败时
/// 返回 `None`（不计入），避免把坏包算成有效数据。
fn decode_point_count(payload: &[u8]) -> Option<u64> {
    let body = payload.strip_prefix(&[0x00u8])?;
    let reports: Vec<serde_json::Value> = serde_json::from_slice(body).ok()?;
    Some(
        reports
            .iter()
            .filter_map(|r| r.get("points").and_then(|p| p.as_array()))
            .map(|points| points.len() as u64)
            .sum(),
    )
}
