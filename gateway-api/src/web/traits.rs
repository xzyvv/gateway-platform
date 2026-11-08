//! Web 层 trait 注入接口（M2-15）
//!
//! 将 AppState 中的关键字段（scheduler/queue/gateway/ota）抽象为 trait 对象，
//! 便于在集成测试中注入 mock 实例，避免依赖真实硬件/SQLite/串口。
//!
//! 设计原则（A-2 风格，与 gateway_core::south::protocol::Collector 保持一致）：
//! 使用原生 async fn（手写 Pin<Box<dyn Future>>），替代 async_trait 宏，
//! 保持对象安全（支持 Arc<dyn Trait>）。

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use gateway_core::config::{BusConfig, GatewayConfig, PointConfig};
use gateway_core::engine::error::GatewayResult;
use gateway_core::north::mqtt::MqttClient;
use gateway_core::platform::ota::OtaManager;
use gateway_core::queue::DataQueue;
use gateway_core::south::protocol::Collector;
use gateway_core::south::scheduler::{DeviceStatus, ScheduleTimeline, Scheduler};
use gateway_core::support::app::Gateway;

/// 死信队列门面（Webhook 不可投递批次）。
///
/// 为什么需要这一层：`web/api/**` 的架构约束只允许引用
/// `gateway_core::{config, engine, crypto, license, support}`，
/// 而 outbox 存储位于 `gateway_core::north::mqtt::outbox`。
/// 因此把对 outbox 的访问收在这个 trait 后面，handler 只见 DTO；
/// 具体实现放在本文件（非 `web/api/**`，不受白名单约束）。
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

/// Scheduler trait（仅包含 web handler 实际调用的方法）
pub trait SchedulerTrait: Send + Sync {
    /// 返回所有 collector（按 bus_name 索引），供点位实时读取定位 collector
    fn collectors(&self) -> HashMap<String, Arc<dyn Collector>>;
    /// 返回所有设备的运行状态（用于 /api/status 等只读端点）
    fn device_status(&self) -> Vec<DeviceStatus>;

    /// F-11：采集调度真实时序快照（每组真实执行窗口 + 同总线真实重叠）。
    ///
    /// 默认返回空快照：mock 调度器无需关心该能力，只有真实 `Scheduler` 覆盖。
    fn schedule_timeline(&self, window_ms: u64) -> ScheduleTimeline {
        ScheduleTimeline::empty(window_ms)
    }

    /// E1：暂停某总线的采集（调试独占前让路）。
    ///
    /// 接收者用 `self: Arc<Self>`：暂停/恢复内部要调 `reload_points_incremental`，
    /// 后者需要 `Arc<Self>` 才能 spawn 采集任务（与 OtaTrait 同一写法，保持对象安全）。
    /// 默认实现返回「不支持」：mock 调度器无需关心该能力，只有真实 `Scheduler` 才覆盖。
    fn pause_bus(
        self: Arc<Self>,
        bus: String,
        all_points: Vec<PointConfig>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> {
        let _ = (bus, all_points);
        Box::pin(async { Err("当前调度器不支持暂停总线采集".to_string()) })
    }

    /// E1：恢复被暂停的总线采集
    fn resume_bus(
        self: Arc<Self>,
        bus: String,
        all_points: Vec<PointConfig>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> {
        let _ = (bus, all_points);
        Box::pin(async { Err("当前调度器不支持恢复总线采集".to_string()) })
    }

    /// E1：当前被暂停采集的总线名
    fn paused_buses(&self) -> Vec<String> {
        Vec::new()
    }

    /// E1：某总线是否处于暂停采集状态（默认 false，mock 视为未暂停）
    fn is_bus_paused(&self, bus: &str) -> bool {
        let _ = bus;
        false
    }
}

/// DataQueue trait（仅包含 web handler 实际调用的方法）
pub trait DataQueueTrait: Send + Sync {
    /// 队列长度（积压监控 / 健康检查用）
    fn len<'a>(&'a self) -> Pin<Box<dyn Future<Output = usize> + Send + 'a>>;
    /// 队列是否为空
    fn is_empty<'a>(&'a self) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        // 默认实现：len == 0
        Box::pin(async move { self.len().await == 0 })
    }
    /// 历史查询（P2-1，N-10 起为唯一历史入口）：按点位 + 时间窗口查询，
    /// 可选时间桶降采样，返回样本与分页元信息（`total`/`truncated`/`has_more`）。
    /// `bucket_secs = None`/0 表示不降采样（原始行）。
    fn history_ex<'a>(
        &'a self,
        point_id: &'a str,
        start: i64,
        end: i64,
        limit: usize,
        bucket_secs: Option<u32>,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = gateway_core::engine::error::GatewayResult<
                        gateway_core::queue::HistoryQueryResult,
                    >,
                > + Send
                + 'a,
        >,
    >;
    /// v2.0：最旧一条持久化数据的时间戳（unix 毫秒），None = 无历史数据。
    ///
    /// 用于 license 配额用量统计（实际历史保留跨度 = now - oldest_ts）。
    /// 只暴露基础类型，避免 core 的 queue 类型泄漏进 web/api。
    fn oldest_data_timestamp<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Option<i64>> + Send + 'a>> {
        Box::pin(async { None })
    }
    /// 累计因队列容量上限被物理删除的行数：(总行数, 其中未送达行数)。
    ///
    /// 淘汰是**不可恢复的数据丢失**，单机网关没有异地副本兜底。此前只写
    /// debug 日志（默认不输出），现场表现为"数据一直在丢、界面一直安静"。
    /// 这里以同步方法暴露给健康快照，供 `/api/health/report` 与前端红标消费。
    ///
    /// 默认 (0, 0)：mock / 无容量上限的后端不产生淘汰。
    fn evicted_stats(&self) -> (i64, i64) {
        (0, 0)
    }
}

