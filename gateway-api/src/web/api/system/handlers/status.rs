//! 系统 API 处理函数（从 system.rs 拆分）
use super::*;
use crate::web::services::{
    capacity_pressure_rows, queue_health, QueueHealth, BACKLOG_WARN_ROWS, BACKPRESSURE_ROWS,
};
use gateway_core::engine::monitor::health::{
    DiskLevel, DISK_BLOCKING_THRESHOLD_PCT, DISK_CRITICAL_THRESHOLD_PCT, DISK_WARN_THRESHOLD_PCT,
};
use gateway_core::engine::monitor::host;
use std::sync::atomic::Ordering;

pub(crate) async fn health_live() -> impl IntoResponse {
    // 局域网自动发现场景：浏览器从任意 origin 访问 /health/live。
    // 不能受 web.cors_origins 配置限制。健康检查本身不携带凭据。
    // 显式放行 CORS，body 仍照常返回。
    (
        StatusCode::OK,
        [(axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")],
        Json(serde_json::json!({
            "status": "ok",
            "service": "gateway-platform",
            "discoverable": true,
        })),
    )
}

/// 组件健康快照（`/health/ready` 与 `/api/health/report` 共用）
///
/// 抽出来的理由：两个接口都要"process / mqtt / queue / ota / storage"这套口径，
/// 分开各写一份必然在后续改动中分叉（一个改了阈值另一个没改）。
pub(crate) struct ComponentSnapshot {
    pub components: std::collections::BTreeMap<&'static str, &'static str>,
    pub queue_len: usize,
    /// 当前生效的队列表行数硬上限（配置项 `queue.max_rows`，运行期现取）。
    pub max_queue_rows: i64,
    pub ota_state: String,
    /// 是否注入了 MQTT 客户端（= 配置里启用了 MQTT）
    pub mqtt_configured: bool,
    pub mqtt_connected: bool,
    /// 是否存在会消费队列的北向消费者（MQTT publish 循环 或 内置模拟消费者）。
    ///
    /// 与 `mqtt_configured` 是**两个语义**：后者只回答「MQTT 通了吗」，
    /// 前者回答「队列里的数据有没有人会取走」——模拟消费者运行期启停，
    /// 但它同样会 dequeue → ack，队列存量因此是「积压」而非「本地历史存量」。
    pub has_north_consumer: bool,
    /// 磁盘水位档位（分级告警的唯一真源，见 `engine::monitor::health::DiskStatus`）。
    pub disk_level: DiskLevel,
    /// 最近一次检测到的磁盘剩余百分比（未检测过时为 None）。
    pub disk_free_pct: Option<f64>,
    /// 累计因队列容量上限被物理删除的行数（进程内累计，重启归零）。
    ///
    /// 单机网关无异地副本，被淘汰的行不可恢复。此前只打 debug 日志，
    /// 现场表现为「数据一直在丢，界面一直安静」；现作为健康快照的一部分
    /// 对外暴露，供 `/health/ready`、`/api/health/report` 与前端红标使用。
    pub evicted_rows: i64,
    /// 上述淘汰中未送达（delivered=0）的行数 —— 彻底的数据丢失。
    pub evicted_undelivered_rows: i64,
    /// 死信表当前行数（Webhook 上报被永久拒绝 / 重试用尽、已保留但未送达的批次）。
    ///
    /// 与 `evicted_rows` 的区别：淘汰是**已销毁**，死信是**仍保留、可重投**，
    /// 因此这里只报 warning，不报 critical；但绝不能不报 —— 静默积压等于换个
    /// 名字的静默丢数据。
    pub dead_letter_rows: i64,
    /// 死信库是否可用（outbox 库打不开时为 false）。
    ///
    /// 必须与 `dead_letter_rows == 0` 区分：不可用时行数无意义，
    /// 报「0 条」会让现场误判成「没有未送达数据」。
    pub dead_letter_available: bool,
}

pub(crate) async fn collect_components(state: &Arc<AppState>) -> ComponentSnapshot {
    use std::collections::BTreeMap;
    let mut components: BTreeMap<&'static str, &'static str> = BTreeMap::new();
    components.insert("process", "ok");

    // MQTT 状态：未注入 client 视为未配置 → ok（与历史 health_ready 口径一致）
    let mqtt = state.north.gateway.mqtt();
    let mqtt_configured = mqtt.is_some();
    let mqtt_connected = mqtt.map(|m| m.is_connected()).unwrap_or(false);
    components.insert(
        "mqtt",
        if mqtt_configured && !mqtt_connected {
            "down"
        } else {
            "ok"
        },
    );

    // 队列积压状态（P0-1：唯一口径 = gateway_core::queue::queue_health，经 services 门面）
    //
    // 队列同时充当本地历史库：只有北向消费者（MQTT publish 循环 / 内置模拟消费者）
    // 会在处理成功后 ack 删除行。没有任何消费者时长度只受历史保留天数
    // （固定 30 天，与 license 无关）约束，增长属于历史的正常存量而非积压；但「逼近硬上限」与消费者无关，
    // 必须降级。
    // 旧实现在本文件里硬编码 `queue_len < 100_000`，与 watchdog.rs / 告警任务各写
    // 一套阈值，同一队列长度会同时得到 ok / critical / Restarting 三种互斥结论。
    //
    // `has_north_consumer` 取 `NorthRefs::has_north_consumer()`（唯一口径），
    // 不能用 `mqtt_configured` 代替：内置模拟消费者没有 MQTT 客户端，
    // 但它照样消费队列。
    let queue_len = state.south.queue.len().await;
    let has_north_consumer = state.north.has_north_consumer();
    // 行数上限是配置项，运行期现取：改配置后水位判定立即跟着走
    let max_queue_rows = state.config.config.load().max_queue_rows();
    let queue_health = queue_health(queue_len as i64, max_queue_rows, has_north_consumer);
    components.insert("queue", queue_health.label());

    // OTA 状态：PendingReboot 视为 degraded（切流会丢点）
    let ota_info = state.north.ota.status();
    let ota_state = format!("{:?}", ota_info.state);
    let ota_ok = ota_info.state == crate::web::traits::OtaStateView::Idle;
    components.insert("ota", if ota_ok { "ok" } else { "degraded" });

    // P9：存储组件（磁盘压力）。磁盘剩余空间低于阈值时网关会**阻断入队**、
    // 丢弃低优先级点——这是真实的生产影响，此前没有任何对外信号，现场只能
    // 从"点没上来"反推。前端健康横幅已有 `storage` 组件的期望（见
    // `gateway-web/src/composables/layout/useLayoutClock.spec.ts`），后端补齐。
    //
    // 语义：磁盘压力 → "degraded"（可服务但不健康）；正常 → "ok"。
    //
    // **分级只在 `blocking` 档才把 storage 标为 degraded**：`warn` / `critical`
    // 两档磁盘仍能正常写入，若此时让 `/health/ready` 返回 503，会让 k8s /
    // 进程守护在"磁盘还没满"时重启网关——重启不会腾出空间，反而丢数据。
    // 分级信息通过 `disk_level` / `problems` 传达，不劫持就绪语义。
    let disk_status = state.north.gateway.disk_status();
    let disk_level = disk_status.level();
    let disk_free_pct = disk_status.free_pct();
    components.insert(
        "storage",
        if disk_status.is_pressure() || disk_level == DiskLevel::Blocking {
            "degraded"
        } else {
            "ok"
        },
    );

    // 容量淘汰：真的删了数据就必须让用户看见。
    //
    // 注意**不能**把它塞进 `components`：`all_ok` 由 components 全 ok 判定，
    // 而淘汰是「本进程生命周期内曾经发生」的事实，一旦发生过就永远为真，
    // 会让 `/health/ready` 永久停在 degraded —— systemd / 探针会把一个
    // 历史事件误判为当前不健康，进而反复重启。因此只作为独立字段暴露，
    // 由前端红标与告警规则消费，不参与就绪判定。
    let (evicted_rows, evicted_undelivered_rows) = state.south.queue.evicted_stats();

    // 死信：查询失败不代表 0 条，用 available 区分「没有」与「查不到」。
    let dead_letter = &state.north.dead_letter;
    let dead_letter_available = dead_letter.available();
    let dead_letter_rows = if dead_letter_available {
        dead_letter.count(None).await.unwrap_or(0).max(0)
    } else {
        0
    };

    ComponentSnapshot {
        components,
        queue_len,
        max_queue_rows,
        ota_state,
        mqtt_configured,
        mqtt_connected,
        has_north_consumer,
        disk_level,
        disk_free_pct,
        evicted_rows: evicted_rows.max(0),
        evicted_undelivered_rows: evicted_undelivered_rows.max(0),
        dead_letter_rows,
        dead_letter_available,
    }
}

pub(crate) async fn health_ready(state: State<Arc<AppState>>) -> impl IntoResponse {
    let snap = collect_components(&state).await;

    // 综合状态：所有核心组件 ok 即 ready
    let all_ok = snap.components.values().all(|v| *v == "ok");
    let status = if all_ok { "ok" } else { "degraded" };
    let code = if all_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    let resp = serde_json::json!({
        "status": status,
        "components": snap.components,
        "queue_len": snap.queue_len,
        "ota_state": snap.ota_state,
    });
    (code, Json(resp))
}

/// CPU 使用率（%）：跨平台采集，首次无基线时短暂等待后复采。
///
/// P2-2：原实现在本文件里单读 `/proc/stat`，Windows 上文件不存在 → 恒返回 0，
/// 而 `/api/diagnostics/gateway` 走的是 `gateway_core::engine::monitor::host` 的
/// 跨平台实现（Windows 用 `GetSystemTimes`）。两套实现并存导致同机两个页面
/// 显示完全不同的 CPU 数字。现统一复用 host 模块，只保留此处对「首次采样无基线」
/// 的补偿（与 gateway_diag 同一策略）。
async fn cpu_usage_pct() -> f32 {
    let mut usage = host::cpu_usage_pct_now();
    for _ in 0..3 {
        if usage.is_some() {
            break;
        }
        // Windows 系统计数器约 15ms 才推进一次，无增量时短暂等待后复采
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
        usage = host::cpu_usage_pct_now();
    }
    usage.unwrap_or(0.0) as f32
}

/// 内存使用率（%）：跨平台采集（Linux `/proc/meminfo`，Windows `GlobalMemoryStatusEx`）。
/// 读不到时返回 0（`RunStatus.mem_usage` 为非可空字段，前端按数字展示）。
fn mem_usage_pct() -> f32 {
    host::mem_usage_pct_now().unwrap_or(0.0) as f32
}

/// 从点位运行态行汇总「设备总数 / 在线设备数」。
///
/// `device_status()` 每行是**一个点位**的运行态（模板展开点位也在此列），
/// 直接 `count()` 会把「1 台设备 4 个点位」算成「4 台在线设备」。
/// 在线设备 = 名下任一点位最近一次采集成功的设备数（与 /api/health/report 同口径）；
/// 旧配置的点位可能没标 device_id，此时退化为「一点位 = 一设备」保持原语义。
fn summarize_devices(rows: &[DeviceStatus]) -> (usize, usize) {
    use std::collections::HashSet;
    let mut all: HashSet<String> = HashSet::new();
    let mut online: HashSet<String> = HashSet::new();
    for row in rows {
        let key = if row.device_id.is_empty() {
            format!("point:{}", row.point_id)
        } else {
            row.device_id.clone()
        };
        all.insert(key.clone());
        if row.consecutive_failures == 0 {
            online.insert(key);
        }
    }
    (all.len(), online.len())
}

pub(crate) async fn get_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let queue_len = state.south.queue.len().await;
    // 点位运行态行：既用于设备在线汇总，也用于点位总数（见 summarize_devices 注释）
    let device_rows = state.south.scheduler.device_status();
    let (runtime_devices, online) = summarize_devices(&device_rows);

    // 汇总字段：避免前端 N+1 拉取（之前前端 listDevices+status+health+metrics 五次轮询）
    // 字段来源：cfg.buses / cfg.points / cfg.devices —— 启动期模板展开后的实际规模。
    // 注意：模板化配置下点位由设备模板展开生成，并不存在于 cfg.points，
    // 因此点位总数取「配置点位数」与「调度器实际展开点位数」的较大值。
    let cfg = state.config.load_full();
    let total_devices = cfg
        .devices
        .as_ref()
        .map(|d| d.len())
        .unwrap_or(0)
        .max(runtime_devices);
    let total_points = cfg.points.len().max(device_rows.len());
    let bus_count = cfg.buses.len();

    // 当日告警 / 当日采集：来自 Metrics 的当日滚动计数器（UTC 跨日自动清零）。
    // 展示「今日」而非累计值（网关可能已运行数周，累计值对用户无意义）。
    // 读前先滚动一次，避免跨日后首条采集告警尚未到达时读到昨日末值。
    let metrics = &state.platform.metrics;
    metrics.roll_day_if_needed();
    let today_alerts = metrics
        .daily_alerts
        .load(Ordering::Relaxed)
        .min(u32::MAX as u64) as u32;
    let collect_today = metrics
        .daily_collect
        .load(Ordering::Relaxed)
        .min(u32::MAX as u64) as u32;

    Json(RunStatus {
        online_devices: online,
        total_devices,
        total_points,
        bus_count,
        queue_length: queue_len,
        // BUG-L：阈值与 backend 判定（queue_health）同源下发，前端不再另写一套
        queue_thresholds: Some(crate::web::types::QueueThresholds {
            warning: BACKLOG_WARN_ROWS,
            error: BACKPRESSURE_ROWS,
        }),
        cpu_usage: cpu_usage_pct().await,
        mem_usage: mem_usage_pct(),
        today_alerts,
        collect_today,
        // 点位粒度（P3-3）：每行是一个点位而非一台设备，字段名如实表达粒度
        point_rows: device_rows,
    })
}

pub(crate) async fn get_metrics(
    state: axum::extract::State<std::sync::Arc<crate::web::types::AppState>>,
) -> impl IntoResponse {
    let mut body = state.platform.metrics.prometheus_export();
    // 北向第 5 层：积压/背压/已投递批数。抓取时读发送端原子量，无采样任务。
    if let Some(tx) = state.north.north_feed.as_ref() {
        body.push_str(&tx.stats().prometheus_lines());
    }
    // 广播 Lagged 丢批：按消费端打标签。此前该计数只存在于内部与节流日志里，
    // 指标端点看不到「哪个下游在掉队」（north-status 只是 JSON 且仅覆盖北向）。
    body.push_str(&state.south.realtime_bus.lagged_counts().prometheus_lines());
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
}

// ============================================================
// B 组调试增强端点（蜂窝 / 证书 / 透传会话 / 实时订阅者）
// ============================================================

/// B3: 蜂窝网络状态
/// CellularManager 未注入时返回 enabled=false 占位
pub(crate) async fn cellular_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // 4G 拨号依赖 pppd，仅 Linux 支持；非 Linux 只运行 monitor/failover。
    // 显式回传该能力，避免前端把「配置已启用」误当作「可拨号上网」。
    let dial_supported = cfg!(target_os = "linux");
    let platform = std::env::consts::OS;
    match &state.platform.cellular {
        Some(m) => Json(serde_json::json!({
            "enabled": true,
            "dial_supported": dial_supported,
            "platform": platform,
            "status": m.status(),
            "link_state": m.link_state(),
        })),
        None => Json(serde_json::json!({
            "enabled": false,
            "dial_supported": dial_supported,
            "platform": platform,
            "status": serde_json::Value::Null,
            "link_state": serde_json::Value::Null,
        })),
    }
}

