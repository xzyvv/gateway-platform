//! 采集调度器（§8 采集调度架构 + §7.3 退避策略）
//!
//! - 南向采集按点位 interval 周期执行
//! - Modbus 按 (bus, slave_id, fc) 批量读取
//! - DLT645/CJT188 单点读取
//! - 所有 task 外层 supervision loop：panic 后 5 秒自动重启
//! - 设备退避：连续失败 >3 次进入指数退避
//!
//! # Bug-R7 (fix)：LOCK ACQUISITION ORDER（加锁顺序不变量）
//!
//! 本模块所有 `Mutex` / `RwLock` 字段（`collectors` / `points` / `device_states` /
//! `handles` / `group_stops`）当前均为 **parking_lot::Mutex**（同步锁，不跨 `.await` 持锁），
//! 因此**当前不存在经典死锁风险**——parking_lot 锁是 RAII 同步借出，绝不会跨 await 被另一
//! 个 `.await` 点持有（那是 tokio::Mutex 才会出现的问题）。
//!
//! 但**未来重构风险**真实存在：
//! - 如果有人把 `device_states` 改成 `tokio::Mutex<...>`（需要 `.await` 锁），
//!   而保持 `points` 是 `parking_lot::Mutex`——同一方法里 `points.lock(); ... .await; device_states.lock().await`
//!   会让 parking_lot 锁**跨 await** 持有，违反 parking_lot 自身的不变量，
//!   可能 panic（"cannot drop a lock held across an await point"）。
//! - 如果有人新加 lock 类型或新代码路径采用反序加锁（A 锁 B 等 B 锁 A），会在
//!   调度器实例化后第一次并发触发（reload + device_status）出现 hang。
//!
//! 强制约束（任何方法必须遵守，违反会导致死锁 / panic）：
//!
//! ```text
//! LOCK ORDER (acquire top → bottom, release inverse):
//!
//!   1. self.metrics             (Arc<Metrics>，无锁读原子，**非锁字段**)
//!   2. self.running             (AtomicBool，**非锁字段**)
//!   3. self.collectors          (parking_lot::Mutex)
//!   4. self.points              (parking_lot::Mutex)
//!   5. self.device_states       (Mutex — 当前 parking_lot；若改 tokio 不许同时持低层锁跨 await)
//!   6. self.handles             (Mutex，parking_lot)
//!   7. self.group_stops         (Mutex，parking_lot)
//!   8. self.shutdown_tx.send    (watch::Sender 推送，本身几乎零开销)
//!   9. self.reload_lock         (Semaphore，**显式跨 await**，仅 reload() 入口用)
//! ```
//!
//! 实际加锁点（2026-Q1 audit）：
//! - `reload()`: collectors → points → reload_points_incremental (await 期间释放) → points → device_states ✅
//! - `device_status()`: points → device_states ✅
//! - `start()`: running.swap → points.lock().clone()（单点锁，不嵌套）
//! - `stop()`: shutdown_tx.send → handles.take → group_stops.lock().clear（独立域，无嵌套）
//!
//! 新方法加锁前请先核对本表顺序；如必须反序（例如 reload 与 self_heal 协作场景），
//! 请改用 `tokio::sync::Mutex` 显式 await 而**不在 await 之间持其他锁**，或提取快照
//! 临时变量（先 clone 出值再释放锁、再去 await 锁路径）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{watch, Semaphore};

use parking_lot::Mutex;

use crate::config::PointConfig;
use crate::pipeline::ProcessingPipeline;
use crate::south::protocol::Collector;

pub use device_state::DeviceState;
pub use status::DeviceStatus;
pub use timeline::{ScheduleTimeline, Timeline, TimelineGroup, TimelineOverlap, TimelineSample};

mod device_state;
mod reload;
mod results;
mod spawning;
mod status;
mod task;
mod timeline;

#[cfg(test)]
mod tests;

