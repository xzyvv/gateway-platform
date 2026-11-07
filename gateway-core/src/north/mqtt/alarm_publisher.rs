//! 报警上报端口的 MQTT 实现（依赖倒置的"依赖方实现"侧）。
//!
//! `engine::ports::AlarmPublisher` 由引擎定义（它只关心"把报警载荷发出去"），
//! MQTT 是它当前唯一的实现方。引擎不感知 MQTT，本模块也不感知引擎的业务逻辑——
//! 只做一件事：把已序列化的载荷投到指定 topic。

use std::future::Future;
use std::pin::Pin;

use super::MqttClient;
use crate::engine::ports::AlarmPublisher;

impl AlarmPublisher for MqttClient {
    /// 发布一条已序列化的报警载荷。
    ///
    /// 语义沿用 [`MqttClient::publish_raw`]：QoS=AtLeastOnce、不保留、
    /// 失败不重试（报警允许偶尔丢失），断路器打开或已有发布在飞时直接丢弃。
    fn publish_alarm<'a>(
        &'a self,
        topic: &'a str,
        payload: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let ok = self.publish_raw(topic, payload).await;
            if !ok {
                tracing::debug!("alarm publish skipped or failed: topic={}", topic);
            }
        })
    }
}
