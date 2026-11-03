//! 设备 API 共享数据模型
//!
//! 从原 handlers_ops.rs 抽出的所有响应 / 请求结构体，集中管理便于 ts-rs 类型导出。

// ============================================================
// 远程控制（点位写入）
// ============================================================

#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct WritablePoint {
    pub id: String,
    pub name: String,
    pub data_type: String,
    pub device_id: String,
    pub device_name: String,
}

#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ControlCommand {
    pub id: String,
    pub device_id: String,
    pub device_name: String,
    pub point_id: String,
    pub point_name: String,
    pub write_value: f64,
    pub operator: String,
    pub status: String,
    pub created_at: i64,
    pub executed_at: Option<i64>,
    pub result: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct SendCommandRequest {
    pub point_id: String,
    pub value: f64,
}

// ============================================================
// 设备拓扑图
// ============================================================

#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct TopoNode {
    pub id: String,
    pub name: String,
    pub node_type: String,
    pub status: Option<String>,
    pub meta: Option<String>,
}

#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct TopoLink {
    pub source: String,
    pub target: String,
}

#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct TopologyData {
    pub nodes: Vec<TopoNode>,
    pub links: Vec<TopoLink>,
    /// 拓扑聚合统计（原独立端点 `/api/devices/topology/stats` 的内容，
    /// 与本响应同源同判据）
    pub stats: TopoStats,
}

#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct TopoStats {
    pub bus_count: usize,
    pub device_count: usize,
    pub online_count: usize,
    pub offline_count: usize,
    pub warning_count: usize,
    pub point_count: usize,
}

// ============================================================
// 点位数据质量
// ============================================================

#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct PointQuality {
    pub point_id: String,
    pub point_name: String,
    pub device_id: String,
    pub device_name: String,
    pub value: Option<f64>,
    pub quality: String,
    pub consecutive_failures: u32,
    pub last_update_at: i64,
}

#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct QualityStats {
    pub good: u32,
    pub bad: u32,
    pub stale: u32,
    pub unknown: u32,
    pub total: u32,
}

// ============================================================
// 采集退避状态（§7.3 退避策略可视化）
// ============================================================

/// 单点位采集退避状态
///
/// 数据来源：Scheduler device_states（§3.9 设备退避状态）。
/// backoff_multiplier 由前端用 current/base 推算也行，这里直接给出便于展示。
#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct PointBackoff {
    pub point_id: String,
    pub point_name: String,
    pub device_id: String,
    pub device_name: String,
    pub bus: String,
    /// 配置的基础采集间隔（毫秒）
    pub base_interval_ms: u64,
    /// 当前实际采集间隔（退避时放大，毫秒）
    pub current_interval_ms: u64,
    /// 退避倍率 = current / base（1 表示未退避）
    pub backoff_multiplier: u32,
    /// 连续失败次数（>=3 进入退避）
    pub consecutive_failures: u32,
    /// 连续重发次数（失败期间用 last_good 值重发的次数）
    pub stale_count: u32,
    /// 上次采集成功的值
    pub last_good_value: Option<f64>,
    /// 上次采集成功时间戳（毫秒，0 表示从未成功）
    pub last_good_ts: i64,
    /// 当前质量码（good/stale/bad）
    pub quality: String,
    /// 是否处于退避中（consecutive_failures >= 3）
    pub backing_off: bool,
}
