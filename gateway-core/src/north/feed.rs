//! 北向专用无损队列（RealtimeBus 第 5 层）。
//!
//! 背景：`RealtimeBus` 是 broadcast 通道，每个订阅者各自维护游标，慢消费者
//! 掉队时只能 `Lagged` 丢帧。对 WS / 报警 / 快照而言「丢最新之外的旧帧」可以
//! 接受，但北向上报是**上行数据缺口**，不能静默漏批。
//!
//! 因此给北向单独开一条 mpsc 队列：
//! - 处理管道直接投递同一份 `Arc<Vec<DataPoint>>`（零拷贝共享）；
//! - 队列满时 `send().await` 挂起，把背压传回处理管道（宁可降速，不丢上报）；
//! - 广播总线只保留给可容忍丢帧的下游（WS / 报警 / 最新值快照）。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio::sync::mpsc;

use crate::engine::model::DataPoint;

/// 默认队列容量（单位是「批」，每批可含多个点位）。
pub const DEFAULT_NORTH_FEED_CAPACITY: usize = 1024;

/// 背压告警节流间隔：队列持续满时避免每批都刷一条日志。
const BACKPRESSURE_WARN_INTERVAL: Duration = Duration::from_secs(10);

/// 北向专用队列的发送端（可克隆，共享同一队列与计数）。
#[derive(Clone)]
pub struct NorthSender {
    tx: mpsc::Sender<Arc<Vec<DataPoint>>>,
    stats: Arc<NorthFeedStats>,
}

/// 投递统计（跨克隆共享）。
struct NorthFeedStats {
    /// 成功入队的批次数
    sent_batches: AtomicU64,
    /// 队列满导致 send 被挂起（背压生效）的次数
    backpressure_events: AtomicU64,
    /// 上一次背压告警时间（用于 10s 节流）
    last_warn: Mutex<Option<Instant>>,
}

/// 北向队列统计快照（测试 / 运维观测用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NorthFeedStatsSnapshot {
    pub sent_batches: u64,
    pub backpressure_events: u64,
    /// 队列容量（可容纳的批数）。
    pub capacity: usize,
    /// 当前积压批数（`capacity - 剩余槽位`）：判断「上游投递快于下游消费」的直接依据。
    pub pending_batches: usize,
}

impl NorthFeedStatsSnapshot {
    /// 转成 Prometheus 文本片段。
    ///
    /// 抓取时实时计算（`/apin` 直接读发送端计数器），不需要后台采样任务：
    /// 采样任务在停机时还得额外收口，而这里的读取本身是无锁原子操作。
    pub fn prometheus_lines(&self) -> String {
        format!(
            "# HELP gateway_north_feed_pending_batches North feed queue depth (batches)\n\
             # TYPE gateway_north_feed_pending_batches gauge\n\
             gateway_north_feed_pending_batches {}\n\
             # TYPE gateway_north_feed_capacity_batches gauge\n\
             gateway_north_feed_capacity_batches {}\n\
             # TYPE gateway_north_feed_sent_batches_total counter\n\
             gateway_north_feed_sent_batches_total {}\n\
             # TYPE gateway_north_feed_backpressure_events_total counter\n\
             gateway_north_feed_backpressure_events_total {}\n",
            self.pending_batches, self.capacity, self.sent_batches, self.backpressure_events
        )
    }
}

impl NorthSender {
    /// 投递一批点位。
    ///
    /// 队列满时不会丢弃，而是挂起当前任务等待空位——背压由此传回
    /// `ProcessingPipeline`，最终传导到采集调度（降低采集频率）。
    pub async fn send(
        &self,
        batch: Arc<Vec<DataPoint>>,
    ) -> Result<(), mpsc::error::SendError<Arc<Vec<DataPoint>>>> {
        // capacity() 返回剩余槽位，0 表示本次 send 会被挂起。
        if self.tx.capacity() == 0 {
            self.stats
                .backpressure_events
                .fetch_add(1, Ordering::Relaxed);
            self.warn_throttled();
        }
        self.tx.send(batch).await?;
        self.stats.sent_batches.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// 队列是否已关闭（接收端 drop）。
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }

    /// 统计快照。
    pub fn stats(&self) -> NorthFeedStatsSnapshot {
        let capacity = self.tx.max_capacity();
        let remaining = self.tx.capacity();
        NorthFeedStatsSnapshot {
            sent_batches: self.stats.sent_batches.load(Ordering::Relaxed),
            backpressure_events: self.stats.backpressure_events.load(Ordering::Relaxed),
            capacity,
            pending_batches: capacity.saturating_sub(remaining),
        }
    }