/// Gateway trait（仅包含 web handler 实际调用的方法）
pub trait GatewayTrait: Send + Sync {
    /// 热重载配置（应用增量变更），返回应用说明列表；采集组重建失败时返回 `Err`
    fn reload_config<'a>(
        &'a self,
        old: &'a GatewayConfig,
        new: &'a GatewayConfig,
        expanded_points: Vec<PointConfig>,
    ) -> Pin<
        Box<
            dyn Future<Output = gateway_core::engine::error::GatewayResult<Vec<String>>>
                + Send
                + 'a,
        >,
    >;
    /// 强制重建串口总线（自愈 `ResetSerial` 动作），返回重建的串口数量；
    /// `only` 为 `Some(port_name)` 时只重建该总线（定向恢复），`None` 重建全部串口总线；
    /// 采集组重建失败时返回 `Err`
    fn reset_serial_buses<'a>(
        &'a self,
        buses: &'a [BusConfig],
        points: &'a [PointConfig],
        only: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = gateway_core::engine::error::GatewayResult<usize>> + Send + 'a>>;
    /// 北向 MQTT 客户端（用于 /health/ready、/api/status 等查询）
    fn mqtt(&self) -> Option<Arc<MqttClient>>;
    /// 1.1: 磁盘降采样状态（供 health_monitor spawn_disk_monitor 使用）
    fn disk_status(&self) -> Arc<gateway_core::engine::monitor::health::DiskStatus>;
    /// 按 `port_name` 获取调试总线句柄（P0-1 真实帧下发 / 报文回放）。
    ///
    /// 返回 `None` 表示该总线未在网关中注册（端口未打开 / 已下线），
    /// handler 据此报 `bus_offline`。返回值是契约视图（[`crate::web::debug_bus::DebugBusTrait`]），
    /// 不泄漏 core 的总线内部状态，便于测试注入假总线。
    fn debug_bus(&self, name: &str) -> Option<Arc<dyn crate::web::debug_bus::DebugBusTrait>>;
    /// C1: 协议转换命中记账快照（rule_id → 最近一次转换）。
    ///
    /// 返回 Web 契约视图而不暴露 core 的 `pipeline::convert` 模块，
    /// 使 `web/api` handler 满足 arch_deps 白名单（pipeline 为禁引模块）。
    fn convert_hits(&self) -> HashMap<String, ConvertHitView>;
    /// 虚拟点位最近求值状态快照（point_id → 状态）。
    ///
    /// 返回 Web 契约视图而不暴露 core 的 `pipeline::virtual_point` 模块，
    /// 使 `web/api` handler 满足 arch_deps 白名单（pipeline 为禁引模块）。
    fn virtual_point_states(&self) -> HashMap<String, VirtualPointStateView>;
    /// 立即重算单个虚拟点位，返回最新状态（点不存在时 `None`）。
    fn recalc_virtual_point(&self, id: &str) -> Option<VirtualPointStateView>;
}

/// C1: 协议转换命中记账的 Web 契约视图（与 core 的 `ConvertHit` 字段对齐）
#[derive(Debug, Clone)]
pub struct ConvertHitView {
    /// 转换后的展示值（linear=换算后数值；enum=映射文本；raw=原值）
    pub last_value: String,
    /// 命中时刻（UTC 毫秒）
    pub last_converted_at_ms: i64,
}

