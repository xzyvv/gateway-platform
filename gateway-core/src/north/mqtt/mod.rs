//! MQTT 客户端（§6）
//!
//! - rumqttc MQTT 5.0 客户端，QoS1（至少一次送达）
//! - 每 publish_interval 秒批量上传（JSON 数组）
//! - 断线指数退避自动重连：1s → 2s → 5s → 10s → 30s
//! - 重连期间数据继续入 SQLite 队列，恢复后按 ID 顺序补传

pub mod broker;
pub mod outbox;

pub use outbox::{OutboxStore, OutgoingMsg};

mod payload;
mod publish;
mod status;

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use rumqttc::{AsyncClient, Event, EventLoop, Incoming, LastWill, MqttOptions, QoS, Transport};
use tokio::sync::{mpsc, oneshot, Mutex as AsyncMutex, Notify};
use tokio::task::JoinHandle;

use crate::config::MqttConfig;
use crate::crypto::cert::CertPair;
use crate::engine::error::GatewayResult;
use crate::engine::model::DataPoint;
use crate::north::template::{TemplateEngine, TemplateError};
use crate::queue::DataQueue;

/// 断路器：连续失败超过此阈值则断开
const CIRCUIT_FAILURE_THRESHOLD: u32 = 5;
/// 断路器：断开后此时间内拒绝所有 publish
const CIRCUIT_COOLDOWN_SECS: u64 = 30;

/// C-6 修复：并发 publish 数量上限。
///
/// rumqttc 0.24 的 `AsyncClient::publish` 不返回 packet_id（pkid），`Incoming::PubAck`
/// 字段也不对外暴露（只导出 `Incoming` type alias 隐藏内部），所以无法用
/// `HashMap<u16, oneshot::Sender>` 做严格 pkid→sender 映射。旧实现基于
/// "rumqttc QoS1 保序 → push_back/pop_front FIFO 严格成立" 的假设，但
/// `MAX_CONCURRENT_PUBLISH = 4` 时多个 publish 并发 in-flight，**PubAck
/// 到达顺序由 broker 决定，不一定等于 push_back 顺序**——一旦乱序，错误的
/// sender 会被错误地 signal，触发：
/// 1. 数据未真正出队却误释放（重复上报），或
/// 2. 已 ack 的 sender 永远等不到自己的 ack（pending_ack 队列堆积）。
///
/// 工程上唯一不依赖 pkid 的可靠方案是 **publish 串行**：一次只允许一个
/// publish 在飞，FIFO 严格成立。代价是单次 publish 间隔内的吞吐降低到 1，
/// 但 publish 路径还有 dequeue + build_payload + maybe_compress 等 CPU 密集
/// 步骤，串行后端到端延迟可控。如未来升级到能拿 pkid 的 rumqttc 版本（或
/// 切换到 mqttv5 类型化 PubAck），可恢复并发并改用 HashMap 映射。
const MAX_CONCURRENT_PUBLISH: usize = 1;

/// 订阅消息转发通道（透传模式等需要接收下行指令时设置）。
/// 外层 `Arc<Mutex<..>>` 支持运行期挂接/替换订阅者，`None` 表示尚未挂接。
pub(crate) type IncomingTxChannel =
    Arc<parking_lot::Mutex<Option<mpsc::Sender<(String, Vec<u8>)>>>>;

/// P3: outbox 写入器命令（解耦 publish 路径与 outbox 持久化）
enum OutboxCmd {
    Persist(OutgoingMsg),
    Ack(i64),    // data_id（批次首个队列 id）
    Remove(i64), // data_id
}

/// 待确认发布槽位（P1-4 修复）。
///
/// 每个 QoS1 publish 在发出**之前**登记一个槽位，PubAck 到达时按 FIFO
/// 顺序 pop_front 找回对应 sender。`ack_id` 是本槽位的进程内唯一编号，
/// 用于超时/失败路径**按身份**移除槽位（而非盲 pop_front），避免误删
/// FIFO 中其它批次的槽位；被移除槽位若仍有迟到的 PubAck 在飞，由
/// `orphan_acks` 计数对账消化（见 `reconcile_puback`）。
pub(crate) struct PendingAckSlot {
    pub(crate) ack_id: u64,
    pub(crate) data_id: i64,
    pub(crate) tx: oneshot::Sender<()>,
}

/// 生成下一个槽位 ack_id（进程内唯一递增，从 1 开始）。
pub(crate) fn next_ack_id(gen: &std::sync::atomic::AtomicU64) -> u64 {
    gen.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        .wrapping_add(1)
}

/// 按 ack_id 定位移除槽位（P1-4 修复：替代盲 pop_front）。
///
/// 返回 `true` 表示槽位确实存在并被移除——此时 publish 已发出但 PubAck
/// 尚未到达，迟到的 PubAck 必须由 `orphan_acks` 计数对账消化，否则会
/// 偷走 FIFO 头部（后续批次的真实 ack 槽位），造成假确认 → 数据未送达
/// 却被 ack 删除。返回 `false` 表示槽位已不存在（例如 PubAck 已先于
/// 超时到达并正常 pop_front），调用方**不应**递增孤儿计数。
pub(crate) fn remove_slot_by_ack_id(
    pending_ack: &Arc<parking_lot::Mutex<VecDeque<PendingAckSlot>>>,
    ack_id: u64,
) -> bool {
    let mut q = pending_ack.lock();
    match q.iter().position(|s| s.ack_id == ack_id) {
        Some(pos) => {
            q.remove(pos);
            true
        }
        None => false,
    }
}

