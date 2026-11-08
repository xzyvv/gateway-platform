//! 自愈事件模型与存储
//!
//! 对应 `/api/watchdog/events` 的前端契约。事件由**真实执行路径**写入
//! （手动重启入口，以及后续的自愈执行器），`result` 反映实际成败，
//! 不再由审计日志"回放"伪造。

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gateway_core::config::BusConfig;
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use crate::web::error::ApiError;
use crate::web::types::AppState;

/// 自愈动作类型（前端 API 契约：`gateway-web/src/api/watchdog.ts` 的 `ACTION_LABEL` 覆盖全部变体）。
///
/// `RestartService`（手动重启入口 / 自愈执行器）、`ResetNetwork`（自愈执行器）与
/// `ResetSerial`（自愈执行器 P3）均已真实产出。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub enum HealAction {
    RestartService,
    ResetSerial,
    ResetNetwork,
}

/// 自愈事件记录（`result` 为真实执行结果：`"success"` / `"failed"`）
#[derive(Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct HealEvent {
    pub id: String,
    pub time: u64,
    pub action: HealAction,
    pub target: String,
    pub reason: String,
    pub result: String,
    pub detail: Option<String>,
}

/// 自愈事件容量上限（超出淘汰最旧记录）
const MAX_HEAL_EVENTS: usize = 500;

/// 自愈事件存储（进程内有界环形缓冲，线程安全）。
///
/// 自愈执行器（F4 第二层）与手动重启入口通过 [`HealEventStore::record`] 写入真实执行结果，
/// `/api/watchdog/events` 经 [`HealEventStore::recent`] 读取。当前不持久化，进程重启后清空。
pub struct HealEventStore {
    inner: Mutex<VecDeque<HealEvent>>,
}

impl HealEventStore {
    /// 创建空存储
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(VecDeque::with_capacity(MAX_HEAL_EVENTS)),
        }
    }

    /// 记录一条自愈事件（超过容量时淘汰最旧记录）
    pub fn record(&self, event: HealEvent) {
        let mut guard = self.inner.lock();
        if guard.len() >= MAX_HEAL_EVENTS {
            guard.pop_front();
        }
        guard.push_back(event);
    }

    /// 返回最近 `limit` 条事件（最新在前）
    pub fn recent(&self, limit: usize) -> Vec<HealEvent> {
        let guard = self.inner.lock();
        guard.iter().rev().take(limit).cloned().collect()
    }
}

impl Default for HealEventStore {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// P0: 健康快照采集（只读、无副作用）
// ============================================================================

/// 单个设备的健康快照（来源：`SchedulerTrait::device_status`）。
#[derive(Debug, Clone)]
pub struct DeviceHealth {
    pub id: String,
    pub consecutive_failures: u32,
    pub quality: String,
    /// 设备所属总线名（用于 `ResetSerial` 定位异常串口总线）
    pub bus: String,
    /// 该设备是否挂在**串口**总线上（`tcp_addr` 为空）；TCP 总线不参与 ResetSerial
    pub serial: bool,
}

/// 蜂窝链路健康快照（仅当配置并实例化了 cellular 模块时存在）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellularHealth {
    /// 拨号失败（pppd 拨号未成功）
    pub dial_failed: bool,
    /// 链路不可用（以太网/4G 均 Down）
    pub link_down: bool,
}

/// 周期性健康快照：自愈决策层的唯一输入。只读采集，无任何副作用。
#[derive(Debug, Clone)]
pub struct HealthSnapshot {
    pub ts: u64,
    pub devices: Vec<DeviceHealth>,
    pub mqtt_connected: bool,
    pub mqtt_circuit_open: bool,
    pub queue_len: usize,
    pub cellular: Option<CellularHealth>,
    /// 心跳超时的任务循环名（来源：`engine::monitor::heartbeat`）
    pub stale_loops: Vec<String>,
}