/// B11: 透传会话查看
/// 返回 MqttPacketPassthrough 的活跃会话快照
pub(crate) async fn passthrough_sessions(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match &state.south.passthrough {
        Some(m) => Json(serde_json::json!({
            "enabled": true,
            "sessions": m.list_sessions(),
        })),
        None => Json(serde_json::json!({
            "enabled": false,
            "sessions": Vec::<serde_json::Value>::new(),
        })),
    }
}

// ============================================================
// C1 + D10 端点：北向统一状态 + 磁盘健康历史
// ============================================================

/// C1: 北向服务统一状态页
/// 聚合所有上行通道（MQTT/Webhook）的启用状态、连接状态、队列积压，
/// 对应网关核心使命"上传到数据库"：一眼看清数据上行链路是否健康。
///
/// 响应结构与前端 NorthStatus.vue 对齐。
/// - channels[].extra：通道专属字段（broker/circuit_open/success_count/...）
/// - aggregated：跨通道汇总统计（通道数/已连接数/队列积压/上行消息总数）
pub(crate) async fn north_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let cfg = state.config.load_full();
    let mqtt_enabled = cfg.mqtt.enabled;
    let mqtt_connected = match state.north.gateway.mqtt() {
        Some(m) => m.is_connected(),
        None => false,
    };
    let mqtt_circuit_open = match state.north.gateway.mqtt() {
        Some(m) => m.is_circuit_open(),
        None => false,
    };
    let queue_len = state.south.queue.len().await;
    let webhook_enabled = cfg.webhook.as_ref().map(|w| w.enabled).unwrap_or(false);
    let webhook_url = cfg
        .webhook
        .as_ref()
        .map(|w| w.url.clone())
        .unwrap_or_default();

    // N-7：各 connector 的真实 health()（不再由配置推断）。
    // 配置已启用但连接器未注册（如 outbox 打开失败）时，如实报「未连接 + 原因」，
    // 避免状态页把故障通道显示成「已启用」而不暴露问题。
    let connectors = state.north.conversion.connector_status();
    let find = |name: &str| connectors.iter().find(|c| c.name == name);
    let mqtt_health = find("mqtt");
    let mqtt_last_error = mqtt_health.and_then(|c| c.last_error.clone());
    let (webhook_connected, webhook_last_error) = match find("webhook") {
        Some(c) => (Some(c.connected), c.last_error.clone()),
        None if webhook_enabled => (
            Some(false),
            Some("connector not initialized (see gateway startup logs)".to_string()),
        ),
        None => (None, None),
    };

    // RealtimeBus 按消费端类别统计的 Lagged 丢批（north / websocket / alarm）。
    // 同一份快照同时供旧字段（north）与按类别展开的新字段使用，避免口径漂移。
    let lagged = state.south.realtime_bus.lagged_counts();

    let metrics = &state.platform.metrics;
    let wh_success = metrics
        .webhook
        .publish_success
        .load(std::sync::atomic::Ordering::Relaxed);
    let wh_failure = metrics
        .webhook
        .publish_failure
        .load(std::sync::atomic::Ordering::Relaxed);
    let mqtt_success = metrics
        .mqtt
        .publish_success
        .load(std::sync::atomic::Ordering::Relaxed);

    Json(build_north_status_json(
        &NorthStatusParams {
            mqtt_enabled,
            mqtt_connected,
            mqtt_circuit_open,
            mqtt_last_error,
            mqtt_broker: cfg.mqtt.broker.clone(),
            webhook_enabled,
            webhook_connected,
            webhook_last_error,
            webhook_url,
            wh_success,
            wh_failure,
            queue_len,
            mqtt_success,
            // N-7：把实时广播丢帧接入状态响应，Ops 可在页面/接口直接看到背压缺口。
            lagged_dropped_batches: lagged.north,
            lagged_by_consumer: lagged,
            // 第 1 期冗余收口：原独立端点 `/api/north/servers` 并入本响应。
            servers: state
                .north
                .north_servers
                .iter()
                .map(|srv| NorthServerInfo {
                    name: srv.name().to_string(),
                    running: srv.is_running(),
                })
                .collect(),
        },
        &state.platform.metrics,
    ))
}
/// D10: 磁盘健康趋势历史
/// 返回最近 N 条磁盘剩余空间采样，对应网关核心使命"暂存数据"。
/// 本地存储承载待上传数据，磁盘空间趋势直接决定数据能否安全暂存。
pub(crate) async fn disk_health_history(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let samples = state.platform.disk_history.list();
    Json(serde_json::json!({
        "samples": samples,
        "count": samples.len(),
    }))
}

