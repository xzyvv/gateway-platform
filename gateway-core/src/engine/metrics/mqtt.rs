//! MQTT 北向指标（counter）
//!
//! 取代旧 `Metrics.mqtt_publish_success` / `mqtt_publish_failure` /
//! `mqtt_connack_refused_total` 三个顶层字段。
//!
//! `render` 输出的 HELP/TYPE/样本行与拆分前逐字节一致（Prom 契约不变）。
//! `mqtt_in_flight` 属 Gauge 且语义为「跨模块共享状态」，仍留在 `Metrics` 顶层。

use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

#[derive(Default)]
pub struct MqttMetrics {
    /// 发布成功次数。
    pub publish_success: AtomicU64,
    /// 发布失败次数。
    pub publish_failure: AtomicU64,
    /// R2: ConnAck 被拒次数（鉴权失败 / 服务不可用 / 标识被拒）。
    pub connack_refused: AtomicU64,
    /// X2: outbox 命令通道 (try_send) 满时退化为同步 `spawn_blocking` 持久化的次数。
    ///
    /// `publish.rs` 的正常路径是：
    ///   1) `try_send(OutboxCmd::Persist(msg))` 进命令通道；
    ///   2) `try_send` 失败（通道满）→ `sync_msg = Some(msg)`，后续在
    ///      `spawn_blocking` 里同步写 sqlite。
    ///
    /// 第 2 步退化本身是设计选择（背压超额时立即落盘不让采集侧阻塞），
    /// 但**只能在日志里看到频率**——上一轮 Bug-N1 给 `outbox_degraded_total`
    /// 加了 sqlite open 失败的兜底（不同失败模式），这条通道满到退化路径
    /// 仍未被任何 metric 覆盖。
    ///
    /// 告警示例：
    ///   `rate(gateway_mqtt_outbox_sync_fallback_total[5m]) > 1 for 5m` ——
    /// 表示 outbox task 已成吞吐瓶颈，需调大 outbox capacity 或后端落盘速度。
    pub outbox_sync_fallback: AtomicU64,
}

impl MqttMetrics {
    /// 输出到 Prometheus 文本缓冲区（与拆分前同文案）。
    pub fn render(&self, buf: &mut String) {
        use std::fmt::Write as _;
        let _ = writeln!(
            buf,
            "# HELP gateway_mqtt_publish_total MQTT publish attempts\n\
             # TYPE gateway_mqtt_publish_total counter\n\
             gateway_mqtt_publish_total{{result=\"success\"}} {}\n\
             gateway_mqtt_publish_total{{result=\"failure\"}} {}",
            self.publish_success.load(Ordering::Relaxed),
            self.publish_failure.load(Ordering::Relaxed),
        );
        let _ = writeln!(
            buf,
            "# HELP gateway_mqtt_connack_refused_total MQTT ConnAck refused by broker (auth/permission)\n\
             # TYPE gateway_mqtt_connack_refused_total counter\n\
             gateway_mqtt_connack_refused_total {}",
            self.connack_refused.load(Ordering::Relaxed),
        );
        let fallback = self.outbox_sync_fallback.load(Ordering::Relaxed);
        let _ = writeln!(
            buf,
            "# HELP gateway_mqtt_outbox_sync_fallback_total MQTT outbox task fell back to synchronous sqlite persist (cmd channel full)\n\
             # TYPE gateway_mqtt_outbox_sync_fallback_total counter\n\
             gateway_mqtt_outbox_sync_fallback_total {fallback}"
        );
    }
}
