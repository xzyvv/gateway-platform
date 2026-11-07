//! 基础设施端口（Ports）定义。
//!
//! # 为什么需要这一层
//!
//! `engine` 是规则引擎（报警判定、磁盘监控、指标采集），`north`（北向输出）与
//! `queue`（数据队列）是它**下游**的业务设施。但两者都需要 `engine` 的核心模型
//! （`DataPoint` / `Quality` / `Metrics`），于是天然形成双向依赖：
//!
//! ```text
//! engine ──→ north  (报警要发到 MQTT)
//! north  ──→ engine (北向用 engine 的 DataPoint/Metrics)
//! engine ──→ queue  (磁盘监控要通知队列磁盘状态)
//! queue  ──→ engine (队列存 engine 的 DataPoint)
//! ```
//!
//! 与 `domain/license.rs` 消除 `config ↔ license` 循环的做法一致：**把双方都需要的
//! 契约抽到中立层，由依赖方实现**。本模块就是这个中立层——`engine` 定义 trait
//! （依赖倒置），`north` / `queue` 提供实现。改造后 `engine` 不再依赖 `north` / `queue`。
//!
//! # 约定
//!
//! - trait 定义在本模块，实现放在依赖方（`north` / `queue`）的模块里；
//! - trait 名以 `Sink` / `Publisher` 结尾，语义是"把数据推给谁"；
//! - 若某实现方不需要被替换（如只有唯一实现且无测试替身），**不要**为它造 trait。

use std::future::Future;
use std::pin::Pin;

use crate::engine::error::GatewayResult;

/// 报警事件上报端口。
///
/// 由 `engine` 定义、`north` 实现。当前唯一实现是 MQTT（报警发到 `<topic>/alarm`），
/// 但引擎本身不应知道"发到 MQTT"这件事——将来接 Webhook / 短信 / 本地文件时，
/// 新增实现即可，引擎不动。
pub trait AlarmPublisher: Send + Sync {
    /// 发布一条已序列化的报警载荷。
    ///
    /// - `topic`: 目标 topic（含前缀，不含 `/alarm` 后缀，由实现方拼接）
    /// - `payload`: 已 JSON 序列化的 [`crate::engine::alarm::AlarmEvent`]
    fn publish_alarm<'a>(
        &'a self,
        topic: &'a str,
        payload: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

/// 磁盘健康状态通知端口。
///
/// 由 `engine` 定义、`queue` 实现。磁盘监控只关心两件事：
/// 把"磁盘是否可写"告诉队列（队列据此阻断入队 / 放行），
/// 以及磁盘恢复时把 fallback 缓冲刷回 SQLite。
pub trait DiskHealthSink: Send + Sync {
    /// 设置磁盘可写标志。`false` 时队列阻断低优先级写入，`true` 时恢复。
    fn set_disk_ok(&self, ok: bool);

    /// 磁盘恢复时把 fallback 缓冲刷回持久化存储。
    fn flush_fallback<'a>(&'a self)
        -> Pin<Box<dyn Future<Output = GatewayResult<()>> + Send + 'a>>;
}
