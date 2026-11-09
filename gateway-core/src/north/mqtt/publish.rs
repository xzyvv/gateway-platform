//! MQTT 发布逻辑
//!
//! 批量发布、压缩、令牌桶限速和 JSON 负载构建。
//!
//! Q10/Q11: build_payload 改为按 device_id 分组嵌套输出，
//! 单条 publish 仍发到 self.config.topic，payload 为设备报文数组。
//! 后续可拆分为多 topic（gateway/{gw_id}/device/{device_id}）。

use std::io::Write;
use std::time::Duration;

use flate2::write::GzEncoder;
use flate2::Compression;
use rumqttc::QoS;
use tokio::sync::oneshot;

use crate::engine::model::DataPoint;
use crate::queue::QueuedItem;

use super::{
    next_ack_id, remove_slot_by_ack_id, MqttClient, OutboxCmd, OutgoingMsg, PendingAckSlot,
    CIRCUIT_COOLDOWN_SECS,
};

/// 自适应单批条数下限。
///
/// 下限保持 100：批次过小会让 MQTT 往返次数线性上升（每条一次 PubAck 等待），
/// 串行 publish 路径下吞吐反而下降。
const MIN_BATCH_SIZE: usize = 100;

/// 自适应单批条数上限。
///
/// 上限 500：单批 payload 过大时 broker 侧内存与单条消息处理耗时会显著上升，
/// 且断链重发时单条超大会拖慢恢复速度。
const MAX_BATCH_SIZE: usize = 500;

/// 令牌桶突发容量系数（cap = rate * 该值）。
///
/// 允许在补发开始时短时突发，但长期平均速率仍受 rate（条/秒）约束。
const TOKEN_BUCKET_BURST_FACTOR: u64 = 2;

/// 单次限速等待的上限（毫秒）。
///
/// 令牌不足时按"还需多少令牌 / rate"算出等待时长，但单次最多等 1s：
/// 期间可能有关停请求，拆成多次短等待可让 publish_loop 及时响应停机。
const MAX_THROTTLE_WAIT_MS: u64 = 1_000;

/// 单批发布的结局。
///
/// publish_loop 依据它决定「周期内是否继续排空积压」（P0-1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BatchOutcome {
    /// 成功发布一批（携带实际发送条数）
    Sent(usize),
    /// 队列已空，无待发数据
    Empty,
    /// 被令牌桶限速：额度不足，需在 `wait_ms` 毫秒后重试
    Throttled { wait_ms: u64 },
    /// 串行发布信号量被占用（其它发布路径正在发布）：本轮应停止，等待下个周期
    Busy,
    /// 发布失败（断路器打开 / 网络错误 / PubAck 超时）：本轮应停止，等待下个周期
    Failed,
}