/// 断连 / 事件循环出错时清空 pending_ack 槽位（避免僵尸槽位永久滞留）。
///
/// 背景：rumqttc `clean_session=true` 下断线时未完成的 inflight 会被
/// 客户端丢弃，broker 端 PubAck 不再到达。`publish_bypass_queue` / `publish_raw`
/// 透传路径推入的槽位 rx 已被丢弃、且无超时回收；`publish_batch` 的
/// 槽位若也卡在 inflight 中，断线后同样永无 PubAck。这些槽位若不主动
/// 清空会：
///   1. 无界增长（数月运行内存爆）
///   2. 永久占住 FIFO 头，重连后 `reconcile_puback` pop_front 拿到的是
///      僵尸槽位的旧 sender，真实批次的槽位得不到 signal → 超时 → release
///      → 重复投递
///
/// 取向：清空 = at-least-once 重复优于死锁。断线后真实未送达数据由
/// outbox 持久化保留（`replay_pending_outbox` 在 ConnAck 成功路径触发），
/// 重连后会重投，broker 端去重由上游业务保证。
///
/// 顺带把 orphan_acks 计数一并清零，防止重连后对账基线错位。
// 编译器对 `pub(crate) fn` + 跨模块同名路径调用的 dead_code 判定会漏报
// （event_loop.rs 通过 super::* 路径引用），加 allow 避免误报。
#[allow(dead_code)]
pub(crate) fn fail_all_pending(
    pending_ack: &PendingAckQueue,
    orphan_acks: &std::sync::atomic::AtomicU64,
) -> usize {
    let mut q = pending_ack.lock();
    let dropped = q.len();
    q.clear();
    orphan_acks.store(0, std::sync::atomic::Ordering::Relaxed);
    dropped
}

/// r41-MQTT1：连接丢失（收到 `Disconnect` / 事件循环 `poll()` 出错）的统一收尾。
///
/// 完成两件事：
/// 1. `connected = false`——立即对外反映断连。旧实现只在 `Disconnect` 分支置 false，
///    网络掉线走的 `Err` 分支遗漏了这一步：长时间无 `Disconnect` 事件时
///    `is_connected()` 会在整段重连期间持续误报在线（连接器健康 / `publish_raw` /
///    `publish_bypass_queue` 的门控因此失真）。把两个分支收敛到本函数杜绝再次漂移。
/// 2. 清空 pending 槽位并清零 orphan 对账基线（见 [`fail_all_pending`]）。
///
/// 返回被清空的槽位数。
pub(crate) fn mark_disconnected(
    pending_ack: &PendingAckQueue,
    orphan_acks: &std::sync::atomic::AtomicU64,
    connected: &std::sync::atomic::AtomicBool,
) -> usize {
    connected.store(false, std::sync::atomic::Ordering::Relaxed);
    fail_all_pending(pending_ack, orphan_acks)
}

/// r46-MQTT1：ConnAck 成功（clean_session 新建会话）时的统一收尾。
///
/// 置 `connected=true`，并**重置孤儿对账基线**、防御性清空残留槽位（正常情况下
/// ConnAck 成功时 pending 已在上一次断连被清空，此处仅兜底）。
///
/// 为什么必须重置 orphan：连接失败期间（ConnAck 被拒 / broker 不可用）登记或
/// 超时移除的发布永远等不到 PubAck，其 `orphan_acks++` **没有 Disconnect 事件
/// 来清零**（故障期事件循环只反复收到 ConnAck 被拒，历史实现未收敛到
/// [`mark_disconnected`]）。这条陈旧基线若不清理，重连成功后**每个真实 PubAck
/// 都会被当作孤儿消化** → 头部槽位永不确认 → 超时重发 → 每条消息永久重复投递
/// （计费口径翻倍）。clean_session=true 下新会话不携带旧 inflight，旧连接 socket
/// 已关闭、旧 ack 不可能串到新连接，故此处清零绝对安全。
///
/// 返回被清空的残留槽位数。
pub(crate) fn mark_connected_fresh_session(
    pending_ack: &PendingAckQueue,
    orphan_acks: &std::sync::atomic::AtomicU64,
    connected: &std::sync::atomic::AtomicBool,
) -> usize {
    connected.store(true, std::sync::atomic::Ordering::Relaxed);
    fail_all_pending(pending_ack, orphan_acks)
}

/// r48-MQTT1：可存储的关停等待（对齐 `engine::shutdown::ShutdownToken::wait`
/// 的 Bug 159 修复模式）。
///
/// 纯 `Notify::notify_waiters()` 在「无 waiter 正挂起」时会**丢失通知**：
/// `stop()` / 热重载期间，事件循环常阻塞在 `eventloop.poll()`、发布循环常阻塞在
/// `publish_batch()`（PubAck 超时最长 `ack_timeout`，默认 10s）。此刻 stop() 的
/// `notify_waiters()` 既无 waiter 可唤醒、也不存储许可 → 两个后台任务漏停：
///   1. 事件循环继续 `poll()`、发布循环继续上报；
///   2. `stop()` 只能等满 5s 超时后放弃，旧客户端在 reload 后仍持有同一
///      `client`/`queue` 继续 publish/subscribe → 重复上报 + 任务泄漏。
///
/// 修复：结合 `stopping` 标志——先 `enable()` 立即注册 waiter，再复查标志，
/// 覆盖「置位与注册之间」的时间窗口（与本仓库 `platform/ntp.rs`（Bug T10）、
/// `engine/shutdown.rs`（Bug 159）完全一致的写法，避免再次漂移）。
pub(crate) async fn wait_shutdown(shutdown: &Notify, stopping: &std::sync::atomic::AtomicBool) {
    use std::sync::atomic::Ordering;
    if stopping.load(Ordering::SeqCst) {
        return;
    }
    let notified = shutdown.notified();
    tokio::pin!(notified);
    notified.as_mut().enable();
    if stopping.load(Ordering::SeqCst) {
        return;
    }
    notified.await;
}

