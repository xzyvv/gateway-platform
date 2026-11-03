//! 任务心跳监控（任务级软看门狗）
//!
//! 监控关键任务循环的心跳。如果某个循环超过超时时间未心跳，
//! 说明可能发生了死锁或卡死，记录严重错误日志供外部监控/重启。
//!
//! 与 `platform::hardware_watchdog` 的区别：
//! - 本模块只暴露 metrics + 日志，不直接 abort 进程
//! - `platform::hardware_watchdog` 才是真正会 abort 进程的硬件看门狗
//!
//! 设计：不直接调用 std::process::exit（避免数据丢失），
//! 而是通过 metrics 和日志暴露状态，由外部 systemd/supervisor 决定是否重启。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use once_cell::sync::OnceCell;
use tokio::sync::Mutex;

/// 全局看门狗实例
static WATCHDOG: OnceCell<Arc<Watchdog>> = OnceCell::new();

/// 单个被监控循环的心跳记录
pub struct Heartbeat {
    name: String,
    /// 上次心跳时刻（ms epoch）。`0` 表示「从未 kick 过」——此时 `is_stale` 必须
    /// 返 `false`（任务还在启动、第一次循环尚未跑完），否则短 timeout 配置下
    /// 启动期会被误报僵死（r54-WD2 修复）。
    last_kick: AtomicU64,
    timeout_ms: u64,
    /// r54-WD1：边沿触发标志，记录「当前是否已被判为 stale」。`Watchdog::run`
    /// 在 `is_stale` 返 true 时检查本字段：若为 false 则置 true 并记 metric
    /// （healthy→stale 边沿），后续 tick 同一 stall 期间不再重复计 metric——
    /// 避免 5min hang 在 5s 间隔下产生 60 次告警计数。
    stale_armed: AtomicU64,
}

impl Heartbeat {
    pub fn new(name: impl Into<String>, timeout_ms: u64) -> Self {
        // r54-WD2：last_kick 初值改为 0，含义「从未 kick」。这与 r15 之前
        // 「now_ms()」的初值不同——之前的问题是：短 timeout（如 1s）下，
        // 任务在 register() 与第一次循环 kick() 之间的启动期会被误判 stale。
        // 改为 0 + is_stale 特判后，契约变为：
        //   - last_kick = 0：从未 kick，is_stale 永远返 false（启动期免误报）
        //   - last_kick > 0：常规语义，elapsed = now - last_kick
        Self {
            name: name.into(),
            last_kick: AtomicU64::new(0),
            timeout_ms,
            stale_armed: AtomicU64::new(0),
        }
    }

    /// 更新心跳时间
    pub fn kick(&self) {
        self.last_kick.store(now_ms(), Ordering::Relaxed);
    }

