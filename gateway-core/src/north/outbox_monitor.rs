//! P8：outbox 积压深度采样（`gateway_outbox_pending` gauge 接线）
//!
//! 背景：`gateway_outbox_pending` 是 Prometheus / OTel 暴露的「北向待发积压」指标，
//! 是运维判断「上行通道是否堵死」的核心信号。但生产代码中**没有任何写入点**
//! （`Metrics::set_outbox_pending` 零调用），该 gauge 恒为 0——
//! outbox 积压到 10 万条时监控面板依然显示 0，告警规则永远不触发。
//!
//! ## 为什么不在 MQTT / Webhook / Kafka 各自的维护点写
//!
//! 1. **口径冲突**：三方共享同一个 outbox SQLite 文件（`protocol` 列隔离），
//!    任一方的 `pending_count(Some(protocol))` 只覆盖自己那一段。三个写入点
//!    互相覆盖同一个 gauge，最终显示的是「最后写入方」的协议数而非总量。
//! 2. **触发条件不齐**：MQTT 的维护块约 50s 才跑一次，且 MQTT 未启用时整个
//!    publish 循环不存在；Webhook 的 flush 周期默认 1s（每秒一次 COUNT 过重）；
//!    Kafka 客户端根本没有 metrics 字段。
//!
//! 故本模块在 north 层开一个**独立采样任务**：自己持有一条指向共享 outbox
//! 文件的连接，每 10s 统计一次**全协议合计**待发数并写入 gauge，
//! 与协议启用组合完全解耦。
//!
//! ## 降级语义
//!
//! - outbox 文件尚不存在（MQTT / Webhook 全未启用，无人创建）→ 报 0，不凭空建库。
//! - `OutboxStore::open` 降级到内存模式 → 计数恒为 0，是**假值**，
//!   不用它覆盖真实积压：只告警、不写 gauge。
//!
//! 采样走 `spawn_blocking`（rusqlite 是同步 API，不能占住 tokio 工作线程）。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::engine::error::GatewayResult;
use crate::engine::metrics::Metrics;
use crate::north::mqtt::OutboxStore;

/// 采样周期
///
/// 10s 兼顾两点：outbox 积压是「分钟级」量级事件，无需秒级；
/// 每次采样是一次带索引的 COUNT（`idx_outbox_status(status, id)`），
/// 10s 一次对 SQLite 读压力可忽略。
const SAMPLE_INTERVAL: Duration = Duration::from_secs(10);

/// 启动 outbox 积压采样任务（受 supervision 监管，panic 自动重启）。
///
/// 由 gatewayd bootstrap 调用一次，传入共享 outbox 文件路径与 metrics 实例。
pub fn spawn_outbox_pending_monitor(db_path: PathBuf, metrics: Arc<Metrics>) {
    crate::engine::supervision::spawn_supervised("outbox_pending_monitor", move || {
        let db_path = db_path.clone();
        let metrics = metrics.clone();
        async move {
            run_outbox_pending_monitor(db_path, metrics).await;
        }
    });
}

/// 采样循环主体。
async fn run_outbox_pending_monitor(db_path: PathBuf, metrics: Arc<Metrics>) {
    // 连接懒加载：outbox 文件可能尚未被任何北向通道创建。
    let mut store: Option<Arc<parking_lot::Mutex<OutboxStore>>> = None;
    loop {
        tokio::time::sleep(SAMPLE_INTERVAL).await;

        if store.is_none() {
            // 文件不存在 = 没有任何待发数据（MQTT/Webhook/Kafka 全未启用）→ 报 0。
            // 不主动建库，避免制造空 outbox 文件。
            if !db_path.exists() {
                metrics.set_outbox_pending(0);
                continue;
            }
            match OutboxStore::open(&db_path) {
                Ok(s) => {
                    if s.is_degraded() {
                        // 内存模式下计数恒为 0，会用假值覆盖真实积压 → 只告警不写。
                        tracing::warn!(
                            "outbox_pending_monitor: outbox 库降级到内存模式，积压采样不可用 — db_path={:?}",
                            db_path
                        );
                        continue;
                    }
                    store = Some(Arc::new(parking_lot::Mutex::new(s)));
                }
                Err(e) => {
                    tracing::warn!(
                        "outbox_pending_monitor: open outbox failed: {}, retry in {:?}",
                        e,
                        SAMPLE_INTERVAL
                    );
                    continue;
                }
            }
        }

        // 上面的分支要么给 `store` 赋值、要么 `continue`，此处必然是 `Some`。
        // 用 `match` 而非 `expect`/`unwrap`：非 test 编译下二者被 clippy deny。
        let s = match store.as_ref() {
            Some(s) => s.clone(),
            None => {
                metrics.set_outbox_pending(0);
                continue;
            }
        };
        let result = tokio::task::spawn_blocking(move || sample_pending(&s)).await;
        match result {
            Ok(Ok(n)) => metrics.set_outbox_pending(n),
            Ok(Err(e)) => tracing::warn!("outbox_pending_monitor: pending_count failed: {}", e),
            Err(e) => tracing::warn!("outbox_pending_monitor: sample task join error: {}", e),
        }
    }
}