/// PubAck 到达时的对账决策（P1-4 修复，纯函数便于测试）。
///
/// rumqttc 0.24 不暴露 packet_id，无法把 PubAck 与槽位严格映射，只能
/// 依赖 FIFO。超时/失败路径按 ack_id 移除槽位时会递增 `orphan_acks`
/// （该 publish 可能已到达 broker，PubAck 只是迟到）。迟到的 PubAck
/// 到达时若 orphan>0，说明它属于已被移除的槽位——减一并跳过 pop_front；
/// 否则正常 pop_front 并投递 sender。
///
/// 取向：宁可重复（at-least-once）不可丢数据。若孤儿计数误判，后果是
/// 某槽位多等一个周期后超时重发，而不是丢数据。
pub(crate) fn reconcile_puback(
    pending_ack: &Arc<parking_lot::Mutex<VecDeque<PendingAckSlot>>>,
    orphan_acks: &std::sync::atomic::AtomicU64,
) -> Option<PendingAckSlot> {
    if orphan_acks.load(std::sync::atomic::Ordering::Relaxed) > 0 {
        orphan_acks.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        return None;
    }
    pending_ack.lock().pop_front()
}

/// 待确认的发布：收到 PubAck 时 signal，publish_batch 据此删除队列数据。
///
/// 设计说明（C-6 / P1-4 修复）：
/// rumqttc 0.24 的 `publish()` 不返回 packet_id，`Incoming::PubAck` 字段
/// 也不对外暴露，因此无法用 `HashMap<u16, Sender>` 做严格 pkid 映射。
/// 采用 FIFO 队列方案，依赖两条不变量：
/// - **publish 强制串行**（`publish_semaphore` permit=1），broker 收到 publish
///   的顺序严格等于 `push_back` 顺序；
/// - **所有发布路径（publish_batch / publish_bypass_queue / publish_raw / replay）
///   都在 publish 之前 push_back 槽位**（P1-4：原 publish_bypass_queue / publish_raw
///   绕过登记，其 PubAck 会偷走 FIFO 头部，误 signal 其它批次槽位）。
///
/// PubAck 到达时 `reconcile_puback` 先对账孤儿计数（超时/失败路径按
/// ack_id 移除槽位时递增），正常情况 pop_front 即可严格映射。
/// 超时/失败路径用 `remove_slot_by_ack_id` 按身份移除自有槽位，
/// 绝不盲 pop_front（盲 pop 会误删 FIFO 中其它批次的槽位）。
/// `data_id` 为防御性携带：cleanup_outbox / queue.release 按 data_id
/// 定位（见 publish.rs::outbox_ack / cleanup_outbox_on_failure）。
pub(crate) type PendingAckQueue = Arc<parking_lot::Mutex<VecDeque<PendingAckSlot>>>;

/// MQTT 上传客户端
pub struct MqttClient {
    client: AsyncClient,
    queue: Arc<DataQueue>,
    config: MqttConfig,
    shutdown: Arc<Notify>,

    /// r48-MQTT1：关停标志。`stop()` 先置位再 `notify_waiters()`；两个后台任务
    /// （事件循环 / 发布循环）统一走 [`wait_shutdown`]，先 `enable()` 注册 waiter
    /// 再复查标志，消除纯 `notify_waiters()` 在无 waiter 挂起时通知丢失导致的漏停。
    stopping: Arc<std::sync::atomic::AtomicBool>,

    /// 待确认的发布：收到 PubAck 时 signal，publish_batch 据此删除队列数据。
    /// 类型与设计不变量见 `PendingAckSlot` / `PendingAckQueue` 的文档注释。
    pending_ack: PendingAckQueue,

    /// P1-4：孤儿 PubAck 计数。超时/失败路径按 ack_id 移除槽位时递增
    /// （publish 可能已发出、PubAck 迟到）；事件循环 `reconcile_puback`
    /// 用它对账迟到的 PubAck，防止其偷走 FIFO 头部槽位。
    orphan_acks: Arc<std::sync::atomic::AtomicU64>,

    /// P1-4：槽位 ack_id 生成器（进程内唯一递增）。
    ack_id_gen: Arc<std::sync::atomic::AtomicU64>,

    /// MQTT 连接状态（供健康检查使用）
    connected: Arc<std::sync::atomic::AtomicBool>,

    /// 事件循环 task 句柄（用于 stop 时等待退出）
    eventloop_handle: parking_lot::Mutex<Option<JoinHandle<()>>>,

    /// 发布循环 task 句柄
    publish_handle: parking_lot::Mutex<Option<JoinHandle<()>>>,

    /// 断点续传令牌桶：当前可用令牌数
    rate_tokens: std::sync::atomic::AtomicU64,

    /// 上次补充令牌的时间戳（ms）
    rate_last_refill: std::sync::atomic::AtomicU64,

    /// QoS1 持久化 outbox（可选，启用后未确认消息持久化到 SQLite）
    outbox: Option<Arc<AsyncMutex<OutboxStore>>>,

