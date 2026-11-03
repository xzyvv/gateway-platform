//! 内置模拟消费者（SimConsumer）指标
//!
//! 为什么要有这三个指标：模拟消费者是「无 MQTT 通道时的兜底消费者」——
//! 它会把队列数据真的消费掉（出队 → 处理 → ack）但**不外发**。现场只看
//! `gateway_queue_depth` 无法区分「队列在涨（没人消费）」与「队列在降
//! （消费者在跑，数据被吃掉但没上报）」这两种完全相反的故障方向。
//!
//! - `gateway_sim_consumer_consumed_total`：累计消费条数（counter）
//! - `gateway_sim_consumer_batches_total`：累计消费批次数（counter）
//! - `gateway_sim_consumer_running`：运行状态（gauge，1 = 运行中）
//!
//! 注意：`SimConsumer` 允许不注入 metrics（测试 / 未接入监控的部署），
//! 此时本子结构不被写入，指标恒为 0——绝不能因此 panic。

use std::sync::atomic::AtomicI64;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

#[derive(Default)]
pub struct SimConsumerMetrics {
    /// 累计成功消费（ack）的数据条数。
    pub consumed_total: AtomicU64,
    /// 累计成功消费的批次数。
    pub batches_total: AtomicU64,
    /// 运行状态：1 = 运行中，0 = 未运行。
    pub running: AtomicI64,
}

impl SimConsumerMetrics {
    /// 输出到 Prometheus 文本缓冲区。
    pub fn render(&self, buf: &mut String) {
        use std::fmt::Write as _;
        let _ = writeln!(
            buf,
            "# HELP gateway_sim_consumer_consumed_total Points consumed (acked) by the built-in simulated consumer\n\
             # TYPE gateway_sim_consumer_consumed_total counter\n\
             gateway_sim_consumer_consumed_total {}",
            self.consumed_total.load(Ordering::Relaxed),
        );
        let _ = writeln!(
            buf,
            "# HELP gateway_sim_consumer_batches_total Batches consumed (acked) by the built-in simulated consumer\n\
             # TYPE gateway_sim_consumer_batches_total counter\n\
             gateway_sim_consumer_batches_total {}",
            self.batches_total.load(Ordering::Relaxed),
        );
        let _ = writeln!(
            buf,
            "# HELP gateway_sim_consumer_running Built-in simulated consumer running state (1 = running)\n\
             # TYPE gateway_sim_consumer_running gauge\n\
             gateway_sim_consumer_running {}",
            self.running.load(Ordering::Relaxed),
        );
    }
}