impl HealthSnapshot {
    /// 从当前 AppState 采集一份快照（只读，不产生任何副作用）。
    ///
    /// 经 AppState 的 trait 门面（scheduler / queue / gateway）访问底层，
    /// 不直接引用 south / north 内部模块，符合分层与架构守护约束。
    pub async fn sample(state: &crate::web::types::AppState) -> Self {
        // 串口总线名集合（tcp_addr 为空即串口）：用于判定设备是否挂在串口总线上。
        let serial_ports: HashSet<String> = {
            let cfg = state.config.load_full();
            cfg.buses
                .iter()
                .filter(|b| b.tcp_addr.is_none())
                .map(|b| b.port_name.clone())
                .collect()
        };

        let devices = state
            .south
            .scheduler
            .device_status()
            .iter()
            .map(|v| DeviceHealth {
                id: v.point_id.clone(),
                consecutive_failures: v.consecutive_failures,
                quality: v.quality.clone(),
                serial: serial_ports.contains(&v.bus),
                bus: v.bus.clone(),
            })
            .collect();

        let (mqtt_connected, mqtt_circuit_open) = match state.north.gateway.mqtt() {
            Some(m) => (m.is_connected(), m.is_circuit_open()),
            None => (false, false),
        };

        let queue_len = state.south.queue.len().await;

        let cellular = state.platform.cellular.as_ref().map(|c| CellularHealth {
            dial_failed: matches!(
                c.status().state,
                gateway_core::platform::cellular::CellularState::DialFailed
            ),
            link_down: matches!(
                c.link_state(),
                gateway_core::platform::cellular::LinkState::Down
            ),
        });

        let stale_loops = gateway_core::engine::monitor::heartbeat::get()
            .stale_loops()
            .await
            .into_iter()
            .map(|(name, _)| name)
            .collect();

        Self {
            ts: now_sec(),
            devices,
            mqtt_connected,
            mqtt_circuit_open,
            queue_len,
            cellular,
            stale_loops,
        }
    }
}

// ============================================================================
// P1: 自愈决策（纯逻辑，无 IO / 无时钟依赖）
// ============================================================================

/// 自愈策略参数。P4 接入 `[self_heal]` 配置前使用 [`SelfHealSettings::default`]。
#[derive(Debug, Clone)]
pub struct SelfHealSettings {
    /// 是否启用自愈（默认关闭，opt-in）
    pub enabled: bool,
    /// 快照采集/评估周期（执行器循环间隔）
    pub check_interval_secs: u64,
    /// 连续异常达到该次数才触发动作（防抖）
    pub debounce_k: u32,
    /// 同一 target 两次动作的最小间隔（冷却）
    pub cooldown_secs: u64,
    /// 限速滚动窗口长度
    pub window_secs: u64,
    /// 滚动窗口内允许的最大动作数（限速）
    pub max_actions_per_window: u32,
    /// 连续恢复失败达到该次数则熔断该 target（停止自动动作）
    pub fail_streak_to_fuse: u32,
}

impl Default for SelfHealSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            check_interval_secs: 15,
            debounce_k: 3,
            cooldown_secs: 300,
            window_secs: 3600,
            max_actions_per_window: 10,
            fail_streak_to_fuse: 3,
        }
    }
}

/// 由 `[self_heal]` 配置节构造运行时决策参数（1:1 字段映射）。
impl From<gateway_core::config::SelfHealConfig> for SelfHealSettings {
    fn from(c: gateway_core::config::SelfHealConfig) -> Self {
        Self {
            enabled: c.enabled,
            check_interval_secs: c.check_interval_secs,
            debounce_k: c.debounce_k,
            cooldown_secs: c.cooldown_secs,
            window_secs: c.window_secs,
            max_actions_per_window: c.max_actions_per_window,
            fail_streak_to_fuse: c.fail_streak_to_fuse,
        }
    }
}

/// 决策输出：执行器据此执行动作并回报结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealDecision {
    pub action: HealAction,
    pub target: String,
    pub reason: String,
}

/// 设备连续失败达到该次数（进入退避）即视为其所在串口总线异常。
const SERIAL_FAIL_THRESHOLD: u32 = 3;

/// 派生「处于异常」的串口总线 target 集合：挂在串口总线上且已进入退避的设备，
/// 其所属总线记为一个异常目标（形如 `bus:/dev/ttyS1`）。同总线多设备自动去重。
fn bad_serial_buses(snap: &HealthSnapshot) -> HashSet<String> {
    let mut out = HashSet::new();
    for d in &snap.devices {
        if d.serial && d.consecutive_failures >= SERIAL_FAIL_THRESHOLD {
            out.insert(format!("bus:{}", d.bus));
        }
    }
    out
}