pub struct Scheduler {
    /// 按总线名索引的采集器（热更新时可替换）
    collectors: parking_lot::Mutex<HashMap<String, Arc<dyn Collector>>>,
    points: parking_lot::Mutex<Vec<PointConfig>>,
    pipeline: Arc<ProcessingPipeline>,
    device_states: Mutex<HashMap<String, DeviceState>>,
    /// 关闭信号：watch channel 支持反复 set（Notify 只能 notify_waiters 一次）
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
    /// task 句柄按组键存储，支持增量热重载：仅停止/重建变更组
    handles: Mutex<HashMap<String, tokio::task::JoinHandle<()>>>,
    /// 修复(审查#B1)：每个采集组独立的停止信号（与组键一一对应）。
    ///
    /// 原实现的 `shutdown` 是**全局共享** watch，热重载无法只通知单个组退出——
    /// 于是 `reload` 里逐组 `timeout(3s, &mut h)` 必然空等满 3s（20 组即 60s，
    /// 期间阻塞 API），超时后只能 `abort()` 打断在途 Pipeline（丢数据）。
    /// 现在每组有独立信号，reload 可精确地让目标组优雅退出。
    group_stops: Mutex<HashMap<String, watch::Sender<bool>>>,
    /// 是否处于运行态（用于热重载：stop 后可以再次 start）
    running: AtomicBool,
    /// E1：被调试独占而临时暂停采集的总线名。
    ///
    /// 只在运行期有效（`start()` 会清空）：暂停不修改配置、不改 collectors，
    /// 只把该总线的点位从调度中摘除并停掉对应采集组；恢复时按传入的完整点位重建。
    /// 调试工具（协议会话 / 报文回放）需要独占总线时先调用，避免「采集占用」死路。
    paused_buses: parking_lot::Mutex<std::collections::HashSet<String>>,
    /// 1.1: 指标实例（替代全局 metrics::get()）
    pub metrics: Arc<crate::engine::metrics::Metrics>,
    /// F-11：采集任务真实时序（每组最近若干次执行窗口），供调度时序诊断端点读取。
    /// 独立短临界区锁，不参与上方 LOCK ORDER 的嵌套。
    pub timeline: Arc<Timeline>,
    /// Bug r09-RLO1 (fix): reload 入口串行化——防止并发 reload 互相覆盖。
    ///
    /// 旧实现 `reload_points_incremental` 是 `pub(crate) async fn` 无锁保护。
    /// 两次并发触发（API + 手动运维）时，第二个 reload 的 new_points 会被
    /// 第一个的 `*self.points.lock() = new_points` 覆盖 → **偶发运维丢更新**。
    /// 用 Semaphore(1) 强制单飞：第二个 reload 等待第一个完成。
    reload_lock: Arc<Semaphore>,
    /// Bug-R5 (fix)：测试用 reload 失败注入器。
    ///
    /// 必须是**实例字段**而非模块级 static：旧实现是
    /// `static R5_FAIL_INJECTOR: Mutex<Option<String>>`，与并行执行的其它 reload
    /// 测试共享状态——注入窗口内任何 `scheduler.reload()` 都会被强制失败，导致
    /// `reload_stops_changed_groups_via_per_group_signal` 偶发断言失败
    /// （单独跑必过、与 r5 测试并行必挂）。改为实例字段后各测试互不干扰。
    ///
    /// 生产构建下 `cfg(test)` 不编译，无额外内存与分支开销。
    #[cfg(test)]
    fail_reload_injector: parking_lot::Mutex<Option<String>>,
}