    /// 是否超时
    pub fn is_stale(&self) -> bool {
        let last = self.last_kick.load(Ordering::Relaxed);
        // r54-WD2：last_kick=0 表示「从未 kick」。启动期内任务可能因业务初始化
        // 延迟（如连接 broker、加载配置）未到第一次循环 kick，saturating_sub
        // 会得到 now_ms() 这个大数，远大于 timeout_ms → 误报僵死。
        // 显式返 false 让 Watchdog::run 跳过该心跳。
        if last == 0 {
            return false;
        }
        // Bug 48: 使用 saturating_sub 防止时钟回拨（NTP 校时 / 系统时间被修改）时
        // 下溢导致 elapsed 变成超大值，错误触发 watchdog 超时告警。
        let elapsed = now_ms().saturating_sub(last);
        elapsed > self.timeout_ms
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn elapsed_ms(&self) -> u64 {
        let last = self.last_kick.load(Ordering::Relaxed);
        // r54-WD2：last_kick=0 时 elapsed 没有物理意义，返 0 让上游显示
        // 「刚启动 / 尚未 kick」而不被误报为 timeout_ms+ 的超时。
        if last == 0 {
            return 0;
        }
        // Bug 48: 同上，防止时钟回拨下溢
        now_ms().saturating_sub(last)
    }

    /// r54-WD1：边沿触发——`is_stale` 状态从 healthy 转到 stale 时返 true，
    /// 同一 stall 期间后续 tick 返 false 直到下一次 healthy→stale 转换。
    /// 配合 `Watchdog::run` 在「边沿=true」时 `inc_watchdog_timeout` 实现
    /// 一次 stall 只记 1 次告警计数。
    pub fn try_arm_stale(&self) -> bool {
        use std::sync::atomic::Ordering;
        if !self.is_stale() {
            // 已恢复健康：重置 armed 标志以便下次 stall 重新计 1 次。
            self.stale_armed.store(0, Ordering::Relaxed);
            return false;
        }
        // 当前 stale：用 compare_exchange 抢占「本次是首次进入 stale」的角色。
        // 返回 true 表示本 tick 是 healthy→stale 边沿。
        self.stale_armed
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    }
}

/// 看门狗：管理多个心跳，定期检查
pub struct Watchdog {
    /// 心跳弱引用列表：任务退出（`Arc<Heartbeat>` 被 drop）后条目自动失效，
    /// 在注册/检查时顺带清理，避免 reload 后旧条目永久驻留并被误报僵死。
    heartbeats: Mutex<Vec<Weak<Heartbeat>>>,
    check_interval: Duration,
    /// 1.2: 注入的指标实例
    metrics: Arc<crate::engine::metrics::Metrics>,
}

impl Watchdog {
    pub fn new(check_interval: Duration, metrics: Arc<crate::engine::metrics::Metrics>) -> Self {
        Self {
            heartbeats: Mutex::new(Vec::new()),
            check_interval,
            metrics,
        }
    }

    /// 注册一个心跳监控（同名注册幂等：替换旧条目而非追加）
    ///
    /// 背景：采集组任务 / 发布循环在配置热重载或重连后会以相同名字重新注册，
    /// 旧实现只 push、不去重也不清理，导致同名心跳在列表中无限累积，
    /// `stale_loops()` 因此出现重复项，并驱动自愈执行器对已不存在的循环反复
    /// 下达 RestartService（假阳性）。任务退出后其 `Arc<Heartbeat>` 被 drop，
    /// 对应的 Weak 条目也会在此处与检查循环中一并清理。
    pub async fn register(&self, name: impl Into<String>, timeout_ms: u64) -> Arc<Heartbeat> {
        let name = name.into();
        let hb = Arc::new(Heartbeat::new(name.clone(), timeout_ms));
        let mut list = self.heartbeats.lock().await;
        // 一并清理：已退出（Weak 升级失败）的条目 + 同名旧条目
        list.retain(|w| match w.upgrade() {
            Some(existing) => existing.name() != name,
            None => false,
        });
        list.push(Arc::downgrade(&hb));
        hb
    }

    /// 返回当前超时（stale）的循环名与滞后毫秒数。
    ///
    /// 供自愈执行器只读消费（F4 第二层）：本模块自身只 log + metrics，
    /// 由上层决定是否恢复。返回值按注册顺序排列，滞后毫秒可用于诊断。
    pub async fn stale_loops(&self) -> Vec<(String, u64)> {
        let mut hbs = self.heartbeats.lock().await;
        // 顺带清理任务已退出（Weak 失效）的条目，避免长期累积
        hbs.retain(|w| w.upgrade().is_some());
        hbs.iter()
            .filter_map(|w| w.upgrade())
            .filter(|hb| hb.is_stale())
            .map(|hb| (hb.name().to_string(), hb.elapsed_ms()))
            .collect()
    }