/// 自愈决策器（纯逻辑）。
///
/// 时间由入参传入，不读取系统时钟，便于完整单测；所有状态仅在本结构体内，
/// 不涉及任何 IO。执行器每周期调用 [`HealPolicy::evaluate`] 取决策，执行后
/// 调用 [`HealPolicy::record_outcome`] 回报成败以驱动熔断。
pub struct HealPolicy {
    settings: SelfHealSettings,
    /// target -> 连续异常次数
    bad_streak: HashMap<String, u32>,
    /// target -> 上次动作时间（秒）
    last_action_at: HashMap<String, u64>,
    /// 滚动窗口内的动作时间戳
    action_window: VecDeque<u64>,
    /// 已熔断的 target（连续恢复失败达阈值）
    fused: HashSet<String>,
    /// target -> 动作后连续失败次数
    fail_streak_after_action: HashMap<String, u32>,
    /// 已动作、但结果尚未被下轮快照确认「异常已消除」的 target 集合。
    ///
    /// 动作执行成功（`record_outcome(success=true)`）后**保留**：若下一轮该 target 仍异常，
    /// 说明上次恢复未奏效（如配置重载无法真正重启心跳僵死的任务循环），此时计入
    /// `fail_streak_after_action`。真正恢复时随 [`HealPolicy::update_streaks`] 清零。
    awaiting_resolution: HashSet<String>,
}

impl HealPolicy {
    pub fn new(settings: SelfHealSettings) -> Self {
        Self {
            settings,
            bad_streak: HashMap::new(),
            last_action_at: HashMap::new(),
            action_window: VecDeque::new(),
            fused: HashSet::new(),
            fail_streak_after_action: HashMap::new(),
            awaiting_resolution: HashSet::new(),
        }
    }

    pub fn settings(&self) -> &SelfHealSettings {
        &self.settings
    }

    /// 评估一次快照，返回至多一条待执行决策（`None` 表示本次不动手）。
    ///
    /// 施加顺序：异常累计（防抖）→ 熔断 → 冷却 → 窗口限速。
    /// 多异常并存时只返回优先级最高者（心跳僵死 > 蜂窝掉线），
    /// 避免同一周期并发多个恢复动作叠加风险。
    pub fn evaluate(&mut self, snap: &HealthSnapshot, now: u64) -> Option<HealDecision> {
        if !self.settings.enabled {
            return None;
        }
        self.update_streaks(snap);
        for cand in self.candidates(snap) {
            if self.fused.contains(&cand.target) {
                continue;
            }
            let streak = self.bad_streak.get(&cand.target).copied().unwrap_or(0);
            if streak < self.settings.debounce_k {
                continue;
            }
            if self.in_cooldown(&cand.target, now) {
                continue;
            }
            self.prune_window(now);
            if self.action_window.len() as u32 >= self.settings.max_actions_per_window {
                continue;
            }
            // 上次动作已执行成功但本 target 至今仍异常：说明该动作并未真正消除异常
            // （例如配置重载无法重启心跳僵死的任务循环）。此时在本次动作前先计入一次
            // 「恢复失败」，达阈值即熔断并停止自动动作，避免每过冷却期就重复全量重载。
            if self.awaiting_resolution.contains(&cand.target) {
                let n = self
                    .fail_streak_after_action
                    .entry(cand.target.clone())
                    .or_insert(0);
                *n += 1;
                if *n >= self.settings.fail_streak_to_fuse {
                    self.fused.insert(cand.target.clone());
                    continue;
                }
            }
            self.last_action_at.insert(cand.target.clone(), now);
            self.action_window.push_back(now);
            self.awaiting_resolution.insert(cand.target.clone());
            return Some(cand);
        }
        None
    }

    /// 执行器回报动作结果。
    ///
    /// - 失败（命令本身报错 / 超时）：计入一次恢复失败，达阈值熔断该 target。
    /// - 成功：**不清零**失败计数——「命令返回成功」不等于「异常已消除」，是否真正恢复
    ///   由下一轮快照判定（见 [`HealPolicy::evaluate`]）：若下轮仍异常则继续累计，
    ///   直到熔断或真正恢复。真正恢复时由 [`HealPolicy::update_streaks`] 统一清零。
    pub fn record_outcome(&mut self, target: &str, success: bool) {
        if success {
            return;
        }
        self.awaiting_resolution.remove(target);
        let n = self
            .fail_streak_after_action
            .entry(target.to_string())
            .or_insert(0);
        *n += 1;
        if *n >= self.settings.fail_streak_to_fuse {
            self.fused.insert(target.to_string());
        }
    }