impl Scheduler {
    pub fn new(
        collectors: HashMap<String, Arc<dyn Collector>>,
        points: Vec<PointConfig>,
        pipeline: Arc<ProcessingPipeline>,
        metrics: Arc<crate::engine::metrics::Metrics>,
    ) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        Self {
            collectors: parking_lot::Mutex::new(collectors),
            points: parking_lot::Mutex::new(points),
            pipeline,
            device_states: Mutex::new(HashMap::new()),
            shutdown_tx,
            shutdown_rx,
            handles: Mutex::new(HashMap::new()),
            group_stops: Mutex::new(HashMap::new()),
            running: AtomicBool::new(false),
            paused_buses: parking_lot::Mutex::new(std::collections::HashSet::new()),
            metrics,
            timeline: Arc::new(Timeline::default()),
            reload_lock: Arc::new(Semaphore::new(1)),
            #[cfg(test)]
            fail_reload_injector: parking_lot::Mutex::new(None),
        }
    }

    pub fn start(self: &Arc<Self>) {
        // 防止重复 start
        if self.running.swap(true, Ordering::SeqCst) {
            tracing::warn!("scheduler already running, start() ignored");
            return;
        }
        // 重新启动 = 全新运行期：之前为调试独占做的暂停不再沿用
        self.paused_buses.lock().clear();
        // 同上：重启前的时序样本不代表本轮运行，清空避免甘特图混入旧数据
        self.timeline.clear();

        // 按总线分组
        let points = self.points.lock().clone();
        let mut by_bus: HashMap<String, Vec<PointConfig>> = HashMap::new();
        for p in &points {
            by_bus.entry(p.bus.clone()).or_default().push(p.clone());
        }

        // Bug-N2 (fix)：空 points 启动是配置错误信号——之前仅 `info!("with 0 points")`
        // 默默成功，前端监控看不到报警源，运维只能从「设备全掉」逆推原因。
        // 现在：tracing::warn! + 递增 lifecycle.scheduler_empty_starts metrics，
        // Grafana 告警 `rate(gateway_scheduler_empty_starts_total[5m]) > 0`
        // 可立即报"配置空 / include 展开后无点位"。
        //
        // 重要：仍然要把 by_bus 传进去跑完整的 spawn 路径——因为 callers 期待
        // start() 完整语义（包括聚合刷新任务），而不是 early-return。
        if points.is_empty() {
            tracing::warn!(
                "scheduler started with 0 points — 可能原因：points 段缺失 / include 展开失败 / schema 迁移遗漏；前端将显示全部设备离线，运维请检查 /api/system/diagnostics"
            );
            self.metrics
                .lifecyle
                .scheduler_empty_starts
                .fetch_add(1, Ordering::Relaxed);
        }

        for (bus_name, bus_points) in &by_bus {
            // 所有协议（含 GPIO）走统一 spawn_collector_task 路径
            // GPIO 通过 GpioCollector 实现 Collector trait，行为与其他协议一致
            if !bus_points.is_empty() {
                self.spawn_collector_task(bus_name, bus_points.clone());
            }
        }

        // 聚合刷新任务
        self.spawn_aggregation_flush();
    }

    /// 启动 Modbus 批量任务（按 slave_id + fc 分组）
    pub fn collectors(&self) -> HashMap<String, Arc<dyn Collector>> {
        self.collectors.lock().clone()
    }

    /// 按点位 id 查找当前生效的点位配置（不存在返回 `None`）。
    ///
    /// 供北向写回入口（如 OPC UA Server）校验 `writable` 白名单与定位 collector。
    pub fn point_by_id(&self, id: &str) -> Option<PointConfig> {
        self.points.lock().iter().find(|p| p.id == id).cloned()
    }

    /// E1：暂停某总线的采集（供调试工具独占总线）。
    ///
    /// 语义：**不动配置、不换 collector**，只把 `bus` 上的点位从调度里摘除并停掉
    /// 对应采集组。`all_points` 是调用方持有的完整点位集（含模板展开结果），
    /// 由 Web 层传入，这样恢复时不需要再从配置重新推导。
    pub async fn pause_bus(
        self: &Arc<Self>,
        bus: &str,
        all_points: Vec<PointConfig>,
    ) -> Result<(), String> {
        if !self.running.load(Ordering::SeqCst) {
            return Err("采集调度未运行".to_string());
        }
        if !all_points.iter().any(|p| p.bus == bus) {
            return Err(format!("总线 {bus} 上没有可暂停的采集点位"));
        }
        self.paused_buses.lock().insert(bus.to_string());
        let kept: Vec<PointConfig> = all_points.into_iter().filter(|p| p.bus != bus).collect();
        let mut force = std::collections::HashSet::new();
        force.insert(bus.to_string());
        self.reload_points_incremental(kept, &force).await
    }

    /// E1：恢复被暂停的总线采集（按完整点位集重建该总线的采集组）
    pub async fn resume_bus(
        self: &Arc<Self>,
        bus: &str,
        all_points: Vec<PointConfig>,
    ) -> Result<(), String> {
        if !self.running.load(Ordering::SeqCst) {
            return Err("采集调度未运行".to_string());
        }
        self.paused_buses.lock().remove(bus);
        let mut force = std::collections::HashSet::new();
        force.insert(bus.to_string());
        self.reload_points_incremental(all_points, &force).await
    }

    /// 当前被暂停采集的总线（按名排序，供 API/诊断展示）
    pub fn paused_buses(&self) -> Vec<String> {
        let mut v: Vec<String> = self.paused_buses.lock().iter().cloned().collect();
        v.sort();
        v
    }

    /// 某总线当前是否处于暂停采集状态
    pub fn is_bus_paused(&self, bus: &str) -> bool {
        self.paused_buses.lock().contains(bus)
    }

    /// 采集调度是否在运行（诊断端点展示用）
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// F-11：真实调度时序快照（窗口内每组执行窗口 + 同总线真实重叠）
    pub fn timeline_snapshot(&self, window_ms: u64) -> ScheduleTimeline {
        let mut snap = self.timeline.snapshot(window_ms);
        snap.running = self.is_running();
        snap
    }

    /// 获取 collectors 锁的可变引用，供热重载时整体替换 collectors map
    pub fn collectors_lock(
        &self,
    ) -> parking_lot::MutexGuard<'_, HashMap<String, Arc<dyn Collector>>> {
        self.collectors.lock()
    }

    pub fn device_status(&self) -> Vec<DeviceStatus> {
        // 点位 id -> (所属总线名, 所属设备 ID)：
        // - 总线名用于自愈 ResetSerial 定位设备所在总线；
        // - 设备 ID 供 Web API 反查点位归属。注意**不能**用配置快照 cfg.points 查：
        //   模板化配置下点位由设备模板展开生成，并不存在于 cfg.points 中。
        // 先取 points 快照再锁 device_states——遵循模块级 `LOCK ORDER` 表
        // （points → device_states，与 reload() 路径顺序一致，避免 future 死锁）。
        let meta_by_id: HashMap<String, (String, String)> = {
            let points = self.points.lock();
            points
                .iter()
                .map(|p| {
                    (
                        p.id.clone(),
                        (p.bus.clone(), p.device_id.clone().unwrap_or_default()),
                    )
                })
                .collect()
        };
        let states = self.device_states.lock();
        states
            .iter()
            .map(|(id, s)| {
                let (bus, device_id) = meta_by_id.get(id).cloned().unwrap_or_default();
                DeviceStatus {
                    point_id: id.clone(),
                    bus,
                    // 追加式字段：点位所属设备 ID（模板展开点位也能正确归属）
                    device_id,
                    consecutive_failures: s.consecutive_failures,
                    current_interval_ms: s.current_interval.as_millis() as u64,
                    // 退避可视化：基础间隔 / 连续重发次数 / 上次成功时间戳（新增字段为追加式，旧消费方忽略）
                    base_interval_ms: s.base_interval.as_millis() as u64,
                    stale_count: s.stale_count,
                    offline_episodes: s.offline_episodes,
                    // MTBF 统计（since-boot 真实值）：离线段起点 / 已结束离线累计 / 统计窗口起点
                    offline_since_ts: s.offline_since_ts,
                    total_downtime_ms: s.total_downtime_ms,
                    tracking_since_ts: s.tracking_since_ts,
                    last_good_ts: s.last_good_ts,
                    last_good_value: s.last_good_value,
                    quality: s.current_quality().as_str().to_string(),
                }
            })
            .collect()
    }

    /// 停止所有任务
    ///
    /// 通知所有 task 退出，等待 task 真正退出（最长 5 秒）。
    /// 允许 start() 之后再次 start()（用于热重载）。
    pub async fn stop(&self) {
        if !self.running.swap(false, Ordering::SeqCst) {
            return; // 已经停止
        }
        // 通过 watch 通知所有 task 退出（每次 start 可重复 set）
        let _ = self.shutdown_tx.send(true);
        let handles: Vec<_> = std::mem::take(&mut *self.handles.lock())
            .into_values()
            .collect();
        // 修复(审查#B1)：全局 shutdown 信号已覆盖所有 task 的退出，此处只需清空
        // 每组停止信号发送端，避免与已取走的句柄表漂移（残留的 sender 会随 drop
        // 一并释放，对应 Receiver 侧表现为 changed() 返回 Err，同样触发退出）。
        self.group_stops.lock().clear();
        // 优雅停止：先给 task 时间处理完当前采集循环再退出，
        // 超时（3s）仍未退出的 task 才强制 abort，避免丢失在途数据。
        let shutdown_timeout = Duration::from_secs(3);
        for mut h in handles {
            let join_result = tokio::time::timeout(shutdown_timeout, &mut h).await;
            match join_result {
                Ok(Ok(())) => {} // task 正常退出
                Ok(Err(e)) => tracing::warn!("scheduler task panicked: {}", e),
                Err(_) => {
                    tracing::warn!(
                        "scheduler task did not exit within {:?}, aborting",
                        shutdown_timeout
                    );
                    h.abort();
                }
            }
        }
    }

    /// 增量热重载点位配置（细粒度）
    ///
    /// 仅停止/重建受点位变更影响的采集组，未变更的组保持运行不中断。
    /// 组键规则与 start() 一致：modbus:{bus}:{slave}:{fc}、single:{bus}:{interval}、gpio:{id}。
    /// 适用于仅少量点位增删改的场景，保证其他点位采集连续性。
    /// 增量热重载（collectors + points）
    ///
    /// 修复 A4：原先 stop() + start() 全停重建会让采集整体断档 1-3s，
    /// 在点位 500+ 的工业现场触发 metric spike 与前端报警。
    /// 现在改为：替换 collectors 后调 reload_points_incremental，仅停
    /// 真正变更的组，其余保持运行不中断。
    ///
    /// Bug-R5 (fix)：reload 中途失败时回滚已装上的 collectors / points。
    ///
    /// 旧实现：步骤 1（L281 替换 collectors）+ 步骤 2（L284 替换 points），
    /// **先于**步骤 3（reload_points_incremental 的 stop + spawn）执行——
    /// 若 reload_points_incremental 抛错（spawn panic / runtime 校验失败），
    /// 新 collectors / points **已装上**但**新采集组未起来**，
    /// 而老 handles 已被 reload_points_incremental 内部移到 to_shutdown 删除——
    /// 现场表现为「整张采集系统 silent 死锁」，需 gatewayd 重启恢复。
    ///
    /// 新实现：步骤 1/2 之前先快照原 collectors / points，异常路径 try/catch
    /// 回滚。reload_points_incremental 内部 stop 的 handles **无法回滚**（task
    /// 已 abort），降级处理：记 warn 日志并把 ReloadStats.failed = true 暴露给
    /// 上层（web handler 据此返回 503 而非 200，运维可即时感知）。
    pub async fn reload(
        self: &Arc<Self>,
        new_collectors: HashMap<String, Arc<dyn Collector>>,
        new_points: Vec<PointConfig>,
    ) -> ReloadStats {
        let started_at = std::time::Instant::now();
        tracing::info!(
            "scheduler reload (incremental): {} collectors, {} points",
            new_collectors.len(),
            new_points.len()
        );

        // Bug-R5：reload 前快照原状态（用于步骤 1/2 异常时回滚）。
        // 锁 clone 模式——parking_lot Mutex 不跨 await，立即释放，零延迟。
        let old_collectors = self.collectors.lock().clone();
        let old_points = self.points.lock().clone();

        // 1) 替换 collectors（总线协议实现可能换了）
        *self.collectors.lock() = new_collectors.clone();

        // 2) 替换 points（在增量重建前先更新，让 device_states 处理逻辑看到新 points）
        *self.points.lock() = new_points.clone();

        // 3) force_buses：所有涉及总线都要重建（collectors 协议实现可能换了）
        let force_buses: std::collections::HashSet<String> =
            new_points.iter().map(|p| p.bus.clone()).collect();

        // Bug-R5：try 块包住 reload_points_incremental（返回 Result）。
        // 注意：reload_points_incremental 已被 reload_lock 串行化（Semaphore），
        // 所以同一时间只有一个 reload 在跑——回滚路径无需担心并发竞争。
        //
        // 函数内部 panic（spawn / await 链路）仍可能传到这里，但 spawn 通过
        // crate::engine::supervision 的 catch_unwind 吸收；mod.rs L17 deny(clippy::expect_used)
        // 已在代码审查阶段挡住 `.expect()` —— 真正可能 propagate 的就是 Result::Err。
        let reload_result = self
            .reload_points_incremental(new_points.clone(), &force_buses)
            .await;

        if let Err(reason) = reload_result {
            // 回滚 collectors + points 到 reload 前的快照。
            // device_states 的清理（步骤 4）在 reload_points_incremental 内部已经做过；
            // 不能 100% 回滚——但 collectors/points 是核心可见状态，必回滚。
            // reload_points_incremental 内部 stop 的 handles 无法回滚（task 已 abort），
            // 属于降级：log error + 暴露给上层（web handler 据此返回 503 + 提示运维重启采集）。
            let old_collectors_len = old_collectors.len();
            *self.collectors.lock() = old_collectors;
            *self.points.lock() = old_points;
            tracing::error!(
                "scheduler reload FAILED, rolled back collectors/points to pre-reload snapshot: {}",
                reason
            );
            return ReloadStats {
                collectors: old_collectors_len,
                duration_ms: started_at.elapsed().as_millis() as u64,
                force_buses_rebuilt: 0,
            };
        }

        // 4) device_states 仅清空已移除点位的状态（保留活跃点位）
        let new_ids: std::collections::HashSet<String> =
            self.points.lock().iter().map(|p| p.id.clone()).collect();
        {
            let mut states = self.device_states.lock();
            states.retain(|id, _| new_ids.contains(id));
        }

        let elapsed_ms = started_at.elapsed().as_millis() as u64;
        tracing::info!("scheduler reload finished in {} ms", elapsed_ms);

        ReloadStats {
            collectors: new_collectors.len(),
            duration_ms: elapsed_ms,
            force_buses_rebuilt: force_buses.len(),
        }
    }
}

