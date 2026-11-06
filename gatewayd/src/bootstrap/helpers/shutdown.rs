//! 优雅停机辅助函数（从 assemble.rs 拆出）
//!
//! 提供 `graceful_shutdown`：按序停 scheduler -> pipeline -> 北向 fan-out
//! -> cellular -> health monitors -> drain queue -> north servers -> mqtt。
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tracing::info;

use gateway_api::web::AppState;
use gateway_core::engine::metrics::Metrics;
use gateway_core::north::mqtt::MqttClient;
use gateway_core::north::server::Server;
use gateway_core::north::ConversionManager;
use gateway_core::queue::DataQueue;
use gateway_core::south::scheduler::Scheduler;

/// r64-SD1：北向 run 循环退出的等待上限。
///
/// webhook 的 `run` 循环在收到 `stop()` 后需等到下一次 `interval.tick()`
/// 才能检查 `stopping` 并 break（select 的丢失唤醒窗口），其间若 interval_ms
/// 较大（默认 1s）退出会稍有延迟，5s 覆盖默认间隔并留足余量；超时仅告警不
/// abort（abort 可能撕裂进行中的 SQLite 写），停机序继续推进。
const RUN_EXIT_TIMEOUT: Duration = Duration::from_secs(5);

/// r68-SD1：统一"await 后台任务 JoinHandle + 三态日志"模板。
///
/// r64/r66 累计在 graceful_shutdown 中写了 4 处相同的 `match tokio::time::timeout(...).await`
/// （webhook_run / conversion_run / self_heal.handle 等），分支结构与日志
/// 措辞高度雷同。抽到一处便于：
/// 1. 未来新增北向通道或后台 task 时直接复用，不必再抄一份 9 行 match。
/// 2. 三态日志（正常 / panic / 超时）口径统一——之前 webhook 写「run loop exited」、
///    conversion 写「manager exited」、self_heal
///    写「stopped」，运维查日志要按 component 名字匹配，行为一致后归一为
///    「`<component> exited cleanly / join error / still running」。
///
/// 调用方负责先发「信号」（如 `wh.stop()` / `conversion_mgr.shutdown_token().shutdown()`），
/// ——本 helper 只等 JoinHandle，不发信号。两步分离更明确：哪些任务需要先停，
/// 哪些只需等（spawn_supervised 内部已注入 panic 处理），调用处一目了然。
///
/// 超时不 abort（统一约定，参考 RUN_EXIT_TIMEOUT 注释）——abort 可能造成
/// in-flight SQLite 写或 mpsc channel 收发中途撕裂，导致数据丢失。
pub(crate) async fn await_join_with_log(
    component: &str,
    handle: tokio::task::JoinHandle<()>,
    timeout: Duration,
) {
    match tokio::time::timeout(timeout, handle).await {
        Ok(Ok(())) => tracing::info!("graceful_shutdown: {} exited cleanly", component),
        Ok(Err(e)) => tracing::warn!(
            "graceful_shutdown: {} join error (panicked?): {}",
            component,
            e
        ),
        Err(_) => tracing::warn!(
            "graceful_shutdown: {} still running after {:?}, continuing",
            component,
            timeout
        ),
    }
}