    pub fn is_fused(&self, target: &str) -> bool {
        self.fused.contains(target)
    }

    /// 依据快照更新各 target 的连续异常计数；已恢复的 target 清零并解除熔断。
    fn update_streaks(&mut self, snap: &HealthSnapshot) {
        let mut present: HashSet<String> = HashSet::new();
        for name in &snap.stale_loops {
            present.insert(format!("loop:{name}"));
        }
        if let Some(c) = &snap.cellular {
            if c.link_down || c.dial_failed {
                present.insert("cellular".to_string());
            }
        }
        // 串口总线异常：任一设备进入退避（连续失败达阈值）即视为该总线异常。
        present.extend(bad_serial_buses(snap));
        for t in &present {
            *self.bad_streak.entry(t.clone()).or_insert(0) += 1;
        }
        let recovered: Vec<String> = self
            .bad_streak
            .keys()
            .filter(|k| !present.contains(*k))
            .cloned()
            .collect();
        for k in recovered {
            self.bad_streak.remove(&k);
            self.fused.remove(&k);
            self.fail_streak_after_action.remove(&k);
            self.awaiting_resolution.remove(&k);
        }
    }

    /// 按优先级派生候选决策。
    ///
    /// 当前覆盖：心跳僵死 → `RestartService`、蜂窝掉线 → `ResetNetwork`、
    /// 串口总线设备持续采集失败 → `ResetSerial`。多异常并存时只返回优先级最高者
    /// （心跳僵死 > 蜂窝掉线 > 串口总线），避免同一周期并发多个恢复动作叠加风险。
    fn candidates(&self, snap: &HealthSnapshot) -> Vec<HealDecision> {
        let mut out = Vec::new();
        for name in &snap.stale_loops {
            out.push(HealDecision {
                action: HealAction::RestartService,
                target: format!("loop:{name}"),
                reason: format!("task loop '{name}' heartbeat stale"),
            });
        }
        if let Some(c) = &snap.cellular {
            if c.link_down || c.dial_failed {
                out.push(HealDecision {
                    action: HealAction::ResetNetwork,
                    target: "cellular".to_string(),
                    reason: "cellular link down / dial failed".to_string(),
                });
            }
        }
        // 每条异常串口总线一条候选（按总线名去重，避免同总线多设备重复触发）。
        let mut buses: Vec<String> = bad_serial_buses(snap).into_iter().collect();
        buses.sort();
        for target in buses {
            out.push(HealDecision {
                action: HealAction::ResetSerial,
                target: target.clone(),
                reason: format!("serial bus '{}' devices persistently failing", target),
            });
        }
        out
    }

    fn in_cooldown(&self, target: &str, now: u64) -> bool {
        self.last_action_at
            .get(target)
            .is_some_and(|&t| now.saturating_sub(t) < self.settings.cooldown_secs)
    }

    fn prune_window(&mut self, now: u64) {
        let cutoff = now.saturating_sub(self.settings.window_secs);
        while let Some(&front) = self.action_window.front() {
            if front < cutoff {
                self.action_window.pop_front();
            } else {
                break;
            }
        }
    }
}

fn now_sec() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 自愈事件 id 序号（进程内单调递增）。
///
/// 事件 id 原为 `{秒}-{action}-{target}`：同一秒内相同 action+target 会生成重复 id，
/// 前端以 id 作列表 key 时冲突（渲染错位/复用）。追加自增序号保证全局唯一。
static HEAL_EVENT_SEQ: AtomicU64 = AtomicU64::new(1);

/// 生成自愈事件唯一 id：`{秒}-{序号}-{action}-{target}`。
///
/// 手动重启入口（`watchdog::record_heal_event`）与自愈执行器共用，确保两处 id 口径一致。
pub(crate) fn next_heal_event_id(now: u64, action: &HealAction, target: &str) -> String {
    let seq = HEAL_EVENT_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{}-{}-{:?}-{}", now, seq, action, target)
}

// ============================================================================
// P2: 自愈执行器（受控执行层）
// ============================================================================

/// 单个自愈动作的执行超时：超时计为失败，避免动作自身卡死拖垮执行器循环。
const ACTION_TIMEOUT: Duration = Duration::from_secs(30);