    /// 断路器：连续失败计数
    circuit_failures: std::sync::atomic::AtomicU32,
    /// 断路器：是否处于断开状态（拒绝 publish）
    circuit_open: std::sync::atomic::AtomicBool,
    /// 断路器：上次断开时间戳（ms），用于冷却后自动重置
    circuit_opened_at: std::sync::atomic::AtomicU64,

    /// publish 串行信号量（permit=1，见 MAX_CONCURRENT_PUBLISH）。
    /// Arc 包装以便事件循环 task 共享（replay 路径同样遵守串行）。
    publish_semaphore: Arc<tokio::sync::Semaphore>,
    /// P3: outbox 异步写入器命令通道（解耦 publish 路径与锁竞争）
    outbox_cmd_tx: arc_swap::ArcSwapOption<mpsc::Sender<OutboxCmd>>,
    /// P3: outbox 写入器 task 句柄
    outbox_writer_handle: parking_lot::Mutex<Option<JoinHandle<()>>>,

    /// 订阅消息转发通道（透传模式等需要接收下行指令时设置）
    incoming_tx: IncomingTxChannel,

    /// f5: 已建立的订阅 topic 列表。`clean_session=true` 会在每次重连后清空 broker
    /// 侧的订阅关系（且 rumqttc 不会自动重订阅），事件循环在 ConnAck 成功后据此
    /// 重新订阅，否则透传等下行通道断线一次后永久静默失效。
    desired_subscriptions: Arc<parking_lot::Mutex<Vec<String>>>,

    /// 自定义上行 payload 模板（编译后缓存）。
    /// - None 表示未启用自定义模板（走默认 JSON 分组格式）
    /// - Some(Err) 表示模板编译失败（启动时记 warn，运行时降级为默认）
    /// - Some(Ok) 表示已编译成功
    payload_template_engine: once_cell::sync::OnceCell<Result<TemplateEngine, TemplateError>>,

    /// 点位配置表（point_id → PointConfig），用于模板渲染时查找 point_name/unit/device_name 等元数据。
    /// 启动时为空（仅 QueuedItem 基础字段可用），运行时可通过 `set_point_configs` 热更新。
    point_configs:
        Arc<parking_lot::RwLock<std::collections::HashMap<String, crate::config::PointConfig>>>,

    /// 1.2: 注入的指标实例（替代全局 metrics::get()）
    metrics: Arc<crate::engine::metrics::Metrics>,

    /// R3: 模板未解析变量的「已告警」标志。首次渲染发现未解析变量时告警一次，
    /// 之后静默——避免每个发布周期重复刷日志。
    unresolved_template_warned: std::sync::atomic::AtomicBool,

    /// 协议转换引擎（批次 B：enum 标签回查）。批次 C 起 QueuedItem 已带
    /// value_text 字段（入队时落库的标签），但 MQTT 报文口径不变——仍在
    /// payload 构建期对每条 item 调 `ConvertEngine::enum_label_for` 补回枚举文本。
    /// 存 Arc 共享引擎本体：规则热重载（update_rules 原地 swap）后读到当前规则，
    /// 不是配置快照。未注入（None）时行为与旧版一致——不报 value_text。
    convert_engine: arc_swap::ArcSwapOption<crate::pipeline::ConvertEngine>,
}

mod alarm_publisher;
mod connection;
mod event_loop;
mod state;

#[cfg(test)]
mod tests;

/// `replay_pending_outbox` 的共享句柄集合。
/// 用结构体承载以避免超长参数列表（clippy::too_many_arguments）。
struct ReplayHandles<'a> {
    outbox: &'a Arc<AsyncMutex<OutboxStore>>,
    client: &'a AsyncClient,
    pending_ack: &'a PendingAckQueue,
    orphan_acks: &'a std::sync::atomic::AtomicU64,
    ack_id_gen: &'a std::sync::atomic::AtomicU64,
    publish_semaphore: &'a tokio::sync::Semaphore,
    queue: &'a Arc<DataQueue>,
    replay_in_progress: &'a std::sync::atomic::AtomicBool,
}

