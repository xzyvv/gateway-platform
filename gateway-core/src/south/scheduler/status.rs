//! 点位运行态快照（强类型）
//!
//! 此前 `Scheduler::device_status()` 返回 `Vec<serde_json::Value>`：调度器把状态
//! 序列化成 JSON 文本、各消费方再逐字段 `as_str()` / `as_u64()` 反解回来。
//! 这条「强类型 → JSON → 强类型」的中转有三个实际代价：
//! - 每个消费点重复做一次字符串解析（点位数 × 消费点数）；
//! - 类型信息在 JSON 里丢失，`as_u64().unwrap_or(0)` 之类的兜底会静默吞掉异常；
//! - 字段名拼错只能在运行时暴露，编译期无感知。
//!
//! 现在调度器直接产出强类型行，序列化只发生在 HTTP 响应边界（`serde` 派生），
//! 消费方按字段直接读取。字段名与历史 JSON 完全一致，HTTP 契约不变。

use serde::Serialize;

/// 单个点位（模板展开后的采集点）的运行态快照。
///
/// 一行 = 一个**点位**（不是一台设备）：一台 4 点位的设备会产出 4 行。
/// 设备级在线数请按 `device_id` 去重后统计。
#[derive(Debug, Clone, Default, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct DeviceStatus {
    /// 点位 ID（模板展开点位为 `{设备名}_{参数名}`）
    pub point_id: String,
    /// 所属总线名（自愈 ResetSerial 定位设备所在总线）
    pub bus: String,
    /// 所属设备 ID（模板展开点位也带此字段，是点位归属的真值源）
    pub device_id: String,
    /// 连续失败次数（>= 3 进入指数退避）
    pub consecutive_failures: u32,
    /// 当前实际采集间隔（毫秒；退避时相对基础间隔放大）
    #[ts(type = "number")]
    pub current_interval_ms: u64,
    /// 基础采集间隔（毫秒；配置值，恢复时还原）
    #[ts(type = "number")]
    pub base_interval_ms: u64,
    /// 连续重发次数（失败期间用上次成功值重发的次数）
    pub stale_count: u32,
    /// 离线次数（「正常 → 首次失败」的累计次数，成功不归零）
    pub offline_episodes: u32,
    /// 当前离线段的起始时间戳（毫秒；0 = 不在离线段）
    #[ts(type = "number")]
    pub offline_since_ts: i64,
    /// 已结束离线段的累计时长（毫秒）
    #[ts(type = "number")]
    pub total_downtime_ms: i64,
    /// 状态创建时间戳（毫秒；统计窗口起点）
    #[ts(type = "number")]
    pub tracking_since_ts: i64,
    /// 上次成功采集的原始时间戳（毫秒；0 表示无成功历史）
    #[ts(type = "number")]
    pub last_good_ts: i64,
    /// 上次成功采集的值；从未成功为 null
    pub last_good_value: Option<f64>,
    /// 当前质量码：good / stale / bad
    pub quality: String,
}
