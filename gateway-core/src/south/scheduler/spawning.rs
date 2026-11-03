//! 任务孵化：按协议类型 spawn 采集 task
//!
//! 从 scheduler/mod.rs 拆分。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::config::PointConfig;
use crate::engine::model::Quality;
use crate::pipeline::CollectedBatch;
#[cfg(feature = "tracing")]
use tracing::Instrument;

use super::task::supervision_loop;
use super::Scheduler;

impl Scheduler {
    /// 统一采集任务孵化（所有协议：modbus/dlt645/cjt188/gpio/opcua/iec104/bacnet/s7comm/...）
    ///
    /// 通过 `Collector::read_batch` 统一读取：
    /// - Modbus 在 read_batch 内部按 (slave_id, fc) 批量合并
    /// - DLT645/CJT188/GPIO 默认逐点读取（read_batch 默认实现）
    ///
    /// 组键 `format!("{}:{}", collector.name(), bus_name)`，与 group_key_for_point 一致。
    pub(crate) fn spawn_collector_task(self: &Arc<Self>, bus_name: &str, points: Vec<PointConfig>) {
        let collector = match self.collectors.lock().get(bus_name) {
            Some(c) => c.clone(),
            None => {
                tracing::warn!("no collector for bus {}", bus_name);
                return;
            }
        };

        let self_arc = self.clone();
        let shutdown = self.shutdown_rx.clone();
        // 修复(审查#B1)：为本组单独建一个停止信号，供热重载精确停掉「这一组」，
        // 而不必依赖全局 shutdown（全局信号会连带停掉所有组）。
        let (group_stop_tx, group_stop_rx) = tokio::sync::watch::channel(false);
        // 退避间隔：取组内所有点位 current_interval 的最小值
        let point_ids: Vec<String> = points.iter().map(|p| p.id.clone()).collect();
        let self_for_interval = self.clone();
        let interval_fn = move || {
            let states = self_for_interval.device_states.lock();
            point_ids
                .iter()
                .map(|id| {
                    states
                        .get(id)
                        .map(|s| s.current_interval)
                        .unwrap_or(Duration::from_secs(1))
                })
                .min()
                .unwrap_or(Duration::from_secs(1))
        };

        let task_name = if points.len() == 1 {
            format!("collector@{}", points[0].id)
        } else {
            format!(
                "collector_{}@{}[{}]",
                collector.name(),
                bus_name,
                points.len()
            )
        };

        // #13: 组键统一为 `protocol:bus`（group_key），由该组点位自身推导，
        // 不再用 `collector.name()`。旧实现两处不一致：
        //  - 与 reload/group_key_for_point 口径漂移 → reload 判定"组变了"→ 全停全建抖动；
        //  - 同一 bus 挂多协议时两组生成同一键 → 第二次 insert 覆盖第一个句柄，
        //    前一个 task 失去跟踪变成孤儿，与后建 task 同时读写同一条 RS485 总线。
        let proto = points
            .first()
            .map(|p| p.protocol.as_str())
            .unwrap_or_else(|| collector.name());
        let key = super::group_key(proto, bus_name);
        // F-11：时序记录的归属信息（与组键口径一致），在采集循环尾部写入真实执行窗口
        let record_key = key.clone();
        let record_proto = proto.to_string();
        let record_bus = bus_name.to_string();
        // 修复(审查#B2)：组级读超时预算按「点位数 × 单点预算」自适应，
        // 不再硬编码 5s（原实现对逐点串行的 DLT645/CJT188 大组必然误判超时）。
        let group_timeout = collector.batch_timeout_budget(points.len());
        // 优化：Arc 包裹后闭包内只 clone Arc（引用计数），避免每采集周期深拷贝 Vec<PointConfig>
        let points = Arc::new(points);
        let handle = tokio::spawn(async move {
            supervision_loop(task_name, shutdown, group_stop_rx, interval_fn, move |interval| {
                let self_arc = self_arc.clone();
                let collector = collector.clone();
                let points = points.clone();
                // F-11：在本轮闭包内克隆时序归属键（每轮一次 String 拷贝，非每点）
                let record_key = record_key.clone();
                let record_proto = record_proto.clone();
                let record_bus = record_bus.clone();
                // tracing: 采集周期根 span，便于现场按 collector 定位采集链路。feature off 时 cfg 剥离为零成本。
                #[cfg(feature = "tracing")]
                let span = tracing::info_span!("collect", collector = %collector.name(), n_points = points.len());
                let fut = async move {
                    // F-11：记录本轮真实执行窗口（起止 wall-clock 毫秒），供调度时序端点
                    let t0_ms = super::timeline::now_ms();
                    // 批量读取：Modbus 内部按 (slave,fc) 合并；其他协议默认逐点
                    // 超时保护：防止协议栈卡死拖垮整个采集循环
                    let start = std::time::Instant::now();
                    let all_results = match tokio::time::timeout(
                        group_timeout,
                        collector.read_batch(&points),
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(_) => {
                            tracing::error!(
                                "collector {} read_batch timeout after {:?}, {} points skipped",
                                collector.name(),
                                group_timeout,
                                points.len()
                            );
                            // 超时：所有点位返回 Timeout 错误（区别于 IoError，
                            // 便于监控按"协议栈卡死/无响应"与"链路 IO 故障"分类告警）
                            points
                                .iter()
                                .map(|_| {
                                    Err(crate::south::protocol::CollectorError::Timeout(
                                        group_timeout,
                                    ))
                                })
                                .collect()
                        }
                    };
                    let elapsed_us = start.elapsed().as_micros() as u64;
                    for (point, result) in points.iter().zip(all_results.iter()) {
                        // 成功判定必须同时看错误和质量码：modbus `read_batch` 把失败
                        // 回填成 `Ok(DataPoint{quality:Bad})`（见 protocol/modbus/mod.rs），
                        // 只看 `result.is_ok()` 会让 gateway_collect_failures 对 Modbus
                        // 恒为 0，gateway_errors_total 随之失真。此处口径与
                        // `handle_results` 的 `record_failure` 判定保持一致。
                        let error = match result {
                            Ok(dp) if dp.quality == Quality::Good => None,
                            Ok(_) => Some(crate::engine::metrics::CollectErrorCode::Other),
                            Err(e) => Some(e.error_code()),
                        };
                        let success = error.is_none();
                        // 同时累计总线 / 点位维度，供运维报表与健康评分使用
                        self_arc.metrics.record_collect_scoped(
                            &point.bus,
                            &point.id,
                            point.device_id.as_deref().unwrap_or(""),
                            success,
                            elapsed_us,
                            error,
                        );
                        // P3：原实现在此向 EventBus 发布 DataCollected 事件。
                        // 该事件的生产侧订阅者为零（EventBus 全文只有测试在订阅），
                        // 且发布位于「每点每轮」热路径，每次都要 clone 一次 point.id
                        // 构造事件再被立即丢弃——纯开销。采集结果已由上面的
                        // record_collect_scoped 记入指标（成功/失败/耗时），语义完整。
                    }
                    self_arc.handle_results(&points, all_results).await;
                    self_arc.timeline.record(
                        &record_key,
                        &record_proto,
                        &record_bus,
                        interval,
                        t0_ms,
                        super::timeline::now_ms(),
                    );
                };
                #[cfg(feature = "tracing")]
                { fut.instrument(span) }
                #[cfg(not(feature = "tracing"))]
                { fut }
            })
            .await;
        });
        self.handles.lock().insert(key.clone(), handle);
        // 修复(审查#B1)：登记本组停止信号发送端；若该组键已存在（重建）则覆盖，
        // 旧 sender 随 drop 释放，旧 task 的 Receiver 收到 Err 也会退出（不泄漏）。
        self.group_stops.lock().insert(key, group_stop_tx);
    }