impl MqttClient {
    /// 批量发布
    ///
    /// tracing: MQTT 批量发布 span，tracing feature off 时 cfg_attr 剥离为零成本。
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), level = "debug"))]
    pub(crate) async fn publish_batch(&self, cutoff: i64) -> BatchOutcome {
        let _permit = match self.publish_semaphore.try_acquire() {
            Ok(p) => p,
            Err(_) => {
                // 其它发布路径（如 outbox replay）正持有 publish 串行锁。
                // 不与之争抢：本轮排空结束，下个周期再试。
                return BatchOutcome::Busy;
            }
        };
        if !self.config.enabled {
            return BatchOutcome::Failed;
        }

        // 断路器检查：若处于断开状态，检查是否已过冷却期
        if self.circuit_open.load(std::sync::atomic::Ordering::Relaxed) {
            let opened_at = self
                .circuit_opened_at
                .load(std::sync::atomic::Ordering::Relaxed);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if circuit_cooldown_active(opened_at, now, CIRCUIT_COOLDOWN_SECS) {
                tracing::debug!(
                    "circuit open, skipping publish (cooldown {}s/{})",
                    now.saturating_sub(opened_at),
                    CIRCUIT_COOLDOWN_SECS
                );
                return BatchOutcome::Failed;
            }
            // 冷却期过，半开：允许一次尝试
            tracing::info!("circuit half-open, attempting publish");
            self.circuit_open
                .store(false, std::sync::atomic::Ordering::Relaxed);
        }

        // C-6 修复：publish_semaphore permit=1（见 mod.rs）强制 publish 串行，
        // QoS1 + broker 保序保证 PubAck 到达顺序严格等于 push_back 顺序。
        // P1-4：所有失败路径按 ack_id 移除自有槽位（非盲 pop_front），
        // 迟到的 PubAck 由 orphan_acks 计数对账（见 mod.rs::reconcile_puback）。
        //
        // r53-MQTT1：删除原 `replay_in_progress` AtomicBool 检查。
        // 原因：原实现 `replay_pending_outbox` 在**整个分页循环**期间
        // （包括 SQL 查询、inter-iteration 间隔）持有 flag=true，
        // `publish_batch` 在此期间被强制 return false——但 `publish_semaphore`
        // 此刻**未被持有**（replay 的 permit 只在 inner loop 的 `let _permit =` 块
        // 作用域内）。后果：
        //   1) 大型 outbox replay 期间，队列里新到的采集数据**完全无法 publish**，
        //      全部积压到 replay 全部页处理完（可能数分钟）——延迟违反端到端 SLA。
        //   2) replay 与 publish_batch 实际已被 `publish_semaphore`（permit=1）
        //      正确序列化，多余的 AtomicBool 检查只阻止了不冲突的 publish 尝试。
        // 修复：直接依赖 `publish_semaphore` 的 try_acquire 作为互斥，replay
        // 的 function-level flag 保留为「replay-vs-replay」单飞（防两路并发 replay
        // 重复读 outbox 同一页），不再用于 replay-vs-publish_batch 互斥。
        // 收益：replay 期间 publish_batch 仍可消费队列新数据，at-least-once
        // 不变（replay 失败 / 中断的 outbox 行仍在 DB 等下轮 replay），FIFO 严格
        // 保持（perm=1 + broker 保序不变）。

        // P0-2：单批条数按待发量自适应。
        // `pending` 是队列中尚未 ack 的条数（含 in_flight），clamp 到
        // [MIN_BATCH_SIZE, MAX_BATCH_SIZE]：积压大时用大包摊薄 MQTT 往返
        // 开销，积压小时退回小包避免无谓的大 payload。
        let pending = self.queue.len().await;
        if pending == 0 {
            return BatchOutcome::Empty;
        }
        let batch_target = pending.clamp(MIN_BATCH_SIZE, MAX_BATCH_SIZE);

        // P0-3：`resume_rate_limit` 的语义是"条/秒"速率上限（None = 不限速）。
        // 令牌桶容量 = rate * 突发系数，允许短时突发，长期平均速率受 rate 约束。
        // 令牌按**实际发送条数**扣除（成功路径），失败路径不扣——数据已 release
        // 回队列待重发，重发时再计费。
        let rate_limit = match self.config.resume_rate_limit {
            Some(r) if r > 0 => Some(r as u64),
            _ => None,
        };
        let dequeue_limit = match rate_limit {
            Some(rate) => {
                let available = self.rate_available(rate);
                if available == 0 {
                    // 令牌不足：本轮不发，等补充后由 publish_loop 重试。
                    let wait_ms = throttle_wait_ms(batch_target, available, rate);
                    return BatchOutcome::Throttled { wait_ms };
                }
                batch_target.min(available as usize)
            }
            None => batch_target,
        };

        // snapshot 模式：每个上报周期内每个点位只取最新一行，更早样本已由
        // 周期开头的折叠标记为已送达；`cutoff` 进一步排除本周期排空期间新到的样本。
        // 与 stream 的差别只在出队语句，报文结构与 ack/重发语义完全一致。
        let dequeued = if self.config.is_snapshot() {
            self.queue.dequeue_snapshot(dequeue_limit, cutoff).await
        } else {
            self.queue.dequeue(dequeue_limit).await
        };
        let batch = match dequeued {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("dequeue failed: {}", e);
                return BatchOutcome::Failed;
            }
        };
        if batch.is_empty() {
            return BatchOutcome::Empty;
        }

        // 构建 JSON 负载并（可选）压缩
        let payload_bytes = self.build_payload(&batch);
        let payload_bytes = self.maybe_compress(&payload_bytes);

        let (tx, rx) = oneshot::channel();
        // P1-4 修复：槽位携带唯一 ack_id，先登记后 publish（顺序颠倒会让
        // 先到的 PubAck 被 FIFO 中其它批次槽位吃掉）。publish_semaphore
        // permit=1 + 登记先于 publish 保证 push_back 顺序 == broker 收到
        // publish 顺序，pop_front 拿到的就是本次 sender。data_id 携带是
        // 为防御性目的：FIFO 错位时日志可定位是哪个批次。
        let ids: Vec<i64> = batch.iter().map(|i| i.id).collect();
        let data_id_for_ack = ids.first().copied().unwrap_or(0);
        let ack_id = next_ack_id(&self.ack_id_gen);
        self.pending_ack.lock().push_back(PendingAckSlot {
            ack_id,
            data_id: data_id_for_ack,
            tx,
        });

        // P3: 异步持久化到 outbox（通过 channel 解耦，不阻塞 publish 路径）
        let data_id = ids.first().copied().unwrap_or(0);
        let msg = OutgoingMsg {
            data_id,
            data_ids: ids.clone(),
            topic: self.config.topic.clone(),
            payload: payload_bytes.clone(),
            qos: 1,
            protocol: "mqtt".into(),
        };
        let mut sync_msg: Option<OutgoingMsg> = None;
        let outbox_tx = self.outbox_cmd_tx.load_full();
        if let Some(tx) = &outbox_tx {
            if let Err(tokio::sync::mpsc::error::TrySendError::Full(OutboxCmd::Persist(m))) =
                tx.try_send(OutboxCmd::Persist(msg))
            {
                // X2: 退化到同步落盘——背压超额时不让采集侧阻塞，但必须
                // 量化"退化频率"，否则运维侧只能看 logs。计数与日志顺序：
                // 先 inc 后 warn，避免 warn 已发出但计数还未更新导致告警
                // 数值短期滞后于实际退化次数。
                self.metrics.inc_mqtt_outbox_sync_fallback();
                tracing::warn!("outbox channel full, degrading to sync persist");
                sync_msg = Some(m);
            }
        } else {
            sync_msg = Some(msg);
        }
        if let Some(m) = sync_msg {
            if let Some(outbox) = &self.outbox {
                // A3: persist 放到 spawn_blocking，避免 SQLite INSERT 阻塞 tokio runtime
                let outbox_clone = outbox.clone();
                match tokio::task::spawn_blocking(move || outbox_clone.blocking_lock().persist(&m))
                    .await
                {
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => {
                        tracing::warn!("outbox sync persist failed (data_id={}): {}", data_id, e)
                    }
                    Err(e) => tracing::error!("outbox persist spawn_blocking join failed: {}", e),
                }
            }
        }

        match self
            .client
            .publish(&self.config.topic, QoS::AtLeastOnce, false, payload_bytes)
            .await
        {
            Ok(_) => {
                let ack_timeout = Duration::from_secs(self.config.ack_timeout_secs.unwrap_or(10));
                match tokio::time::timeout(ack_timeout, rx).await {
                    Ok(Ok(())) => {
                        // #4 修复（双重上报计费口径）：原实现先 outbox_ack 再
                        // queue.ack，queue.ack 失败时行回到 in_flight=0（未送达），
                        // 下次 dequeue 选回重发 + outbox 已被删——结果"看似只重发一次"。
                        // 但反过来（outbox_ack 失败 + queue.ack 成功）：行标 delivered=1
                        // 等 retention 删，outbox 保留 data_id → 重连后
                        // replay_pending_outbox 再发一次，**与本次发送重复**。
                        // 改为：先 queue.ack（先关掉"队列重新 dequeue"这条路），
                        // 再 outbox_ack（关掉"outbox replay"这条路）。任一失败，
                        // 都按"失败路径"统一 cleanup outbox——保证「队列 ↔ outbox」
                        // 只有一个保留重发入口，杜绝重复上报计费口径。
                        let queue_ack_ok = match self.queue.ack(ids).await {
                            Ok(()) => true,
                            Err(e) => {
                                tracing::warn!("queue.ack failed: {}", e);
                                false
                            }
                        };
                        if !queue_ack_ok {
                            // queue.ack 失败：行已 release（in_flight=0, delivered=0），
                            // 下次 dequeue 选回重发。**必须**删 outbox，否则重连
                            // 后还会通过 replay 重发同一批 → 双重上报。
                            self.cleanup_outbox_on_failure(data_id).await;
                        } else {
                            // queue.ack 成功：关掉队列重发路径。outbox 清理失败
                            // 允许 at-least-once 短暂重复（QoS1 允许，由业务去重）。
                            self.outbox_ack(data_id).await;
                        }
                        tracing::debug!(
                            "published {} points to {}",
                            batch.len(),
                            self.config.topic
                        );
                        self.metrics.inc_mqtt_publish(true);
                        // 断路器：成功则重置失败计数
                        self.circuit_failures
                            .store(0, std::sync::atomic::Ordering::Relaxed);
                        // P0-3：按**实际发送条数**扣减令牌（令牌单位=条）。
                        // 仅在限速生效时扣；不限速（None）时不消耗任何额度。
                        if rate_limit.is_some() {
                            self.rate_consume(batch.len());
                        }
                        BatchOutcome::Sent(batch.len())
                    }
                    Ok(Err(_)) => {
                        tracing::warn!("mqtt ack channel closed, releasing in_flight for retry");
                        self.metrics.inc_mqtt_publish(false);
                        // P1-4：按 ack_id 移除自有槽位（非盲 pop_front）；消息
                        // 可能已发出，orphan++ 对账迟到的 PubAck。
                        if remove_slot_by_ack_id(&self.pending_ack, ack_id) {
                            self.orphan_acks
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                        self.queue.release(ids).await;
                        // #9: 统一清理 outbox——避免重连后 replay 与 queue 重新 dequeue 重复上报
                        self.cleanup_outbox_on_failure(data_id).await;
                        self.record_circuit_failure();
                        BatchOutcome::Failed
                    }
                    Err(_) => {
                        tracing::warn!("mqtt PubAck timeout, releasing in_flight for retry");
                        self.metrics.inc_mqtt_publish(false);
                        // P1-4：同上——按 ack_id 移除自有槽位并 orphan++
                        // （publish 可能已到达 broker，PubAck 迟到，由事件循环
                        // reconcile_puback 对账消化，不偷 FIFO 头部）。
                        if remove_slot_by_ack_id(&self.pending_ack, ack_id) {
                            self.orphan_acks
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                        self.queue.release(ids).await;
                        // #9: 同上，超时也要清理 outbox
                        self.cleanup_outbox_on_failure(data_id).await;
                        self.record_circuit_failure();
                        BatchOutcome::Failed
                    }
                }
            }
            Err(e) => {
                // P1-4：publish 调用本身失败，消息未进入 rumqttc 发送队列，
                // 不会有 PubAck 到达——只移除槽位，不递增孤儿计数。
                remove_slot_by_ack_id(&self.pending_ack, ack_id);
                self.queue.release(ids).await;
                tracing::warn!("mqtt publish failed: {}", e);
                if self.config.is_snapshot() {
                    // 现场排障提示：snapshot 语义下被更新样本取代的旧行不会重发，
                    // 只有每点位最新一行会在下个周期重试——不要按 stream 的"全量补发"预期排查。
                    tracing::warn!(
                        "snapshot 模式：已释放的旧样本不会重发（将被更新值折叠），仅每点位最新值于下周期重试"
                    );
                }
                self.metrics.inc_mqtt_publish(false);
                // #9: publish 调用本身失败同样清理 outbox
                self.cleanup_outbox_on_failure(data_id).await;
                self.record_circuit_failure();
                BatchOutcome::Failed
            }
        }
    }

    /// #9: publish 失败时统一清理 outbox 记录。
    ///
    /// 失败路径已通过 `queue.release(ids)` 把队列 ids 退回（数据仍在 SQLite 队列，
    /// 可被下次 dequeue 重新发出）。若 outbox 里还留有对应的 pending 记录，
    /// 重连后 `replay_pending` 会再发一次，与队列重新 dequeue 的同批数据形成重复上报。
    ///
    /// 因此失败时调用本方法物理删除 outbox 记录，确保"只有队列重 dequeue 这一条重发路径"。
    /// 删除失败仅告警，不影响主流程（最坏情况是重启后 outbox 重发一次，QoS1 允许重复）。
    /// P3: 通过 channel 异步标记 outbox 已确认（by data_id）
    ///
    /// r47-MQTT1 修复：原实现用 `try_send(...).is_ok()`，channel **满（Full）** 时
    /// 静默回退为同步直写 DB。但此时对应的 `OutboxCmd::Persist` 可能仍在
    /// `outbox_writer_loop` 的 batch 缓冲中未 flush 落盘——同步 `mark_acked_by_data_id`
    /// 作用在**尚不存在的行**上是空操作，随后 flush 才把该 pending 行写库，于是留下
    /// **陈旧 pending** → 重启 `replay_pending_outbox` 重复上报（计费口径翻倍）。这破坏了
    /// `outbox_writer_loop` 声明的「Persist 先于 Ack 入队、Ack 早于 flush 时用 state map
    /// 的 pre-ack 标志兜底」不变量。
    ///
    /// 改为 `send(...).await`：进入 channel 才有 FIFO 顺序保证与 pre-ack 跟踪；仅当
    /// `send` 返回 `Err`（channel 已关闭，即 writer 已停止）时才回退同步直写。
    async fn outbox_ack(&self, data_id: i64) {
        let tx = self.outbox_cmd_tx.load_full();
        if let Some(tx) = &tx {
            if tx.send(OutboxCmd::Ack(data_id)).await.is_ok() {
                return;
            }
            // channel 已关闭（writer 已退出）：退化为同步直写兜底
        }
        if let Some(outbox) = &self.outbox {
            // A3: mark_acked_by_data_id 放到 spawn_blocking
            // r78-OBX1：传 protocol="mqtt" 避免误删 http 行
            let outbox_clone = outbox.clone();
            let _ = tokio::task::spawn_blocking(move || {
                outbox_clone
                    .blocking_lock()
                    .mark_acked_by_data_id(data_id, "mqtt")
            })
            .await;
        }
    }

    /// #9 失败路径：物理删除 outbox 记录，确保"只有队列重 dequeue"这一条重发路径。
    ///
    /// r47-MQTT1：与 [`Self::outbox_ack`] 同理，用 `send(...).await` 而非 `try_send`
    /// 保证 Remove 进入 channel 并保留 pre-remove 跟踪；channel 满时若直接同步
    /// `remove_by_data_id`，会漏删尚未 flush 的行，flush 后陈旧 pending 残留 →
    /// replay 与队列重 dequeue 形成重复上报。
    async fn cleanup_outbox_on_failure(&self, data_id: i64) {
        let tx = self.outbox_cmd_tx.load_full();
        if let Some(tx) = &tx {
            if tx.send(OutboxCmd::Remove(data_id)).await.is_ok() {
                return;
            }
            // channel 已关闭（writer 已退出）：退化为同步直写兜底
        }
        if let Some(outbox) = &self.outbox {
            // A3: remove_by_data_id 放到 spawn_blocking
            // r78-OBX1：传 protocol="mqtt" 避免误删 http 行
            let outbox_clone = outbox.clone();
            let _ = tokio::task::spawn_blocking(move || {
                outbox_clone
                    .blocking_lock()
                    .remove_by_data_id(data_id, "mqtt")
            })
            .await;
        }
    }

    /// #16: 绕队列直发（bypass_queue）——直接把 DataPoint 转发到 MQTT，不经过 SQLite 队列。
    ///
    /// 命名说明：原方法名 `publish_direct` 与「透传」混淆——这里既不是 MQTT 报文透传，
    /// 也不是 Modbus 网关桥接，只是**跳过队列**的直发通道，故改名 `publish_bypass_queue`。
    ///
    /// 用于透传模式（GatewayMode::Passthrough 或点位配置 passthrough=true）：
    /// 采集到的数据不进入边缘聚合窗口、不落 SQLite 队列，直接构建 payload 发布。
    /// 优势：端到端延迟最低（无 dequeue/ack 往返）；代价：断网期间数据丢失（无队列补传）。
    ///
    /// 复用 build_payload（DataPoint → QueuedItem 临时转换，id=0 表示非队列来源），
    /// 保证透传与批量上报的 payload schema 一致，Go+Vue 调试软件无需区分两种格式。
    ///
    /// QoS1 下 rumqttc event loop 会内部重传未确认消息，但本方法不等待
    /// PubAck（槽位 sender 无接收者，PubAck 到达时 send 失败忽略即可）。
    /// P1-4：透传发布**必须**登记 pending_ack 槽位——否则本消息先于
    /// 后续 publish_batch 的 publish 发出，其 PubAck 到达时会被事件循环
    /// pop_front 偷走，误 signal 后续批次的槽位，导致数据未送达却被
    /// ack 删除（数据丢失）。断连重连后由 event loop 尽力重发，不保证不丢。
    ///
    /// C-7 修复：透传也走 `publish_semaphore`（permit=1 等价同步等待）。
    /// 原因：`client.publish` 与 `publish_batch` 共享同一个 rumqttc 内部
    /// 通道。若透传绕过 semaphore，`client.publish` 调用顺序由 tokio 调度
    /// 决定（不保证），broker 端收到的 publish 顺序就可能与 `pending_ack`
    /// 的 push_back 顺序错位——event_loop pop_front 拿到的 sender 不是同
    /// 一个批次，触发 C-6 描述的"重复上报/丢数据"。所有 publish 路径
    /// 共享 semaphore 才能保证 broker 端严格保序。
    ///
    /// C-7 一致性修复：成功路径也调用 `reset_circuit_failures`（之前只失败路径
    /// 累加失败计数，成功路径不重置——导致连续成功透传永远不能恢复断路器）。
    /// 失败路径保留 `record_circuit_failure` 行为不变。
    ///
    /// tracing: 透传发布 span（n=points.len()），tracing feature off 时 cfg_attr 剥离为零成本。
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, points), fields(n = points.len()), level = "debug"))]
    pub async fn publish_bypass_queue(&self, points: &[DataPoint]) -> bool {
        if points.is_empty() {
            return true;
        }
        // 断路器打开或未连接时直接丢弃（透传模式下不缓存，接受丢失）
        if self.is_circuit_open() || !self.is_connected() {
            self.metrics.inc_mqtt_publish(false);
            tracing::debug!(
                "passthrough publish skipped: circuit_open={} connected={}",
                self.is_circuit_open(),
                self.is_connected()
            );
            return false;
        }

        // C-7 修复：透传也走 publish_semaphore。所有 publish 路径串行才能保证
        // broker 端 publish 接收顺序与 pending_ack push_back 顺序一致。
        // 不用 `.expect`（crate 级 `deny(clippy::expect_used)`）：信号量只在停机时
        // 才会被 close，Err 理论不可达；真出现时按「本次透传跳过」处理，而不是 panic。
        let _permit = match self.publish_semaphore.acquire().await {
            Ok(permit) => permit,
            Err(_) => {
                tracing::warn!("publish_semaphore closed; skipping passthrough publish");
                self.metrics.inc_mqtt_publish(false);
                return false;
            }
        };

        // P1-4 修复：先登记槽位再 publish（data_id=0 表示非队列来源，rx
        // 丢弃——透传不等待 ack，但槽位必须存在以维持 FIFO 对齐）。
        let ack_id = next_ack_id(&self.ack_id_gen);
        let (tx, _rx) = oneshot::channel();
        self.pending_ack.lock().push_back(PendingAckSlot {
            ack_id,
            data_id: 0,
            tx,
        });

        // DataPoint → QueuedItem（id=0，仅用于 build_payload 序列化）
        let items: Vec<QueuedItem> = points.iter().map(QueuedItem::from_datapoint).collect();
        let payload_bytes = self.build_payload(&items);
        let payload_bytes = self.maybe_compress(&payload_bytes);

        match self
            .client
            .publish(&self.config.topic, QoS::AtLeastOnce, false, payload_bytes)
            .await
        {
            Ok(_) => {
                tracing::debug!(
                    "passthrough published {} points to {}",
                    points.len(),
                    self.config.topic
                );
                self.metrics.inc_mqtt_publish(true);
                // C-7 一致性：成功路径重置断路器失败计数，与 publish_batch 行为一致
                self.reset_circuit_failures();
                true
            }
            Err(e) => {
                // P1-4：publish 调用本身失败，消息未入 rumqttc 队列，
                // 不会有 PubAck 到达——只移除槽位，不递增孤儿计数。
                remove_slot_by_ack_id(&self.pending_ack, ack_id);
                tracing::warn!("passthrough publish failed: {}", e);
                self.metrics.inc_mqtt_publish(false);
                self.record_circuit_failure();
                false
            }
        }
    }

    /// 对负载进行可选的 gzip 压缩
    ///
    /// 返回字节数组，首字节为标志：0x00=未压缩，0x01=gzip 压缩。
    pub(crate) fn maybe_compress(&self, payload: &[u8]) -> Vec<u8> {
        let threshold = self.config.compression_threshold.unwrap_or(1024);
        let should_compress =
            self.config.compression.as_deref() == Some("gzip") && payload.len() > threshold;

        if !should_compress {
            let mut out = Vec::with_capacity(payload.len() + 1);
            out.push(0x00);
            out.extend_from_slice(payload);
            return out;
        }

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        if let Err(e) = encoder.write_all(payload) {
            tracing::warn!("gzip encode failed: {}", e);
            let mut out = Vec::with_capacity(payload.len() + 1);
            out.push(0x00);
            out.extend_from_slice(payload);
            return out;
        }
        match encoder.finish() {
            Ok(compressed) => {
                let mut out = Vec::with_capacity(compressed.len() + 1);
                out.push(0x01);
                out.extend_from_slice(&compressed);
                out
            }
            Err(e) => {
                tracing::warn!("gzip finish failed: {}", e);
                let mut out = Vec::with_capacity(payload.len() + 1);
                out.push(0x00);
                out.extend_from_slice(payload);
                out
            }
        }
    }

    /// 查询当前可用的发送额度（条数），并推进令牌桶补充。
    ///
    /// P0-3：`resume_rate_limit` 的语义是"条/秒"，因此令牌桶的令牌单位是
    /// **条**（而非旧实现的"批"）。旧实现 `take` 恒被 `max_size=100` 卡死，
    /// 令牌桶从未真正限速——本方法改为返回当前可用的**条数**额度，
    /// 调用方据此裁剪本次 dequeue 的批量。
    ///
    /// 本方法**不扣减**令牌：真正的扣减发生在 `rate_consume`（按实际发送
    /// 条数、成功路径才扣）。两阶段拆分保证"发布失败的数据不会消耗额度"。
    ///
    /// 注：调用点由 `publish_semaphore`（permit=1）串行，故此处对两个原子的
    /// 读-算-写无需 CAS 循环。
    pub(crate) fn rate_available(&self, rate: u64) -> u64 {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let (last, tokens) = token_bucket_refill(
            self.rate_last_refill
                .load(std::sync::atomic::Ordering::Relaxed),
            self.rate_tokens.load(std::sync::atomic::Ordering::Relaxed),
            rate,
            now_ms,
        );
        self.rate_last_refill
            .store(last, std::sync::atomic::Ordering::Relaxed);
        self.rate_tokens
            .store(tokens, std::sync::atomic::Ordering::Relaxed);
        tokens
    }

    /// 按**实际发送条数**扣减令牌（仅成功路径调用）。
    ///
    /// P0-3：令牌单位为"条"。失败路径（网络错误 / PubAck 超时）不扣减——
    /// 数据已 release 回队列待重发，重发成功时再计费，避免"失败也扣额度"
    /// 导致补发速率被系统性低估。
    pub(crate) fn rate_consume(&self, count: usize) {
        let count = count as u64;
        let current = self.rate_tokens.load(std::sync::atomic::Ordering::Relaxed);
        self.rate_tokens.store(
            current.saturating_sub(count),
            std::sync::atomic::Ordering::Relaxed,
        );
    }
}