/// 重放 outbox 中未确认的消息（自由函数，供事件循环 ConnAck 和 replay_outbox 方法共用）
///
/// P1-4 修复：replay 与常规 publish 路径遵循同一套不变量——
/// 1. 每批先 `publish_semaphore`（与 publish_batch 串行共享）；
/// 2. **先登记槽位再 publish**（原实现顺序颠倒：先 publish 后
///    push_back，先到的 PubAck 会被 FIFO 中上一批的槽位吃掉）；
/// 3. 超时/通道关闭按 ack_id 移除槽位并 orphan++（消息可能已发出，
///    迟到的 PubAck 由事件循环 reconcile_puback 对账），publish 调用
///    本身失败则只移除槽位、不 orphan++（消息未入队不会有 PubAck）。
/// 4. P1-5：成功送达后调用 `replay_ack_converged` 同步确认队列行——
///    否则 publish_batch 通道会把同一批数据再次投递（系统性重复上报）。
async fn replay_pending_outbox(handles: ReplayHandles<'_>, ack_timeout_secs: Option<u64>) {
    // 解构回原局部名，函数体保持不变
    let ReplayHandles {
        outbox,
        client,
        pending_ack,
        orphan_acks,
        ack_id_gen,
        publish_semaphore,
        queue,
        replay_in_progress,
    } = handles;
    // 互斥靠 replay_in_progress AtomicBool（compare_exchange 单飞）+
    // publish_semaphore（与 publish_batch 串行共享，见 PendingAckQueue
    // 设计注释；旧注释"无需 publish_lock"是错的——不持信号量时 replay
    // 与 publish_batch 并发 publish 会破坏 broker 端保序前提）。

    // 单飞检查，防止 replay 与 publish_batch 并发
    if replay_in_progress
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        )
        .is_err()
    {
        tracing::debug!("outbox replay already in progress, skipping");
        return;
    }
    // 确保 flag 在函数退出时重置
    struct ReplayGuard<'a>(&'a std::sync::atomic::AtomicBool);
    impl Drop for ReplayGuard<'_> {
        fn drop(&mut self) {
            self.0.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }
    let _guard = ReplayGuard(replay_in_progress);

    // P2: 分页 replay，避免积压时一次性把整表 pending 载入内存。
    // 每页按 outbox id 升序取 REPLAY_BATCH_SIZE 条，处理完再取下一页
    // （游标 after_id 推进到本页最大 id）。
    const REPLAY_BATCH_SIZE: i64 = 50;
    let ack_timeout = Duration::from_secs(ack_timeout_secs.unwrap_or(10));
    // r67-OBX1：游标从持久化层加载（outbox_replay_cursor 表）而非每次从 0 开始。
    // 旧实现 `after_id = 0` 导致每次 replay 调用都全表重扫，且 in-flight 行
    // （publish 已发出但 PubAck 未到）会被下一轮 re-fetch 后再次 publish 到
    // broker——broker 侧重复。持久化游标使下次调用从「已安全推进到的下界」
    // 继续：safe 行被 status=0 过滤自然跳过、in-flight 行由游标直接越过。
    let mut after_id: i64 = outbox_blocking(outbox.clone(), move |s| s.load_replay_cursor("mqtt"))
        .await
        .unwrap_or(0);
    // 外层分页循环；内层遇到「publish 在飞 / 失败 / 超时」需中止整个 replay 时，
    // 用 labeled break 直接跳出分页循环（等价于旧实现 break 出 for 后返回）。
    'pages: loop {
        // A3: replay_pending_batch 放到 spawn_blocking，避免阻塞 tokio runtime
        let page = match outbox_blocking(outbox.clone(), move |s| {
            s.replay_pending_batch("mqtt", after_id, REPLAY_BATCH_SIZE)
        })
        .await
        {
            Some(Ok(v)) => v,
            Some(Err(e)) => {
                tracing::warn!("outbox replay_pending failed: {}", e);
                return;
            }
            None => return,
        };
        if page.is_empty() {
            break;
        }
        tracing::debug!(
            "outbox replaying page: {} batch(es) after_id={}",
            page.len(),
            after_id
        );
        // r67-OBX1：try_acquire fail 时的回退锚点。try_acquire 失败时 publish 未被
        // 调用、broker 也没有该消息——下次 replay 必须能再次拉到这行。记下本页
        // 进入时的 after_id 作为「回退目标」，失败时把 after_id 拨回此值并写盘。
        //
        // r75-OBX1：扩展为「整页所有失败路径都回退到 page_anchor」。
        // 旧实现只 try_acquire fail 回退，publish/ack 失败路径把 after_id 留
        // 在 break 行的 oid——下次 replay `id > oid` 跳过该行永久丢失。
        let page_anchor = after_id;
        for (oid, msg) in page {
            // r75-OBX1：游标推进改为**乐观**——先假设本行能成功送达，
            // **失败**则回退到 page_anchor。成功送达（publish + ack 成功）
            // 才把 after_id 推进到本行 oid。
            //
            // 关键不变式：本页中**任何一行**失败，after_id 都不超过 page_anchor，
            // 下次 replay 仍能从 `id > page_anchor` 拉回本页全部行重试。
            // 与 try_acquire fail 同等待遇，是已验证的 r67 语义扩展。
            if oid > after_id {
                after_id = oid;
            }
            // P1-4：先持 semaphore 与常规路径串行，保证 broker 端 publish
            // 顺序与槽位 push_back 顺序一致。try_acquire 失败说明
            // publish_batch 正在等 ack（replay_in_progress 单飞窗口的竞态），
            // 放弃本次 replay，等下个 ConnAck 再试。
            let _permit = match publish_semaphore.try_acquire() {
                Ok(p) => p,
                Err(_) => {
                    // r67-OBX1：try_acquire fail 时 publish 未发出，回退 after_id
                    // 到本页锚点（page_anchor），下次重扫仍能从 `id > page_anchor`
                    // 找到当前行。`after_id` 拨回后写入游标表，让持久化层与
                    // 内存一致——否则下次启动仍会从 `after_id=oid` 开始，跳过此行。
                    after_id = page_anchor;
                    tracing::debug!("outbox replay skipped: publish in flight");
                    break 'pages;
                }
            };

            // P1-4：先登记槽位再 publish（顺序不能颠倒，见函数文档）。
            let ack_id = next_ack_id(ack_id_gen);
            let (tx, rx) = oneshot::channel();
            pending_ack.lock().push_back(PendingAckSlot {
                ack_id,
                data_id: msg.data_id,
                tx,
            });

            // r75-OBX1：失败回退 flag。publish 失败、ack 超时、ack channel 关闭
            // 任意一种触发，都把 after_id 拨回 page_anchor。回退在 match 块内
            // 显式写，避免依赖控制流细微差别。
            let mut failed = false;

            match client
                .publish(&msg.topic, QoS::AtLeastOnce, false, msg.payload.clone())
                .await
            {
                Ok(_) => match tokio::time::timeout(ack_timeout, rx).await {
                    Ok(Ok(())) => {
                        // P1-5：replay 与 publish_batch 是两条独立重发通道。
                        // 送达成功后必须同步 ack 队列中同一批数据（data_ids），
                        // 否则队列（崩溃恢复后 in_flight 复位 / 仍 pending）会把
                        // 已送达的数据交给 publish_batch 再次投递。
                        replay_ack_converged(outbox, queue, oid, msg.data_ids.clone()).await;
                        tracing::info!("outbox replay batch {} acked", oid);
                    }
                    Ok(Err(_)) => {
                        // sender 被移除但未 signal：按 ack_id 定位移除自有槽位
                        // （非盲 pop_front）；消息可能已发出，orphan++ 对账。
                        if remove_slot_by_ack_id(pending_ack, ack_id) {
                            orphan_acks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                        // r75-OBX1：ack channel 关闭——broker 可能已收到但 PubAck
                        // 丢失。行 status=0 必须能下次被拉到，回退 page_anchor。
                        tracing::warn!(
                            "outbox replay ack channel closed (batch {}), will retry",
                            oid
                        );
                        failed = true;
                    }
                    Err(_) => {
                        // P1-4：超时按 ack_id 移除自有槽位并 orphan++（publish
                        // 可能已到达 broker，PubAck 迟到，由事件循环
                        // reconcile_puback 对账消化，不偷 FIFO 头部）。
                        if remove_slot_by_ack_id(pending_ack, ack_id) {
                            orphan_acks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                        // r75-OBX1：ack 超时——同样回退 page_anchor。
                        tracing::warn!("outbox replay batch {} timeout, will retry", oid);
                        failed = true;
                    }
                },
                Err(e) => {
                    // publish 调用本身失败：消息未进入 rumqttc 发送队列，
                    // 不会有 PubAck 到达——只移除槽位，不递增孤儿计数。
                    remove_slot_by_ack_id(pending_ack, ack_id);
                    // r75-OBX1：publish 失败——消息未发出，回退 page_anchor。
                    tracing::warn!("outbox replay publish failed: {}", e);
                    failed = true;
                }
            }

            if failed {
                after_id = page_anchor;
                break 'pages;
            }
        }
    }
    // r67-OBX1：函数退出前把 after_id 落盘。
    //
    // 该值在不同退出路径下的语义：
    // - try_acquire fail（已在循环内回退到 page_anchor）：等于本页入口的 after_id，
    //   保证当前未 publish 的行下次被重新拉到；
    // - r75-OBX1：publish/ack 失败路径也回退到 page_anchor——保证未送达的行
    //   下次仍能被 `id > page_anchor` 拉到并重试，不会被 `id > oid` 跳过而
    //   永久丢失（broker 端是否真收到存在不确定性，绝不能假设已送达）；
    // - 自然完成（page 空 / 所有页扫完）：等于最后处理的 oid，下一次 replay
    //   跳过该 id 之前的全部 pending——它们要么已 mark_acked（status=1 过滤
    //   掉），要么仍在 pending 中（重发本就是要做的）。
    //
    // SQL 错误路径（replay_pending_batch 失败）的 early return 在循环顶部即
    // 返回，after_id 仍为加载值——save_replay_cursor 是幂等的，重复写同值
    // 安全无副作用。
    let persist_after_id = after_id;
    let _ = outbox_blocking(outbox.clone(), move |s| {
        s.save_replay_cursor("mqtt", persist_after_id);
    })
    .await;
}

