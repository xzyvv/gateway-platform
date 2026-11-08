//! 磁盘水位分级告警：四档判定 + 档位变化时的主动外呼。
//!
//! 此前磁盘只有**一档**：剩余空间低于 10% 就阻断入队（保护 SQLite 不被写坏），
//! 低优先级点随之被丢弃。问题在于**触顶前没有任何信号**——现场看到的只是
//! "数据没上来"，运维无法区分是设备离线、协议不通还是磁盘满了。
//!
//! 本模块把水位分成四档，让"写满前几天"就变成可观测事实：
//!
//! | 档位 | 剩余空间 | 含义 |
//! |------|----------|------|
//! | `ok` | ≥ 25% | 正常 |
//! | `warn` | 15% ~ 25% | 预警：还能写，但需要安排清理/归档 |
//! | `critical` | 10% ~ 15% | 严重：接近阻断线，应尽快处理 |
//! | `blocking` | < 10% | 阻断：入队已被拒绝，低优先级点正在被丢弃 |
//!
//! 10% 这一档沿用 `health::DISK_FREE_THRESHOLD_PCT`（阻断语义完全不变，
//! 本模块只增强**告警**，不改**阻断**）。
//!
//! # 单一真源：档位存放在 DiskStatus
//!
//! 当前档位由 [`super::DiskStatus`] 持有（`AtomicU8`），而不是由本模块的
//! notifier 另存一份。两个来源必然出现分叉：API 读到"ok"而告警刚发过
//! "critical"，现场会看到自相矛盾的信号。故本模块只做**纯函数判定**与
//! **外呼**，档位状态一律读写 `DiskStatus`。
//!
//! # 与点级报警的语义分离
//!
//! 点级报警（`engine::alarm::AlarmEvent`）有 `point_id` / `alarm_type` / `value`，
//! 描述"某个测点的值越限"。磁盘是**系统级**事件，没有任何测点，
//! 故单列载荷 [`DiskAlarmEvent`] 与独立 topic（`<mqtt_topic>/system/alarm`）。
//!
//! # 外呼契约
//!
//! 通过 [`AlarmPublisher`] 端口外呼（当前唯一实现是 MQTT），因此
//! `engine` 侧依旧不感知 MQTT。档位**变化时**才发（不变化不重复刷屏），
//! 与队列积压告警的边沿判定同型。

use std::sync::Arc;

use serde::Serialize;

use crate::engine::ports::AlarmPublisher;

/// 预警线：剩余空间低于该值进入 `warn` 档（仍能正常写入，需安排清理/归档）。
pub const DISK_WARN_THRESHOLD_PCT: f64 = 25.0;

/// 严重线：剩余空间低于该值进入 `critical` 档（接近阻断线，应尽快处理）。
pub const DISK_CRITICAL_THRESHOLD_PCT: f64 = 15.0;

/// 阻断线：剩余空间低于该值时入队被阻断。
///
/// 直接引用 `health` 模块的同一常量，避免"告警阈值"与"阻断阈值"两处漂移
/// ——一旦漂移，运维会收到"磁盘正常但数据在丢"或"磁盘告警但没阻断"的矛盾信号。
pub const DISK_BLOCKING_THRESHOLD_PCT: f64 = super::DISK_FREE_THRESHOLD_PCT;

/// 磁盘水位档位。派生顺序即严重度顺序（`Ok < Warn < Critical < Blocking`），
/// 故可直接用 `cmp` 判断"变严重了"还是"回落了"。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DiskLevel {
    /// 正常（≥ 25%）。
    Ok,
    /// 预警（15% ~ 25%）。
    Warn,
    /// 严重（10% ~ 15%）。
    Critical,
    /// 阻断（< 10%，入队已被拒绝）。
    Blocking,
}