/// 超时后仍在后台执行的恢复动作句柄。
///
/// [`run_action_with_timeout`] 超时只判定本轮失败、**不取消**动作（避免半更新），
/// 因此执行器必须持有该句柄：
/// 1. 上一动作未结束时**跳过本轮采样与决策**，防止第二个恢复动作叠加在仍在执行的
///    总线上（旧实现超时后立刻进入下一轮，15s 后可能对另一条总线再发一个动作，
///    两个恢复动作在同一进程/同一 `reload_lock` 上排队，最坏演化成重连风暴）；
/// 2. 停机（`cancel`）时等待其完成，兑现 [`spawn_executor`] 文档中
///    「先 cancel 再 await 句柄，确保在飞动作完成，避免半写」的契约。
#[derive(Default)]
struct InFlightAction {
    handle: Option<tokio::task::JoinHandle<Result<(), String>>>,
}

impl InFlightAction {
    /// 回收已结束的句柄；返回 `true` 表示仍有动作在后台执行。
    fn is_busy(&mut self) -> bool {
        if self.handle.as_ref().is_some_and(|h| h.is_finished()) {
            // 结果已在超时那一轮按失败记录，这里只需回收句柄。
            self.handle = None;
        }
        self.handle.is_some()
    }

    /// 把手柄交给执行器跟踪（动作仍在后台跑，超时轮已记失败）。
    fn track(&mut self, handle: tokio::task::JoinHandle<Result<(), String>>) {
        self.handle = Some(handle);
    }

    /// 停机时等待超时遗留的动作完成（最多 `grace`），避免半写。
    async fn drain(&mut self, grace: Duration) {
        let Some(handle) = self.handle.take() else {
            return;
        };
        match tokio::time::timeout(grace, handle).await {
            Ok(Ok(Ok(()))) => {
                tracing::debug!("self-heal: leftover action finished during shutdown")
            }
            Ok(Ok(Err(e))) => tracing::warn!("self-heal: leftover action failed: {}", e),
            Ok(Err(join_err)) => {
                tracing::warn!("self-heal: leftover action task failed: {}", join_err)
            }
            Err(_) => tracing::warn!(
                "self-heal: leftover action still running after {:?} on shutdown",
                grace
            ),
        }
    }
}

/// 启动自愈执行器后台任务，返回 `(停机信号, 任务句柄)`。
///
/// 仅当 `settings.enabled` 为真时由 bootstrap 调用（默认关闭，opt-in，存量部署零影响）。
/// 循环：采样健康快照 → [`HealPolicy::evaluate`] 决策 → 执行动作（带超时）→ 记录
/// [`HealEvent`] → [`HealPolicy::record_outcome`] 回报结果。
///
/// 任务用 `spawn_supervised` 包裹（panic 自动重启）；通过返回的 [`CancellationToken`]
/// 优雅退出：停机时 bootstrap 先 `cancel` 再 `await` 句柄，确保在飞动作
/// （如配置重载）完成，避免半写。执行器自身不新增恢复旁路，动作全部复用既有收敛路径。
pub fn spawn_executor(
    state: Arc<AppState>,
    settings: SelfHealSettings,
) -> (CancellationToken, tokio::task::JoinHandle<()>) {
    let cancel = CancellationToken::new();
    let cancel_for_task = cancel.clone();
    let interval_secs = settings.check_interval_secs.max(1);
    let handle = gateway_core::engine::supervision::spawn_supervised("self_heal", move || {
        let state = state.clone();
        let settings = settings.clone();
        let cancel = cancel_for_task.clone();
        async move {
            let mut policy = HealPolicy::new(settings);
            let mut in_flight = InFlightAction::default();
            let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs));
            // 采样本身耗时不长；若某一轮被拖长，用 Delay 跳过错过的 tick，避免补跑堆积。
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        tracing::info!("self-heal executor cancelled, exiting");
                        // 超时遗留的动作可能仍在后台跑（reload / 总线重建）：先等它收尾，
                        // 否则停机期间会留下「运行时已换、内存/磁盘未换」的半更新。
                        in_flight.drain(ACTION_TIMEOUT).await;
                        return;
                    }
                    _ = ticker.tick() => {}
                }
                run_once(&state, &mut policy, &mut in_flight).await;
            }
        }
    });
    (cancel, handle)
}