/// 北向通道统一的停机信号抽象（P3）。
///
/// 各北向客户端都提供 `async fn stop(&self)`，但类型互不相同（webhook /
/// HA / 报警 / 透传）。用本 trait 擦除类型后，停机序不再为每个通道手写一段
/// 「stop + await + 三态日志」。新增北向通道时只需：
/// 1. 启动处构造一个 [`NorthChannel`] 并 push 进通道表；
/// 2. 若该客户端尚无 `stop()`，在下方补一个 3 行 impl。
pub(crate) trait ChannelStopSignal: Send + Sync {
    fn signal_stop(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

impl ChannelStopSignal for gateway_core::north::webhook::WebhookClient {
    fn signal_stop(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(self.stop())
    }
}

impl ChannelStopSignal for gateway_core::engine::alarm::AlarmEngine {
    fn signal_stop(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(self.stop())
    }
}

impl ChannelStopSignal for gateway_core::north::server::MqttPacketPassthrough {
    fn signal_stop(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(self.stop())
    }
}

/// 内置模拟消费者的停机信号。
///
/// 与 webhook 的差别：模拟消费者的 `stop()` 内部**已经**等待 run 循环
/// 真正退出（上限 5s，超时仅告警），故 NorthChannel 侧的 run 句柄传 None，
/// 不再重复 await 同一个句柄——「发信号 + 等退出」在这一条通道上是一次完成的。
impl ChannelStopSignal for gateway_core::north::sim_consumer::SimConsumer {
    fn signal_stop(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(self.stop())
    }
}

/// 北向通道的统一停机描述符（P3）：停机信号 + run 句柄 + 等待上限。
///
/// stop 与 await 分离的语义与旧实现一致：先让 run 循环看到 stopping 标志
/// （下一轮迭代 break），再 await 其真正退出；超时仅告警、不 abort。
pub(crate) struct NorthChannel {
    /// 日志/诊断用通道名（如 "webhook" / "HA" / "alarm engine"）。
    name: &'static str,
    /// 停机信号；None 表示该通道无需信号（当前无此场景，保留扩展位）。
    signal: Option<Arc<dyn ChannelStopSignal>>,
    /// run 循环 JoinHandle；信号发出后 await 其真正退出。
    run: Option<tokio::task::JoinHandle<()>>,
    /// await run 的等待上限（统一 RUN_EXIT_TIMEOUT）。
    timeout: Duration,
}

impl NorthChannel {
    /// 由「客户端 + run 句柄」构造通道；客户端为 None（该通道未启用）时返回 None。
    ///
    /// 返回 `Option<Self>` 以便调用处直接用 `extend()` 收集：未启用的通道
    /// 自然不进入停机序，无需再写 `if let Some`。
    pub(crate) fn new<T>(
        name: &'static str,
        client: Option<Arc<T>>,
        run: Option<tokio::task::JoinHandle<()>>,
    ) -> Option<Self>
    where
        T: ChannelStopSignal + 'static,
    {
        client.map(|c| Self {
            name,
            signal: Some(c as Arc<dyn ChannelStopSignal>),
            run,
            timeout: RUN_EXIT_TIMEOUT,
        })
    }

    /// 停机：先发信号，再 await run 退出。
    pub(crate) async fn shutdown(self) {
        if let Some(sig) = self.signal {
            sig.signal_stop().await;
            tracing::info!("graceful_shutdown: {} stop signalled", self.name);
        }
        if let Some(h) = self.run {
            await_join_with_log(&format!("{} run loop", self.name), h, self.timeout).await;
        }
    }
}

/// 自愈执行器的停机信号与句柄（仅 `[self_heal]` opt-in 启用时 Some）。
///
/// 停机时先 `cancel` 再 `await` 句柄：确保在飞的自愈动作（如配置重载 / 蜂窝重启）
/// 完成后才继续拆解运行时，避免动作被打断造成半写，也避免停机途中被自愈动作抢跑。
pub(crate) struct SelfHealShutdown {
    pub(crate) cancel: tokio_util::sync::CancellationToken,
    pub(crate) handle: tokio::task::JoinHandle<()>,
}

/// `graceful_shutdown` 的输入集合：bootstrap 装配点唯一。
/// 用结构体承载以避免超长参数列表（clippy::too_many_arguments）。
pub(crate) struct ShutdownInputs {
    pub(crate) scheduler: Arc<Scheduler>,
    pub(crate) pipeline: Arc<gateway_core::pipeline::ProcessingPipeline>,
    pub(crate) conversion_mgr: Arc<ConversionManager>,
    pub(crate) mqtt: Option<Arc<MqttClient>>,
    pub(crate) queue: Arc<DataQueue>,
    pub(crate) state: Arc<AppState>,
    pub(crate) cellular: Option<Arc<gateway_core::platform::cellular::CellularManager>>,
    pub(crate) health_handles: Vec<tokio::task::JoinHandle<()>>,
    pub(crate) north_servers: Vec<Arc<dyn Server>>,
    /// 自愈执行器停机句柄（默认关闭时为 None）。
    pub(crate) self_heal: Option<SelfHealShutdown>,
    /// r66-SD1：ConversionManager run 循环 JoinHandle。
    /// `shutdown_token().shutdown()` 之后 await 其真正退出。
    pub(crate) conversion_run: Option<tokio::task::JoinHandle<()>>,
    /// P3：北向 run 循环通道表（webhook / HA / 报警 / 透传）。
    ///
    /// 每项自带「停机信号 + run 句柄 + 等待上限」，停机序按表顺序统一执行；
    /// 新增北向通道只需在启动处 push 一项，无需再改本结构体与停机分支。
    pub(crate) north_channels: Vec<NorthChannel>,
    /// r74-SD1：Web 优雅停机信号（step 13a 为 Some），停机序最前触发。
    pub(crate) web_shutdown: Option<gateway_api::web::ShutdownSignal>,
    /// r74-SD1：Web run 循环 JoinHandle，触发停机信号后 await。
    pub(crate) web_run: Option<tokio::task::JoinHandle<()>>,
    /// S04: 注入 metrics 用于上报 `shutdown_forced_total`（总宽限超时计数）
    pub(crate) metrics: Arc<Metrics>,
}

/// 看门狗解除守卫：Drop 时解除硬件看门狗的 armed 状态。
///
/// r15-SD3 (fix): 原实现在 `graceful_shutdown_inner` **最后一行**调用
/// `stop_platform_watchdog()` 解除 `/dev/watchdog` 的 armed 状态。该函数被
/// 外层 `tokio::time::timeout(80s)` 包裹——一旦总宽限命中（某子阶段卡死），
/// inner future 被 drop，**最后一行永不执行** → `/dev/watchdog` 保持 armed。
/// 现场表现：设备优雅停机后被 systemd 拉起，硬件看门狗仍在计时，无人喂狗
/// → 整机被误复位，表现为「重启后莫名其妙又重启一次」，且日志里没有任何线索。
///
/// 守卫放在**外层** `graceful_shutdown`，其 Drop 在以下三种情况都会执行：
/// - inner 正常返回
/// - 外层 timeout 命中（inner future 被 drop）
/// - inner panic（栈展开）
///
/// `stop_platform_watchdog()` 本身幂等（fd 被 take 后二次调用是 no-op）。
struct WatchdogDisarmGuard;

impl Drop for WatchdogDisarmGuard {
    fn drop(&mut self) {
        gateway_core::platform::hardware_watchdog::stop_platform_watchdog();
    }
}

/// 优雅停机：先停采集 -> drain queue -> 停 MQTT -> 释放资源。
///
/// #10: 完整停机序列——stop scheduler -> drain pipeline -> stop 北向 fan-out
///        -> stop ntp/health/cellular -> drain queue -> stop mqtt -> 关闭资源
/// S04: 优雅停机总宽限（默认 80s，留 10s 给 systemd TimeoutStopSec=90s 余量）。
///
/// 整体 wrap `tokio::time::timeout`——任一子阶段（self-heal / scheduler / pipeline /
/// fan-out / cellular / health / queue drain / north servers / mqtt）累计超过 80s，
/// 立即放弃剩余步骤，+1 `shutdown_forced_total`，避免 systemd SIGKILL 造成 SQLite 写不
/// 完整 / MQTT 通道残留未发布消息。各子阶段的 inner timeout（3s/5s/30s/35s）保留——
/// 它们是单步硬上限，总宽限是"所有步骤累计软上限"，互不冲突。
async fn graceful_shutdown_inner(inputs: ShutdownInputs) {
    let ShutdownInputs {
        scheduler,
        pipeline,
        conversion_mgr,
        mqtt,
        queue,
        state: _state,
        cellular,
        health_handles,
        north_servers,
        self_heal,
        // r66-SD1：ConversionManager run 循环 JoinHandle，shutdown_token() 之后 await。
        conversion_run,
        // P3：北向 run 循环通道表，queue drain 之后按表顺序统一 stop + await。
        north_channels,
        // r74-SD1：Web 停机信号 + run 句柄，停机序最前触发。
        web_shutdown,
        web_run,
        // S04: metrics 由外层 graceful_shutdown 包装函数的 timeout 命中时累加；
        // inner 不使用，仅保留字段引用计数。
        metrics: _,
    } = inputs;

    // r74-SD1：Web/API 优雅停机——**停机序第一步**。
    //
    // 为什么最前：旧实现 web_server 的 `spawn_supervised` 句柄被丢弃且无关闭原语，
    // 整个停机窗口（最多 80s）内 HTTP 仍接受写请求，与后续的 scheduler.stop、
    // `queue.drain_and_wait()`、mqtt/north_servers 退出并发——可能出现
    // 「drain 期间下发新配置 / 控制指令打到正在退出的子系统」。
    // 先触发 with_graceful_shutdown（停止接受新连接，等待在飞请求结束），
    // 再 await run 句柄，确保控制面在采集/北向拆解之前已静默。
    if let Some(sig) = web_shutdown {
        sig.shutdown().await;
        tracing::info!("graceful_shutdown: web server graceful shutdown signalled");
    }
    if let Some(h) = web_run {
        await_join_with_log("web server", h, RUN_EXIT_TIMEOUT).await;
    }

    // 自愈执行器优先停机：先 cancel 再 await，确保在飞动作完成后再拆解运行时。
    // r68-SD1：统一 await_join_with_log，35s 仍保留——自愈动作如配置重载可能
    // 涉及大量 IO，比北向 run 循环长）。
    if let Some(sh) = self_heal {
        sh.cancel.cancel();
        await_join_with_log("self-heal executor", sh.handle, Duration::from_secs(35)).await;
    }

    scheduler.stop().await;
    tracing::info!("graceful_shutdown: scheduler stopped");

    pipeline.shutdown();
    tokio::time::sleep(Duration::from_millis(500)).await;
    tracing::info!("graceful_shutdown: pipeline flushed");

    // 停北向 fan-out（ConversionManager 与所有 sink/connector 共享同一关闭信号）。
    // 必须在 drain queue 之前触发：确保 pipeline 最后一批数据已 fan-out 到
    // MQTT connector（入队）等下游，避免 drain 完成后还有迟到数据入队。
    // 触发后各 sink task 会自行把通道内积压数据写完再退出，后续 queue drain /
    // north servers stop / mqtt stop 的时间即为它们收尾的天然宽限期。
    conversion_mgr.shutdown_token().shutdown();
    tracing::info!(
        "graceful_shutdown: north fan-out signalled ({} connector(s))",
        conversion_mgr.connector_count()
    );
    // r66-SD1：await ConversionManager run 循环真正退出（旧实现只发信号）。
    //
    // `ConversionManager::run` 是 RealtimeBus 唯一订阅者，shutdown 信号在
    // select 中以 biased 优先级立即触发，下次 loop 检查 `shutdown.wait()` 即 break。
    // 5s 超时与 webhook 一致；超时不 abort（spawn_supervised 的 task 若被
    // 中途 abort 可能撕裂 RealtimeBus sender 计数），仅告警继续。
    // r68-SD1：复用 await_join_with_log——component 为 "conversion manager"。
    if let Some(h) = conversion_run {
        await_join_with_log("conversion manager", h, RUN_EXIT_TIMEOUT).await;
    }

    if let Some(cell) = cellular {
        cell.stop().await;
        tracing::info!("graceful_shutdown: cellular stopped");
    }

    for h in health_handles {
        h.abort();
    }
    tracing::info!("graceful_shutdown: health monitors stopped");

    // r58-SD1 修复：用 `DataQueue::drain_and_wait` 替换手写 len() 轮询。
    //
    // 旧实现手工 `queue.len().await` 做 200ms 轮询，最多等 30s。
    //   1) 轮询频率比 `drain_and_wait` 内部 100ms 慢一倍，shutdown 关键路径
    //      多浪费最多 ~10s（30s 内多次 200ms 间隔叠加）；
    //   2) 30s 硬上限与外层 `TOTAL_DEADLINE=80s` 失配——若 30s 不到队列空但
    //      80s 仍有余量，drain 会假超时退出，约 50s 完全浪费。
    //   3) 30s 到时仅 warn 但不强行推进——后续 mqtt/north_servers 继续跑，
    //      实际并未"放弃 drain"，造成日志"queue 30s timeout"与真实 drain
    //      完成时间不一致，误导运维定位。
    //
    // 修复策略：用 drain_and_wait（queue 自带语义化接口）取代 len() 轮询。
    // 并用 `tokio::time::timeout` 统一节流为 30s（保留与原实现的预算一致）。
    // 后续 mqtt/north_servers 仍按原序执行，timeout 命中时仅 warn，不强行
    // 推进 shutdown——未排空的数据落 SQLite 下次启动自动 replay。
    let drain_deadline = Duration::from_secs(30);
    let drain_start = std::time::Instant::now();
    let drain_result = tokio::time::timeout(drain_deadline, queue.drain_and_wait()).await;
    let drain_elapsed = drain_start.elapsed();
    match drain_result {
        Ok(()) => {
            tracing::info!("graceful_shutdown: queue drained in {:?}", drain_elapsed);
        }
        Err(_elapsed) => {
            // 超时：drain_and_wait 未在 30s 内完成，记录剩余条数供运维定位
            // （`len()` 在 mpsc 关闭 / storage 损坏时返 0，兼容容错）。
            let remaining = queue.len().await;
            tracing::warn!(
                "graceful_shutdown: queue drain timeout ({:?}/{:?}), {} item(s) remaining (persisted in SQLite, will replay on next start)",
                drain_elapsed,
                drain_deadline,
                remaining
            );
        }
    }

    // P3：北向 run 循环统一停机——按通道表顺序逐个「发信号 -> await 退出」。
    //
    // 时机保持不变：queue.drain_and_wait 之后、north_servers stop 之前。
    // 表内顺序即停机序（webhook -> HA -> 报警 -> 透传），其中报警/透传
    // 必须在末尾 `mqtt.stop()` 之前停干净——否则停机窗口内仍会评估报警 / 处理
    // 下行透传指令并向已停的 MQTT 上报，pending 只能等下次启动 replay。
    for ch in north_channels {
        ch.shutdown().await;
    }

    // r60-SD1：north_servers 停机并行化。
    //
    // 旧实现 `for srv in &north_servers { tokio::time::timeout(3s, srv.stop()).await }`
    // 串行 stop：N 个 server 累计最多 N×3s。在 N=3（opcua + modbus + modbus_passthrough）
    // 时最多 9s 才能从 server 阶段退出，与外层 `TOTAL_DEADLINE=80s` 失配——
    // 9s 浪费在 server 阶段，而后续 mqtt.stop(5s) 又串行 5s。
    // 改为 spawn 任务并行 stop + tokio::join! 等价（gatewayd 不直接依赖 futures crate），
    // 3s 超时对**整组**生效（与 connection.rs 的 join3 风格一致），最多 3s 完成
    // server 阶段；后续 mqtt.stop 仍有 5s。server 内部 listen socket drop +
    // in-flight 连接 drain 是独立任务，互相不共享 fd，join 不会产生新的数据
    // 竞争——仅节省串行等待时间。
    if !north_servers.is_empty() {
        let stop_handles: Vec<tokio::task::JoinHandle<&'static str>> = north_servers
            .iter()
            .map(|srv| {
                let srv = srv.clone();
                tokio::spawn(async move {
                    let name = srv.name();
                    let _ = tokio::time::timeout(Duration::from_secs(3), srv.stop()).await;
                    name
                })
            })
            .collect();
        let mut names: Vec<&'static str> = Vec::with_capacity(stop_handles.len());
        for h in stop_handles {
            match h.await {
                Ok(name) => names.push(name),
                Err(e) => tracing::warn!(
                    "graceful_shutdown: north_server stop task join failed: {}",
                    e
                ),
            }
        }
        tracing::info!(
            "graceful_shutdown: {} north server(s) stopped (parallel): {:?}",
            names.len(),
            names
        );
    }

    if let Some(mqtt_client) = mqtt {
        let _ = tokio::time::timeout(Duration::from_secs(5), mqtt_client.stop()).await;
    }

    // r15-WD2: 最后解除硬件看门狗 armed 状态。
    //
    // 放在停机序**末尾**：停机期间硬件看门狗仍在保护进程（写卡死 → 整机
    // 复位），全部资源释放完成后再写 magic char 'V' 解除 armed。
    //
    // 历史问题：等价逻辑（`Watchdog::stop`）**全仓零调用**——Linux 优雅
    // 停机从不写 'V'，`/dev/watchdog` 保持 armed；下次上电后若无人喂狗，
    // 整机被 watchdog 误复位，表现为"设备重启后莫名其妙又重启一次"。
    gateway_core::platform::hardware_watchdog::stop_platform_watchdog();

    info!("graceful shutdown complete");
}

/// S04: 外层 `graceful_shutdown` 包装——`tokio::time::timeout` 命中总宽限时
/// `+1` `shutdown_forced_total` 并 warn。正常完成路径（inner 在 80s 内返回）
/// 不增加指标。
pub(crate) async fn graceful_shutdown(inputs: ShutdownInputs) {
    const TOTAL_DEADLINE: Duration = Duration::from_secs(80);
    // r15-SD3 (fix): 守卫必须在**外层**——只有它能覆盖「外层 timeout 命中」
    // 与「inner panic 栈展开」两种 inner 最后一行不可达的路径。
    let _watchdog_disarm = WatchdogDisarmGuard;
    let inputs_for_outer = inputs;
    let timed_out = tokio::time::timeout(
        TOTAL_DEADLINE,
        graceful_shutdown_inner(ShutdownInputs {
            metrics: inputs_for_outer.metrics.clone(),
            ..inputs_for_outer
        }),
    )
    .await;
    if timed_out.is_err() {
        // 走 metrics facade（`inc_shutdown_forced`），不直写子结构字段——
        // 直写会让「facade 是唯一写入入口」的约定失效（其它写入点已全部走 facade）。
        inputs_for_outer.metrics.inc_shutdown_forced();
        tracing::warn!(
            "graceful_shutdown: total deadline (80s) exceeded, forcing exit (drained not complete, possible data loss in unsent queue/in-flight publishes)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::await_join_with_log;
    use std::time::Duration;

    /// r68-SD1 契约：正常完成路径——`handle` 立即 resolve 时不应阻塞到 timeout。
    /// 这是 4 个调用点（self-heal / conversion_mgr / webhook）的共同基础。
    #[tokio::test]
    async fn r68_await_join_with_log_normal_completion() {
        let h = tokio::spawn(async {});
        // 1s 宽限远超实际需要（future 立即完成），用以检测「不必要的等待」
        let start = std::time::Instant::now();
        await_join_with_log("test-normal", h, Duration::from_secs(1)).await;
        assert!(
            start.elapsed() < Duration::from_millis(200),
            "r68: 正常完成路径必须立即返回，不应等待 timeout"
        );
    }

    /// r68-SD1 契约：超时路径——`handle` 长时间不 resolve 时必须在 timeout 后
    /// 退出，**不 abort**（abort 可能撕裂 in-flight IO）。本测试验证总耗时
    /// 接近 timeout 而非持续挂起。
    #[tokio::test]
    async fn r68_await_join_with_log_timeout_returns_without_abort() {
        // spawn 一个 30s 阻塞 task；helper 在 100ms timeout 后必须返回
        let h = tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        let start = std::time::Instant::now();
        await_join_with_log("test-timeout", h, Duration::from_millis(100)).await;
        let elapsed = start.elapsed();
        // 100ms ± 合理 jitter；不应远大于 timeout（否则说明在等真的完成）。
        // 关键约束：helper 不 abort——spawn 的 30s sleep task 仍在运行。
        // 离开测试函数时由 tokio runtime drop 自动清理（detached task
        // 在 runtime shutdown 时被取消）。
        assert!(
            elapsed >= Duration::from_millis(100) && elapsed < Duration::from_millis(500),
            "r68: 超时路径必须在 ~timeout 后返回，elapsed={:?}",
            elapsed
        );
    }

    /// r68-SD1 契约：panic 路径——`handle` 内的 future panic 时仍必须 resolve。
    /// 4 个调用点都依赖此保证（self-heal / conversion_mgr / webhook
    /// 都用 `spawn_catch_unwind` 或 `spawn_supervised` 包裹 panic，handle
    /// 会 resolve 为 Ok(())——但 spawn_blocking 路径的 panic 不会被
    /// catch_unwind 吸收，会变成 JoinError。本测试覆盖「未受保护的 panic」场景。
    /// ——helper 必须不能因为 JoinError 而 panic 自身。
    #[tokio::test]
    async fn r68_await_join_with_log_panic_does_not_propagate() {
        let h = tokio::spawn(async {
            panic!("r68: injected panic");
        });
        // 必须不 panic，不挂起；用 timeout 兜底以防 helper 实现错误
        let res = tokio::time::timeout(
            Duration::from_secs(1),
            await_join_with_log("test-panic", h, Duration::from_secs(2)),
        )
        .await;
        assert!(
            res.is_ok(),
            "r68: panic 路径必须在 timeout 内返回（helper 不能因 JoinError 而 panic 自身）"
        );
    }
}
