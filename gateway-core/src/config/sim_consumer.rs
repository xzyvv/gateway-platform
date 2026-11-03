//! 内置模拟消费者（SimConsumer）配置
//!
//! 配置类型归属 `config` 层，避免 `config` 反向依赖 `north::sim_consumer`
//! （与 `config/webhook.rs` 的分层做法一致）。
//!
//! 为什么需要它：生产上行只有 MQTT 发布循环会 dequeue/ack 队列。现场没有可用
//! MQTT 通道时，队列只进不出——`queue_health(has_consumer=false)` 会把存量当
//! 成「本地历史库」而不是故障，但数据永远走不完「出队 → 处理 → ack」链路。
//! 打开本开关后，内置的模拟消费者会按批出队并 ack（不外发任何数据），
//! 让链路完整走通，同时消除「有积压却没有消费者」的告警歧义。

/// 模拟消费者配置
#[derive(
    Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize, schemars::JsonSchema, ts_rs::TS,
)]
#[ts(export)]
pub struct SimConsumerConfig {
    /// 是否启用模拟消费者。
    ///
    /// 默认关闭：有真实北向通道（MQTT）时不需要它——两者同时开启会让两个
    /// 消费者分流同一批队列数据，导致上报丢数。
    #[serde(default)]
    pub enabled: bool,
    /// 单批出队条数（与 MQTT 发布循环的批量语义对齐）。
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
    /// 队列为空时的轮询间隔（毫秒）。
    #[serde(default = "default_interval_ms")]
    #[ts(type = "number")]
    pub interval_ms: u64,
    /// 每消费满多少条打一条 INFO 摘要日志（0 = 不打摘要）。
    #[serde(default = "default_log_every")]
    pub log_every: usize,
}

fn default_batch_size() -> usize {
    100
}
fn default_interval_ms() -> u64 {
    1000
}
fn default_log_every() -> usize {
    100
}

impl Default for SimConsumerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            batch_size: default_batch_size(),
            interval_ms: default_interval_ms(),
            log_every: default_log_every(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SimConsumerConfig;

    /// 契约：缺省字段必须全部落到默认值——老配置里没有 `sim_consumer` 段时
    /// 解析结果必须是「关闭 + 批量 100 + 间隔 1s」，不得因为新增字段起不来。
    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let cfg: SimConsumerConfig = serde_json::from_str("{}").expect("空对象应解析成功");
        assert!(!cfg.enabled, "默认必须关闭（存量部署零影响）");
        assert_eq!(cfg.batch_size, 100);
        assert_eq!(cfg.interval_ms, 1000);
        assert_eq!(cfg.log_every, 100);
    }

    /// 契约：显式给出的字段覆盖默认值。
    #[test]
    fn explicit_fields_override_defaults() {
        let cfg: SimConsumerConfig = serde_json::from_str(
            r#"{"enabled": true, "batch_size": 20, "interval_ms": 250, "log_every": 5}"#,
        )
        .expect("完整配置应解析成功");
        assert!(cfg.enabled);
        assert_eq!(cfg.batch_size, 20);
        assert_eq!(cfg.interval_ms, 250);
        assert_eq!(cfg.log_every, 5);
    }
}
