//! 报警引擎（轻量版）
//!
//! 功能：
//! - 限值报警（高限 alarm_high / 低限 alarm_low）
//! - 状态去重：从正常→超限才触发，从超限→正常才发恢复通知
//! - MQTT 上报：报警/恢复事件发到 <mqtt_topic>/alarm
//!
//! 后续可扩展：变化率报警、持续时间去抖、多渠道推送、报警历史存储。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;
use serde::Serialize;

/// r33-ALM1: 点级报警去抖持续时间（毫秒）。点持续超限 N 毫秒才触发报警，
/// 避免设备抖动（传感器在阈值 ±0.5% 振荡）反复 trigger/recover 导致上行风暴。
/// 默认 0 毫秒（向后兼容旧行为）。点位配置字段（待后续 config 迁移）：
/// `PointConfig::alarm_debounce_ms: Option<u64>`，None 即用 0。
const ALARM_DEBOUNCE_DEFAULT_MS: u64 = 0;

use crate::config::PointConfig;
use crate::engine::bus::RealtimeBus;
use crate::engine::metrics::Metrics;
use crate::engine::model::DataPoint;
use crate::engine::model::Quality;
use crate::engine::ports::AlarmPublisher;

/// 报警事件类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AlarmType {
    /// 高限报警
    High,
    /// 低限报警
    Low,
}

/// 报警事件（MQTT 上报载荷）
#[derive(Debug, Clone, Serialize)]
pub struct AlarmEvent {
    /// 点位 ID
    pub point_id: String,
    /// 报警类型
    pub alarm_type: AlarmType,
    /// 触发时的值
    pub value: f64,
    /// 阈值
    pub threshold: f64,
    /// 事件：trigger=触发报警，recover=恢复正常
    pub event: String,
    /// 时间戳（UTC 毫秒）
    pub timestamp: i64,
}

/// 报警引擎
pub struct AlarmEngine {
    /// 点位配置（id → PointConfig）
    points: HashMap<String, PointConfig>,
    /// 报警状态（point_id → 当前激活的报警类型；不存在表示未报警）
    alarm_state: Arc<Mutex<HashMap<String, AlarmType>>>,
    /// r33-ALM1: 去抖候选（point_id → 首次观测到超限的时刻）。
    /// 点持续超限 N 毫秒后才正式 trigger，存中间态。
    /// 正常范围时移除；trigger 成功后也移除（已转 alarm_state）。
    pending_since: Arc<Mutex<HashMap<String, Instant>>>,
    /// 报警上报端口（上报用），None=测试模式不上报
    ///
    /// 依赖倒置：`engine` 只认 [`AlarmPublisher`] 契约，不认 MQTT 实现——
    /// 引擎本身不应知道"发到 MQTT"这件事，将来接 Webhook / 短信时引擎不动。
    mqtt: Option<Arc<dyn AlarmPublisher>>,
    /// MQTT topic 前缀（报警发到 <topic>/alarm）
    mqtt_topic: String,
    /// 指标采集器（统计告警触发次数），None = 测试模式不统计
    metrics: Option<Arc<Metrics>>,
    /// r73-SD1：与 r61/r62 同型停机标志 + Notify。
    /// 旧实现 run 依赖 `RealtimeBus` closed 才退出，而 bus 的 Arc 被 bootstrap 的
    /// `NorthProcessing` 持有到进程结束 → 报警引擎在整个停机窗口内持续评估并向
    /// MQTT 上报。停机时 alarm_engine 已停（mqtt.stop() 是停机序最后一步），
    /// 上报失败 / 停机窗口内的告警仍会写入 pending outbox。
    stopping: Arc<std::sync::atomic::AtomicBool>,
    stop_notify: Arc<tokio::sync::Notify>,
}

