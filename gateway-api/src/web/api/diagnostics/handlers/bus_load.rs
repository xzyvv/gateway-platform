//! 总线负载率监测 handler
//!
//! 数据来自 [`ScopedCollectMetrics`] 的真实累计计数：采集循环每完成一次读取，
//! 就按 `point.bus`（= `BusConfig.port_name`）累加次数、成功次数与耗时。
//!
//! 速率与错误率是**区间量**，必须两次采样求差才能得到：
//! - 每次请求读取一次累计值，与上次请求的累计值求差 → 包/秒、错误率、平均延迟；
//! - 首次请求（或计数器重置后）没有上一帧，速率类字段返回 `null`（前端渲染「—」），
//!   绝不伪造；
//! - 历史曲线保存的是**真实采样点**（按请求时刻采样，最多 31 点），不补零、不插值。
//!
//! `backlog` 恒为 `null`：队列是全局单队列，无法按总线拆分，此前是按 hash 生成假数。

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, OnceLock};

use axum::extract::{Path, State};
use axum::Json;
use parking_lot::Mutex;
use serde::Serialize;
use ts_rs::TS;

use gateway_core::engine::metrics::ScopeStats;

use crate::web::types::{ApiError, ApiResponse, AppState};

/// 总线负载历史采样点（折线图用）
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct BusLoadHistory {
    /// 时间戳（毫秒）
    #[ts(type = "number")]
    pub time_ms: i64,
    /// 该时刻发送速率（包/秒）
    pub tx: i32,
    /// 该时刻接收速率（包/秒）
    pub rx: i32,
}

/// 单条总线的负载统计
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct BusLoad {
    /// 总线标识 = `BusConfig.port_name`（与点位、采集引擎口径一致）
    pub bus: String,
    /// 协议类型（modbus/dlt645/cjt188/...）
    pub bus_type: String,
    /// 发送速率（包/秒）；首帧无基线时为 null
    pub tx_rate: Option<f64>,
    /// 接收速率（成功应答包/秒）；首帧无基线时为 null
    pub rx_rate: Option<f64>,
    /// 错误率（%）= 区间内失败次数 / 总次数；区间内无采集为 null
    pub collision_rate: Option<f64>,
    /// 平均响应延迟（ms）；区间内无采集为 null
    pub avg_latency_ms: Option<i32>,
    /// 积压请求数：全局队列无法按总线拆分，恒为 null
    pub backlog: Option<i32>,
    /// 利用率（%）= 平均延迟 × 发送速率（总线忙碌时间占比）
    pub utilization: Option<f64>,
    /// 真实采样点（最多 31 个）
    pub history: Vec<BusLoadHistory>,
}

/// 累计计数快照（自进程启动以来，取全部日桶）
#[derive(Debug, Clone, Copy, Default)]
struct Cumulative {
    total: u64,
    success: u64,
    failed: u64,
    duration_sum_us: u64,
}

impl Cumulative {
    fn from_stats(s: &ScopeStats) -> Self {
        Self {
            total: s.total,
            success: s.success,
            failed: s.failed,
            duration_sum_us: s.duration_sum_us,
        }
    }
}

/// 单条总线的采样状态（上一次累计值 + 历史速率点）
#[derive(Default)]
struct BusSampler {
    last: Option<(i64, Cumulative)>,
    history: VecDeque<BusLoadHistory>,
}

/// 采样器全局表。放在 handler 侧而非 core：负载曲线是「被观测时才采样」的展示数据，
/// 不属于采集引擎的业务状态。
fn samplers() -> &'static Mutex<HashMap<String, BusSampler>> {
    static SAMPLERS: OnceLock<Mutex<HashMap<String, BusSampler>>> = OnceLock::new();
    SAMPLERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 区间计算结果
#[derive(Debug, Clone, Default)]
struct Interval {
    tx_rate: Option<f64>,
    rx_rate: Option<f64>,
    collision_rate: Option<f64>,
    avg_latency_ms: Option<i32>,
}

