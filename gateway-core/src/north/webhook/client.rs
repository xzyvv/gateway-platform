use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::config::WebhookConfig;
use crate::engine::error::{GatewayError, GatewayResult};
use crate::engine::model::DataPoint;
use crate::north::mqtt::outbox::{OutboxStore, OutgoingMsg};

use super::retry::*;
/// Webhook 上报客户端。
pub struct WebhookClient {
    config: WebhookConfig,
    client: reqwest::Client,
    outbox: Arc<Mutex<OutboxStore>>,
    /// Bug C13: flush 单飞标志——上一轮未结束时跳过本轮，见 `flush_pending`。
    flushing: Arc<AtomicBool>,
    /// Bug 171: 按 outbox_id 记录重试次数与下次可重试时刻，用于指数退避与尝试上限。
    retries: Arc<Mutex<HashMap<i64, RetryState>>>,
    /// r49-WH1：本轮 flush 探测到的最大已处理 outbox_id，下次 flush 从
    /// `after_id = last_cursor` 继续向后推进，避免「首页被退避阻塞的 50 条
    /// 永远占满 REPLAY_BATCH_SIZE、其后的所有新 pending 消息被饥饿」。仅
    /// 本轮成功取到至少一条消息时前进；遇到空 / 错误时回退到 0 重新探测。
    last_flush_cursor: Arc<AtomicI64>,
    /// r61-SD1：与 r48-MQTT1 同型——run 循环无内置退出机制，靠 stopping 标志 +
    /// Notify 让 `stop()` 可唤醒 `interval.tick().await` 阻塞；避免被 systemd
    /// SIGKILL 后 outbox 中 http 协议 pending 行永久残留（重启后 replay）。
    stopping: Arc<AtomicBool>,
    stop_notify: Arc<tokio::sync::Notify>,
    /// r65-SD1：上一轮 detached flush 任务的 JoinHandle。
    ///
    /// `run()` 的 tick 会 `spawn_catch_unwind("webhook_flush", ...)` 甩出 flush
    /// ——spawn 后句柄被丢弃，若停机时 flush 仍在飞行，进程退出可能撕裂
    /// SQLite 写。改为把句柄存到本字段，下次 loop 迭代时 await 它（带超时），
    /// 并在 run 退出前 await 一次，保证 `webhook.run` 真正等完所有 in-flight 任务。
    /// `parking_lot::Mutex`——持锁极短，await 会在锁释放后执行。
    last_flush: Arc<parking_lot::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    /// r79-OBX1：指标采集器——give-up / permanent failure 路径递增
    /// `webhook_data_lost_total` / `webhook_giveup_total`，运维可告警发现
    /// 「webhook 数据丢失」事件。None = 测试模式不统计。
    metrics: Option<Arc<crate::engine::metrics::Metrics>>,
    /// N-7：最近一次发送失败的原因（成功后清空）。
    ///
    /// 旧 `health()` 的 `last_error` 恒为 None、`connected` 仅看配置，
    /// 端点 4xx/5xx、TLS 失败等情况在状态页完全不可见；此处记录真实错误，
    /// 由 `NorthConnector::health()` 上报给 `/api/north/status`。
    last_error: Arc<parking_lot::RwLock<Option<String>>>,
}

impl WebhookClient {
    /// 创建 Webhook 客户端。
    ///
    /// r79-OBX1：增加 `metrics: Arc<Metrics>` 必传参数——webhook give-up / permanent
    /// 失败路径**物理删除 outbox 行**（即数据真的丢了），必须递增 `webhook_data_lost_total`
    /// / `webhook_giveup_total` 指标，运维通过 Grafana 告警可立即发现。
    /// 旧实现仅 `tracing::error!`，无指标 —— 数据丢失静默，只能事后翻日志定位。
    pub fn new(
        config: WebhookConfig,
        outbox: OutboxStore,
        metrics: Arc<crate::engine::metrics::Metrics>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
        // B3: connect_timeout 限制 DNS 解析 + TCP 建连 + TLS 握手阶段。
        // 单纯 timeout 在 DNS/TCP 阶段挂起时仍会等到 timeout 秒；
            // connect_timeout 提前 10s 失败，让 outbox 改走重试逻辑。
            .connect_timeout(Duration::from_secs(10))
            .build()
            // Bug 134: fallback 客户端也有 timeout——默认 reqwest::Client::new()
            // 无超时限制，可能导致网络 hang 时整个上报循环阻塞。
            // 这里用 30s 兜底（与 config.timeout_secs 默认值一致）。
            .unwrap_or_else(|_| {
                reqwest::Client::builder()
                    .timeout(Duration::from_secs(30))
                    .connect_timeout(Duration::from_secs(10))
                    .build()
                    .unwrap_or_else(|_| reqwest::Client::new())
            });
        Self {
            config,
            client,
            outbox: Arc::new(Mutex::new(outbox)),
            flushing: Arc::new(AtomicBool::new(false)),
            retries: Arc::new(Mutex::new(HashMap::new())),
            // r49-WH1：游标初值 0，等价「从头探测」——首次 flush 与原行为一致。
            last_flush_cursor: Arc::new(AtomicI64::new(0)),
            // r61-SD1：与 r48 同型 shutdown 标志 + Notify 配套。
            stopping: Arc::new(AtomicBool::new(false)),
            stop_notify: Arc::new(tokio::sync::Notify::new()),
            // r65-SD1：detached flush 句柄槽，run 循环写入 + 取走。
            last_flush: Arc::new(parking_lot::Mutex::new(None)),
            // r79-OBX1：metrics 句柄。生产路径在 new() 直接传，测试路径用
            // Arc::new(Metrics::new()) 即可。give-up / permanent 失败递增
            // webhook_data_lost_total / webhook_giveup_total。
            metrics: Some(metrics),
            // N-7：最近错误初始为空（尚未发送过，也就没有可报告的失败）。
            last_error: Arc::new(parking_lot::RwLock::new(None)),
        }
    }