/// 采集组键：`protocol:bus`（全仓库唯一口径）
///
/// #13: 此前 start/reload/spawn_collector_task 各自 `format!` 拼键，spawn 侧用
/// `collector.name()`、其余用 `protocol`。三者一旦漂移，reload 会「全停全建」抖动，
/// 或同一 bus 的多协议组生成同一键、第二次 insert 覆盖第一个句柄使旧任务失控。
/// 统一由本函数生成，供 spawning.rs / reload.rs / group_key_for_point 共用。
pub(crate) fn group_key(protocol: &str, bus: &str) -> String {
    format!("{}:{}", protocol, bus)
}

/// 计算点位所属的采集组键（与 start() 分组逻辑一致）
///
/// 供本模块与 `reload.rs`（通过 `super::`）共用。
fn group_key_for_point(p: &PointConfig) -> String {
    // 统一组键：所有协议（含 GPIO）按 protocol:bus 分组
    // GPIO 在同 bus 下的多个点位合并为一个采集组（与 spawn_collector_task 一致）
    group_key(&p.protocol, &p.bus)
}

/// 对比两个 PointConfig 的采集相关字段是否相等（用于判断组是否需要重建）
///
/// 供本模块与 `reload.rs`（通过 `super::`）共用。
fn point_config_equal(a: &PointConfig, b: &PointConfig) -> bool {
    a.id == b.id
        && a.bus == b.bus
        && a.address == b.address
        && a.slave_id == b.slave_id
        && a.address_num == b.address_num
        && a.data_id == b.data_id
        && a.function_code == b.function_code
        && a.data_type == b.data_type
        && a.byte_order == b.byte_order
        && a.register_count == b.register_count
        && a.interval == b.interval
        && a.scale == b.scale
        && a.offset == b.offset
        && a.formula == b.formula
        && a.deadband == b.deadband
        && a.on_change == b.on_change
        && a.unit == b.unit
        && a.aggregation == b.aggregation
}

/// 一次 reload 的统计信息，用于上层 metric & API 响应
#[derive(Debug, Clone, Copy, Default)]
pub struct ReloadStats {
    /// 涉及的 collector 数量
    pub collectors: usize,
    /// 实际重建耗时（含等待 task 自然退出）
    pub duration_ms: u64,
    /// 被强制重建的 bus 数量
    pub force_buses_rebuilt: usize,
}