/// 执行一轮：采样 → 决策 → 执行 → 记录事件 → 回报结果。
///
/// 每个周期至多执行一个决策（见 [`HealPolicy::evaluate`] 的多异常优先级说明），
/// 避免同一周期并发多个恢复动作叠加风险。上一轮超时后仍在后台执行的动作未结束时，
/// **整轮跳过**（不采样、不决策），防止第二个动作叠加。
async fn run_once(state: &Arc<AppState>, policy: &mut HealPolicy, in_flight: &mut InFlightAction) {
    if in_flight.is_busy() {
        tracing::warn!("self-heal: previous action still in flight, skipping this cycle");
        return;
    }
    let now = now_sec();
    let snap = HealthSnapshot::sample(state).await;
    let Some(decision) = policy.evaluate(&snap, now) else {
        return;
    };
    let outcome = execute_action(state, &decision, in_flight).await;
    record_execution(state, &decision, &outcome);
    policy.record_outcome(&decision.target, outcome.is_ok());
}

/// 执行决策对应的动作（带超时保护，但**超时不取消在飞动作**）。
async fn execute_action(
    state: &Arc<AppState>,
    decision: &HealDecision,
    in_flight: &mut InFlightAction,
) -> Result<(), String> {
    match decision.action {
        HealAction::ResetNetwork => {
            let st = state.clone();
            run_action_with_timeout(
                ACTION_TIMEOUT,
                "reset network",
                async move { reset_network(&st).await },
                in_flight,
            )
            .await
        }
        HealAction::RestartService => {
            let st = state.clone();
            let target = decision.target.clone();
            run_action_with_timeout(
                ACTION_TIMEOUT,
                "restart service",
                async move { restart_service(&st, &target).await },
                in_flight,
            )
            .await
        }
        HealAction::ResetSerial => {
            let st = state.clone();
            let target = decision.target.clone();
            run_action_with_timeout(
                ACTION_TIMEOUT,
                "reset serial",
                async move { reset_serial(&st, &target).await },
                in_flight,
            )
            .await
        }
    }
}

/// 在后台任务中执行动作，仅对「等待其完成」施加超时——**超时只判定本轮失败，不取消动作**。
///
/// 旧实现直接 `timeout(fut)` 包裹动作，超时会 drop 掉在飞的 future。而
/// `RestartService`（reload → store → save 的 A5 原子序列）与 `ResetSerial`（重建总线）
/// 被中途取消会留下「运行时已换、内存/磁盘未换」的半更新，与 [`reload_services`] 文档
/// 「返回 Err 时运行时未产生半更新」的保证相矛盾，也与执行器停机语义
/// （`spawn_executor` 注释：先 cancel 再 await 句柄「确保在飞动作完成，避免半写」）自相矛盾。
///
/// 改为 `spawn` 后只对 JoinHandle 计时：超时仅把本轮记为失败，动作仍在后台跑完，
/// 从而保证原子序列不被撕裂。超时值以参数传入，便于用短超时做回归测试。
async fn run_action_with_timeout<F>(
    timeout: Duration,
    action: &str,
    fut: F,
    in_flight: &mut InFlightAction,
) -> Result<(), String>
where
    F: std::future::Future<Output = Result<(), String>> + Send + 'static,
{
    let mut handle = tokio::spawn(fut);
    match tokio::time::timeout(timeout, &mut handle).await {
        Ok(Ok(r)) => r,
        Ok(Err(join_err)) => Err(format!("{} task failed: {}", action, join_err)),
        Err(_) => {
            // 动作仍在后台执行：交给执行器跟踪。下一轮据此跳过决策，停机时等待收尾。
            in_flight.track(handle);
            Err(format!(
                "{} timed out after {:?} (action continues in background to avoid half-update)",
                action, timeout
            ))
        }
    }
}