    /// 按 10s 节流输出背压告警。
    fn warn_throttled(&self) {
        let mut last = self.stats.last_warn.lock();
        let now = Instant::now();
        if last.is_none_or(|t| now.duration_since(t) >= BACKPRESSURE_WARN_INTERVAL) {
            *last = Some(now);
            tracing::warn!(
                target: "north_feed",
                "[hot-path] north feed full (capacity {}): backpressure applied, \
                 uplink slower than collection (cumulative {} event(s))",
                self.tx.max_capacity(),
                self.stats.backpressure_events.load(Ordering::Relaxed),
            );
        }
    }
}

/// 北向专用队列：创建后返回（发送端, 接收端）。
///
/// 发送端交给 `ProcessingPipeline`，接收端交给 `ConversionManager`。
pub struct NorthFeed;

impl NorthFeed {
    /// 创建队列，`capacity` 为可积压的批次数（至少 1）。
    pub fn channel(capacity: usize) -> (NorthSender, mpsc::Receiver<Arc<Vec<DataPoint>>>) {
        let (tx, rx) = mpsc::channel(capacity.max(1));
        (
            NorthSender {
                tx,
                stats: Arc::new(NorthFeedStats {
                    sent_batches: AtomicU64::new(0),
                    backpressure_events: AtomicU64::new(0),
                    last_warn: Mutex::new(None),
                }),
            },
            rx,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 小容量 + 慢消费者：send 会等待而不是丢批，最终接收端应收到全部批次。
    #[tokio::test]
    async fn send_blocks_instead_of_dropping_when_full() {
        let (tx, mut rx) = NorthFeed::channel(1);
        let total = 8u64;

        let producer = tokio::spawn(async move {
            for i in 0..total {
                let batch = Arc::new(vec![DataPoint::good(
                    &format!("p{}", i),
                    "d1",
                    i as f64,
                    i as f64,
                    "u16",
                )]);
                tx.send(batch).await.expect("接收端未关闭前 send 不应失败");
            }
            tx
        });

        // 慢消费：每批之间让出，制造队列满场景
        let mut received = Vec::new();
        while let Some(batch) = rx.recv().await {
            received.push(batch[0].point_id.clone());
            tokio::task::yield_now().await;
            if received.len() as u64 == total {
                break;
            }
        }

        let tx = producer.await.expect("producer 任务 panic");
        assert_eq!(received.len() as u64, total, "背压期间不得丢批");
        let stats = tx.stats();
        assert!(
            stats.backpressure_events > 0,
            "容量 1 + 慢消费应至少触发一次背压"
        );
        assert_eq!(stats.sent_batches, total);
        // 消费端取空后积压应归零，容量等于构造时传入的值。
        assert_eq!(stats.capacity, 1);
        assert_eq!(stats.pending_batches, 0, "全部消费后不得残留积压");
    }

    /// Prometheus 输出必须带齐 4 个指标名与当前值（Grafana 依赖固定名字）。
    #[test]
    fn prometheus_lines_expose_all_counters() {
        let snapshot = NorthFeedStatsSnapshot {
            sent_batches: 3,
            backpressure_events: 2,
            capacity: 10,
            pending_batches: 4,
        };
        let text = snapshot.prometheus_lines();
        assert!(
            text.contains("gateway_north_feed_pending_batches 4"),
            "{text}"
        );
        assert!(
            text.contains("gateway_north_feed_capacity_batches 10"),
            "{text}"
        );
        assert!(
            text.contains("gateway_north_feed_sent_batches_total 3"),
            "{text}"
        );
        assert!(
            text.contains("gateway_north_feed_backpressure_events_total 2"),
            "{text}"
        );
    }

    /// 接收端 drop 后 send 返回错误（供 pipeline 在停机窗口记录告警）。
    #[tokio::test]
    async fn send_fails_after_receiver_dropped() {
        let (tx, rx) = NorthFeed::channel(1);
        drop(rx);
        let err = tx
            .send(Arc::new(vec![DataPoint::good("p", "d", 1.0, 1.0, "u16")]))
            .await
            .expect_err("接收端已关闭，send 必须失败");
        assert!(err.0.len() == 1);
        assert!(tx.is_closed());
    }
}