    /// r61-SD1：触发 webhook run 循环退出（不等待最后一次 flush 收尾——callers
    /// 应在 stop 之前先 await 一次 flush_pending 让 in-flight 落地）。
    ///
    /// 设计参考 r48-MQTT1 / r60-SD2：先置位 stopping 再 notify_waiters，覆盖
    /// `interval.tick().await` 与 wait 到 stop 之间的丢失唤醒窗口。
    /// webhook 没有真正的「事件循环」/「pending 任务」分离——run 内单一 loop，
    /// 唤醒后下一次 `interval.tick().await` 会立即因 stopping=true break。
    pub async fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.stop_notify.notify_waiters();
    }

    /// 启动 Webhook flush 循环
    ///
    /// 1.2: publish_batch 直接调用 persist_batch（数据入 outbox），
    /// 本循环仅定时从 outbox 取 pending 消息发送（flush_pending）。
    pub async fn run(self: Arc<Self>) {
        if !self.config.enabled {
            tracing::info!("webhook disabled, skip");
            return;
        }
        if self.config.url.is_empty() {
            tracing::warn!("webhook enabled but url is empty, skip");
            // 同步写 last_error：否则 `health()` 仍按「url 非空 + 无错误」判定
            // connected=true，状态页会把「run 循环已退出」的通道显示为正常。
            *self.last_error.write() = Some("webhook url is empty".to_string());
            return;
        }
        // Bug 135: 协议前缀校验
        if !(self.config.url.starts_with("http://") || self.config.url.starts_with("https://")) {
            tracing::error!(
                "webhook url must start with http:// or https://, got '{}'",
                self.config.url
            );
            // 同上：run 循环退出后 health() 必须能如实报出未连接及原因。
            *self.last_error.write() = Some(format!(
                "invalid webhook url (must be http:// or https://): {}",
                self.config.url
            ));
            return;
        }

        let client = self;

        tracing::info!(
            "webhook started (persistent), url={}, batch_size={}, interval={}ms",
            client.config.url,
            client.config.batch_size,
            client.config.interval_ms
        );

        // 启动后 replay 所有 pending 的 http 消息
        client.flush_pending().await;

        // r51-WH1：进程重启后 in-memory `RetryState` 丢失，单纯靠
        // `attempts` 计数会被清零，故障端点下同一行可被无限重试。
        // 这里在启动首轮先把「attempt_count 已达 MAX_WEBHOOK_ATTEMPTS」的
        // pending 物理删除，保留持久化层的不变量：
        // - 首次 persist 时 attempt_count=0
        // - 每次 Retriable send_once 路径后 record_attempt+1
        // - attempt_count 命中上限则删除（本次启动前已用尽 + 本次启动内累计达限）
        // 删除条件 `>= max_attempts` 等价于「尝试用尽」，与内存里 give-up 一致。
        match client.drop_exhausted_pending().await {
            Ok(0) => {}
            Ok(n) => tracing::error!(
                "webhook startup: dropped {} pending rows that already exhausted retries (data lost)",
                n
            ),
            Err(e) => tracing::warn!("webhook startup: drop_exhausted_pending failed: {}", e),
        }

        // P0：重启前可能已积累超限的 pending（上次运行端点长期不可达 + 未走到入队回收
        // 就停机），启动首轮先做一次容量回收，避免带着巨量积压进入 flush。
        client.enforce_pending_limit().await;

        // B-1: interval_ms=0 会让 tokio::time::interval 内部断言 panic。
        // panic 被 spawn_catch_unwind 吞掉后本任务永久停摆，转发静默停止且无告警。
        // 与 kafka/mod.rs 保持一致做 .max(1) 兜底（北向 0 间隔无业务意义）。
        let mut interval =
            tokio::time::interval(Duration::from_millis(client.config.interval_ms.max(1)));

        // r61-SD1：loop 监听 stop_notify + stopping 标志——与 r48-MQTT1 同型。
        // 旧实现 `loop { interval.tick().await; ...
        // }` 无退出机制：systemd SIGKILL
        // 后 outbox 中 protocol='http' 的 pending 行只能等下次启动 replay，但在
        // SIGKILL 时正在 flush 写入会丢数据。stop() 触发后本循环立即 break，
        // 让 graceful_shutdown 知道 webhook 任务已退出。
        //
        // r65-SD1：loop 迭代顶部 await 上一轮 detached flush（带超时），并在
        // break 前再 await 一次——保证 run 退出时 in-flight flush 不会拖到
        // 进程退出后被 SIGKILL 撕裂。详见 WebhookClient::last_flush 字段注释。
        let stopping = client.stopping.clone();
        let stop_notify = client.stop_notify.clone();
        let last_flush = client.last_flush.clone();
        // 死信维护节流游标：run 循环是单任务，用局部变量即可，无需共享状态。
        // 初值取 now 而非 now-INTERVAL：启动首轮的 `drop_exhausted_pending` 刚搬完，
        // 立刻再全表 COUNT 一次没有信息增量。
        let mut last_dl_maint = Instant::now();
        loop {
            // 立刻检查 stopping——若 stop() 在 enable 之前已置位，立刻退出。
            if stopping.load(Ordering::SeqCst) {
                tracing::info!("webhook run loop exiting (stopping already set)");
                break;
            }
            // r65-SD1：开始 tick 前先等上一轮 detached flush（避免在飞任务累计，
            // 或 N 次 + 启动时一次报 5xx 风暴）；超时仅告警继续，不 abort。
            // 先 take 到局部变量再 if let——`if let ... = lock().take()` 的
            // scrutinee 临时量会活到块结束，guard 将跨过下方 await 被持有。
            let pending_flush = last_flush.lock().take();
            if let Some(h) = pending_flush {
                match tokio::time::timeout(PREV_FLUSH_WAIT, h).await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => tracing::warn!(
                        "webhook run loop: previous flush task join error (panicked?): {}",
                        e
                    ),
                    Err(_) => tracing::warn!(
                        "webhook run loop: previous flush still running after {:?}, starting new flush anyway",
                        PREV_FLUSH_WAIT
                    ),
                }
            }
            tokio::select! {
                biased;
                _ = stop_notify.notified() => {
                    tracing::info!("webhook run loop exiting (stop notify)");
                    break;
                }
                _ = interval.tick() => {
                    let c = client.clone();
                    let handle = crate::engine::supervision::spawn_catch_unwind(
                        "webhook_flush",
                        async move {
                            c.flush_pending().await;
                        },
                    );
                    // r65-SD1：记下新 flush 句柄，下一 tick 顶部再
                    // break 时 await。
                    *last_flush.lock() = Some(handle);

                    // 死信维护：刷新 gauge + 兜底按行数上限裁剪。
                    // 独立 spawn，避免全表 COUNT/大幅 DELETE 拖慢上报节拍。
                    // 句柄有意不保存：维护是幂等的，停机时被打断也无副作用
                    // （裁剪只按行数进行，下次启动会重新收敛）。
                    if last_dl_maint.elapsed() >= DEAD_LETTER_MAINT_INTERVAL {
                        last_dl_maint = Instant::now();
                        let c2 = client.clone();
                        // `drop(JoinHandle)` 等价于 detach（不取消任务），
                        // 用它而不是 `let _ =` 是为了显式表达"有意不等待"。
                        drop(crate::engine::supervision::spawn_catch_unwind(
                            "webhook_dead_letter_maint",
                            async move {
                                c2.maintain_dead_letters().await;
                            },
                        ));
                    }
                }
            }
        }
        // r65-SD1：退出前 await 最后一次 in-flight flush（无超时？给个短上限避免
        // 极端情况 flush 永久卡停机序；超时不 abort，仅告警继续）。
        // 同上：先 take 再 if let，避免 guard 临时量跨 await。
        let final_flush = last_flush.lock().take();
        if let Some(h) = final_flush {
            match tokio::time::timeout(FINAL_FLUSH_WAIT, h).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::warn!(
                    "webhook run loop: final flush task join error (panicked?): {}",
                    e
                ),
                Err(_) => tracing::warn!(
                    "webhook run loop: final flush still running after {:?}, exiting anyway",
                    FINAL_FLUSH_WAIT
                ),
            }
        }
        tracing::info!("webhook run loop exited");
    }

    /// 死信维护（周期调用）：刷新 `gateway_dead_letter_rows` gauge，并在超出
    /// `OutboxStore::MAX_DEAD_LETTER_ROWS` 时按最旧优先裁剪。
    ///
    /// 为什么裁剪必须在这里做而不是在 `move_to_dead_letter` 里：搬移是异常路径，
    /// 每次搬移都 COUNT 一次全表代价高；而维护循环天然按周期运行，成本可控。
    /// 代价是最多 20 万分之一秒级延迟——对"人工处置"场景无影响。
    async fn maintain_dead_letters(&self) {
        // 降级（sqlite 打不开、走内存模式）时死信表不存在，直接跳过，
        // 避免每 60s 刷一条无意义的错误日志。
        if self.outbox.lock().is_degraded() {
            return;
        }
        let outbox = self.outbox.clone();
        let res = tokio::task::spawn_blocking(move || {
            let mut g = outbox.lock();
            let total = g.count_dead_letters(None)?;
            let trimmed = if total > OutboxStore::MAX_DEAD_LETTER_ROWS {
                g.trim_dead_letters(OutboxStore::MAX_DEAD_LETTER_ROWS)?
            } else {
                0
            };
            Ok::<_, GatewayError>((total, trimmed))
        })
        .await;

        match res {
            Ok(Ok((total, trimmed))) => {
                let remaining = total - trimmed as i64;
                if let Some(m) = &self.metrics {
                    m.set_dead_letter_rows(remaining);
                    // #6：除累计条数外，再记「上次裁剪的时间/条数」——累计值
                    // 回答不了"上次丢数据是什么时候"，而那正是现场最先问的问题。
                    m.record_dead_letter_trim(trimmed as i64);
                }
                if trimmed > 0 {
                    // error 而非 warn：这是**不可恢复**的数据丢弃，且现场大概率
                    // 没人知道接收端一直拒绝——必须能在日志里被 grep 到。
                    tracing::error!(
                        "dead letter overflow: trimmed {} oldest rows (cap {}), {} rows kept — \
                         these {} batches are permanently lost, check the receiver endpoint",
                        trimmed,
                        OutboxStore::MAX_DEAD_LETTER_ROWS,
                        remaining,
                        trimmed
                    );
                }
            }
            Ok(Err(e)) => tracing::warn!("dead letter maintenance: query/trim failed: {}", e),
            Err(e) => tracing::warn!("dead letter maintenance: blocking join failed: {}", e),
        }
    }

    /// r51-WH1：把 http 协议下已超过 `MAX_WEBHOOK_ATTEMPTS` 的 pending 行移入死信表。
    ///
    /// 调用 `OutboxStore::move_exhausted_pending_to_dead_letter`：本函数是它在
    /// webhook 层的薄包装，统一走 spawn_blocking 避免阻塞 tokio 运行时（rusqlite 无异步）。
    /// 仅在启动首轮调用一次；周期 `flush_pending` 中的内存 give-up 路径
    /// 仍走 in-memory `RetryState`，无需每次 SQL 检查。
    pub(crate) async fn drop_exhausted_pending(&self) -> GatewayResult<usize> {
        let outbox = self.outbox.clone();
        let moved = tokio::task::spawn_blocking(move || {
            // parking_lot::Mutex::lock() 永不自毒、无需 unwrap。
            // 在 spawn_blocking 上下文中是同步调用，不阻塞 tokio 调度。
            outbox.lock().move_exhausted_pending_to_dead_letter(
                "http",
                MAX_WEBHOOK_ATTEMPTS,
                "retry_exhausted",
            )
        })
        .await
        .map_err(|e| GatewayError::Message(format!("join drop_exhausted_pending: {}", e)))??;
        if let Some(m) = &self.metrics {
            m.add_dead_letter_moved(moved as i64);
        }
        Ok(moved)
    }

    /// 将一批数据持久化到 outbox（protocol='http'）。
    async fn persist_batch(&self, points: Vec<DataPoint>) {
        if points.is_empty() {
            return;
        }
        // Bug P1-B6 (fix): 旧实现对整批 points 一次性 serialize + persist，
        // 5000 条 payload 约 5MB，HTTP 慢端点会触发 reqwest timeout_secs 失败
        // 8 次后整批丢弃。改为按 self.config.batch_size（默认 100）切片，
        // 每片独立 outbox 行：
        //   1) 单片 payload 在 ~100KB 以内，POST < 1s 完成；
        //   2) 单片失败只重试该片，不影响其他片；
        //   3) flush_pending 的 REPLAY_BATCH_SIZE 仍可控制 DB 读批量。
        // 兜底：batch_size=0（异常配置）退化为 100，避免无限切片或 0 切片。
        let batch_size = self.config.batch_size.clamp(1, 1000);
        for chunk in points.chunks(batch_size) {
            self.persist_one_chunk(chunk).await;
        }
        // P0：每轮入队后做一次容量上限回收。端点长期不可达时 pending 行只增不减
        // （5xx / 超时 / 连不上都保留行），MQTT / Kafka 早已有同等保护，webhook 缺这一环。
        // 放在 persist_batch 末尾（而非每片）——一圈采集只做一次 COUNT+DELETE。
        self.enforce_pending_limit().await;
    }

    /// P0：限制 outbox 中 `protocol='http'` 的 pending 行数，超出按最旧物理删除。
    ///
    /// 与 `mqtt/event_loop.rs`、`kafka/mod.rs` 的 `enforce_pending_limit` 同级保护。
    /// `Some(60_000)` 宽限窗口：不删 `last_attempt_at` 较新的行，避免把「已 POST 出去、
    /// 响应在途」的行当垃圾回收（reqwest timeout 默认 30s，60s 是 2 倍余量）。
    async fn enforce_pending_limit(&self) {
        let outbox = self.outbox.clone();
        let joined = tokio::task::spawn_blocking(move || {
            // parking_lot::Mutex 永不自毒；spawn_blocking 内是同步调用，不阻塞运行时。
            let mut store = outbox.lock();
            store.enforce_pending_limit("http", MAX_WEBHOOK_PENDING, Some(60_000))
        })
        .await;
        match joined {
            Ok(Ok(0)) => {}
            Ok(Ok(n)) => tracing::error!(
                "webhook outbox over limit ({}): dropped {} oldest pending rows (data lost)",
                MAX_WEBHOOK_PENDING,
                n
            ),
            Ok(Err(e)) => tracing::error!("webhook enforce_pending_limit failed: {}", e),
            Err(e) => tracing::warn!("webhook enforce_pending_limit join error: {}", e),
        }
    }

    /// 单片持久化（Bug P1-B6 提取：persist_batch 切片后单片复用）
    async fn persist_one_chunk(&self, chunk: &[DataPoint]) {
        if chunk.is_empty() {
            return;
        }
        let payload = match serde_json::to_vec(chunk) {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("webhook serialize failed: {}", e);
                return;
            }
        };
        // Bug C17: 不能用裸 timestamp_millis()（同毫秒两批会撞唯一索引被丢弃）。
        // 改用带进程内序号的自增生成器，见 `webhook_data_id`。
        let data_id = webhook_data_id();
        // Bug C4: http 消息不参与队列 ack（flush 不用 mark_acked 删 outbox 行）。
        // 原先伪造的 (0..len) 序号在共享 outbox 表里是误导性脏数据，置空。
        let data_ids: Vec<i64> = Vec::new();
        let msg = OutgoingMsg {
            data_id,
            data_ids,
            topic: self.config.url.clone(),
            payload,
            qos: 1,
            protocol: "http".into(),
        };
        let outbox = self.outbox.clone();
        let result = tokio::task::spawn_blocking(move || {
            // Bug 57: parking_lot::Mutex.lock() 永不中毒，无需 unwrap
            outbox.lock().persist(&msg)
        })
        .await;
        // Bug 152: 旧实现 `if let Err(e) = result` 只匹配外层 JoinError，
        // 内层 `Ok(Err(_))`（outbox.persist 失败）无分支被静默吞掉，整批数据
        // 既未入 outbox 也无任何日志，永久丢失。现补齐内层错误分支并记 error。
        //
        // 注：persist() 返回 `Result<i64, GatewayError>`（i64 = last_insert_rowid），
        // 此处用模式 `Ok(_)` 吞掉 rowid 仅做错误处理——webhook 路径不需要这个 id。
        match result {
            Ok(Ok(_rowid)) => {}
            Ok(Err(e)) => {
                tracing::error!("webhook persist failed, batch dropped: {}", e);
            }
            Err(e) => {
                tracing::error!("webhook persist join error: {}", e);
            }
        }
    }

    /// 从 outbox 取出 pending 的 http 消息并发送。
    ///
    /// tracing: Webhook 批量 flush span，tracing feature off 时 cfg_attr 剥离为零成本。
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), level = "debug"))]
    async fn flush_pending(&self) {
        // Bug C13: 单飞——若上一轮 flush 尚未结束（端点慢 / 请求最长 timeout_secs），
        // 本轮直接跳过，避免多轮并发重发同一批 pending 造成重复上报与乱序 ack。
        // `run()` 用 spawn_catch_unwind 分离任务，无法靠串行 await 保证互斥。
        if self.flushing.swap(true, Ordering::AcqRel) {
            tracing::debug!("webhook flush skipped: previous flush still in flight");
            return;
        }
        let _flush_guard = FlushGuard(self.flushing.clone());

        let outbox = self.outbox.clone();
        // r49-WH1：用本进程持久游标作为分页起点；上一轮成功取到至少一条时
        // 由本函数尾部前移。空 / 错误时回退到 0 重新探测。
        let after_id = self.last_flush_cursor.load(Ordering::Acquire);
        let msgs = match tokio::task::spawn_blocking(move || {
            // Bug 57: parking_lot::Mutex.lock() 永不中毒，无需 unwrap
            // Bug C4/C19: 在 SQL 内按 protocol 过滤后再分页，避免"LIMIT + filter"的饥饿。
            // r49-WH1：after_id 由调用方传入的持久游标驱动，分页窗口随本轮处理前进。
            outbox
                .lock()
                .replay_pending_batch("http", after_id, REPLAY_BATCH_SIZE)
        })
        .await
        {
            Ok(Ok(m)) => m,
            Ok(Err(e)) => {
                // r49-WH1：replay 失败时不要前移游标，下次仍从相同位置重试。
                // 但也不能让游标卡住导致之后的查询一直空——保守把游标回退到 0，
                // 让下一轮从 outbox 头重新探测，规避"游标越过某待发行"的漏发风险。
                self.last_flush_cursor.store(0, Ordering::Release);
                tracing::error!("webhook replay failed: {}", e);
                return;
            }
            Err(e) => {
                // spawn_blocking join 错误：与上面同理回退游标后退。
                self.last_flush_cursor.store(0, Ordering::Release);
                tracing::error!("webhook replay join error: {}", e);
                return;
            }
        };

        // Bug C4/C19: 已由 SQL 保证只返回 protocol='http' 的消息，此处不再二次过滤。
        if msgs.is_empty() {
            // r49-WH1：本次窗口无新数据——回到 outbox 头重新探测，避免本进程
            // 永远停在旧游标、漏掉旧 flush 期间新插入的高 id 行（例如重启后
            // last_flush_cursor 残留 >0，但启动期间新 id 仍可能为 pending）。
            // 这是 fail-open 取向：宁可多扫一次也不要漏发。
            self.last_flush_cursor.store(0, Ordering::Release);
            return;
        }

        // r49-WH1：记录本轮窗口的最大 outbox_id，作为下次 flush 的 after_id。
        // msgs 按 id ASC 排序，末元素即为最大 id。回写采用 AcqRel 与开头 load
        // 的 Acquire 配对，保证游标推进在单进程内单调可见。
        //
        // r59-WH1：游标推进**只在被本轮实际处理过的行**之间取最大。退避跳过的
        // 行（`continue`）不计入 max_id——否则：50 条全部处于退避期，本轮一条都
        // 没发请求，但 max_id 仍推进到窗口末——下一轮 `replay_pending_batch` 的
        // `WHERE id > max_id` 完全跳过这些 pending 行，但它们 status=0 仍占 outbox
        // 槽位、id < 等于 max_id 永远查不到（直到重启清空游标或 cleanup 触发）。
        // 同步：循环内维护 `processed[i]`，尾部调 `next_processed_max_id`
        // 取实际处理过的最大 id。
        let mut processed: Vec<bool> = Vec::with_capacity(msgs.len());

        // Bug 171: 逐条发送并按结果分类处理：
        // - 成功(2xx)：ack 删除，清空重试状态；
        // - 可重试(网络错误 / 408 / 425 / 429 / 5xx)：保留在 outbox，按指数退避延后重发；
        // - 永久失败(其余 4xx)：重发也不会成功，显式记 error 后从 outbox 移除，避免无限重发。
        // 原实现把「非 2xx」一律视为可重试——端点持续 4xx 时无限重发（MQTT 关闭时更无全局上限）。
        for (outbox_id, msg) in msgs.iter() {
            let outbox_id = *outbox_id;
            let now = Instant::now();
            // 退避闸门：若该消息尚未到下次可重试时刻，跳过本轮（避免每轮间隔都轰炸故障端点）。
            // r59-WH1：跳过时不计入 processed——见函数尾 r59 注释。
            {
                let retries = self.retries.lock();
                if let Some(st) = retries.get(&outbox_id) {
                    // E-5：attempts 已达上限但仍留在 in-memory 表里，说明上次删 outbox 行失败
                    // （DB 故障）。此时必须**停止重试**：否则本轮及后续会无限重发死信。
                    if st.attempts >= MAX_WEBHOOK_ATTEMPTS || now < st.next_at {
                        processed.push(false);
                        continue;
                    }
                }
            }
            // r59-WH1：进入 send_once 即视为「本轮处理过」。
            processed.push(true);

            match self.send_once(msg).await {
                SendOutcome::Ack => {
                    self.retries.lock().remove(&outbox_id);
                    let outbox = self.outbox.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        // Bug P0-B2 (fix): 成功路径改用 remove 物理删除。
                        // 与失败路径一致——避免 mark_acked 保留行至 cleanup_acked
                        // 触发（默认 1 小时），长期运行 outbox 表持续膨胀。
                        // 唯一索引 idx_outbox_pending_data_id 只在 status=0 时生效，
                        // 部分索引，物理删除不影响其他 pending 行的去重约束。
                        outbox.lock().remove(outbox_id)
                    })
                    .await;
                    tracing::debug!("webhook acked & removed outbox_id={}", outbox_id);
                }
                SendOutcome::Retriable(reason) => {
                    // 更新该消息的重试状态（计算全部在锁内完成，不跨 await 持锁）。
                    let attempts = {
                        let mut retries = self.retries.lock();
                        let st = retries.entry(outbox_id).or_insert(RetryState {
                            attempts: 0,
                            next_at: now,
                        });
                        st.attempts += 1;
                        st.next_at = now + retry_backoff(self.config.interval_ms, st.attempts);
                        st.attempts
                    };
                    // r51-WH1：每次 Retriable 都把 attempt_count 持久化到 outbox，
                    // 否则进程重启后 in-memory 计数清零，故障端点下同一行可能
                    // 无限重试。`record_attempt` 用 spawn_blocking，逻辑上独立于
                    // in-memory 重试状态，失败仅 warn 不影响主流程。outbox_id 是
                    // i64（Copy），在 give-up 分支仍需用：先复制再 move 进闭包。
                    let outbox = self.outbox.clone();
                    let id_for_record = outbox_id;
                    let _ = tokio::task::spawn_blocking(move || {
                        outbox.lock().record_attempt(id_for_record)
                    })
                    .await;
                    if attempts >= MAX_WEBHOOK_ATTEMPTS {
                        // 达到尝试上限仍失败：移入**死信表**并删除 outbox 行，
                        // 避免永久重试占满带宽。
                        //
                        // Bug P0-B2 原实现是直接 remove（物理删除，数据永久消失）。
                        // 改为死信：这批数据在接收端修好（路径/鉴权/白名单）之后
                        // 仍然可以人工重投，现场不必接受"丢一批"。
                        tracing::error!(
                            "webhook give up after {} attempts, moving outbox_id={} to dead letter",
                            attempts,
                            outbox_id
                        );
                        // r79-OBX1：give-up 路径递增 giveup 指标——区分「持续不可达
                        // 重试用尽」与 4xx 直接拒」，便于告警分流。
                        if let Some(m) = &self.metrics {
                            m.inc_webhook_giveup();
                        }
                        let outbox = self.outbox.clone();
                        // E-5：只有 outbox 行确实搬走才重置 attempt 计数。搬移失败时保留
                        // （attempts 已 >= 上限），由退避闸门永久跳过，避免死信无限重发。
                        let moved = tokio::task::spawn_blocking(move || {
                            outbox
                                .lock()
                                .move_to_dead_letter(outbox_id, "retry_exhausted")
                        })
                        .await;
                        match moved {
                            Ok(Ok(true)) => {
                                if let Some(m) = &self.metrics {
                                    m.add_dead_letter_moved(1);
                                }
                                self.retries.lock().remove(&outbox_id);
                            }
                            Ok(Ok(false)) => {
                                // outbox 行已被并发 ack/删除，无需计数
                                self.retries.lock().remove(&outbox_id);
                            }
                            Ok(Err(e)) => tracing::error!(
                                "webhook give-up: move outbox_id={} to dead letter failed: {} \
                                 (attempt count kept, will not retry until restart)",
                                outbox_id,
                                e
                            ),
                            Err(e) => tracing::error!(
                                "webhook give-up: dead-letter join failed for outbox_id={}: {} \
                                 (attempt count kept, will not retry until restart)",
                                outbox_id,
                                e
                            ),
                        }
                    } else {
                        tracing::warn!(
                            "webhook send failed (retriable), outbox_id={} attempt={} retry_in={}ms: {}",
                            outbox_id,
                            attempts,
                            retry_backoff(self.config.interval_ms, attempts).as_millis(),
                            reason
                        );
                    }
                }
                SendOutcome::Permanent(reason) => {
                    // 4xx 永久失败：**移入死信表**而不是物理删除。
                    //
                    // 现场最常见的成因不是"数据无效"，而是接收端配置错误
                    // （路径改了、Token 过期、IP 白名单变更）。这类问题修好后
                    // 数据本可正常投递，原实现却直接丢弃，等于把一次配置失误
                    // 变成一段永久的数据空洞。
                    //
                    // D6：原因直接取自本次 `send_once` 的返回值，不再读共享槽
                    // `last_error`——那个槽是"全局最近一次错误"，同批次里后一条
                    // 消息的失败会把它覆盖，导致先失败那条的死信 reason 指向别人。
                    tracing::error!(
                        "webhook permanent failure ({}), moving outbox_id={} to dead letter",
                        reason,
                        outbox_id
                    );
                    // r79-OBX1：永久失败递增 data_lost 指标。
                    // 语义已从「数据丢失」变为「数据进入死信（需人工处置）」，
                    // 指标名保持不变以兼容既有 Grafana 面板。
                    if let Some(m) = &self.metrics {
                        m.inc_webhook_data_lost();
                    }
                    let outbox = self.outbox.clone();
                    let reason_for_task = reason.clone();
                    let moved = tokio::task::spawn_blocking(move || {
                        outbox
                            .lock()
                            .move_to_dead_letter(outbox_id, &reason_for_task)
                    })
                    .await;
                    // E-5：搬移成功才清 in-memory 重试状态；失败时保留 attempts 且标记为
                    // 「已达上限」，退避闸门会永久跳过，避免 4xx 死信无限重发。
                    match moved {
                        Ok(Ok(true)) => {
                            if let Some(m) = &self.metrics {
                                m.add_dead_letter_moved(1);
                            }
                            self.retries.lock().remove(&outbox_id);
                        }
                        Ok(Ok(false)) => {
                            // outbox 行已被并发 ack/删除，无需计数
                            self.retries.lock().remove(&outbox_id);
                        }
                        Ok(Err(e)) => {
                            tracing::error!(
                                "webhook permanent failure: dead-letter outbox_id={} failed: {}",
                                outbox_id,
                                e
                            );
                            let mut retries = self.retries.lock();
                            retries.insert(
                                outbox_id,
                                RetryState {
                                    attempts: MAX_WEBHOOK_ATTEMPTS,
                                    next_at: now,
                                },
                            );
                        }
                        Err(e) => {
                            tracing::error!(
                                "webhook permanent failure: dead-letter join failed for outbox_id={}: {}",
                                outbox_id,
                                e
                            );
                            let mut retries = self.retries.lock();
                            retries.insert(
                                outbox_id,
                                RetryState {
                                    attempts: MAX_WEBHOOK_ATTEMPTS,
                                    next_at: now,
                                },
                            );
                        }
                    }
                }
            }
        }

        // r49-WH1：本轮处理完所有取出的消息后，把游标前移到 max_id，使下一轮
        // 自动跳过本轮已处理窗口——避免"首页 50 条卡在退避期、后续 51+ 全部
        // 饥饿"这类持久积压。Retriable 消息会保留在 outbox 中（status=0），
        // 下一轮仍会被取到（id > max_id 不命中），但由 attempt_count 与
        // in-memory retries map 之外由退避闸门（next_at 早于 now 才放行）控制
        // 实际发送频次——可重试消息会持续被处理，attempt_count 累计保障。
        // 这里 max_id 仅推进"已尝试过"的窗口边界，不是"已成功"的边界。
        // 换言之：即便本轮所有 50 条都 Retriable，下一轮仍会从 max_id 之后
        // 探测并尝试新窗口——这正是修复"首页饥饿"的关键。
        //
        // r59-WH1：游标只前移到 `next_processed_max_id`（**实际被处理过**的最大 id），
        // 而非窗口末元素 id。退避闸门 `continue` 跳过的行不计入——否则这些行
        // 的 outbox id 落在 (`last_flush_cursor`, 上界 max_id] 之间，但下一轮用
        // 新游标 = 到 max_id 查询时会被 `id > ?2` 过滤掉，永远不再被 flush。
        // 全部退避期行都满足上述条件时 `next_processed_max_id` 返回 0，本函数
        // 选择**不前进**游标（保留原 `after_id`）——保留这些行被重试的机会。
        let processed_max_id = next_processed_max_id(&msgs, &processed);
        if processed_max_id > 0 {
            self.last_flush_cursor
                .store(processed_max_id, Ordering::Release);
        }
    }

    /// 发送单个 outbox 消息，并按 HTTP 状态码/错误分类结果（Bug 171）。
    ///
    /// 返回 [`SendOutcome`] 由 `flush_pending` 决定 ack 删除 / 退避重试 / 放弃。
    ///
    /// tracing: Webhook 单条发送 span，tracing feature off 时 cfg_attr 剥离为零成本。
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, msg), fields(data_id = msg.data_id), level = "debug"))]
    pub(super) async fn send_once(&self, msg: &OutgoingMsg) -> SendOutcome {
        let mut req = self
            .client
            .post(&msg.topic)
            .header("Content-Type", "application/json")
            .body(msg.payload.clone());

        for (k, v) in &self.config.headers {
            req = req.header(k, v);
        }

        match req.send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    // N-7：成功即恢复健康，清空上一次失败记录。
                    *self.last_error.write() = None;
                    // P7：成功发送计数——此前 webhook 侧从未写过 publish_success，
                    // 导致 `gateway_uplink_total`（= mqtt + webhook）恒等于 MQTT 单边值，
                    // `gateway_webhook_publish_total{result="success"}` 恒 0。
                    if let Some(m) = &self.metrics {
                        m.inc_webhook_publish(true);
                    }
                    SendOutcome::Ack
                } else if is_retriable_status(status) {
                    tracing::warn!("webhook retriable http error: status={}", status);
                    let reason = format!("http status {status}");
                    *self.last_error.write() = Some(reason.clone());
                    if let Some(m) = &self.metrics {
                        m.inc_webhook_publish(false);
                    }
                    SendOutcome::Retriable(reason)
                } else {
                    tracing::error!("webhook non-retriable http error: status={}", status);
                    let reason = format!("http status {status} (non-retriable)");
                    *self.last_error.write() = Some(reason.clone());
                    if let Some(m) = &self.metrics {
                        m.inc_webhook_publish(false);
                    }
                    SendOutcome::Permanent(reason)
                }
            }
            Err(e) => {
                // Bug r08-WH1 (fix): reqwest 错误分类——旧实现所有错误一律 Retriable，
                // 导致 URL 拼错 / TLS 证书错误等永久失败也经 8 次重试 + 5min 退避后才丢弃，
                // 期间该条消息在 outbox 占位 ~5min。
                //
                // 分类语义：
                // - is_timeout() / is_connect()  → 网络抖动，可重试
                // - is_builder()                 → URL 解析/构造阶段失败（如方法非法），永久
                // - is_request()                 → 请求阶段（DNS 失败 / TLS 握手失败 / 协议错），永久
                // - is_decode()                  → 响应解析失败，永久
                // - 其它（罕见）→ 保守为 Retriable
                if e.is_timeout() || e.is_connect() {
                    tracing::warn!("webhook retriable transport error: {}", e);
                    let reason = e.to_string();
                    *self.last_error.write() = Some(reason.clone());
                    if let Some(m) = &self.metrics {
                        m.inc_webhook_publish(false);
                    }
                    SendOutcome::Retriable(reason)
                } else if e.is_builder() || e.is_request() || e.is_decode() {
                    tracing::error!(
                        "webhook permanent transport error (config/protocol issue): {}",
                        e
                    );
                    let reason = e.to_string();
                    *self.last_error.write() = Some(reason.clone());
                    if let Some(m) = &self.metrics {
                        m.inc_webhook_publish(false);
                    }
                    SendOutcome::Permanent(reason)
                } else {
                    tracing::warn!("webhook unknown transport error (treated retriable): {}", e);
                    let reason = e.to_string();
                    *self.last_error.write() = Some(reason.clone());
                    if let Some(m) = &self.metrics {
                        m.inc_webhook_publish(false);
                    }
                    SendOutcome::Retriable(reason)
                }
            }
        }
    }
}