/// 统计**全协议合计**待发数（`status = 0`，不分 protocol）。
///
/// 与 `gateway_outbox_pending` 的 HELP 文案「Outbox pending messages」一致——
/// 该 gauge 无 protocol 标签，语义就是全通道合计积压。
fn sample_pending(store: &parking_lot::Mutex<OutboxStore>) -> GatewayResult<i64> {
    store.lock().pending_count(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::north::mqtt::OutgoingMsg;
    use std::sync::atomic::Ordering;

    fn tempdir() -> PathBuf {
        use std::sync::atomic::AtomicU64;
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "outbox-monitor-test-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).expect("should succeed");
        p
    }

    /// P8 契约：`gateway_outbox_pending` 必须反映真实积压（而非恒 0）。
    ///
    /// 关键点：写入方（MQTT/Webhook/Kafka）各自持独立连接，但**共享同一文件**，
    /// 用 `protocol` 列隔离。采样任务用自己的一条连接统计 `status = 0` 的全部行，
    /// 因此能同时看到 mqtt / http / kafka 三段的积压。
    #[test]
    fn p8_pending_gauge_reflects_real_backlog() {
        let dir = tempdir();
        let path = dir.join("outbox.db");

        // 模拟三个协议各自写入待发消息
        {
            let mut store = OutboxStore::open(&path).expect("should succeed");
            store
                .persist(&OutgoingMsg {
                    data_id: 1,
                    data_ids: vec![1],
                    topic: "t/mqtt".into(),
                    payload: vec![1],
                    qos: 1,
                    protocol: "mqtt".into(),
                })
                .expect("should succeed");
            store
                .persist(&OutgoingMsg {
                    data_id: 2,
                    data_ids: vec![2],
                    topic: "http://example.invalid/".into(),
                    payload: vec![2],
                    qos: 1,
                    protocol: "http".into(),
                })
                .expect("should succeed");
        }

        // 采样任务用独立连接统计
        let store = Arc::new(parking_lot::Mutex::new(
            OutboxStore::open(&path).expect("should succeed"),
        ));
        let metrics = Metrics::new();
        let n = sample_pending(&store).expect("should succeed");
        assert_eq!(n, 2, "应统计到全协议合计 2 条待发");
        metrics.set_outbox_pending(n);

        assert_eq!(metrics.outbox_pending.load(Ordering::Relaxed), 2);
        let text = metrics.prometheus_export();
        assert!(
            text.contains("gateway_outbox_pending 2"),
            "Prometheus 导出应包含 gateway_outbox_pending 2，实际:\n{}",
            text
        );
    }

    /// P8 契约：ack 之后积压必须回落（否则 gauge 只会单调增长，变成假告警源）。
    #[test]
    fn p8_pending_gauge_falls_back_after_ack() {
        let dir = tempdir();
        let path = dir.join("outbox_ack.db");
        let mut writer = OutboxStore::open(&path).expect("should succeed");
        let id = writer
            .persist(&OutgoingMsg {
                data_id: 7,
                data_ids: vec![7],
                topic: "t/mqtt".into(),
                payload: vec![7],
                qos: 1,
                protocol: "mqtt".into(),
            })
            .expect("should succeed");
        writer.mark_acked(id).expect("should succeed");

        let store = Arc::new(parking_lot::Mutex::new(
            OutboxStore::open(&path).expect("should succeed"),
        ));
        assert_eq!(sample_pending(&store).expect("should succeed"), 0);
    }
}
