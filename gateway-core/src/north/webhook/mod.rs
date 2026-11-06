//! HTTP Webhook 北向上报（持久化版）
//!
//! 与 MQTT 并行的另一条北向上报通道。数据先持久化到 outbox（protocol='http'），
//! 再异步 POST 到配置的 URL，发送成功后 ack 删除。
//!
//! 可靠性：与 MQTT 同级，进程重启后 replay 未确认消息重发，不丢数据。
//!
//! 按职责拆分自原单文件 `webhook.rs`：`client`（发送 / replay / flush 主循环）、
//! `retry`（重试分类与退避）、`tests`（回归用例）。

mod client;
mod retry;

#[cfg(test)]
#[path = "retry_tests.rs"]
mod retry_tests;
#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[cfg(test)]
use client::next_processed_max_id;
pub use client::WebhookClient;

#[cfg(test)]
use crate::north::mqtt::outbox::OutgoingMsg;
#[cfg(test)]
use retry::{webhook_data_id, SendOutcome};