impl DiskLevel {
    /// 档位的中文/英文展示名（与载荷中的 `level` 字段一致）。
    pub fn as_str(self) -> &'static str {
        match self {
            DiskLevel::Ok => "ok",
            DiskLevel::Warn => "warn",
            DiskLevel::Critical => "critical",
            DiskLevel::Blocking => "blocking",
        }
    }

    /// 该档位对应的阈值百分比（`ok` 档返回预警线，即"回到该线以上即视为正常"）。
    pub fn threshold_pct(self) -> f64 {
        match self {
            DiskLevel::Ok => DISK_WARN_THRESHOLD_PCT,
            DiskLevel::Warn => DISK_WARN_THRESHOLD_PCT,
            DiskLevel::Critical => DISK_CRITICAL_THRESHOLD_PCT,
            DiskLevel::Blocking => DISK_BLOCKING_THRESHOLD_PCT,
        }
    }

    /// 数值编码（`Ok=0 / Warn=1 / Critical=2 / Blocking=3`），供指标与前端使用。
    pub fn code(self) -> i64 {
        match self {
            DiskLevel::Ok => 0,
            DiskLevel::Warn => 1,
            DiskLevel::Critical => 2,
            DiskLevel::Blocking => 3,
        }
    }

    /// 编码为 `u8`，供 [`super::DiskStatus`] 的 `AtomicU8` 存档位使用。
    ///
    /// 与 [`Self::code`] 同源：编码只保留一个真源，调用方不再各自 `as u8`。
    pub fn as_u8(self) -> u8 {
        self.code() as u8
    }

    /// 从指标层 `i64` 还原档位（越界或负值按最严重的 `Blocking` 处理）。
    ///
    /// `metrics.disk_level` 以 `i64` 原子量承载 Prometheus gauge 编码，
    /// 读取侧统一走这里，避免各调用点自己 `as u8` 再转一次。
    pub fn from_i64(v: i64) -> DiskLevel {
        u8::try_from(v)
            .map(DiskLevel::from_u8)
            .unwrap_or(DiskLevel::Blocking)
    }

    /// 从 `u8` 编码还原档位（越界值按最严重的 `Blocking` 处理，保守优先）。
    ///
    /// 编码约定与枚举声明顺序一致，供 [`super::DiskStatus`] 用单个
    /// `AtomicU8` 存档位（免 `Mutex`）。
    pub fn from_u8(v: u8) -> DiskLevel {
        match v {
            0 => DiskLevel::Ok,
            1 => DiskLevel::Warn,
            2 => DiskLevel::Critical,
            _ => DiskLevel::Blocking,
        }
    }
}

/// 由剩余空间百分比推导水位档位。
///
/// 边界与 `health::disk_ok_from_free_pct` 保持一致：恰好 10% **不算**阻断
/// （含入 ok 侧），因为入队放行判定用的是 `>= 10.0`。
///
/// NaN（采样异常）按最保守的 `Blocking` 处理——宁可误报一次阻断级告警，
/// 也不能在数据正在丢失时还报"正常"。
pub fn disk_level_from_free_pct(free_pct: f64) -> DiskLevel {
    if free_pct.is_nan() {
        return DiskLevel::Blocking;
    }
    if free_pct < DISK_BLOCKING_THRESHOLD_PCT {
        DiskLevel::Blocking
    } else if free_pct < DISK_CRITICAL_THRESHOLD_PCT {
        DiskLevel::Critical
    } else if free_pct < DISK_WARN_THRESHOLD_PCT {
        DiskLevel::Warn
    } else {
        DiskLevel::Ok
    }
}

/// 系统级磁盘告警载荷（MQTT topic：`<mqtt_topic>/system/alarm`）。
///
/// 与点级 `AlarmEvent` 分开：这里没有 `point_id` / `alarm_type`，
/// 描述的是"网关自身的数据落盘水位"。
#[derive(Debug, Clone, Serialize)]
pub struct DiskAlarmEvent {
    /// `trigger` = 水位恶化进入更严档；`recover` = 回落到更宽松档。
    pub event: String,
    /// 变化后的档位：`ok` / `warn` / `critical` / `blocking`。
    pub level: String,
    /// 变化前的档位。
    pub prev_level: String,
    /// 当前剩余空间百分比。
    pub free_pct: f64,
    /// 该档位对应的阈值百分比。
    pub threshold_pct: f64,
    /// 中文说明（现场可直接照做的描述）。
    pub message: String,
    /// UTC 毫秒时间戳。
    pub timestamp: i64,
}

/// 构造一次档位变化事件；档位未变返回 `None`（边沿判定）。
///
/// 纯函数：便于在不触碰任何共享状态的前提下把边界语义钉死在单测里。
pub fn disk_alarm_event(
    prev: DiskLevel,
    level: DiskLevel,
    free_pct: f64,
) -> Option<DiskAlarmEvent> {
    if prev == level {
        return None;
    }
    let worsened = level > prev;
    Some(DiskAlarmEvent {
        event: if worsened { "trigger" } else { "recover" }.to_string(),
        level: level.as_str().to_string(),
        prev_level: prev.as_str().to_string(),
        free_pct,
        threshold_pct: level.threshold_pct(),
        message: describe(level, free_pct, worsened),
        timestamp: chrono::Utc::now().timestamp_millis(),
    })
}