/// `ResetSerial`：只重建**目标**串口总线（重新打开串口 → 替换该 BusManager/collector →
/// 重建受影响采集组），其他串口总线与 TCP 总线保持不动。
///
/// 旧实现忽略 `target`，一条串口总线异常就把**所有**串口总线全部重建：单一健康快照
/// 里的一个异常信号会放大成全局动作，其他正常总线上的设备在重开期间全部拿到
/// `BusClosed`（一次误判 = 全站串口中断开）。现按 `target`（`bus:{port_name}`）限定范围。
///
/// 复用配置热重载已验证的总线重建路径（`Gateway::reset_serial_buses`），
/// 不新增独立旁路恢复逻辑；走 `reload_lock` 串行化，与并发配置重载互斥。
async fn reset_serial(state: &Arc<AppState>, target: &str) -> Result<(), String> {
    let cfg = state.config.load_full();
    let bus_name = serial_bus_target(&cfg.buses, target)?;
    let points = crate::web::config_apply::expand_points(state, cfg.as_ref());
    let rebuilt = state
        .north
        .gateway
        .reset_serial_buses(&cfg.buses, &points, Some(&bus_name))
        .await
        .map_err(|e| e.to_string())?;
    if rebuilt == 0 {
        return Err(format!(
            "serial bus '{}' not rebuilt (open failed or config missing)",
            bus_name
        ));
    }
    tracing::info!(
        "self-heal ResetSerial rebuilt {} serial bus(es) (target={})",
        rebuilt,
        target
    );
    Ok(())
}

/// 解析并校验 `ResetSerial` 目标：剥离 `bus:` 前缀，确认它是配置中的串口总线。
fn serial_bus_target(buses: &[BusConfig], target: &str) -> Result<String, String> {
    let name = target.strip_prefix("bus:").unwrap_or(target);
    if name.is_empty() {
        return Err(format!("reset serial: empty target ({})", target));
    }
    let is_serial = buses
        .iter()
        .any(|b| b.tcp_addr.is_none() && b.port_name == name);
    if !is_serial {
        return Err(format!(
            "reset serial: '{}' is not a configured serial bus",
            name
        ));
    }
    Ok(name.to_string())
}

/// `ResetNetwork`：重启蜂窝模块（`CellularManager::restart` = stop → 换新取消令牌 → start）。
async fn reset_network(state: &Arc<AppState>) -> Result<(), String> {
    let cellular = state
        .platform
        .cellular
        .clone()
        .ok_or_else(|| "cellular not configured/instantiated".to_string())?;
    cellular.restart().await;
    Ok(())
}

/// `RestartService`：经统一配置重载入口重启内部服务（与手动重启端点同路径）。
async fn restart_service(state: &Arc<AppState>, target: &str) -> Result<(), String> {
    reload_services(
        state,
        "self_heal",
        &format!("self-heal restart service (target={})", target),
    )
    .await
    .map_err(|e| e.message)
}

/// 统一的「重启内部服务」实现：持有 `config_reload_lock` → 重载磁盘配置 →
/// [`crate::web::config_apply::reload_then_save`]（A5 顺序）。
///
/// 手动重启端点（`/api/watchdog/services/:name/restart`）与自愈执行器的 `RestartService`
/// 动作共用此函数，确保恢复路径唯一、不新增旁路。返回 `Err` 时运行时未产生半更新。
pub(crate) async fn reload_services(
    state: &Arc<AppState>,
    audit_actor: &str,
    audit_detail: &str,
) -> Result<(), ApiError> {
    let _guard = state.config.config_reload_lock.lock().await;
    let old = state.config.load_full();
    let new = state
        .config
        .config_loader
        .reload()
        .map_err(|e| ApiError::internal(format!("reload failed: {}", e)))?;
    let _ = crate::web::config_apply::reload_then_save(state, &old, &new).await?;
    state
        .platform
        .audit
        .log("config.reload", "gateway", audit_detail, audit_actor);
    state.platform.metrics.inc_config_reload();
    Ok(())
}

/// 记录一条真实的自愈执行事件（成功 / 失败均记录，供 `/api/watchdog/events` 展示）。
fn record_execution(state: &Arc<AppState>, decision: &HealDecision, outcome: &Result<(), String>) {
    let now = now_sec();
    let (result, detail) = match outcome {
        Ok(()) => {
            tracing::info!(
                "self-heal action {:?} on '{}' succeeded",
                decision.action,
                decision.target
            );
            ("success", None)
        }
        Err(e) => {
            tracing::warn!(
                "self-heal action {:?} on '{}' failed: {}",
                decision.action,
                decision.target,
                e
            );
            ("failed", Some(e.clone()))
        }
    };
    state.platform.heal_events.record(HealEvent {
        id: next_heal_event_id(now, &decision.action, &decision.target),
        time: now,
        action: decision.action.clone(),
        target: decision.target.clone(),
        reason: decision.reason.clone(),
        result: result.to_string(),
        detail,
    });
}

#[cfg(test)]
mod tests;