// ============================================================================
// 1.2: NorthConnector 统一北向 SPI 实现（Phase A: additive only）
// ============================================================================

#[async_trait::async_trait]
impl crate::north::NorthConnector for WebhookClient {
    fn name(&self) -> &str {
        "webhook"
    }

    async fn publish_batch(&self, batch: &[DataPoint]) -> crate::engine::error::GatewayResult<()> {
        if batch.is_empty() {
            return Ok(());
        }
        // 复用 persist_batch：数据入 outbox，run() 循环定时 flush_pending 发送。
        self.persist_batch(batch.to_vec()).await;
        Ok(())
    }

    fn health(&self) -> crate::north::ConnectorHealth {
        // N-7：HTTP 无长连接，用「最近一次发送是否成功」作为健康判据。
        // 尚未发送过（last_error 为空）时按配置可用性判定，避免刚启动即报未连接。
        let last_error = self.last_error.read().clone();
        crate::north::ConnectorHealth {
            connected: self.config.enabled && !self.config.url.is_empty() && last_error.is_none(),
            last_error,
        }
    }

    fn is_enabled(&self) -> bool {
        self.config.enabled
    }
}

// 注：r49-WH1 的 `next_flush_after_id` 纯函数（窗口末元素取游标）已被 r59
// 替换为 `next_processed_max_id`，r49 函数仅在 r49/r59 历史回归测试中被引用。
// 为消除 `dead_code` 警告，把它的定义和专属 r49 单元测试一并移到下面
// `#[cfg(test)]
// mod tests` 块，限定为测试可见（r69 清理）。
/// r59-WH1：在「每行是否被本轮处理」的标志序列中取最大 id。
///
/// `flush_pending` 内联循环对每条消息按退避闸门 / 实际 send_once 判定
/// `processed[i]`，本函数返回 `processed[i] == true` 的最大 outbox_id。
/// 若全 false（全部退避跳过）则返回 0，由调用方决定是否前移游标（r59 选择**不前进**）。
///
/// `msgs` 与 `processed` 长度必须一致，且 `msgs` 按 id ASC 排序
/// （与 `replay_pending_batch` SQL `ORDER BY id` 一致），否则「最大 id」语义不严谨。
pub(crate) fn next_processed_max_id(msgs: &[(i64, OutgoingMsg)], processed: &[bool]) -> i64 {
    debug_assert_eq!(
        msgs.len(),
        processed.len(),
        "r59: msgs/processed 长度不一致"
    );
    msgs.iter()
        .zip(processed.iter())
        .filter_map(|((id, _), &p)| if p { Some(*id) } else { None })
        .max()
        .unwrap_or(0)
}

impl WebhookClient {
    /// r61-SD1：暴露 stopping 标志供测试断言 stop() 已触发 run 循环退出。
    #[cfg(test)]
    pub(crate) fn is_stopping_set(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }

    /// r65-SD1：暴露 `last_flush` 槽的当前状态供测试断言"已记下句柄 / 已清空"。
    #[cfg(test)]
    pub(crate) fn last_flush_is_some(&self) -> bool {
        self.last_flush.lock().is_some()
    }
}

/// 客户端级回归测试（真实 HTTP 桩 + 真实 sqlite outbox）。
///
/// 必须是 `client` 的**子模块**：用例要驱动私有的 `flush_pending`，并直接读
/// `self.outbox` 断言 outbox / 死信表状态。放在 `webhook/tests.rs` 里够不到这些。
#[cfg(test)]
#[path = "client_tests.rs"]
mod client_tests;
