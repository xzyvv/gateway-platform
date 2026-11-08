//! Counter / 便捷写入方法：`Metrics` 对外唯一写入 facade
//!
//! 方法名保持拆分前不变，内部委托到对应子结构字段——调用方无需感知结构变化。
//! 只保留"需要跨子结构协同"的方法（如 `record_collect` 同时写当日滚动计数器），
//! 单一子结构的写入则直接由子结构自身的 `inc_*` / `record` 方法承担。

use super::Metrics;
use std::sync::atomic::Ordering;

impl Metrics {
    /// 记录一次 MQTT 发布结果
    pub fn inc_mqtt_publish(&self, success: bool) {
        if success {
            self.mqtt.publish_success.fetch_add(1, Ordering::Relaxed);
        } else {
            self.mqtt.publish_failure.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 记录一次串口错误
    pub fn inc_serial_errors(&self) {
        self.collect.serial_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// 记录一次 MQTT ConnAck 被拒（鉴权失败 / 服务不可用 / 标识被拒）
    ///
    /// R2: 与普通断网重连分离统计——ConnAck 被拒不属网络抖动，持续增长即
    /// 提示 broker 凭据/权限配置有误，运维可据此告警。
    pub fn inc_mqtt_connack_refused(&self) {
        self.mqtt.connack_refused.fetch_add(1, Ordering::Relaxed);
    }

    /// 记录一次 outbox 命令通道满导致退化到同步持久化（X2）。
    ///
    /// 见 [`crate::engine::metrics::MqttMetrics::outbox_sync_fallback`]。
    /// 默认由 `north::mqtt::publish::publish_batch` 在 try_send 失败路径调用；
    /// 测试夹具可手动驱动以验证计数与导出。
    pub fn inc_mqtt_outbox_sync_fallback(&self) {
        self.mqtt
            .outbox_sync_fallback
            .fetch_add(1, Ordering::Relaxed);
    }

    /// 记录一次后台任务 panic（由 supervisor 调用）
    pub fn inc_task_panic(&self, _task_name: &str) {
        self.lifecyle.task_panics.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_webhook_publish(&self, success: bool) {
        if success {
            self.webhook.publish_success.fetch_add(1, Ordering::Relaxed);
        } else {
            self.webhook.publish_failure.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// r79-OBX1：webhook 永久失败（4xx 端点拒绝）导致数据**已物理丢弃**。
    /// 每次增加 = 一条数据从此不可恢复，运维必须立即介入（检查端点 / 路由 / payload 格式）。
    pub fn inc_webhook_data_lost(&self) {
        self.webhook.data_lost.fetch_add(1, Ordering::Relaxed);
    }

    /// r79-OBX1：webhook 重试用尽（达到 MAX_WEBHOOK_ATTEMPTS），outbox 行被物理删除。
    /// 与 inc_webhook_data_lost 同等严重，但告警分流用——可定位"端点持续不可达"
    /// vs"端点一直 4xx 拒"两种根本不同的根因。
    pub fn inc_webhook_giveup(&self) {
        self.webhook.giveup.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_watchdog_timeout(&self) {
        self.lifecyle
            .watchdog_timeouts
            .fetch_add(1, Ordering::Relaxed);
    }

    /// S04：优雅停机宽限超时被强制截断。
    pub fn inc_shutdown_forced(&self) {
        self.lifecyle
            .shutdown_forced
            .fetch_add(1, Ordering::Relaxed);
    }

    /// 记录一次配置热重载
    pub fn inc_config_reload(&self) {
        self.lifecyle
            .config_reload_total
            .fetch_add(1, Ordering::Relaxed);
    }

    /// C-2 自检成功：端口收发链路正常，设备静默归因于设备自身。
    pub fn inc_bus_self_check_ok(&self) {
        self.bus.self_check_ok.fetch_add(1, Ordering::Relaxed);
    }

    /// C-2 自检失败：连"最近成功从站"都收不到合法帧，端口/RS485 链路可疑。
    pub fn inc_bus_self_check_fail(&self) {
        self.bus.self_check_fail.fetch_add(1, Ordering::Relaxed);
    }

    /// C-2 自检跳过：尚无历史成功从站，无法区分设备级/端口级，保守不动端口。
    pub fn inc_bus_self_check_skipped(&self) {
        self.bus.self_check_skipped.fetch_add(1, Ordering::Relaxed);
    }

    /// C-2 因自检失败实际丢句柄重开端口（不含冷却期内被抑制的）。
    pub fn inc_bus_port_reopen_by_self_check(&self) {
        self.bus
            .port_reopen_by_self_check
            .fetch_add(1, Ordering::Relaxed);
    }

    /// 记录一次协议解码错误（CRC/校验和失败、帧格式非法等）
    ///
    /// 调用点：south/protocol/*/frames.rs 中 ChecksumError / InvalidFrame 返回前。
    /// 用途：通过 Prometheus 指标 `gateway_protocol_decode_errors_total` 监控解码错误率，
    /// 错误率突增通常意味着总线干扰、设备故障或配置错误。
    pub fn inc_protocol_decode_error(&self) {
        self.collect.inc_decode_error();
    }

    /// 设置 OTA 升级进度（0-100）
    pub fn set_ota_progress(&self, progress: i64) {
        self.ota_progress
            .store(progress.clamp(0, 100), Ordering::Relaxed);
    }

    /// 记录一次采集（含耗时，微秒）
    pub fn record_collect(&self, success: bool, duration_us: u64) {
        self.record_collect_with_reason(success, duration_us, None);
    }

    /// 记录一次采集并带上失败原因（用于 `by_reason` 分桶）。
    ///
    /// `error` 为失败时的稳定错误码（成功传 `None`）；缺省错误码归入 `other` 桶。
    pub fn record_collect_with_reason(
        &self,
        success: bool,
        duration_us: u64,
        error: Option<crate::engine::metrics::CollectErrorCode>,
    ) {
        self.roll_day_if_needed();
        self.daily_collect.fetch_add(1, Ordering::Relaxed);
        self.collect.record(success, duration_us, error);
    }

    /// 记录一次采集并同时累计到总线 / 点位维度（运维报表、总线负载、健康评分读取）。
    ///
    /// `device` 用于总线「在线设备数」；`error` 为失败时的稳定错误码（成功传 `None`）。
    pub fn record_collect_scoped(
        &self,
        bus: &str,
        point: &str,
        device: &str,
        success: bool,
        duration_us: u64,
        error: Option<crate::engine::metrics::CollectErrorCode>,
    ) {
        self.record_collect_with_reason(success, duration_us, error);
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.scoped.record(crate::engine::metrics::CollectSample {
            bus,
            point,
            device,
            success,
            duration_us,
            error,
            now_ms,
        });
    }

    /// 记录一次点位写入结果（下行写入口调用）。
    ///
    /// 调用点：`POST /api/point-tools/write`（第 1 期冗余收口后统一为单入口）、
    /// `POST /api/devices/control/send`、`POST /api/io/:index`。
    /// 写失败此前在运行指标上完全不可见（Dashboard 的 write_error 恒 0），
    /// 现场无法区分「没人下发」与「下发了但全失败」。
    pub fn record_point_write(&self, success: bool) {
        self.point_write_total.fetch_add(1, Ordering::Relaxed);
        if !success {
            self.point_write_failures.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 记录一次总线重连成功（累计到「每总线重连次数」）。
    pub fn inc_bus_reconnect(&self, bus: &str) {
        self.scoped.inc_reconnect(bus);
    }

    /// 记录一次单事务往返延迟（微秒），并区分成功/失败
    ///
    /// 决策表 #25：SLA 埋点。`duration_us` 从提交请求到收到响应（或超时返回）的
    /// 完整耗时，覆盖排队 + 收发 + 协议解析前的链路时间，供 p50/p95/p99 与错误率观测。
    pub fn record_transaction(&self, success: bool, duration_us: u64) {
        self.record_transaction_with_reason(success, duration_us, None);
    }

    /// 记录一次单事务往返延迟并带上失败原因（用于 `by_reason` 分桶）。
    ///
    /// `error` 为失败时的稳定错误码（成功传 `None`）；缺省错误码归入 `other` 桶。
    pub fn record_transaction_with_reason(
        &self,
        success: bool,
        duration_us: u64,
        error: Option<crate::engine::metrics::CollectErrorCode>,
    ) {
        self.transaction.record(success, duration_us, error);
    }

    /// 记录一次告警触发（由 alarm 引擎在「正常→越限」时调用）
    ///
    /// 同时累加累计值（经 `/api/metrics` 以 `gateway_alerts_total` 暴露）
    /// 与当日值（供 Dashboard「今日告警」展示）。
    pub fn inc_alarm_trigger(&self) {
        self.roll_day_if_needed();
        self.alarm.triggers_total.fetch_add(1, Ordering::Relaxed);
        self.daily_alerts.fetch_add(1, Ordering::Relaxed);
    }

    /// 记录一次磁盘水位告警触发（档位恶化时由磁盘监控循环调用）。
    ///
    /// 与点级 `inc_alarm_trigger` 分开计数：磁盘是系统级事件，
    /// 混进点级计数器会让"测点越限"与"网关快写满盘"无法区分。
    /// 不累加 `daily_alerts`（那是"今日点级告警"展示口径）。
    pub fn inc_disk_alarm_trigger(&self) {
        self.alarm
            .disk_triggers_total
            .fetch_add(1, Ordering::Relaxed);
    }

    /// 记录一次队列容量淘汰告警（`spawn_backlog_alarm_monitor` 观测到淘汰增量时调用）。
    ///
    /// 不与 `inc_alarm_trigger` 混用：那个是"测点越限"，累加 `daily_alerts`；
    /// 淘汰是"网关主动删了数据"，性质完全不同，混入会让「今日告警」口径失真。
    pub fn inc_queue_eviction_alarm_trigger(&self) {
        self.alarm
            .queue_eviction_triggers_total
            .fetch_add(1, Ordering::Relaxed);
    }

    /// 记录队列因容量上限淘汰的行数（`enforce_queue_limit` 实际删除后调用）。
    ///
    /// `rows` 为本次物理删除的行数，`undelivered` 为其中 `delivered=0` 的条数。
    /// 单机网关没有异地副本，这个累计值就是"到底丢了多少数据"的唯一凭据，
    /// 因此除了打日志，必须同时进指标（Prometheus）与健康口径（前端红标）。
    pub fn add_queue_evicted(&self, rows: i64, undelivered: i64) {
        if rows > 0 {
            self.queue_persist
                .evicted_rows
                .fetch_add(rows as u64, Ordering::Relaxed);
        }
        if undelivered > 0 {
            self.queue_persist
                .evicted_undelivered_rows
                .fetch_add(undelivered as u64, Ordering::Relaxed);
        }
    }

    /// 刷新死信表当前行数（gauge）。由 webhook 维护循环周期调用。
    pub fn set_dead_letter_rows(&self, rows: i64) {
        self.alarm
            .dead_letter_rows
            .store(rows.max(0) as u64, Ordering::Relaxed);
    }

    /// 记录搬入死信表的行数（`move_to_dead_letter` / 批量搬移成功后调用）。
    pub fn add_dead_letter_moved(&self, rows: i64) {
        if rows > 0 {
            self.alarm
                .dead_letter_moved_total
                .fetch_add(rows as u64, Ordering::Relaxed);
        }
    }

    /// 记录因行数上限被永久裁剪的死信行数。
    pub fn add_dead_letter_trimmed(&self, rows: i64) {
        if rows > 0 {
            self.alarm
                .dead_letter_trimmed_total
                .fetch_add(rows as u64, Ordering::Relaxed);
        }
    }

    /// 记录一次自动裁剪：累计条数 + 「上次裁剪时间/条数」快照。
    ///
    /// 裁剪是后台自动行为、没有操作人，审计只能靠「何时丢了、丢了多少」这两个
    /// 事实回溯；累计值回答不了"上次是什么时候"，因此单独存一份快照。
    pub fn record_dead_letter_trim(&self, rows: i64) {
        if rows <= 0 {
            return;
        }
        self.add_dead_letter_trimmed(rows);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.alarm
            .dead_letter_last_trim_ms
            .store(now_ms, Ordering::Relaxed);
        self.alarm
            .dead_letter_last_trim_rows
            .store(rows as u64, Ordering::Relaxed);
    }

    /// 人工重投成功一次。
    pub fn inc_dead_letter_replay(&self) {
        self.alarm
            .dead_letter_replay_total
            .fetch_add(1, Ordering::Relaxed);
    }

    /// 重投未生效（API 409）一次。
    pub fn inc_dead_letter_replay_conflict(&self) {
        self.alarm
            .dead_letter_replay_conflict_total
            .fetch_add(1, Ordering::Relaxed);
    }

    /// 人工清理删除的死信条数。
    pub fn add_dead_letter_purged(&self, rows: u64) {
        if rows > 0 {
            self.alarm
                .dead_letter_purged_total
                .fetch_add(rows, Ordering::Relaxed);
        }
    }

    /// 自动裁剪的累计条数、上次裁剪时间（ms）、上次裁剪条数。
    ///
    /// 供死信查询接口随列表一起返回，让界面能提示"表已被自动裁剪过"——
    /// 否则运维只看到一张"正常"的表，不知道更早的数据已经没了。
    pub fn dead_letter_trim_stats(&self) -> (u64, u64, u64) {
        (
            self.alarm.dead_letter_trimmed_total.load(Ordering::Relaxed),
            self.alarm.dead_letter_last_trim_ms.load(Ordering::Relaxed),
            self.alarm
                .dead_letter_last_trim_rows
                .load(Ordering::Relaxed),
        )
    }

    /// 记录内置模拟消费者成功消费的条数（`SimConsumer` 记账时调用）。
    pub fn inc_sim_consumer_consumed(&self, n: u64) {
        self.sim_consumer
            .consumed_total
            .fetch_add(n, Ordering::Relaxed);
    }

    /// 记录内置模拟消费者成功消费的批次数（`SimConsumer` 记账时调用）。
    pub fn inc_sim_consumer_batches(&self, n: u64) {
        self.sim_consumer
            .batches_total
            .fetch_add(n, Ordering::Relaxed);
    }

    /// 若 UTC 日期已变化，则把「当日」计数器清零
    ///
    /// 采用「写入前惰性滚动」而非独立后台任务：跨日到首条采集/告警到达之间，
    /// `daily_*` 可能短暂保留昨日末值（通常不到一个采集周期）；`get_status`
    /// 读取前也会调用本方法兜底刷新。
    /// 并发无害：多线程同时跨日最多重复清零，结果仍单调正确。
    pub fn roll_day_if_needed(&self) {
        let today = utc_epoch_day();
        if self.metric_day.load(Ordering::Relaxed) != today {
            self.daily_collect.store(0, Ordering::Relaxed);
            self.daily_alerts.store(0, Ordering::Relaxed);
            self.metric_day.store(today, Ordering::Relaxed);
        }
    }
}

/// 当前 UTC epoch day（1970-01-01 起的天数）
///
/// 取不到系统时间时返回 0：与真实日期永不相等，等价于「未初始化」。
fn utc_epoch_day() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_secs() / 86_400) as i64)
        .unwrap_or(0)
}