/// r41-MQTT2：断路器冷却判定（纯函数，便于回归测试）。
///
/// 旧实现直接做 `now - opened_at`（均为 u64）：系统时钟回拨使 `now < opened_at`
/// （NTP 校时场景可见）时下溢——debug 构建 panic，release 构建回绕成大数、
/// `now - opened_at < cooldown` 为假 → 误判"冷却已过"提前半开。
/// 改为饱和减法：时钟回拨时 `elapsed = 0`，按"刚打开"处理（保守地留在冷却期，
/// 不提前放行对故障 broker 的发布）。
fn circuit_cooldown_active(opened_at: u64, now: u64, cooldown_secs: u64) -> bool {
    now.saturating_sub(opened_at) < cooldown_secs
}

/// P0-3：限速退避时长计算（纯函数）。
///
/// 目标是**攒够一整批**令牌再发，而不是"攒 1 条发 1 条"——后者虽然平均
/// 速率正确，但会让 MQTT 往返次数暴涨（每条一次 PubAck 等待），把串行
/// publish 路径的吞吐压到远低于配置值。
///
/// `need = batch_target - available`（至少 1），所需时间 = `need * 1000 / rate`
/// 毫秒；单次等待封顶 `MAX_THROTTLE_WAIT_MS`，保证关停响应不被拖住
/// （超时后再次计算，仍不足则继续短等待）。
fn throttle_wait_ms(batch_target: usize, available: u64, rate: u64) -> u64 {
    let need = (batch_target as u64).saturating_sub(available).max(1);
    let ms = need
        .saturating_mul(1000)
        .saturating_add(rate.saturating_sub(1))
        / rate.max(1);
    ms.clamp(1, MAX_THROTTLE_WAIT_MS)
}

