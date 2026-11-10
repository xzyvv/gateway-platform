//! 死信（Dead Letter）门面：trait 契约 + DTO + 两套实现。
//!
//! 从 `web::traits` 拆出的原因：`traits.rs` 曾同时承载「五个注入 trait」
//! 与「死信存储门面」，行数一度越过行数预算上限（C24 棘轮）。
//! 死信是一条独立的业务链路（outbox → 死信 → 重投），单独成模块后
//! 契约与实现可各自演进，且不引入新依赖。
//!
//! 本模块在 `web::traits` 下以 `pub use` 重新导出，
//! 因此既有调用点（`crate::web::traits::DeadLetterTrait` 等）无需改动。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// 死信队列门面（Webhook 不可投递批次）。
///
/// 为什么需要这一层：`web/api/**` 的架构约束只允许引用
/// `gateway_core::{config, engine, crypto, license, shared}`，
/// 而 outbox 存储位于 `gateway_core::north::mqtt::outbox`。
/// 因此把对 outbox 的访问收在这个 trait 后面，handler 只见 DTO；
/// 具体实现放在本模块（非 `web/api/**`，不受白名单约束）。
///
/// 语义：**移入死信 ≠ 已交付**。它只保证"没有静默消失"，
/// 是否送达仍取决于接收端修复后的人工重投。
pub trait DeadLetterTrait: Send + Sync {
    /// 分页列出某协议的死亡批次（最近失败在前）。
    fn list<'a>(
        &'a self,
        protocol: &'a str,
        limit: usize,
        offset: usize,
    ) -> Pin<
        Box<
            dyn Future<Output = gateway_core::engine::error::GatewayResult<Vec<DeadLetterDto>>>
                + Send
                + 'a,
        >,
    >;

    /// 统计某协议（`None` = 全部）的死信条数。
    fn count<'a>(
        &'a self,
        protocol: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = gateway_core::engine::error::GatewayResult<i64>> + Send + 'a>>;

    /// 重投一条死信，返回新 outbox 行 id；`0` 表示同一 data_id 已有在途记录。
    fn replay<'a>(
        &'a self,
        id: i64,
    ) -> Pin<Box<dyn Future<Output = gateway_core::engine::error::GatewayResult<i64>> + Send + 'a>>;

    /// 清理死信（`protocol` / `older_than_ms` 为 None 表示不限制），返回删除条数。
    fn purge<'a>(
        &'a self,
        protocol: Option<&'a str>,
        older_than_ms: Option<i64>,
    ) -> Pin<Box<dyn Future<Output = gateway_core::engine::error::GatewayResult<usize>> + Send + 'a>>;

    /// 当前是否可用（outbox 库是否成功打开）。false 时 handler 直接报 503，
    /// 而不是返回"0 条死信"——后者会让运维误以为一切正常。
    fn available(&self) -> bool;
}

/// 死信 DTO（跨层运输对象，避免 core 的 payload 细节泄漏到 API）。
///
/// D3：走 ts-rs 生成前端类型，而不是在 `gateway-web/src/api/deadLetter.ts`
/// 手抄一份 interface——手抄件与后端字段一旦漂移（改名/加字段），前端不会编译报错，
/// 只会在运行时静默拿到 `undefined`（本轮新增 `trimmed_total` 就是活例）。
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct DeadLetterDto {
    // 数字字段统一声明为 `number`：这里全是 epoch 毫秒与行数/字节数，远小于
    // 2^53，`bigint` 只会逼前端在 `timeColumn`、分页等处到处做转换，
    // 却换不来任何精度（与 `ScheduleTimeline` 的处理一致）。
    #[ts(type = "number")]
    pub id: i64,
    pub protocol: String,
    #[ts(type = "number")]
    pub data_id: i64,
    pub topic: String,
    #[ts(type = "number")]
    pub payload_len: i64,
    pub payload_preview: String,
    pub qos: i32,
    /// 消息最初进入 outbox 的时间（epoch 毫秒）
    #[ts(type = "number")]
    pub created_at: i64,
    /// 判定为死信的时间（epoch 毫秒）
    #[ts(type = "number")]
    pub failed_at: i64,
    pub attempt_count: i32,
    pub reason: String,
    /// 重投时间（epoch 毫秒），null 表示尚未重投
    ///
    /// `Option` 的可空性必须写进 `type`：`#[ts(type = ...)]` 会整体覆盖推导，
    /// 只写 `number` 会让前端以为它永不为空。
    #[ts(type = "number | null")]
    pub replayed_at: Option<i64>,
}

/// 未启用 / 打开失败时的占位实现：一切查询返回空、重投返回错误。
pub struct NoopDeadLetter;