/// 磁盘水位告警外呼器。
///
/// 只负责"把已构造好的事件发出去"：档位判定与状态保存在
/// [`super::DiskStatus`] 与 [`disk_alarm_event`]，本结构不持有副本。
///
/// 无 `AlarmPublisher`（未配置 MQTT）时仍可正常判定档位，只是不外呼。
pub struct DiskAlarmNotifier {
    /// 报警上报端口（`None` = 未配置上报通道）。
    publisher: Option<Arc<dyn AlarmPublisher>>,
    /// 上报 topic（已含 `/system/alarm` 后缀）。
    topic: String,
}

impl DiskAlarmNotifier {
    /// 创建外呼器。
    ///
    /// - `publisher`: 报警上报端口；`None` 时只判定不外呼。
    /// - `mqtt_topic`: 数据 topic 前缀，上报到 `<mqtt_topic>/system/alarm`。
    pub fn new(publisher: Option<Arc<dyn AlarmPublisher>>, mqtt_topic: &str) -> Self {
        Self {
            publisher,
            topic: format!("{}/system/alarm", mqtt_topic),
        }
    }

    /// 上报 topic（供测试与诊断查看）。
    pub fn topic(&self) -> &str {
        &self.topic
    }

    /// 主动外呼一条已构造好的告警事件。
    ///
    /// 走独立监管任务（`spawn_catch_unwind`）：MQTT 未连接 / 断路器打开时
    /// 发布会被直接丢弃（告警允许偶尔丢失），但**不能**因此拖住磁盘监控循环。
    pub fn publish(&self, event: &DiskAlarmEvent) {
        let Some(publisher) = self.publisher.clone() else {
            return;
        };
        let payload = match serde_json::to_vec(event) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("disk alarm event serialize failed: {}", e);
                return;
            }
        };
        let topic = self.topic.clone();
        crate::engine::supervision::spawn_catch_unwind("disk_alarm_publish", async move {
            publisher.publish_alarm(&topic, payload).await;
        });
    }
}

/// 一次磁盘采样后的判定结果（供监控循环决定打什么日志）。
pub struct DiskTick {
    /// 本次采样后的档位。
    pub level: DiskLevel,
    /// 是否处于入队阻断状态（`free_pct < 10%`）。
    pub ok: bool,
    /// 阻断状态是否发生翻转（用于"仅变化时打日志"）。
    pub pressure_changed: bool,
}

/// 一次磁盘采样的完整处理：判定档位 → 记录状态 → 变化时外呼。
///
/// 由磁盘监控循环调用。`DiskStatus` 既是档位的**唯一存放点**，也是 API
/// 读取当前档位的来源，故此处先读旧档位、记录新档位，再比较二者。
pub fn tick(
    notifier: Option<&DiskAlarmNotifier>,
    disk_status: &super::DiskStatus,
    free_pct: f64,
    metrics: &crate::engine::metrics::Metrics,
) -> DiskTick {
    let prev = disk_status.level();
    let level = disk_level_from_free_pct(free_pct);
    let ok = super::disk_ok_from_free_pct(free_pct);
    let pressure_changed = disk_status.record(free_pct, !ok);
    metrics.set_disk_level(level);
    if let Some(event) = disk_alarm_event(prev, level, free_pct) {
        if event.event == "trigger" {
            metrics.inc_disk_alarm_trigger();
        }
        if let Some(notifier) = notifier {
            notifier.publish(&event);
        }
    }
    DiskTick {
        level,
        ok,
        pressure_changed,
    }
}

/// 生成中文说明文案。
fn describe(level: DiskLevel, free_pct: f64, worsened: bool) -> String {
    let head = format!("磁盘剩余 {:.1}%", free_pct);
    match (level, worsened) {
        (DiskLevel::Warn, true) => format!("{head}：低于预警线，需清理或归档历史数据"),
        (DiskLevel::Critical, true) => format!("{head}：低于严重线，接近阻断线"),
        (DiskLevel::Blocking, true) => format!("{head}：入队已阻断，低优先级采集点被丢弃"),
        (DiskLevel::Ok, _) => format!("{head}：已恢复正常"),
        (DiskLevel::Warn, false) => format!("{head}：已从严重线回落至预警线"),
        (DiskLevel::Critical, false) => format!("{head}：已从阻断线回落，恢复写入"),
        (DiskLevel::Blocking, false) => format!("{head}：已从阻断线回落"),
    }
}

#[cfg(test)]
mod tests;