    /// 启动看门狗检查循环
    pub async fn run(self: Arc<Self>) {
        let mut interval = tokio::time::interval(self.check_interval);
        loop {
            interval.tick().await;
            let mut hbs = self.heartbeats.lock().await;
            hbs.retain(|w| w.upgrade().is_some());
            for hb in hbs.iter().filter_map(|w| w.upgrade()) {
                // r54-WD1：边沿触发 metric。`try_arm_stale` 在 healthy→stale
                // 转换瞬间返 true（同时记 metric + 日志），同一 stall 期间
                // 后续 tick 返 false —— 一次 hang 事件只产生 1 次告警计数。
                // 旧实现在 stall 期间每个 tick 都 inc_watchdog_timeout，5min
                // hang 在 5s 间隔下会产生 60 次告警，污染告警面板。
                // 语义：log 仍每 tick 输出（方便运维侧抓现场），metric 仅在
                // 边沿处增 1。
                if hb.try_arm_stale() {
                    tracing::error!(
                        "WATCHDOG: loop '{}' became stale! last heartbeat {}ms ago (timeout {}ms)",
                        hb.name(),
                        hb.elapsed_ms(),
                        hb.timeout_ms
                    );
                    // 更新 metrics，供 Prometheus 告警（边沿触发）
                    self.metrics.inc_watchdog_timeout();
                } else if hb.is_stale() {
                    // 同 stall 期间的后续 tick：仍 log（debug 级，避免刷屏）
                    // 但不计 metric——便于运维侧完整看到 stall 持续时间。
                    tracing::debug!(
                        "WATCHDOG: loop '{}' still stale ({}ms / {}ms)",
                        hb.name(),
                        hb.elapsed_ms(),
                        hb.timeout_ms
                    );
                }
            }
        }
    }
}

/// 初始化全局看门狗（在 main 中调用一次）
pub fn init(
    check_interval: Duration,
    metrics: Arc<crate::engine::metrics::Metrics>,
) -> Arc<Watchdog> {
    let wd = Arc::new(Watchdog::new(check_interval, metrics));
    let _ = WATCHDOG.set(wd.clone());
    wd
}

/// 获取全局看门狗实例
pub fn get() -> Arc<Watchdog> {
    WATCHDOG.get().cloned().unwrap_or_else(|| {
        let wd = Arc::new(Watchdog::new(
            Duration::from_secs(5),
            // fallback 路径：正常启动时 init() 已设置全局 Watchdog，
            // 此分支仅在 init() 未调用时触发（如单元测试），使用临时空 Metrics 实例
            Arc::new(crate::engine::metrics::Metrics::new()),
        ));
        let _ = WATCHDOG.set(wd.clone());
        wd
    })
}

fn now_ms() -> u64 {
    use std::time::SystemTime;
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{Heartbeat, Watchdog};
    use crate::engine::metrics::Metrics;
    use std::sync::Arc;
    use std::time::Duration;

    fn watchdog() -> Watchdog {
        Watchdog::new(Duration::from_secs(5), Arc::new(Metrics::new()))
    }

    /// 契约：同名心跳重复注册须替换而非追加（热重载/重连后不得累积重复条目）
    #[tokio::test]
    async fn register_same_name_keeps_single_entry() {
        let wd = watchdog();
        let h1 = wd.register("scheduler:g1", 30_000).await;
        let h2 = wd.register("scheduler:g1", 30_000).await;
        assert_eq!(
            wd.heartbeats.lock().await.len(),
            1,
            "同名注册应替换旧条目，而非累积"
        );
        assert_eq!(h2.name(), "scheduler:g1");
        drop((h1, h2));
    }

    /// 契约：不同名字各自保留（去重不能误删其他循环的条目）
    #[tokio::test]
    async fn register_distinct_names_are_kept() {
        let wd = watchdog();
        let h1 = wd.register("queue", 60_000).await;
        let h2 = wd.register("mqtt-publish", 60_000).await;
        assert_eq!(wd.heartbeats.lock().await.len(), 2);
        drop((h1, h2));
    }

    /// 契约：任务退出（句柄 drop）后条目自动清理，不再进入 stale_loops 误报
    #[tokio::test]
    async fn dropped_heartbeat_is_pruned() {
        let wd = watchdog();
        let h = wd.register("scheduler:gone", 1).await;
        assert_eq!(wd.heartbeats.lock().await.len(), 1);
        drop(h);
        let stale = wd.stale_loops().await;
        assert!(stale.is_empty(), "已退出任务不应出现在 stale_loops");
        assert_eq!(
            wd.heartbeats.lock().await.len(),
            0,
            "失效 Weak 条目应被清理"
        );
    }

    /// 契约：存活且及时 kick 的心跳不应被判为僵死
    #[tokio::test]
    async fn live_fresh_heartbeat_not_reported_stale() {
        let wd = watchdog();
        let h = wd.register("scheduler:live", 30_000).await;
        h.kick();
        assert!(wd.stale_loops().await.is_empty());
    }

    // ===== r54-WD1 / r54-WD2: 边沿触发 metric + 启动期免误报 =====

    /// r54-WD2：刚 register 尚未 kick 的心跳不应被判为 stale，
    /// 无论 timeout 多短（旧实现 last_kick=now_ms() 在 timeout=1ms 下立即超时）。
    #[tokio::test]
    async fn r54_never_kicked_heartbeat_is_not_stale() {
        let wd = watchdog();
        // 故意用极短 timeout 暴露旧实现的"启动期误报"路径
        let h = wd.register("just_registered", 1).await;
        assert!(
            !h.is_stale(),
            "从未 kick 的心跳必须永远不 stale（启动期免误报）"
        );
        assert_eq!(h.elapsed_ms(), 0, "从未 kick 时 elapsed 应为 0");
        // 等几毫秒再检查——仍未 kick，所以仍不 stale
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert!(!h.is_stale());
        drop(h);
    }

    /// r54-WD2：kick() 之后正常进入 stale 判定；timeout=1ms 足以让任意 sleep 后 stale。
    #[tokio::test]
    async fn r54_after_kick_stale_check_uses_elapsed() {
        let h = Heartbeat::new("after_kick", 5);
        h.kick();
        assert!(!h.is_stale(), "刚 kick 不应 stale");
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(h.is_stale(), "kick 20ms 后超过 5ms timeout 应 stale");
    }

    /// r54-WD1：try_arm_stale 边沿触发——同一 stall 期间多次调用只首次返 true。
    /// 模拟「5min hang 在 5s 间隔下被 Watchdog::run 检 60 次」的场景：
    /// 旧实现 inc 60 次 metric；新实现只 inc 1 次。
    #[tokio::test]
    async fn r54_try_arm_stale_edge_trigger_fires_once_per_stall() {
        let h = Heartbeat::new("hang_test", 1);
        // 模拟 stall：先 kick 让 last_kick>0，再等超时
        h.kick();
        tokio::time::sleep(Duration::from_millis(10)).await;

        // 第 1 次：healthy→stale 边沿，应返 true
        assert!(h.try_arm_stale(), "首次检测到 stale 应触发边沿");
        // 后续 N 次：同一 stall 期间，应返 false
        for i in 0..10 {
            assert!(
                !h.try_arm_stale(),
                "第 {} 次重复调用应不触发（避免 metric 重复计）",
                i + 2
            );
        }
    }

    /// r54-WD1：healthy 转换会重置 armed——恢复后再 stall 应再次触发。
    /// 这是边沿触发的正确性保证：stale→healthy→stale 周期里两次独立告警。
    #[tokio::test]
    async fn r54_try_arm_stale_resets_on_recovery() {
        let h = Heartbeat::new("recover_test", 1);
        h.kick();
        tokio::time::sleep(Duration::from_millis(10)).await;

        // 第 1 次 stall 边沿
        assert!(h.try_arm_stale(), "第 1 次 stall 应触发");
        // 同 stall 期间不触发
        assert!(!h.try_arm_stale(), "同 stall 不重复触发");

        // 模拟恢复：kick 重置 last_kick
        h.kick();
        // try_arm_stale 在 healthy 状态下应清零 armed（不触发）
        assert!(!h.try_arm_stale(), "healthy 状态不触发");

        // 再次进入 stall
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(
            h.try_arm_stale(),
            "恢复后再次 stall 应再次触发（独立告警事件）"
        );
    }

    /// r54-WD2 + r54-WD1 集成：`is_stale` 与 `try_arm_stale` 在 last_kick=0 时
    /// 协同：从未 kick 时既不 stale 也不触发告警（启动期完全免打扰）。
    #[tokio::test]
    async fn r54_never_kicked_does_not_arm_stale() {
        let h = Heartbeat::new("never_kicked", 1);
        // 从未 kick：is_stale 必 false
        assert!(!h.is_stale());
        // try_arm_stale 内部分支：is_stale=false → 走"恢复"分支 → 返 false
        // 重置 armed=0，不会触发告警。
        assert!(!h.try_arm_stale(), "从未 kick 时 try_arm_stale 必返 false");
    }
}