// ============================================================
// 健康聚合报告（GET /api/health/report）
// ============================================================

/// 单总线健康汇总（problems 判定与响应体共用的中间结构）
pub(crate) struct BusHealth {
    pub bus: String,
    pub protocol: String,
    /// 该总线上参与采集的设备数（= 有点位落在该总线上的设备）
    pub total_devices: usize,
    pub online_devices: usize,
    pub total_reads: u64,
    pub success: u64,
    pub success_rate: Option<f64>,
    pub avg_latency_ms: Option<f64>,
    /// 超时率（%）；窗口内无采集为 None
    pub timeout_rate: Option<f64>,
    /// 最近一次失败的错误码（窗口内无失败为 None）
    pub last_error_code: Option<String>,
    /// 重连次数（自进程启动以来累计）
    pub reconnect_count: u64,
}

/// `build_health_problems` 的输入（纯数据，便于单测）
pub(crate) struct HealthProblemInput<'a> {
    /// 是否注入了 MQTT 客户端（用于「MQTT 断开」判定）
    pub mqtt_configured: bool,
    pub mqtt_connected: bool,
    /// 是否存在会消费队列的北向消费者（用于队列等级判定）。
    ///
    /// 与 `mqtt_configured` 必须分开：内置模拟消费者没有 MQTT 客户端，
    /// 却同样会 dequeue → ack 队列，队列存量是「积压」而非「本地历史存量」。
    pub has_north_consumer: bool,
    pub queue_len: usize,
    /// 当前生效的队列表行数硬上限（配置项 `queue.max_rows`）。
    pub max_queue_rows: i64,
    pub ota_state: &'a str,
    pub buses: &'a [BusHealth],
    /// 磁盘水位档位（分级告警的唯一真源）。
    pub disk_level: DiskLevel,
    /// 最近一次检测到的磁盘剩余百分比（未检测过时为 None）。
    pub disk_free_pct: Option<f64>,
    /// 本进程生命周期内因容量上限被物理删除的行数（0 = 未发生）。
    ///
    /// 与 `queue_len` 是正交维度：队列长度回答「现在积压多少」，
    /// 本字段回答「已经删掉了多少」——后者不可恢复，必须单独报为问题。
    pub queue_evicted_rows: i64,
    /// 上述淘汰中未送达的行数。
    pub queue_evicted_undelivered_rows: i64,
    /// 死信表当前行数（已保留但未送达的批次）。
    pub dead_letter_rows: i64,
    /// 死信库是否可用（false = 无法确认是否存在未送达数据）。
    pub dead_letter_available: bool,
}

