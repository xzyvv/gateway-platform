//! 任务孵化：按协议类型 spawn 采集 task
//!
//! 从 scheduler/mod.rs 拆分。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::config::PointConfig;
use crate::engine::model::{DataPoint, Quality};
use crate::pipeline::CollectedBatch;
use crate::south::protocol::{CollectorError, CollectorResult, ResultSlots};
#[cfg(feature = "tracing")]
use tracing::Instrument;

use super::task::supervision_loop;
use super::Scheduler;

/// 缺陷 #7：把共享进度槽聚合成与入参等长的结果向量。
///
/// 每个点位恰好一个结果：已写入的 `Some` 用 `take()` 消费（`CollectorError` 未实现
/// Clone），未写入的 `None` 回填 `Timeout`；长度恒为 `n`。纯数值逻辑，不依赖 tokio
/// 时序，便于单测直接锁定「成功保留 / 未完成判超时 / 不漏报重复」三不变量。
pub(crate) fn collect_results(
    slots: ResultSlots,
    n: usize,
    timeout: Duration,
) -> Vec<CollectorResult<DataPoint>> {
    let mut guard = slots.lock().unwrap_or_else(|e| e.into_inner());
    (0..n)
        .map(|i| {
            guard
                .get_mut(i)
                .and_then(|slot| slot.take())
                .unwrap_or(Err(CollectorError::Timeout(timeout)))
        })
        .collect()
}

/// ROI-07（R-3）：点位本轮是否到期。
///
/// `last` 为空表示从未尝试过（新点 / 重载后首轮），一律视为到期。
/// 用 `saturating_duration_since` 防止系统时钟回拨导致 panic。
#[inline]
fn point_due(
    last: Option<std::time::Instant>,
    now: std::time::Instant,
    interval: std::time::Duration,
) -> bool {
    match last {
        Some(t) => now.saturating_duration_since(t) >= interval,
        None => true,
    }
}

/// ROI-10：设备熔断键（组键 + 设备/总线）。
///
/// 带上组键，避免不同总线上的同名 `device_id` 共用熔断状态。
#[inline]
fn breaker_key(group: &str, p: &PointConfig) -> String {
    format!("{group}\u{1f}{}", p.device_id.as_deref().unwrap_or(&p.bus))
}