/// 用累计值差计算区间速率，并维护历史曲线
fn sample(key: &str, stats: &ScopeStats, now_ms: i64) -> (Interval, Vec<BusLoadHistory>) {
    let cur = Cumulative::from_stats(stats);
    let mut guard = samplers().lock();
    let sampler = guard.entry(key.to_string()).or_default();

    let mut interval = Interval::default();
    if let Some((prev_ms, prev)) = sampler.last {
        let elapsed_s = (now_ms - prev_ms) as f64 / 1000.0;
        if elapsed_s > 0.0 {
            let d_total = cur.total.saturating_sub(prev.total);
            let d_success = cur.success.saturating_sub(prev.success);
            let d_failed = cur.failed.saturating_sub(prev.failed);
            let d_us = cur.duration_sum_us.saturating_sub(prev.duration_sum_us);

            interval.tx_rate = Some(d_total as f64 / elapsed_s);
            interval.rx_rate = Some(d_success as f64 / elapsed_s);
            if d_total > 0 {
                interval.collision_rate = Some(d_failed as f64 / d_total as f64 * 100.0);
                interval.avg_latency_ms =
                    Some((d_us as f64 / d_total as f64 / 1000.0).round() as i32);
            }
            // 只有区间内确有采集才落一个真实点，避免空档期画出 0
            if d_total > 0 {
                sampler.history.push_back(BusLoadHistory {
                    time_ms: now_ms,
                    tx: (d_total as f64 / elapsed_s).round() as i32,
                    rx: (d_success as f64 / elapsed_s).round() as i32,
                });
                while sampler.history.len() > 31 {
                    sampler.history.pop_front();
                }
            }
        }
    }
    sampler.last = Some((now_ms, cur));

    (interval, sampler.history.iter().cloned().collect())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn build_bus_load(port_name: &str, bus_type: &str, stats: &ScopeStats, now: i64) -> BusLoad {
    let (interval, history) = sample(port_name, stats, now);
    // 总线忙碌占比 ≈ 平均延迟 × 请求速率（Little 定律的占空比形式）
    let utilization = match (interval.avg_latency_ms, interval.tx_rate) {
        (Some(latency), Some(rate)) => Some((latency as f64 * rate / 10.0).clamp(0.0, 100.0)),
        _ => None,
    };
    BusLoad {
        // BUG-G：总线标识统一用 port_name（= 采集引擎与点位里的 `bus`）。
        // 原先列表输出 tcp_addr、查询却按两种口径找，用户拿列表里的值去查必然 404。
        bus: port_name.to_string(),
        bus_type: bus_type.to_string(),
        tx_rate: interval.tx_rate,
        rx_rate: interval.rx_rate,
        collision_rate: interval.collision_rate,
        avg_latency_ms: interval.avg_latency_ms,
        backlog: None,
        utilization,
        history,
    }
}

pub(crate) async fn list_bus_loads(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<BusLoad>>>, ApiError> {
    let cfg = state.config.load_full();
    let snapshot = state.platform.metrics.scoped.bus_snapshot(0);
    let now = now_ms();
    let list: Vec<BusLoad> = cfg
        .buses
        .iter()
        .map(|b| {
            let stats = snapshot.get(&b.port_name).copied().unwrap_or_default();
            build_bus_load(&b.port_name, &b.protocol, &stats, now)
        })
        .collect();
    Ok(Json(ApiResponse::ok(list)))
}

pub(crate) async fn get_bus_load(
    State(state): State<Arc<AppState>>,
    Path(bus): Path<String>,
) -> Result<Json<ApiResponse<BusLoad>>, ApiError> {
    let cfg = state.config.load_full();
    let b = cfg
        .buses
        .iter()
        .find(|x| x.port_name == bus)
        .ok_or_else(|| ApiError::not_found(format!("bus not found: {}", bus)))?;
    let stats = state
        .platform
        .metrics
        .scoped
        .bus_snapshot(0)
        .get(&b.port_name)
        .copied()
        .unwrap_or_default();
    Ok(Json(ApiResponse::ok(build_bus_load(
        &b.port_name,
        &b.protocol,
        &stats,
        now_ms(),
    ))))
}

pub(crate) async fn refresh_bus_loads(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<BusLoad>>>, ApiError> {
    // 与 list_bus_loads 一致：每次请求都会与上次累计值求差，调用即刷新
    list_bus_loads(State(state)).await
}