/// 虚拟点位求值状态的 Web 契约视图（与 core 的 `VirtualPointState` 字段对齐）
#[derive(Debug, Clone, Default)]
pub struct VirtualPointStateView {
    /// 最近一次成功求值的结果（按配置的 decimals 舍入）
    pub value: Option<f64>,
    /// 最近一次求值错误（无错为 `None`）
    pub error: Option<String>,
    /// 最近一次求值时刻（UTC 毫秒）
    pub updated_at_ms: i64,
}

/// OTA 状态（Web 契约层视图）
///
/// 端口实现负责把 core 的 `OtaState` 映射到本枚举，使 `web/api` 不再直接
/// 依赖 `gateway_core::platform::ota` 的内部状态枚举。`as_str()` 的输出即
/// HTTP 契约（/api/ota/status 的 state 字段），不随 core 枚举改名而漂移。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtaStateView {
    Idle,
    Uploading,
    Verifying,
    Writing,
    /// 写入完成，等待重启
    PendingReboot,
    Failed,
}

impl OtaStateView {
    /// 稳定的机器可读标签（小写），供 HTTP 响应使用。
    pub fn as_str(&self) -> &'static str {
        match self {
            OtaStateView::Idle => "idle",
            OtaStateView::Uploading => "uploading",
            OtaStateView::Verifying => "verifying",
            OtaStateView::Writing => "writing",
            OtaStateView::PendingReboot => "pendingreboot",
            OtaStateView::Failed => "failed",
        }
    }
}

impl From<gateway_core::platform::ota::OtaState> for OtaStateView {
    fn from(state: gateway_core::platform::ota::OtaState) -> Self {
        // 穷尽匹配：core 新增状态时此处编译失败，强制显式补齐 Web 契约。
        match state {
            gateway_core::platform::ota::OtaState::Idle => OtaStateView::Idle,
            gateway_core::platform::ota::OtaState::Uploading => OtaStateView::Uploading,
            gateway_core::platform::ota::OtaState::Verifying => OtaStateView::Verifying,
            gateway_core::platform::ota::OtaState::Writing => OtaStateView::Writing,
            gateway_core::platform::ota::OtaState::PendingReboot => OtaStateView::PendingReboot,
            gateway_core::platform::ota::OtaState::Failed => OtaStateView::Failed,
        }
    }
}

/// OTA 状态信息（Web 契约层视图，仅含 handler 实际使用的字段）
#[derive(Debug, Clone)]
pub struct OtaStatusView {
    pub state: OtaStateView,
    pub progress: u8,
    pub error: Option<String>,
}

/// OTA trait（仅包含 web handler 实际调用的方法）
///
/// 注意：process_firmware / process_delta 使用 self: Arc<Self> receiver，
/// 这是对象安全的（Rust receiver-variants 特性），支持 Arc<dyn OtaTrait> 调用。
pub trait OtaTrait: Send + Sync {
    /// 获取 OTA 当前状态（Web 契约视图，不暴露 core 状态枚举）
    fn status(&self) -> OtaStatusView;
    /// 处理完整固件包并执行升级
    fn process_firmware(
        self: Arc<Self>,
        data: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = GatewayResult<()>> + Send>>;
    /// 处理差分升级包
    fn process_delta(
        self: Arc<Self>,
        data: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = GatewayResult<()>> + Send>>;

    /// 回滚到上一分区（仅 PendingReboot 状态下可用）
    fn rollback(&self) -> GatewayResult<()>;
}

// ===== 为具体类型 impl trait（自动满足 bootstrap 注入） =====
// bootstrap 中 Arc::new(Scheduler::new(...)) 返回 Arc<Scheduler>，
// 赋值给 Arc<dyn SchedulerTrait> 字段时 Rust 自动 unsizing 强转。

impl SchedulerTrait for Scheduler {
    fn collectors(&self) -> HashMap<String, Arc<dyn Collector>> {
        Scheduler::collectors(self)
    }
    fn device_status(&self) -> Vec<DeviceStatus> {
        Scheduler::device_status(self)
    }
    fn schedule_timeline(&self, window_ms: u64) -> ScheduleTimeline {
        Scheduler::timeline_snapshot(self, window_ms)
    }
    fn pause_bus(
        self: Arc<Self>,
        bus: String,
        all_points: Vec<PointConfig>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> {
        Box::pin(async move { Scheduler::pause_bus(&self, &bus, all_points).await })
    }
    fn resume_bus(
        self: Arc<Self>,
        bus: String,
        all_points: Vec<PointConfig>,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> {
        Box::pin(async move { Scheduler::resume_bus(&self, &bus, all_points).await })
    }
    fn paused_buses(&self) -> Vec<String> {
        Scheduler::paused_buses(self)
    }
    fn is_bus_paused(&self, bus: &str) -> bool {
        Scheduler::is_bus_paused(self, bus)
    }
}

impl DataQueueTrait for DataQueue {
    fn len<'a>(&'a self) -> Pin<Box<dyn Future<Output = usize> + Send + 'a>> {
        Box::pin(DataQueue::len(self))
    }
    fn evicted_stats(&self) -> (i64, i64) {
        DataQueue::evicted_stats(self)
    }
    fn history_ex<'a>(
        &'a self,
        point_id: &'a str,
        start: i64,
        end: i64,
        limit: usize,
        bucket_secs: Option<u32>,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = gateway_core::engine::error::GatewayResult<
                        gateway_core::queue::HistoryQueryResult,
                    >,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(DataQueue::history_ex(
            self,
            point_id,
            start,
            end,
            limit,
            bucket_secs,
        ))
    }
    fn oldest_data_timestamp<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Option<i64>> + Send + 'a>> {
        Box::pin(DataQueue::oldest_data_timestamp(self))
    }
}