impl Scheduler {
    /// 统一采集任务孵化（所有协议：modbus/dlt645/cjt188/gpio/opcua/iec104/bacnet/s7comm/...）
    ///
    /// 通过 `Collector::read_batch_with_slots` 统一读取（共享进度槽，超时不丢已完成点）：
    /// - Modbus 在 read_batch_with_slots 内部按 (slave_id, fc) 批量合并并逐结果写槽
    /// - DLT645/CJT188 逐点读取并逐点写槽；其他协议走 trait 默认逐点写槽实现
    ///
    /// 组键 `protocol:bus`（由调用方按 `group_key_for_point` 生成后显式传入）。
    pub(crate) fn spawn_collector_task(
        self: &Arc<Self>,
        key: &str,
        bus_name: &str,
        points: Vec<PointConfig>,
    ) {
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

        // #13 / DD-08：组键统一为 `protocol:bus`（group_key），由调用方显式传入。
        // spawn 不再按 `points.first()` 自行推导——旧 start() 只按 bus 分组，同 bus
        // 挂多协议时整组键取第一个点的协议，与 reload 的 (bus, protocol) 分组口径
        // 漂移：热重载判定「组变了」→ 全停全建抖动，且组键错误会让熔断/退避状态串组。
        let key = key.to_string();
        let proto = points
            .first()
            .map(|p| p.protocol.as_str())
            .unwrap_or_else(|| collector.name());
        // F-11：时序记录的归属信息（与组键口径一致），在采集循环尾部写入真实执行窗口
        let record_key = key.clone();
        let record_proto = proto.to_string();
        let record_bus = bus_name.to_string();
        // 修复(审查#B2)：组级读超时预算按「点位数 × 单点预算」自适应，
        // 不再硬编码 5s（原实现对逐点串行的 DLT645/CJT188 大组必然误判超时）。
        let group_timeout = collector.batch_timeout_budget_for(&points);
        // 优化：Arc 包裹后闭包内只 clone Arc（引用计数），避免每采集周期深拷贝 Vec<PointConfig>
        let points = Arc::new(points);
        // ROI-07（R-3）：逐点「上次尝试时刻」。
        //
        // 旧实现把退避倍率记在**点位**上（`DeviceState.current_interval`），组周期却取
        // 组内 `min` —— 只要同组还有一个健康点，失败点就照样每轮被读、每轮超时，
        // 退避等于没生效。这里改为每轮只读「已到重试时间」的点：健康点保持原周期，
        // 失败点按自己的退避节奏重试，互不拖累。
        let last_attempt: Arc<std::sync::Mutex<HashMap<String, std::time::Instant>>> =
            Arc::new(std::sync::Mutex::new(HashMap::new()));
        let handle = tokio::spawn(async move {
            supervision_loop(task_name, shutdown, group_stop_rx, interval_fn, move |interval| {
                let self_arc = self_arc.clone();
                let collector = collector.clone();
                let points = points.clone();
                let last_attempt = last_attempt.clone();
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
                    // ROI-07（R-3）：只读「已到重试时间」的点，退避按点位生效。
                    // DD-13：先取 device_states 快照并立即释放，再单独锁 breakers。
                    // 原实现在持有 device_states 时嵌套取 breakers，与其它路径的加锁
                    // 顺序相反时会形成锁环路；每轮 clone 一份退避间隔即可避免嵌套。
                    let backoffs: Vec<Duration> = {
                        let states = self_arc.device_states.lock();
                        points
                            .iter()
                            .map(|p| {
                                states
                                    .get(&p.id)
                                    .map(|s| s.current_interval)
                                    .unwrap_or(interval)
                            })
                            .collect()
                    };
                    let (due_idx, all_due) = {
                        let mut last = last_attempt.lock().unwrap_or_else(|e| e.into_inner());
                        // ROI-10：熔断中的设备本轮直接跳过，不占用总线、不刷失败日志。
                        // 跳过时不更新 `last_attempt`，冷却期满后可立刻探测。
                        let mut breakers = self_arc.breakers.lock();
                        let mut due = Vec::with_capacity(points.len());
                        for (i, p) in points.iter().enumerate() {
                            let bkey = breaker_key(&record_key, p);
                            if !breakers.entry(bkey).or_default().allow(start) {
                                continue;
                            }
                            // 未初始化（新点 / 重载后首轮）或没有状态时一律视为到期
                            let is_due = point_due(last.get(&p.id).copied(), start, backoffs[i]);
                            if is_due {
                                last.insert(p.id.clone(), start);
                                due.push(i);
                            }
                        }
                        let all_due = due.len() == points.len();
                        (due, all_due)
                    };
                    // 常态是全部到期：直接借用原切片，避免每周期深拷贝配置
                    let due_owned: Option<Vec<PointConfig>> = if all_due {
                        None
                    } else {
                        Some(due_idx.iter().map(|&i| points[i].clone()).collect())
                    };
                    let read_points: &[PointConfig] = due_owned.as_deref().unwrap_or(&points[..]);
                    if read_points.is_empty() {
                        // 全组都在退避期：不占用总线，也不上送伪失败点。
                        // 陈旧值靠前端按 `source_ts` 自然「变旧」，不伪造刷新时间。
                        self_arc.timeline.record(
                            &record_key,
                            &record_proto,
                            &record_bus,
                            interval,
                            t0_ms,
                            super::timeline::now_ms(),
                        );
                        return;
                    }
                    // 缺陷 #7：共享进度槽——每个点位完成即写槽，组超时 drop 掉 future 后
                    // 已完成结果仍可取回；未完成点位由 collect_results 统一判 Timeout。
                    let slots: ResultSlots = Arc::new(std::sync::Mutex::new(
                        (0..read_points.len()).map(|_| None).collect(),
                    ));
                    let read_fut = collector.read_batch_with_slots(read_points, slots.clone());
                    let timed_out = tokio::time::timeout(group_timeout, read_fut).await.is_err();
                    let all_results = collect_results(slots, read_points.len(), group_timeout);
                    if timed_out {
                        let n_timeout = all_results
                            .iter()
                            .filter(|r| matches!(r, Err(CollectorError::Timeout(_))))
                            .count();
                        let n_returned = all_results.len() - n_timeout;
                        tracing::error!(
                            "collector {} read_batch timeout after {:?}, {} points timed out, {} points returned",
                            collector.name(),
                            group_timeout,
                            n_timeout,
                            n_returned
                        );
                    }
                    let elapsed_us = start.elapsed().as_micros() as u64;
                    // ROI-10：按设备汇总本轮成败，驱动从站级熔断状态机。
                    let mut device_outcomes: HashMap<String, bool> = HashMap::new();
                    for (point, result) in read_points.iter().zip(all_results.iter()) {
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
                        // 设备本轮只要有一个点成功，就不算整轮失败（避免单点偶发误熔断）
                        let outcome = device_outcomes
                            .entry(breaker_key(&record_key, point))
                            .or_insert(false);
                        *outcome = *outcome || success;
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
                    // ROI-10：一轮结束后更新熔断状态；打开/关闭都记一条日志，
                    // 现场可直接看到「某台从站被暂时跳过」及其冷却时长。
                    if !device_outcomes.is_empty() {
                        let now = std::time::Instant::now();
                        let mut breakers = self_arc.breakers.lock();
                        for (bkey, any_success) in device_outcomes {
                            if let Some(b) = breakers.get_mut(&bkey) {
                                match b.record_round(any_success, now) {
                                    Some(super::breaker::BreakerEvent::Opened { cooldown }) => {
                                        tracing::warn!(
                                            "device breaker opened: {}({}), skip {}s",
                                            record_bus,
                                            bkey.rsplit('\u{1f}').next().unwrap_or(""),
                                            cooldown.as_secs()
                                        );
                                    }
                                    Some(super::breaker::BreakerEvent::Closed) => {
                                        tracing::info!(
                                            "device breaker closed: {}({}), resuming normal polling",
                                            record_bus,
                                            bkey.rsplit('\u{1f}').next().unwrap_or("")
                                        );
                                    }
                                    None => {}
                                }
                            }
                        }
                    }
                    self_arc.handle_results(read_points, all_results).await;
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

#[cfg(test)]
mod due_tests {
    use super::point_due;
    use std::time::{Duration, Instant};

    /// 新点 / 重载后首轮没有任何尝试记录，必须立刻采一次。
    #[test]
    fn never_attempted_is_always_due() {
        let now = Instant::now();
        assert!(point_due(None, now, Duration::from_secs(3600)));
    }

    /// R-3 回归：退避中的点在本轮之外不得被读。
    ///
    /// 旧实现组周期取组内 `min`，同组健康点是 1s 时，失败点即使退避到 60s
    /// 也会每秒被读一次、每秒超时 —— 退避形同虚设。
    #[test]
    fn backed_off_point_is_skipped_until_its_own_interval() {
        let last = Instant::now();
        // 60s 退避：5s 后仍未到期（旧实现会在 1s、2s… 每轮都读）
        assert!(!point_due(
            Some(last),
            last + Duration::from_secs(5),
            Duration::from_secs(60)
        ));
        // 1s 健康点：0.5s 时未到期、1.5s 时到期
        assert!(!point_due(
            Some(last),
            last + Duration::from_millis(500),
            Duration::from_secs(1)
        ));
        assert!(point_due(
            Some(last),
            last + Duration::from_millis(1500),
            Duration::from_secs(1)
        ));
        // 退避期满后恢复尝试
        assert!(point_due(
            Some(last),
            last + Duration::from_secs(61),
            Duration::from_secs(60)
        ));
    }
}
