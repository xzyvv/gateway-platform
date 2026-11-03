//! Webhook 北向指标（counter）
//!
//! 取代旧 `Metrics.webhook_publish_success` / `webhook_publish_failure` /
//! `webhook_data_lost_total` / `webhook_giveup_total` 四个顶层字段。
//! `render` 输出与拆分前逐字节一致。

use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

#[derive(Default)]
pub struct WebhookMetrics {
    /// 投递成功（已进入重试队列）次数。
    pub publish_success: AtomicU64,
    /// 投递失败（可重试）次数。
    pub publish_failure: AtomicU64,
    /// r79-OBX1：永久失败（4xx）导致数据物理丢弃次数，运维须立即介入。
    pub data_lost: AtomicU64,
    /// r79-OBX1：重试用尽（MAX_WEBHOOK_ATTEMPTS）后放弃、outbox 行被删除次数。
    pub giveup: AtomicU64,
}

impl WebhookMetrics {
    /// 输出到 Prometheus 文本缓冲区（与拆分前同文案）。
    pub fn render(&self, buf: &mut String) {
        use std::fmt::Write as _;
        let _ = writeln!(
            buf,
            "# HELP gateway_webhook_publish_total Webhook publish attempts\n\
             # TYPE gateway_webhook_publish_total counter\n\
             gateway_webhook_publish_total{{result=\"success\"}} {}\n\
             gateway_webhook_publish_total{{result=\"failure\"}} {}",
            self.publish_success.load(Ordering::Relaxed),
            self.publish_failure.load(Ordering::Relaxed),
        );
        let _ = writeln!(
            buf,
            "# HELP gateway_webhook_data_lost_total Webhook messages permanently lost (4xx terminal)\n\
             # TYPE gateway_webhook_data_lost_total counter\n\
             gateway_webhook_data_lost_total {}",
            self.data_lost.load(Ordering::Relaxed),
        );
        let _ = writeln!(
            buf,
            "# HELP gateway_webhook_giveup_total Webhook messages dropped after exhausting retries\n\
             # TYPE gateway_webhook_giveup_total counter\n\
             gateway_webhook_giveup_total {}",
            self.giveup.load(Ordering::Relaxed),
        );
    }
}