/// P1-5: replay 成功送达后的双库汇合：队列行与 outbox 记录一起确认。
///
/// replay 与 publish_batch 是两条独立重发通道，各自持有同一批数据的一份
/// 副本（outbox 记录 / 队列行）。成功路径若只收尾自己那份，另一份会被
/// 对应通道再次投递——典型场景：崩溃恢复后队列 in_flight 复位，
/// publish_batch 会把 replay 已送达的数据原样重发（系统性重复上报）。
/// 失败路径已由 #9 统一收敛到队列重发，这里补齐成功路径的收敛。
///
/// 顺序：先 ack 队列（关闭系统性重复来源），再 mark_acked outbox。
/// 两库非同一事务，崩溃恰好落在两者之间时最多产生一次 at-least-once
/// 固有的重复投递，不会丢数据。
pub(crate) async fn replay_ack_converged(
    outbox: &Arc<AsyncMutex<OutboxStore>>,
    queue: &Arc<DataQueue>,
    oid: i64,
    data_ids: Vec<i64>,
) {
    if let Err(e) = queue.ack(data_ids).await {
        tracing::warn!(outbox_id = oid, "replay: queue ack failed: {}", e);
    }
    let _ = outbox_blocking(outbox.clone(), move |s| s.mark_acked(oid)).await;
}

/// A3: 把 outbox 同步操作包装到 spawn_blocking，避免 SQLite 调用阻塞 tokio runtime。
///
/// AsyncMutex::blocking_lock() 在 spawn_blocking 闭包（独立 std 线程）中是安全的：
/// 它会同步获取锁（不在 tokio runtime 上下文），SQLite 同步操作不会卡住 tokio worker。
/// 返回 None 表示 spawn_blocking join 失败（极少见，例如 runtime 关闭期间）。
async fn outbox_blocking<F, R>(outbox: Arc<AsyncMutex<OutboxStore>>, f: F) -> Option<R>
where
    F: FnOnce(&mut OutboxStore) -> R + Send + 'static,
    R: Send + 'static,
{
    match tokio::task::spawn_blocking(move || {
        let mut guard = outbox.blocking_lock();
        f(&mut guard)
    })
    .await
    {
        Ok(r) => Some(r),
        Err(e) => {
            tracing::error!("outbox spawn_blocking join failed: {}", e);
            None
        }
    }
}