fn health_problem(level: &str, code: &str, message: String, hint: &str) -> serde_json::Value {
    serde_json::json!({
        "level": level,
        "code": code,
        "message": message,
        "hint": hint,
    })
}

/// 从运行态推导"现场能直接照做"的问题清单。
///
/// 只收高信号项：每一条都对应一个真实可执行的处置动作（去查 broker / 去看线缆 /
/// 去等队列消化）。宁可少报，也不要把"成功率 99.8%"这种正常波动写成问题。
pub(crate) fn build_health_problems(input: &HealthProblemInput<'_>) -> Vec<serde_json::Value> {
    let mut problems = Vec::new();

    if input.mqtt_configured && !input.mqtt_connected {
        problems.push(health_problem(
            "critical",
            "MQTT_DISCONNECTED",
            "MQTT 未连接：采集到的数据无法上传".to_string(),
            "检查 broker 地址 / 端口 / 网络与凭据（系统设置 → 北向通道）",
        ));
    }

    // 容量淘汰：已经删掉的数据不可恢复，严重度高于是「积压」。
    //
    // 放在 queue_health 判定之前：等级告警是**边沿触发**，进入 CapacityPressure
    // 只报一次；而淘汰会持续发生，且队列长度可能在淘汰后又回落（上行恢复），
    // 此时 queue_health 已回到 Ok，但数据确实丢过。单机网关无异地副本，
    // 这条必须独立于等级判定存在。
    if input.queue_evicted_rows > 0 {
        problems.push(health_problem(
            "critical",
            "QUEUE_DATA_EVICTED",
            format!(
                "队列容量上限已淘汰 {} 行数据（其中 {} 行从未上传），这些数据**不可恢复**",
                input.queue_evicted_rows, input.queue_evicted_undelivered_rows
            ),
            "立即确认上行通道（MQTT/Webhook）是否长期不通；如需降低本地留存，请降低采集频率，并尽快通过 GET /api/history/export 做外部归档（历史保留固定 30 天，与 license 无关）",
        ));
    }

    // 死信：数据没送达，但**仍保留可重投** → warning（区别于上面不可恢复的 critical）。
    //
    // `available == false` 单独一条问题：此时「0 条」毫无意义，直接说"查不到"，
    // 否则现场会把查询失败当成"没有未送达数据"。
    if !input.dead_letter_available {
        problems.push(health_problem(
            "warning",
            "DEAD_LETTER_UNAVAILABLE",
            "死信库不可用：无法确认是否存在未送达数据".to_string(),
            "检查 outbox 数据库路径与磁盘状态；北向上报本身不受影响，但异常批次无法查询/重投",
        ));
    } else if input.dead_letter_rows > 0 {
        problems.push(health_problem(
            "warning",
            "DEAD_LETTER_PENDING",
            format!(
                "有 {} 条数据在死信队列中：Webhook 上报被对端永久拒绝或重试用尽，数据已保留但尚未送达",
                input.dead_letter_rows
            ),
            "确认 webhook 接收端地址/鉴权/白名单是否变更；修好后在「系统总览 › 设备端死信」逐条重投，或调用 POST /api/north/dead-letters/{id}/replay",
        ));
    }

    // P0-1：与 /health/ready、/api/watchdog/services 共用 queue_health 的唯一口径。
    // 有消费者时超背压阈值 = 上行故障；无消费者时只是本地历史存量（warning），
    // 而「逼近硬上限」与消费者无关，始终是 critical。
    //
    // 判定用 `has_north_consumer`（MQTT publish 循环 或 内置模拟消费者），
    // 不是 `mqtt_configured`：只有 MQTT 时两者等价，但模拟消费者没有 MQTT
    // 客户端，用后者会把「有消费者在消费」误判成「本地历史存量」。
    match queue_health(
        input.queue_len as i64,
        input.max_queue_rows,
        input.has_north_consumer,
    ) {
        QueueHealth::CapacityPressure => {
            problems.push(health_problem(
                "critical",
                "QUEUE_CAPACITY_PRESSURE",
                format!(
                    "队列表 {} 条，已逼近硬上限 {} 条：存储层开始淘汰最旧数据",
                    input.queue_len,
                    capacity_pressure_rows(input.max_queue_rows)
                ),
                "确认上行通道是否长期停滞；如属正常本地历史存量，请延长采集周期或归档后降低采集频率",
            ));
        }
        QueueHealth::Backpressure => {
            problems.push(health_problem(
                "critical",
                "QUEUE_BACKPRESSURE",
                format!(
                    "待上传队列积压 {} 条，已达背压阈值 {} 条",
                    input.queue_len, BACKPRESSURE_ROWS
                ),
                "上游通道长时间不可用：先确认 MQTT / Webhook 连通性，再观察队列是否继续增长",
            ));
        }
        QueueHealth::Backlog if input.has_north_consumer => {
            problems.push(health_problem(
                "warning",
                "QUEUE_BACKLOG",
                format!(
                    "待上传队列积压 {} 条（≥ {} 预警阈值）",
                    input.queue_len, BACKLOG_WARN_ROWS
                ),
                "数据仍在本地暂存；若持续增长说明上行带宽或对端处理能力不足",
            ));
        }
        QueueHealth::Backlog => {
            problems.push(health_problem(
                "warning",
                "QUEUE_LOCAL_HISTORY",
                format!(
                    "队列存量 {} 条（≥ {} 预警阈值）：当前无运行中的北向消费者",
                    input.queue_len, BACKLOG_WARN_ROWS
                ),
                "队列作为本地历史库按 30 天保留（与 license 无关），无需处置；逼近硬上限后会自动淘汰最旧数据",
            ));
        }
        QueueHealth::Ok => {}
    }

    for b in input.buses {
        if b.total_devices > 0 && b.online_devices == 0 {
            problems.push(health_problem(
                "critical",
                "BUS_ALL_OFFLINE",
                format!("总线 {}：{} 台设备全部离线", b.bus, b.total_devices),
                "先查物理层（接线 / 供电 / 终端电阻），再看从站地址与波特率是否与现场一致",
            ));
        } else if b.online_devices < b.total_devices {
            problems.push(health_problem(
                "warning",
                "BUS_PARTIAL_OFFLINE",
                format!(
                    "总线 {}：{}/{} 台设备在线",
                    b.bus, b.online_devices, b.total_devices
                ),
                "按设备健康列表定位掉线设备，确认是单台从站故障还是总线干扰",
            ));
        }

        // 采样量太小时成功率没有统计意义，不报
        if b.total_reads >= 20 {
            if let Some(rate) = b.success_rate {
                if rate < 50.0 {
                    problems.push(health_problem(
                        "warning",
                        "BUS_LOW_SUCCESS_RATE",
                        format!(
                            "总线 {}：近 24h 成功率仅 {:.1}%（{} 次采集）",
                            b.bus, rate, b.total_reads
                        ),
                        "常见原因：超时过短、校验位/停止位不匹配、从站响应慢，可用协议工具逐帧核对",
                    ));
                }
            }
        }
    }

    // 磁盘水位分级（25% / 15% / 10%）。此前磁盘只有"已阻断"一档信号，
    // 现场看不到"还剩多少余量"，只能等写满后从丢点反推。这里把档位带进
    // problems：`warn` 起就是可处置的预警，`critical` 逼近阻断线。
    //
    // `blocking` 档不再重复报一条独立 problem——此时 `storage` 组件已经是
    // degraded，且队列侧已有"数据被丢弃"的可见后果，此处再报一条只会稀释
    // 现场对高危项的注意力。
    match input.disk_level {
        DiskLevel::Ok => {}
        DiskLevel::Warn => problems.push(health_problem(
            "warning",
            "DISK_SPACE_LOW",
            format!(
                "磁盘剩余空间 {:.1}%：低于 {}% 预警线",
                input.disk_free_pct.unwrap_or(0.0),
                DISK_WARN_THRESHOLD_PCT
            ),
            "尽快归档或清理历史数据，避免继续下降到阻断线后开始丢点",
        )),
        DiskLevel::Critical => problems.push(health_problem(
            "critical",
            "DISK_SPACE_CRITICAL",
            format!(
                "磁盘剩余空间 {:.1}%：低于 {}% 严重线，低于 {}% 后网关将拒绝入队",
                input.disk_free_pct.unwrap_or(0.0),
                DISK_CRITICAL_THRESHOLD_PCT,
                DISK_BLOCKING_THRESHOLD_PCT
            ),
            "立即清理磁盘（历史数据归档 / 删除过期文件），否则即将开始丢弃低优先级采集点",
        )),
        DiskLevel::Blocking => {}
    }

    if input.ota_state != "Idle" {
        problems.push(health_problem(
            "info",
            "OTA_IN_PROGRESS",
            format!("OTA 状态为 {}", input.ota_state),
            "升级期间部分点位可能暂停采集，等状态回到 Idle 后再做验收",
        ));
    }

    problems
}