/// 令牌桶容量（突发上限，单位：条）。
///
/// P0-3：容量 = rate * 突发系数。允许补发开始时短时突发，长期平均速率仍受
/// rate（条/秒）约束。rate=0 时返回 0（调用方不应进入限速分支）。
fn token_bucket_cap(rate: u64) -> u64 {
    rate.saturating_mul(TOKEN_BUCKET_BURST_FACTOR)
}

/// P0-3：令牌桶补充计算（纯函数，便于回归测试）。
///
/// 返回 `(新的补充时间戳, 新的令牌数)`：
/// - `last_refill == 0` 视为首次调用：直接给满桶（= cap），允许一次突发；
/// - 令牌上限 `cap`（= rate * 突发系数），补充量 `rate * elapsed_ms / 1000`
///   （整毫秒取整为 0 时不推进时间戳，避免高频调用把补充量抹平）；
/// - 时钟回拨（now < last_refill）时 elapsed 饱和为 0，保守地不补充也不回退。
///
/// 与旧实现的关键差异：旧 `token_bucket_step` 把 `take` 夹在 `max_size`
/// （= 单批条数上限）之内，令牌桶实际从未限制过发送量；本函数**只做补充**，
/// 可用额度由调用方按"批量目标 vs 可用令牌"取小得出。
fn token_bucket_refill(last_refill: u64, tokens: u64, rate: u64, now_ms: u64) -> (u64, u64) {
    let cap = token_bucket_cap(rate);
    if last_refill == 0 {
        // 首次调用：满桶，允许一次突发（每 rate 条的 2 倍）。
        return (now_ms, cap);
    }
    let mut last_refill = last_refill;
    let mut tokens = tokens.min(cap);
    let elapsed_ms = now_ms.saturating_sub(last_refill);
    if elapsed_ms > 0 {
        let refill = rate.saturating_mul(elapsed_ms) / 1000;
        if refill > 0 {
            last_refill = now_ms;
            tokens = tokens.saturating_add(refill).min(cap);
        }
    }
    (last_refill, tokens)
}