    /// 聚合窗口刷新任务
    pub(crate) fn spawn_aggregation_flush(self: &Arc<Self>) {
        let self_arc = self.clone();
        let shutdown = self.shutdown_rx.clone();
        let handle = crate::engine::supervision::spawn_supervised("aggregation_flush", move || {
            let self_arc = self_arc.clone();
            let mut shutdown = shutdown.clone();
            async move {
                loop {
                    tokio::select! {
                        _ = shutdown.changed() => {
                            if *shutdown.borrow() { break; }
                        }
                        _ = tokio::time::sleep(Duration::from_secs(1)) => {
                            let points_snapshot = self_arc.points.lock().clone();
                            let aggregated = self_arc.pipeline.flush_aggregated(&points_snapshot);
                            if !aggregated.is_empty() {
                                // 聚合下标错配修复：原实现为
                                //   `points_snapshot.iter().cloned().zip(aggregated.into_iter())`
                                // 但 `aggregated` 只包含「配置了聚合的点」的输出（同一点位跨多个
                                // 窗口时还会产出多条），与完整点位表的长度/顺序毫无对应关系。
                                // 按下标配对会把聚合值安到别的点位配置上 → 该值被别的点位的
                                // 死区/变化上传阈值过滤，配置读错、判定错。
                                // 改为按 `point_id` 查表配对；查不到配置的输出（点位已被删除或
                                // 重载移除）直接丢弃。PointConfig 原样透传即可——"聚合值不得再次
                                // 入窗"由 `CollectedBatch::flushed` 标记 + `process_flushed`
                                // 路径保证，不依赖改动点位配置字段。
                                let by_id: HashMap<&str, &PointConfig> = points_snapshot
                                    .iter()
                                    .map(|p| (p.id.as_str(), p))
                                    .collect();
                                let items = aggregated
                                    .into_iter()
                                    .filter_map(|dp| {
                                        by_id
                                            .get(dp.point_id.as_str())
                                            .map(|p| ((*p).clone(), dp))
                                    })
                                    .collect::<Vec<_>>();
                                if !items.is_empty() {
                                    let _ = self_arc
                                        .pipeline
                                        .submit(CollectedBatch::flushed(items))
                                        .await;
                                }
                            }
                        }
                    }
                }
            }
        });
        self.handles
            .lock()
            .insert("_aggregation_flush".to_string(), handle);
    }
}
