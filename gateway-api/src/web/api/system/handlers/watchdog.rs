//! 看门狗与自愈 API
//!
//! 聚合现有组件（进程 / MQTT / 采集调度 / 队列 / OTA / HA）的健康状态，
//! 提供服务列表、自愈事件记录、手动重启入口。
//!
//! 路由：
//! - GET  /api/watchdog/services              -> list_services
//! - GET  /api/watchdog/events                -> list_heal_events
//! - POST /api/watchdog/services/:name/restart -> restart_service

use super::*;
use crate::web::heal::{HealAction, HealEvent};
use gateway_core::engine::monitor::host;
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

/// 服务状态
#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub enum ServiceStatus {
    Running,
    Restarting,
    Stopped,
    Unknown,
}

/// 服务信息
#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ServiceInfo {
    pub name: String,
    pub status: ServiceStatus,
    pub pid: u32,
    pub uptime: u64,
    pub cpu: f32,
    pub mem: u64,
    pub restarts: u64,
}

/// 当前时间（秒级 Unix 时间戳），供自愈事件记录使用
fn now_sec() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// GET /api/watchdog/services — 服务健康列表
pub(crate) async fn list_services(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<ServiceInfo>>>, ApiError> {
    let pid = std::process::id();
    // P3-5：进程运行时长与 /api/diagnostics/gateway 同源（bootstrap 记录的启动时刻），
    // 不再用本模块自己的 OnceLock（首次访问时刻）作为起点
    let uptime = crate::web::uptime::process_uptime_secs();
    // P2-2：与 /api/status 共用 gateway_core::engine::monitor::host 的跨平台采集，
    // 不再依赖仅 Linux 存在的 /proc/stat（Windows 上恒为 0）
    let cpu = host::cpu_usage_pct_now().unwrap_or(0.0) as f32;
    let metrics = &state.platform.metrics;

    let mut services: Vec<ServiceInfo> = Vec::new();

    // 1. 主进程
    services.push(ServiceInfo {
        name: "gatewayd 主进程".to_string(),
        status: ServiceStatus::Running,
        pid,
        uptime,
        cpu,
        mem: 0,
        restarts: metrics.lifecyle.task_panics.load(Ordering::Relaxed),
    });

    // 2. MQTT 上行
    let (mqtt_status, mqtt_restarts) = match state.north.gateway.mqtt() {
        Some(m) => {
            let connected = m.is_connected();
            let circuit_open = m.is_circuit_open();
            let status = if connected {
                ServiceStatus::Running
            } else if circuit_open {
                ServiceStatus::Restarting
            } else {
                ServiceStatus::Stopped
            };
            (status, metrics.mqtt.publish_failure.load(Ordering::Relaxed))
        }
        None => (ServiceStatus::Unknown, 0),
    };
    services.push(ServiceInfo {
        name: "MQTT 上行".to_string(),
        status: mqtt_status,
        pid: 0,
        uptime,
        cpu: 0.0,
        mem: 0,
        restarts: mqtt_restarts,
    });

    // 3. 采集调度
    let devices = state.south.scheduler.device_status();
    let total = devices.len();
    let online = devices
        .iter()
        .filter(|d| d.quality == "good" || d.consecutive_failures == 0)
        .count();
    let collect_failures = metrics.collect.collect_failures.load(Ordering::Relaxed);
    let collect_status = if total > 0 && online == 0 {
        ServiceStatus::Stopped
    } else {
        ServiceStatus::Running
    };
    services.push(ServiceInfo {
        name: "采集调度".to_string(),
        status: collect_status,
        pid: 0,
        uptime,
        cpu: 0.0,
        mem: 0,
        restarts: collect_failures,
    });

    // 4. 数据队列（P0-1：与 /health/ready、/api/health/report 共用 queue_health）
    //
    // 旧实现在这里再写一遍 `queue_len < 100_000`，与 status.rs 的两处各自成一套
    // 口径：直连调试时 status 说 ok、watchdog 说 Restarting，运维无从判断。
    // 现在只映射健康等级 → 服务状态：预警（Backlog）仍算 Running，只有严重态
    // （背压 / 容量压力）才显示 Restarting。
    //
    // `has_north_consumer` 走 `NorthRefs::has_north_consumer()`（唯一口径），
    // 涵盖 MQTT publish 循环与内置模拟消费者；只看 MQTT 会漏掉后者。
    let queue_len = state.south.queue.len().await;
    let has_north_consumer = state.north.has_north_consumer();
    let max_queue_rows = state.config.config.load().max_queue_rows();
    let queue_health =
        crate::web::services::queue_health(queue_len as i64, max_queue_rows, has_north_consumer);
    let queue_status = if queue_health.is_ready() {
        ServiceStatus::Running
    } else {
        ServiceStatus::Restarting
    };
    services.push(ServiceInfo {
        name: "数据队列".to_string(),
        status: queue_status,
        pid: 0,
        uptime,
        cpu: 0.0,
        mem: 0,
        restarts: metrics.lifecyle.watchdog_timeouts.load(Ordering::Relaxed),
    });

    // 5. OTA
    let ota_info = state.north.ota.status();
    let ota_status = match ota_info.state {
        crate::web::traits::OtaStateView::Idle => ServiceStatus::Running,
        crate::web::traits::OtaStateView::PendingReboot => ServiceStatus::Restarting,
        _ => ServiceStatus::Unknown,
    };
    services.push(ServiceInfo {
        name: "OTA 升级".to_string(),
        status: ota_status,
        pid: 0,
        uptime,
        cpu: 0.0,
        mem: 0,
        restarts: metrics.lifecyle.config_reload_total.load(Ordering::Relaxed),
    });

    Ok(Json(ApiResponse::ok(services)))
}

/// GET /api/watchdog/events — 自愈事件列表
///
/// 返回由真实执行路径（手动重启入口 / 自愈执行器）记录的事件，最新在前。
/// 尚未执行过任何自愈动作时为空列表。
pub(crate) async fn list_heal_events(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<HealEvent>>>, ApiError> {
    Ok(Json(ApiResponse::ok(state.platform.heal_events.recent(50))))
}

/// POST /api/watchdog/services/:name/restart — 手动重启服务
///
/// 通过统一配置重载入口（`config_reload_lock` + `reload_then_save`）触发一次热重载。
/// 重启 MQTT / 采集调度 / 北向等内部服务。主进程本身无法通过 API 重启
/// （需外部 systemd / 看门狗进程）。与自愈执行器的 `RestartService` 共用同一实现。
pub(crate) async fn restart_service(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let detail = format!("restart service: {}", name);
    match crate::web::heal::reload_services(&state, "api", &detail).await {
        Ok(()) => {
            record_heal_event(&state, &name, "success", None);
            Ok(Json(ApiResponse::ok(())))
        }
        Err(e) => {
            record_heal_event(&state, &name, "failed", Some(e.message.clone()));
            Err(e)
        }
    }
}

/// 记录一条真实的自愈事件（手动重启入口与后续自愈执行器共用）。
fn record_heal_event(state: &AppState, target: &str, result: &str, detail: Option<String>) {
    let now = now_sec();
    state.platform.heal_events.record(HealEvent {
        id: crate::web::heal::next_heal_event_id(now, &HealAction::RestartService, target),
        time: now,
        action: HealAction::RestartService,
        target: target.to_string(),
        reason: "manual restart".to_string(),
        result: result.to_string(),
        detail,
    });
}