#[cfg(test)]
mod tests {
    use super::{circuit_cooldown_active, throttle_wait_ms, token_bucket_cap, token_bucket_refill};

    /// r41-MQTT2 回归：冷却判定对时钟回拨（now < opened_at）必须安全。
    /// 旧实现 `now - opened_at` 在 u64 下溢（debug panic / release 回绕误判冷却已过）。
    #[test]
    fn circuit_cooldown_handles_clock_rollback() {
        // 正常：冷却期内 → 仍阻断
        assert!(circuit_cooldown_active(1_000, 1_010, 30));
        // 正常：冷却已过 → 放行（半开）
        assert!(!circuit_cooldown_active(1_000, 1_040, 30));
        // 时钟回拨：now < opened_at → 饱和为 0，保守视为仍在冷却期（不 panic / 不提前半开）
        assert!(circuit_cooldown_active(2_000, 1_990, 30));
        assert!(circuit_cooldown_active(2_000, 0, 30));
    }

    /// P0-3 契约：首次调用给满桶（cap = rate * 突发系数），额度单位是"条"。
    #[test]
    fn token_bucket_first_call_fills_to_cap() {
        // rate=10 → cap=20，首次调用直接给 20 条额度
        assert_eq!(token_bucket_refill(0, 0, 10, 1_000), (1_000, 20));
        // 突发系数恒为 2
        assert_eq!(token_bucket_cap(10), 20);
        assert_eq!(token_bucket_cap(1_000), 2_000);
    }