/// P3: outbox 异步写入器循环
///
/// 解耦 publish 路径与 outbox 持久化：publish_batch 通过 channel 投递消息，
/// 本循环批量持久化（每 100ms 或满 50 条），避免锁竞争阻塞 MQTT publish。
///
/// 预确认追踪：channel FIFO 保证 Persist 先于 Ack/Remove 入队，但 Persist 可能
/// 被批量延迟。若 Ack 在 persist flush 之前到达，state map 记录 pre-ack 标志，
/// flush 后立即 mark_acked，避免 stale pending 记录导致重连后重复上报。
async fn outbox_writer_loop(
    outbox: Arc<AsyncMutex<OutboxStore>>,
    mut rx: mpsc::Receiver<OutboxCmd>,
) {
    use std::collections::HashMap;
    // data_id -> (outbox_id, pre_acked, pre_removed)
    // 注：outbox_id == Some(0) 表示 persist 命中 INSERT OR IGNORE（DB 已有同 data_id 的
    // pending 行，本轮未新建行），后续 Ack/Remove 必须改按 data_id 定位该既有记录。
    let mut state: HashMap<i64, (Option<i64>, bool, bool)> = HashMap::new();
    let mut batch: Vec<OutgoingMsg> = Vec::new();
    let flush_interval = Duration::from_millis(100);
    let max_batch = 50usize;

    // r38-OBX1 修复：定时器提到循环外并 pin 住，每次 flush 后 reset。
    //
    // 旧实现在 select! 分支里每次迭代都 `tokio::time::sleep(flush_interval)` 新建定时器：
    // 只要命令持续到达（间隔 < flush_interval），completing 的 recv 分支就会让本次
    // sleep 被丢弃重建，定时器永远无法期满 —— batch 只能靠攒满 max_batch 才落盘。
    // 稳态流量下 outbox 持久化因此被无限推迟（重放窗口拉长、重复上报风险增大）。
    // 固定定时器才是"每 100ms 或满 50 条"的真实语义。
    // r56-MQTT1 修复：去掉多余的 `mut`。
    // 旧实现 `let mut flush_timer = ...; tokio::pin!(flush_timer);`——`mut` 是
    // 死代码：所有使用点（lines 596/602/655/653）都走 `flush_timer.as_mut()` 或
    // `&mut flush_timer`（select! 分支），由 `Pin<&mut Sleep>` 提供内部可变性，
    // 不需要本地绑定为 mut。这是个长期遗留的编译 warning（每轮 build 都刷一次），
    // 本轮顺手清掉。
    let flush_timer = tokio::time::sleep(flush_interval);
    tokio::pin!(flush_timer);

    loop {
        tokio::select! {
            cmd = rx.recv() => {
                match cmd {
                    Some(OutboxCmd::Persist(msg)) => {
                        let was_empty = batch.is_empty();
                        state.entry(msg.data_id).or_insert((None, false, false));
                        batch.push(msg);
                        // 批次从空转非空：从"首条待发消息"起算 100ms（避免定时器长时间空转后立即触发）
                        if was_empty {
                            flush_timer
                                .as_mut()
                                .reset(tokio::time::Instant::now() + flush_interval);
                        }
                        if batch.len() >= max_batch {
                            flush_outbox_batch(&outbox, &mut batch, &mut state).await;
                            flush_timer
                                .as_mut()
                                .reset(tokio::time::Instant::now() + flush_interval);
                        }
                    }
                    Some(OutboxCmd::Ack(data_id)) => {
                        match state.get_mut(&data_id) {
                            Some((Some(oid), _, _)) => {
                                let oid = *oid;
                                // A3: mark_acked 放到 spawn_blocking
                                // r38-OBX2：oid==0 表示 persist 命中 INSERT OR IGNORE
                                // （该 data_id 在 DB 已有 pending 行，本轮未新建行），
                                // 只能按 data_id 定位既有记录 —— mark_acked(0) 是空操作，
                                // 会留下陈旧 pending → 重启 replay 重复上报。
                                if oid == 0 {
                                    // r78-OBX1：传 protocol="mqtt"，避免误删 http 协议行
                                    let _ = outbox_blocking(outbox.clone(), move |s| s.mark_acked_by_data_id(data_id, "mqtt")).await;
                                } else {
                                    let _ = outbox_blocking(outbox.clone(), move |s| s.mark_acked(oid)).await;
                                }
                                state.remove(&data_id);
                            }
                            Some(entry) => { entry.1 = true; }
                            None => {
                                // A3: mark_acked_by_data_id 放到 spawn_blocking
                                // r78-OBX1：传 protocol="mqtt"
                                let _ = outbox_blocking(outbox.clone(), move |s| s.mark_acked_by_data_id(data_id, "mqtt")).await;
                            }
                        }
                    }
                    Some(OutboxCmd::Remove(data_id)) => {
                        match state.get_mut(&data_id) {
                            Some((Some(oid), _, _)) => {
                                let oid = *oid;
                                // A3: remove 放到 spawn_blocking
                                // r38-OBX2：同上，oid==0 时按 data_id 删除既有 pending 行
                                if oid == 0 {
                                    // r78-OBX1：传 protocol="mqtt"
                                    let _ = outbox_blocking(outbox.clone(), move |s| s.remove_by_data_id(data_id, "mqtt")).await;
                                } else {
                                    let _ = outbox_blocking(outbox.clone(), move |s| s.remove(oid)).await;
                                }
                                state.remove(&data_id);
                            }
                            Some(entry) => { entry.2 = true; }
                            None => {
                                // A3: remove_by_data_id 放到 spawn_blocking
                                // r78-OBX1：传 protocol="mqtt"
                                let _ = outbox_blocking(outbox.clone(), move |s| s.remove_by_data_id(data_id, "mqtt")).await;
                            }
                        }
                    }
                    None => break,
                }
            }
            _ = &mut flush_timer, if !batch.is_empty() => {
                flush_outbox_batch(&outbox, &mut batch, &mut state).await;
                flush_timer
                    .as_mut()
                    .reset(tokio::time::Instant::now() + flush_interval);
            }
        }
    }
    // shutdown: final flush
    flush_outbox_batch(&outbox, &mut batch, &mut state).await;
    tracing::info!("outbox writer stopped");
}

