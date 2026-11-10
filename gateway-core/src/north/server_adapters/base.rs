//! 北向从站适配器通用生命周期内核
//!
//! 把 OPC UA / Modbus 等从站适配器共有的「内部 task + mpsc channel +
//! 背压丢弃计数 + 在途深度 metric + 优雅停机」收敛为单一 [`AdapterCore`]。
//!
//! 差异点只有「消费一批点位后如何写回从站」——由构造时传入的 `apply`
//! 回调表达（OPC UA 写地址空间、Modbus 写寄存器空间）。
//!
//! 语义（与重构前逐字保持一致）：
//! - `send` 非阻塞 try_send；满则丢弃 + warn + 计数 metric
//! - 内部 task 串行消费，避免与外部锁竞争
//! - 停机：内部 task 监听 `ShutdownToken`，触发后 drain 收尾队列再退出

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::engine::metrics::{Metrics, SinkId};
use crate::engine::model::DataPoint;
use crate::engine::shutdown::ShutdownToken;

use super::ADAPTER_CHANNEL_CAPACITY;

/// 北向从站适配器通用内核
///
/// 持有 mpsc 发送端、丢弃/在途计数与指标句柄；内部消费 task 由构造时启动。
pub(super) struct AdapterCore {
    tx: mpsc::Sender<Vec<DataPoint>>,
    /// 累计丢弃的批次数（通道满时）
    dropped: Arc<AtomicU64>,
    /// 当前 mpsc 通道中待消费的批次数（实时深度）
    in_flight: Arc<AtomicUsize>,
    /// 注入的指标实例
    metrics: Arc<Metrics>,
    /// metrics key（历史值，如 "opcua" / "modbus"）
    name: &'static str,
    /// P2-2：指标固定槽位。由 `SinkId` 枚举决定，`name` 由它派生，
    /// 保证「日志名」与「指标槽位」不可能不一致。
    sink: SinkId,
}