/// GET /api/health/report：一屏看全网关健康
///
/// 与探针 `/health/live`、`/health/ready`（只回 ok / degraded）不同，本接口面向
/// 现场工程师：组件状态 + 设备/总线汇总 + 逐总线在线数与成功率 + 可直接照做的
/// `problems` 处置提示。前端不必再并发拉 5 个接口自己拼。
///
/// 数据全部来自既有运行时快照（scheduler / queue / scoped metrics / north），
/// 不额外采样、不落盘，开销与 `/api/status` 同量级。
pub(crate) async fn get_health_report(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    use std::collections::{HashMap, HashSet};

    let snap = collect_components(&state).await;
    let cfg = state.config.load_full();

    // 点位级运行态 → 设备级在线情况（模板展开点位也带 device_id）
    let point_rows = state.south.scheduler.device_status();
    let mut device_bus: HashMap<String, String> = HashMap::new();
    let mut device_online: HashSet<String> = HashSet::new();
    for row in &point_rows {
        let bus = row.bus.as_str();
        let dev = row.device_id.as_str();
        if bus.is_empty() || dev.is_empty() {
            continue;
        }
        device_bus
            .entry(dev.to_string())
            .or_insert_with(|| bus.to_string());
        if row.consecutive_failures == 0 {
            device_online.insert(dev.to_string());
        }
    }
    let mut per_bus: HashMap<String, (usize, usize)> = HashMap::new();
    for (dev, bus) in &device_bus {
        let counter = per_bus.entry(bus.clone()).or_insert((0, 0));
        counter.0 += 1;
        if device_online.contains(dev) {
            counter.1 += 1;
        }
    }

    // 近 24h 窗口：与 `/api/diagnostics/stats?period=day` 同口径
    let now = chrono::Utc::now().timestamp_millis();
    let bus_snapshot = state.platform.metrics.scoped.bus_snapshot(now - 86_400_000);

    let buses: Vec<BusHealth> = cfg
        .buses
        .iter()
        .map(|b| {
            let display = b
                .tcp_addr
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or(b.port_name.as_str())
                .to_string();
            let s = bus_snapshot.get(&b.port_name).copied().unwrap_or_default();
            let (total_devices, online_devices) =
                per_bus.get(&b.port_name).copied().unwrap_or((0, 0));
            BusHealth {
                bus: display,
                protocol: b.protocol.clone(),
                total_devices,
                online_devices,
                total_reads: s.total,
                success: s.success,
                success_rate: s.success_rate(),
                avg_latency_ms: s.avg_latency_ms(),
                timeout_rate: s.timeout_rate(),
                last_error_code: s.last_error_code.map(|c| c.code()),
                reconnect_count: s.reconnects,
            }
        })
        .collect();

    let problems = build_health_problems(&HealthProblemInput {
        mqtt_configured: snap.mqtt_configured,
        mqtt_connected: snap.mqtt_connected,
        has_north_consumer: snap.has_north_consumer,
        queue_len: snap.queue_len,
        max_queue_rows: snap.max_queue_rows,
        ota_state: &snap.ota_state,
        buses: &buses,
        disk_level: snap.disk_level,
        disk_free_pct: snap.disk_free_pct,
        queue_evicted_rows: snap.evicted_rows,
        queue_evicted_undelivered_rows: snap.evicted_undelivered_rows,
        dead_letter_rows: snap.dead_letter_rows,
        dead_letter_available: snap.dead_letter_available,
    });

    // 综合状态：critical > degraded > ok。组件非 ok 也算 degraded，
    // 保证和 /health/ready 的判定不会出现"探针说 degraded、报告说 ok"的分叉。
    let components_ok = snap.components.values().all(|v| *v == "ok");
    let has_critical = problems
        .iter()
        .any(|p| p["level"].as_str() == Some("critical"));
    let has_warning = problems
        .iter()
        .any(|p| p["level"].as_str() == Some("warning"));
    let status = if has_critical {
        "critical"
    } else if has_warning || !components_ok {
        "degraded"
    } else {
        "ok"
    };

    let total_reads: u64 = buses.iter().map(|b| b.total_reads).sum();
    let total_success: u64 = buses.iter().map(|b| b.success).sum();
    let success_rate = if total_reads == 0 {
        None
    } else {
        Some((total_success as f64 / total_reads as f64 * 1000.0).round() / 10.0)
    };

    let buses_json: Vec<serde_json::Value> = buses
        .iter()
        .map(|b| {
            serde_json::json!({
                "bus": b.bus,
                "protocol": b.protocol,
                "total_devices": b.total_devices,
                "online_devices": b.online_devices,
                "total_reads": b.total_reads,
                "success_rate": b.success_rate,
                "avg_latency_ms": b.avg_latency_ms,
                "timeout_rate": b.timeout_rate,
                "last_error_code": b.last_error_code,
                "reconnect_count": b.reconnect_count,
            })
        })
        .collect();

    Json(serde_json::json!({
        "generated_at_ms": now,
        "status": status,
        "components": snap.components,
        "summary": {
            // 配置里的设备总数 vs 真正参与采集的设备数分开给：
            // 设备配了点位却没上线时，前者会让人误以为"都离线了"
            "total_devices": cfg.devices.as_ref().map(|d| d.len()).unwrap_or(0),
            "collecting_devices": device_bus.len(),
            "online_devices": device_online.len(),
            "bus_count": cfg.buses.len(),
            "queue_len": snap.queue_len,
            // 容量淘汰：本进程生命周期内被物理删除的行数（不可恢复）。
            // 与 queue_len 是两个维度——queue_len 说「现在积压多少」，
            // 这两个说「已经丢了多少」，后者才是真正需要客户处置的信号。
            "queue_evicted_rows": snap.evicted_rows,
            "queue_evicted_undelivered_rows": snap.evicted_undelivered_rows,
            "queue_data_loss": snap.evicted_rows > 0,
            // 死信：数据已保留但未送达（可重投）。与上面不可恢复的淘汰分开给，
            // 前端据此区分「红了但能救」与「红了救不回来」。
            "dead_letter_rows": snap.dead_letter_rows,
            "dead_letter_available": snap.dead_letter_available,
            "total_reads": total_reads,
            "success_rate": success_rate,
            // 磁盘水位分级（ok / warn / critical / blocking）+ 实际余量。
            // 给出阈值让前端能自己渲染"还剩多少就告警"，不必在前端硬编码一份。
            "disk_level": snap.disk_level.as_str(),
            "disk_free_pct": snap.disk_free_pct,
            "disk_thresholds": {
                "warn": DISK_WARN_THRESHOLD_PCT,
                "critical": DISK_CRITICAL_THRESHOLD_PCT,
                "blocking": DISK_BLOCKING_THRESHOLD_PCT,
            },
        },
        "buses": buses_json,
        "problems": problems,
    }))
}

#[cfg(test)]
mod tests;