/// P3: 批量持久化 outbox 消息，应用预确认/预删除标志
///
/// A3: 整批 flush 改到 spawn_blocking 内执行，避免大量 SQLite INSERT/UPDATE/DELETE
/// 阻塞 tokio runtime。原实现持锁串行调用 store.persist / mark_acked / remove 多次，
/// 每次都是同步 SQLite 操作，积压时会卡住 event loop 线程。
async fn flush_outbox_batch(
    outbox: &Arc<AsyncMutex<OutboxStore>>,
    batch: &mut Vec<OutgoingMsg>,
    state: &mut std::collections::HashMap<i64, (Option<i64>, bool, bool)>,
) {
    if batch.is_empty() {
        return;
    }
    let outbox_clone = outbox.clone();
    let batch_taken = std::mem::take(batch);
    let state_taken = state.clone();
    let join_result = tokio::task::spawn_blocking(move || {
        let mut store = outbox_clone.blocking_lock();
        let mut state = state_taken;
        for msg in batch_taken {
            let data_id = msg.data_id;
            match store.persist(&msg) {
                Ok(oid) => {
                    let (pre_acked, pre_removed) = {
                        let entry = state.entry(data_id).or_insert((None, false, false));
                        entry.0 = Some(oid);
                        (entry.1, entry.2)
                    };
                    // r38-OBX2：oid==0 表示 INSERT OR IGNORE 命中（DB 已有同 data_id 的
                    // pending 行，本轮未新建行）。此时按 rowid 操作是空操作（不存在 id=0 的行），
                    // 必须改按 data_id 定位那条既有记录 —— 否则陈旧 pending 残留，
                    // 重启 replay 时重复上报。
                    if pre_acked {
                        // r78-OBX1：传 protocol="mqtt"
                        let _ = if oid == 0 {
                            store.mark_acked_by_data_id(data_id, "mqtt")
                        } else {
                            store.mark_acked(oid)
                        };
                    }
                    if pre_removed {
                        // r78-OBX1：传 protocol="mqtt"
                        let _ = if oid == 0 {
                            store.remove_by_data_id(data_id, "mqtt")
                        } else {
                            store.remove(oid)
                        };
                    }
                    if pre_acked || pre_removed {
                        state.remove(&data_id);
                    }
                }
                Err(e) => {
                    tracing::warn!("outbox batch persist failed (data_id={}): {}", data_id, e)
                }
            }
        }
        state
    })
    .await;
    match join_result {
        Ok(new_state) => *state = new_state,
        Err(e) => tracing::error!("flush_outbox_batch spawn_blocking join failed: {}", e),
    }
}

use broker::{parse_broker, BrokerScheme};

// ============================================================================
// 1.2: NorthConnector 统一北向 SPI 实现（Phase A: additive only）
// ============================================================================
// 旧 NorthSink / NorthboundSink / PassthroughNode 保持不变，Phase B 再迁移。
// 本 impl 包装现有 MqttClient API，不重写内部逻辑。

#[async_trait::async_trait]
impl crate::north::NorthConnector for MqttClient {
    fn name(&self) -> &str {
        "mqtt"
    }

    async fn stop(&self) {
        // 委托给现有 inherent stop() 方法（inherent 优先于 trait 方法）
        self.stop().await;
    }

    /// N-1 修复（双写双发）：本方法**不再入队**。
    ///
    /// 数据入队由 `ProcessingPipeline` 统一负责（`pipeline/mod.rs` 的 4 处
    /// `queue.enqueue`，与 MqttClient 共用同一个 `Arc<DataQueue>`）；
    /// `publish_loop`（`north/mqtt/publish.rs`）统一出队并发布到 broker。
    ///
    /// 旧实现在此二次入队，会导致同一批点在 `data_queue` 中出现 **2 行**，
    /// broker 收到 **2 条** PUBLISH（系统性重复上报）。且 `data_queue` 无唯一
    /// 约束（`queue/storage/sqlite/db_init.rs`），无法在存储层去重。
    ///
    /// 透传模式（`publish_bypass_queue`）走的是跳过队列的直发通道，不经过本方法。
    async fn publish_batch(&self, _batch: &[DataPoint]) -> GatewayResult<()> {
        // 幂等空实现：数据已在 pipeline 层入队，此处再入队即重复投递。
        Ok(())
    }

    fn health(&self) -> crate::north::ConnectorHealth {
        let last_error = if self.is_circuit_open() {
            Some("circuit breaker open".to_string())
        } else {
            None
        };
        crate::north::ConnectorHealth {
            connected: self.is_connected(),
            last_error,
        }
    }

    fn is_enabled(&self) -> bool {
        self.config.enabled
    }
}