impl AdapterCore {
    /// 创建内核并启动内部消费 task
    ///
    /// - `sink`：内置 sink 标识（`SinkId` 枚举，P2-2）。metrics 标签与 task 名
    ///   均由它派生，编译期即可防止 sink 名拼错导致指标落到错误的槽位。
    /// - `apply`：消费一批点位时的写回动作（同步、在内部 task 中串行执行）。
    ///   返回 `Err(())` 表示**「写入无意义」**（典型：server 未运行 / 内部状态损坏），
    ///   内核会调用 `on_failure` 触发 server 重连/重启并**重试一次**本批。
    ///   `Ok(())` 表示写入完成（包括内存写入成功、外部可读性由 server 健康度保证）。
    /// - `on_failure`：apply 失败后被调用的恢复动作，典型实现 = `server.stop() + server.start()`。
    ///   该闭包本身不应返回 Result：失败时只累计 `sink_reconnect_exhausted_total`，避免无限重连风暴。
    ///
    /// Bug 187 (N17): 旧 API `apply: Fn(&[DataPoint])` 完全不感知 server 状态，
    /// server 进程崩溃 / 被 OOM kill 后 sink 还在写，外部主站读到的是「无变化」
    /// —— 静默失联无告警。改造为：apply 主动检查 server.is_running()，不健康时
    /// 触发 on_failure 重连/重启并重试一次。
    pub(super) fn new<F, G>(
        sink: SinkId,
        shutdown: ShutdownToken,
        metrics: Arc<Metrics>,
        apply: F,
        on_failure: G,
    ) -> Arc<Self>
    where
        F: Fn(&[DataPoint]) -> Result<(), ()> + Send + 'static,
        G: Fn() + Send + 'static,
    {
        let name: &'static str = sink.as_str();
        let (tx, mut rx) = mpsc::channel::<Vec<DataPoint>>(ADAPTER_CHANNEL_CAPACITY);
        let dropped = Arc::new(AtomicU64::new(0));
        let in_flight = Arc::new(AtomicUsize::new(0));
        let in_flight_for_task = in_flight.clone();
        let metrics_for_task = metrics.clone();

        crate::engine::supervision::spawn_catch_unwind(format!("{name}_sink_loop"), async move {
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.wait() => break,
                    opt = rx.recv() => {
                        match opt {
                            Some(points) => {
                                // Bug 187 (N17): apply 失败 → on_failure + 重试一次。
                                if apply(&points).is_err() {
                                    metrics_for_task
                                        .lifecycle.sink_reconnect_attempts
                                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                    tracing::warn!(
                                        target: "north.server_sink",
                                        "{name} apply failed (server unhealthy), triggering on_failure and retrying this batch once"
                                    );
                                    on_failure();
                                    // 重试一次；仍失败则记 exhausted，本批计入 dropped。
                                    if apply(&points).is_err() {
                                        metrics_for_task
                                            .lifecycle.sink_reconnect_exhausted
                                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                        // A2：`saturating_sub` 取代裸 `- 1`：
                                        // 计数一旦被并发路径压到 0，裸减会溢出 panic
                                        // （debug）把整个 sink task 带走。
                                        let n = in_flight_for_task
                                            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed)
                                            .saturating_sub(1);
                                        metrics_for_task.inc_sink_dropped(sink);
                                        // A2：丢弃分支原先只减了计数、没更新 gauge，
                                        // 导致 `sink_channel_depth` 永久偏高
                                        // （每丢一批偏 1）→ 现场看深度只涨不落。
                                        metrics_for_task.set_sink_channel_depth(sink, n as i64);
                                        let total = metrics_for_task.sink_dropped_total(sink);
                                        tracing::error!(
                                            target: "north.server_sink",
                                            "{name} apply still failed after on_failure, batch dropped (sink_dropped_total={}, channel_depth={})",
                                            total, n
                                        );
                                        continue;
                                    } else {
                                        tracing::info!(
                                            target: "north.server_sink",
                                            "{name} apply recovered after on_failure"
                                        );
                                    }
                                }
                                let depth = in_flight_for_task
                                    .fetch_sub(1, Ordering::Relaxed)
                                    .saturating_sub(1);
                                metrics_for_task.set_sink_channel_depth(sink, depth as i64);
                            }
                            None => break,
                        }
                    }
                }
            }
            // 退出时排空：每批都尝试一次。
            while let Ok(points) = rx.try_recv() {
                let _ = apply(&points);
                let depth = in_flight_for_task
                    .fetch_sub(1, Ordering::Relaxed)
                    .saturating_sub(1);
                metrics_for_task.set_sink_channel_depth(sink, depth as i64);
            }
            metrics_for_task.set_sink_channel_depth(sink, 0);
            tracing::info!("{name}_sink: internal task exited");
        });

        Arc::new(Self {
            tx,
            dropped,
            in_flight,
            metrics,
            name,
            sink,
        })
    }

    /// metrics key（与 `send` 内部一致）
    pub(super) fn name(&self) -> &'static str {
        self.name
    }

    /// 投递一批采集数据到通道（非阻塞 try_send）
    pub(super) fn send(&self, points: Vec<DataPoint>) {
        let name = self.name;
        let sink = self.sink;
        // A2：`in_flight` 必须**先于** `try_send` 自增。
        //
        // 旧实现把 `fetch_add(1)` 放在 `try_send` 成功之后，于是存在一个竞态窗口：
        // 消费端拿到批次后会立刻 `fetch_sub(1)`，若此时消费端比生产者的
        // `fetch_add(1)` 先跑，就会把 `in_flight` 从 0 减到 `usize::MAX`
        // （debug 构建下紧接着的 `- 1` 直接 panic → sink task 挂掉 →
        //   通道关闭 → 后续数据全丢；release 下 gauge 变成天文数字）。
        // 正确顺序：先把批次"挂上号"再交给通道，失败时回滚。
        let depth = self.in_flight.fetch_add(1, Ordering::Relaxed) + 1;
        match self.tx.try_send(points) {
            Ok(()) => {
                self.metrics.inc_sink_send(sink);
                self.metrics.set_sink_channel_depth(sink, depth as i64);
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                // 未入队 → 回滚自增
                let depth = self
                    .in_flight
                    .fetch_sub(1, Ordering::Relaxed)
                    .saturating_sub(1);
                self.metrics.set_sink_channel_depth(sink, depth as i64);
                let n = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
                tracing::warn!(
                    target: "north.server_sink",
                    "{name} channel full, dropped batch (total={n})"
                );
                self.metrics.inc_sink_dropped(sink);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                // Bug 153: 消费者内部 task 已退出（panic 被 catch 或提前 break）
                // 导致通道关闭；旧实现空处理 `=> {}`，OPC UA/Modbus 服务端数据
                // 从此永久静默丢失且无任何痕迹。现记 error + 计入 dropped 指标，
                // 暴露"接收端已死"的故障。
                let depth = self
                    .in_flight
                    .fetch_sub(1, Ordering::Relaxed)
                    .saturating_sub(1);
                self.metrics.set_sink_channel_depth(sink, depth as i64);
                let n = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
                tracing::error!(
                    target: "north.server_sink",
                    "{name} channel closed (consumer task exited), dropped batch (total={n})"
                );
                self.metrics.inc_sink_dropped(sink);
            }
        }
    }
}