impl GatewayTrait for Gateway {
    fn reload_config<'a>(
        &'a self,
        old: &'a GatewayConfig,
        new: &'a GatewayConfig,
        expanded_points: Vec<PointConfig>,
    ) -> Pin<
        Box<
            dyn Future<Output = gateway_core::engine::error::GatewayResult<Vec<String>>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(Gateway::reload_config(self, old, new, expanded_points))
    }
    fn reset_serial_buses<'a>(
        &'a self,
        buses: &'a [BusConfig],
        points: &'a [PointConfig],
        only: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = gateway_core::engine::error::GatewayResult<usize>> + Send + 'a>>
    {
        Box::pin(Gateway::reset_serial_buses(self, buses, points, only))
    }
    fn mqtt(&self) -> Option<Arc<MqttClient>> {
        Gateway::mqtt(self)
    }
    fn disk_status(&self) -> Arc<gateway_core::engine::monitor::health::DiskStatus> {
        self.disk_status.clone()
    }
    fn debug_bus(&self, name: &str) -> Option<Arc<dyn crate::web::debug_bus::DebugBusTrait>> {
        // `Gateway::bus_manager` 返回的 Arc<BusManager> 已实现 DebugBusTrait，
        // 这里显式 unsizing 成 trait object，避免每层调用都写 as 转换。
        Gateway::bus_manager(self, name).map(|m| m as Arc<dyn crate::web::debug_bus::DebugBusTrait>)
    }
    fn convert_hits(&self) -> HashMap<String, ConvertHitView> {
        Gateway::convert_hits(self)
            .into_iter()
            .map(|(k, h)| {
                (
                    k,
                    ConvertHitView {
                        last_value: h.last_value,
                        last_converted_at_ms: h.last_converted_at_ms,
                    },
                )
            })
            .collect()
    }
    fn virtual_point_states(&self) -> HashMap<String, VirtualPointStateView> {
        Gateway::virtual_point_states(self)
            .into_iter()
            .map(|(k, s)| {
                (
                    k,
                    VirtualPointStateView {
                        value: s.value,
                        error: s.error,
                        updated_at_ms: s.updated_at_ms,
                    },
                )
            })
            .collect()
    }
    fn recalc_virtual_point(&self, id: &str) -> Option<VirtualPointStateView> {
        Gateway::recalc_virtual_point(self, id).map(|s| VirtualPointStateView {
            value: s.value,
            error: s.error,
            updated_at_ms: s.updated_at_ms,
        })
    }
}

impl OtaTrait for OtaManager {
    fn status(&self) -> OtaStatusView {
        let info = OtaManager::status(self);
        OtaStatusView {
            state: info.state.into(),
            progress: info.progress,
            error: info.error,
        }
    }

    fn process_firmware(
        self: Arc<Self>,
        data: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = GatewayResult<()>> + Send>> {
        Box::pin(async move { OtaManager::process_firmware(self, data).await })
    }

    fn process_delta(
        self: Arc<Self>,
        data: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = GatewayResult<()>> + Send>> {
        Box::pin(async move { OtaManager::process_delta(self, data).await })
    }

    fn rollback(&self) -> GatewayResult<()> {
        OtaManager::rollback(self)
    }
}