impl AlarmEngine {
    /// 创建报警引擎
    ///
    /// - `points`: 所有点位配置（用于查找阈值）
    /// - `mqtt`: 报警上报端口（当前唯一实现是 MQTT，另接渠道时新增实现即可）
    /// - `mqtt_topic`: MQTT 数据 topic，报警发到 `<topic>/alarm`
    /// - `metrics`: 指标采集器（统计触发次数）；None = 测试模式不统计
    pub fn new(
        points: Vec<PointConfig>,
        mqtt: Option<Arc<dyn AlarmPublisher>>,
        mqtt_topic: String,
        metrics: Option<Arc<Metrics>>,
    ) -> Self {
        let mut map = HashMap::with_capacity(points.len());
        for pt in points {
            map.insert(pt.id.clone(), pt);
        }
        Self {
            points: map,
            alarm_state: Arc::new(Mutex::new(HashMap::new())),
            pending_since: Arc::new(Mutex::new(HashMap::new())),
            mqtt,
            mqtt_topic,
            metrics,
            // r73-SD1：停机标志 + Notify（参考 r61/r62）。
            stopping: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            stop_notify: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// r73-SD1：触发报警引擎 run 循环退出（设计参考 r48-MQTT1 / r61-SD1 / r72-SD1）。
    /// 先置位 stopping 再 `notify_waiters`，覆盖 `rx.recv().await` 在 wait 与 stop
    /// 之间的丢失唤醒窗口；唤醒后下一次 loop 检查 stopping 即退出。幂等。
    pub async fn stop(&self) {
        self.stopping
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.stop_notify.notify_waiters();
    }

    /// r73-SD1：暴露 stopping 标志供测试断言 stop() 已生效。
    #[cfg(test)]
    pub(crate) fn is_stopping_set(&self) -> bool {
        self.stopping.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 启动报警引擎：订阅 RealtimeBus，持续判断限值并上报
    pub async fn run(self: Arc<Self>, bus: Arc<RealtimeBus>) {
        let mut rx = bus.subscribe();
        tracing::info!("alarm engine started, topic={}/alarm", self.mqtt_topic);

        loop {
            // r73-SD1：停机标志 + Notify 双保险——避免 recv 与 stop 之间的丢失唤醒。
            if self.stopping.load(std::sync::atomic::Ordering::SeqCst) {
                tracing::info!("alarm engine: stop signalled, exiting");
                break;
            }
            let points = tokio::select! {
                biased;
                _ = self.stop_notify.notified() => break,
                r = rx.recv() => match r {
                    Ok(points) => points,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("alarm engine lagged {} messages", n);
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        tracing::info!("alarm engine: realtime bus closed, exiting");
                        break;
                    }
                },
            };
            for p in points.iter() {
                self.evaluate(p);
            }
        }
    }

    /// 评估单个点位的报警状态
    fn evaluate(&self, p: &DataPoint) {
        // 只对 Good 质量的数据做限值判断（Bad/Stale 不触发限值报警）
        if p.quality != Quality::Good {
            return;
        }
        // Bug r21-ALM2 (fix): NaN 守卫。
        // f64::NaN 与任何值比较均为 false（NaN > high、NaN < low 都 false），
        // 走不到 trigger_or_keep 也不会触发 recover——NaN 静默通过限值判断。
        // 上游链路（除零 / 设备数据损坏）可能产出 Good 质量的 NaN（pipeline
        // 转换未做 NaN→Bad 转换），此时不应当静默吞掉。
        // 决策：NaN 一律视为"未在正常范围"——若之前已报警，保持报警；
        // 未报警则**不触发新报警**（避免 NaN 风暴刷屏），
        // 但也不发 recover（避免错误地告知"已恢复"）。
        if p.value.is_nan() {
            return;
        }

        let cfg = match self.points.get(&p.point_id) {
            Some(c) => c,
            None => return, // 未配置的点位忽略
        };

        // 检查高限
        if let Some(high) = cfg.alarm_high {
            if p.value > high {
                self.trigger_or_keep(p, AlarmType::High, high, ALARM_DEBOUNCE_DEFAULT_MS);
                return; // 高限优先，已报警则不再检查低限
            }
        }

        // 检查低限
        if let Some(low) = cfg.alarm_low {
            if p.value < low {
                self.trigger_or_keep(p, AlarmType::Low, low, ALARM_DEBOUNCE_DEFAULT_MS);
                return;
            }
        }

        // r33-ALM1: 正常范围——清理去抖候选（防"瞬时跨阈又回落"残留）
        self.pending_since.lock().remove(&p.point_id);

        // 正常范围：若之前处于报警状态，发恢复通知（携带实际报警类型）
        // B4: 原实现两次 lock()（先 get 再 remove）。合并为单次 lock，
        // 减少锁竞争，避免 lock 中断时数据竞争误判。
        let prev_alarm = {
            let mut state = self.alarm_state.lock();
            let prev = state.get(&p.point_id).copied();
            if prev.is_some() {
                state.remove(&p.point_id);
            }
            prev
        };
        if let Some(alarm_type) = prev_alarm {
            self.send_recover(p, alarm_type);
        }
    }

    /// 触发报警（仅在从正常→超限时上报，避免重复）
    ///
    /// r33-ALM1: 加 `debounce_ms` 参数——点持续超限 N 毫秒才正式 trigger。
    /// 0 毫秒 = 向后兼容旧行为（瞬时超限立即触发）。
    /// 去抖中点存在 `pending_since`，正常范围时由 evaluate 清理。
    fn trigger_or_keep(
        &self,
        p: &DataPoint,
        alarm_type: AlarmType,
        threshold: f64,
        debounce_ms: u64,
    ) {
        let mut state = self.alarm_state.lock();
        let prev = state.get(&p.point_id).copied();
        match prev {
            None => {
                // 未报警 → 走去抖或立即触发
                if debounce_ms == 0 {
                    // 向后兼容：旧路径直接 trigger
                    state.insert(p.point_id.clone(), alarm_type);
                    drop(state);
                    self.send_alarm(p, alarm_type, threshold);
                } else {
                    drop(state);
                    let now = Instant::now();
                    let mut pending = self.pending_since.lock();
                    let entry = pending.entry(p.point_id.clone()).or_insert_with(|| now);
                    if now.duration_since(*entry).as_millis() as u64 >= debounce_ms {
                        // 持续超限时间已达阈值 → 正式 trigger
                        let p_id = p.point_id.clone();
                        pending.remove(&p_id);
                        drop(pending);
                        let mut state = self.alarm_state.lock();
                        state.insert(p_id, alarm_type);
                        drop(state);
                        self.send_alarm(p, alarm_type, threshold);
                    }
                    // 否则维持去抖候选，安静等待
                }
            }
            Some(prev_type) if prev_type != alarm_type => {
                // Bug 176: 报警类型变化（如高限→低限）——旧类型已结束、新类型刚开始。
                // 原实现仅更新存储类型，不发旧类 recover / 新类 trigger，订阅端会一直
                // 以为自己持有的还是旧类型 → 报警状态永久陈旧。现补发 recover(旧) 与
                // trigger(新)，顺序为先恢复旧类再触发新类。
                state.insert(p.point_id.clone(), alarm_type);
                drop(state);
                // 类型切换时旧去抖候选无效，清除
                self.pending_since.lock().remove(&p.point_id);
                self.send_recover(p, prev_type);
                self.send_alarm(p, alarm_type, threshold);
            }
            Some(_) => {
                // 同类报警持续中：清除去抖候选（已经正式 trigger 了）
                self.pending_since.lock().remove(&p.point_id);
            }
        }
    }

    /// 发送报警事件到 MQTT
    fn send_alarm(&self, p: &DataPoint, alarm_type: AlarmType, threshold: f64) {
        let symbol = match alarm_type {
            AlarmType::High => ">",
            AlarmType::Low => "<",
        };
        tracing::warn!(
            "ALARM trigger: {}={} {} threshold={}",
            p.point_id,
            p.value,
            symbol,
            threshold
        );
        let event = AlarmEvent {
            point_id: p.point_id.clone(),
            alarm_type,
            value: p.value,
            threshold,
            event: "trigger".to_string(),
            timestamp: p.timestamp,
        };
        // 统计触发次数（累计 + 当日），供 /api/metrics 与 Dashboard 展示
        if let Some(m) = &self.metrics {
            m.inc_alarm_trigger();
        }
        self.publish(&event);
    }

    /// 发送恢复通知到 MQTT
    fn send_recover(&self, p: &DataPoint, alarm_type: AlarmType) {
        let event = AlarmEvent {
            point_id: p.point_id.clone(),
            alarm_type, // Bug 42: 使用实际报警类型，而非硬编码 High
            value: p.value,
            threshold: 0.0,
            event: "recover".to_string(),
            timestamp: p.timestamp,
        };
        self.publish(&event);
        tracing::info!("ALARM recover: {}={}", p.point_id, p.value);
    }

    /// 序列化并发布到上报端口
    fn publish(&self, event: &AlarmEvent) {
        let mqtt = match &self.mqtt {
            Some(m) => m.clone(),
            None => return, // 测试模式：无上报端口，跳过上报
        };
        let topic = format!("{}/alarm", self.mqtt_topic);
        match serde_json::to_vec(event) {
            Ok(payload) => {
                crate::engine::supervision::spawn_catch_unwind("alarm_publish", async move {
                    mqtt.publish_alarm(&topic, payload).await;
                });
            }
            Err(e) => tracing::warn!("alarm event serialize failed: {}", e),
        }
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests;