    /// P0-3 契约：补充量按 rate*elapsed/1000 计算，且令牌上限为 cap。
    #[test]
    fn token_bucket_refills_and_caps() {
        // 1s 补 1000 令牌（rate=1000），上限 2000
        assert_eq!(token_bucket_refill(1_000, 0, 1_000, 2_000), (2_000, 1_000));
        // 累积补充被上限 2000 截断（1900 + 补 2000 → 封顶 2000）
        assert_eq!(
            token_bucket_refill(1_000, 1_900, 1_000, 3_000),
            (3_000, 2_000)
        );
    }

    /// 契约：elapsed 的补充量取整为 0 时，不推进时间戳（避免高频调用抹平补充）
    #[test]
    fn token_bucket_small_elapsed_keeps_timestamp() {
        // rate=1000, elapsed=1ms → refill=1 成立，正常推进
        assert_eq!(token_bucket_refill(1_000, 0, 1_000, 1_001), (1_001, 1));
        // rate=1, elapsed=999ms → refill=0，时间戳保持不回退
        assert_eq!(token_bucket_refill(1_000, 0, 1, 1_999), (1_000, 0));
    }

    /// P0-3 契约：时钟回拨（now < last_refill）不得下溢，也不得补充令牌。
    #[test]
    fn token_bucket_handles_clock_rollback() {
        // now=900 < last_refill=1000 → elapsed 饱和为 0，时间戳与令牌都不变
        assert_eq!(token_bucket_refill(1_000, 50, 100, 900), (1_000, 50));
    }

    /// P0-3 契约：退避时长按"攒够一整批"计算，且单次等待不超过上限。
    #[test]
    fn throttle_wait_scales_with_batch_target() {
        // rate=1000，攒 500 条 → 500ms
        assert_eq!(throttle_wait_ms(500, 0, 1_000), 500);
        // rate=200，攒 500 条 → 2500ms，被 MAX_THROTTLE_WAIT_MS(1000) 截断
        assert_eq!(throttle_wait_ms(500, 0, 200), 1_000);
        // 已攒 400 条、还差 100 条，rate=1000 → 100ms
        assert_eq!(throttle_wait_ms(500, 400, 1_000), 100);
        // 至少等 1ms（available 已满时也保持最小退避）
        assert_eq!(throttle_wait_ms(0, 0, 1_000), 1);
    }
}