impl DeadLetterTrait for NoopDeadLetter {
    fn list<'a>(
        &'a self,
        _protocol: &'a str,
        _limit: usize,
        _offset: usize,
    ) -> Pin<
        Box<
            dyn Future<Output = gateway_core::engine::error::GatewayResult<Vec<DeadLetterDto>>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async { Ok(Vec::new()) })
    }
    fn count<'a>(
        &'a self,
        _protocol: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = gateway_core::engine::error::GatewayResult<i64>> + Send + 'a>>
    {
        Box::pin(async { Ok(0) })
    }
    fn replay<'a>(
        &'a self,
        _id: i64,
    ) -> Pin<Box<dyn Future<Output = gateway_core::engine::error::GatewayResult<i64>> + Send + 'a>>
    {
        Box::pin(async {
            Err(gateway_core::engine::error::GatewayError::Message(
                "dead letter store unavailable".to_string(),
            ))
        })
    }
    fn purge<'a>(
        &'a self,
        _protocol: Option<&'a str>,
        _older_than_ms: Option<i64>,
    ) -> Pin<Box<dyn Future<Output = gateway_core::engine::error::GatewayResult<usize>> + Send + 'a>>
    {
        Box::pin(async {
            Err(gateway_core::engine::error::GatewayError::Message(
                "dead letter store unavailable".to_string(),
            ))
        })
    }
    fn available(&self) -> bool {
        false
    }
}

/// 基于真实 outbox SQLite 的实现。
///
/// 与 webhook 客户端各自持有一个连接（同一文件，WAL 模式）：
/// - 好处：不必把 outbox 句柄从 north 引导层穿透到 Web 层，避免破坏
///   「web 不直接依赖 north 运行时」的边界；
/// - 代价：多一个 SQLite 连接（约几十 KB），写锁竞争由 WAL + busy_timeout=5000 化解。
///
/// 所有方法都走 `spawn_blocking`：rusqlite 是同步阻塞 API，
/// 直接在 async 上下文里调用会卡住 tokio worker。
pub struct SqliteDeadLetter {
    outbox: Arc<parking_lot::Mutex<gateway_core::north::mqtt::outbox::OutboxStore>>,
}

impl SqliteDeadLetter {
    /// 打开 outbox 库并返回死信门面。
    pub fn open(path: &std::path::Path) -> gateway_core::engine::error::GatewayResult<Self> {
        let store = gateway_core::north::mqtt::outbox::OutboxStore::open(path)?;
        Ok(Self {
            outbox: Arc::new(parking_lot::Mutex::new(store)),
        })
    }
}

impl DeadLetterTrait for SqliteDeadLetter {
    fn list<'a>(
        &'a self,
        protocol: &'a str,
        limit: usize,
        offset: usize,
    ) -> Pin<
        Box<
            dyn Future<Output = gateway_core::engine::error::GatewayResult<Vec<DeadLetterDto>>>
                + Send
                + 'a,
        >,
    > {
        let outbox = self.outbox.clone();
        let protocol = protocol.to_string();
        Box::pin(async move {
            let rows = tokio::task::spawn_blocking(move || {
                outbox.lock().list_dead_letters(&protocol, limit, offset)
            })
            .await
            .map_err(|e| {
                gateway_core::engine::error::GatewayError::Message(format!(
                    "join list dead letters: {e}"
                ))
            })??;
            Ok(rows
                .into_iter()
                .map(|r| DeadLetterDto {
                    id: r.id,
                    protocol: r.protocol,
                    data_id: r.data_id,
                    topic: r.topic,
                    payload_len: r.payload_len,
                    payload_preview: r.payload_preview,
                    qos: r.qos,
                    created_at: r.created_at,
                    failed_at: r.failed_at,
                    attempt_count: r.attempt_count,
                    reason: r.reason,
                    replayed_at: r.replayed_at,
                })
                .collect())
        })
    }

    fn count<'a>(
        &'a self,
        protocol: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = gateway_core::engine::error::GatewayResult<i64>> + Send + 'a>>
    {
        let outbox = self.outbox.clone();
        let protocol = protocol.map(|p| p.to_string());
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                outbox.lock().count_dead_letters(protocol.as_deref())
            })
            .await
            .map_err(|e| {
                gateway_core::engine::error::GatewayError::Message(format!(
                    "join count dead letters: {e}"
                ))
            })?
        })
    }

    fn replay<'a>(
        &'a self,
        id: i64,
    ) -> Pin<Box<dyn Future<Output = gateway_core::engine::error::GatewayResult<i64>> + Send + 'a>>
    {
        let outbox = self.outbox.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || outbox.lock().replay_dead_letter(id))
                .await
                .map_err(|e| {
                    gateway_core::engine::error::GatewayError::Message(format!(
                        "join replay dead letter: {e}"
                    ))
                })?
        })
    }

    fn purge<'a>(
        &'a self,
        protocol: Option<&'a str>,
        older_than_ms: Option<i64>,
    ) -> Pin<Box<dyn Future<Output = gateway_core::engine::error::GatewayResult<usize>> + Send + 'a>>
    {
        let outbox = self.outbox.clone();
        let protocol = protocol.map(|p| p.to_string());
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                outbox
                    .lock()
                    .purge_dead_letters(protocol.as_deref(), older_than_ms)
            })
            .await
            .map_err(|e| {
                gateway_core::engine::error::GatewayError::Message(format!(
                    "join purge dead letters: {e}"
                ))
            })?
        })
    }

    fn available(&self) -> bool {
        true
    }
}
