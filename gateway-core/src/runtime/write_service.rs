//! 写操作统一执行器（ROI-35）
//!
//! **所有写入口（Web 单写 / Web 批写 / OPC UA 写回）都只经过这里**。此前三处
//! 各写一套：Web 有范围校验与读回核实、OPC UA 没有；被拒写不落审计、不计数；
//! 审计 detail 的前缀还各写各的（DD-33/34/35/36）。
//!
//! 本模块不持有运行态：点位表、collector 表、指标、审计全部由调用方传入，
//! 因此 Web 层可以继续用配置里的点位 + mock collector，北向则用运行态调度器。
//!
//! 这里的职责：
//! 1. `writable` 白名单 + `write_min/write_max` 范围校验（越界不下发）；
//! 2. 三态写语义（[`WriteOutcome`]），`Uncertain` 时读回核实；
//! 3. **每一条**结果都落审计 + 计数（含被拒的写），detail 用统一格式；
//! 4. 审计 `source` 取真实操作人（[`WriteContext::audit_source`]）。

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use crate::config::PointConfig;
use crate::engine::audit::AuditLog;
use crate::engine::metrics::Metrics;
use crate::engine::model::Quality;
use crate::engine::write::{
    write_audit_detail, write_error_message, write_readback_matches, PointWriteResult,
    RejectReason, WriteContext, WriteMode, WriteOutcome, WriteRequest,
};
use crate::south::protocol::Collector;

/// 写结果未知时，**单点**读回核实的超时预算（G-03）。
///
/// 默认总线超时 500ms，留出「写超时后的链路恢复 + 一次完整读事务」的余量。
const SINGLE_READBACK_TIMEOUT: Duration = Duration::from_secs(5);

/// 执行一批写请求，返回与入参**等长且逐位对应**的结果。
///
/// 单写模式（[`WriteMode::Single`]）要求恰好一条；被拒项以
/// [`PointWriteResult::reject`] 标明，由调用方决定 HTTP/OPC UA 映射。
pub async fn execute_writes(
    points: &[PointConfig],
    collectors: &HashMap<String, Arc<dyn Collector>>,
    metrics: &Metrics,
    audit: &AuditLog,
    writes: &[WriteRequest],
    mode: WriteMode,
    ctx: &WriteContext,
) -> Vec<PointWriteResult> {
    if writes.is_empty() {
        return Vec::new();
    }
    if mode == WriteMode::Single {
        return vec![execute_single(points, collectors, metrics, audit, &writes[0], ctx).await];
    }
    execute_batch(points, collectors, metrics, audit, writes, ctx).await
}

async fn execute_single(
    points: &[PointConfig],
    collectors: &HashMap<String, Arc<dyn Collector>>,
    metrics: &Metrics,
    audit: &AuditLog,
    w: &WriteRequest,
    ctx: &WriteContext,
) -> PointWriteResult {
    let Some(point) = points.iter().find(|p| p.id == w.id) else {
        let message = format!("point {} not found", w.id);
        audit_reject(
            audit,
            metrics,
            &w.id,
            None,
            w.value,
            WriteOutcome::Failed,
            &message,
            ctx,
        );
        return PointWriteResult::rejected(
            &w.id,
            None,
            None,
            String::new(),
            w.value,
            RejectReason::NotFound,
            message,
        );
    };
    let point = point.clone();

    if !point.writable {
        let message = whitelist_message(&point);
        audit_reject(
            audit,
            metrics,
            &point.id,
            Some(&point),
            w.value,
            WriteOutcome::Failed,
            &message,
            ctx,
        );
        return PointWriteResult::rejected(
            &point.id,
            point.name.clone(),
            point.device_id.clone(),
            point.bus.clone(),
            w.value,
            RejectReason::NotWritable,
            message,
        );
    }
    // ROI-06（W-1）：越界直接拒绝，不下发到设备。
    if let Some(message) = point.write_range_error(w.value) {
        audit_reject(
            audit,
            metrics,
            &point.id,
            Some(&point),
            w.value,
            WriteOutcome::Failed,
            &message,
            ctx,
        );
        return PointWriteResult::rejected(
            &point.id,
            point.name.clone(),
            point.device_id.clone(),
            point.bus.clone(),
            w.value,
            RejectReason::OutOfRange,
            message,
        );
    }
    let Some(collector) = collectors.get(&point.bus) else {
        let message = format!("collector for bus {} not found", point.bus);
        audit_reject(
            audit,
            metrics,
            &point.id,
            Some(&point),
            w.value,
            WriteOutcome::Failed,
            &message,
            ctx,
        );
        return PointWriteResult::rejected(
            &point.id,
            point.name.clone(),
            point.device_id.clone(),
            point.bus.clone(),
            w.value,
            RejectReason::NoCollector,
            message,
        );
    };

    match collector.write(&point, w.value).await {
        Ok(()) => {
            audit_result(audit, metrics, &point, w.value, WriteOutcome::Ok, None, ctx);
            ok_result(&point, w.value)
        }
        Err(e) => {
            let outcome = e.write_outcome();
            let mut message = write_error_message(&e.to_string(), outcome);
            let quality = Some(e.to_quality());
            let mut readback = None;

            // G-03：写结果未知时读回核实。只有读到**同样的值**才判定写已生效；
            // 读到不同值或读不到，都保持「结果未知」。
            if outcome.is_uncertain() {
                if let Ok(Ok(dp)) =
                    tokio::time::timeout(SINGLE_READBACK_TIMEOUT, collector.read(&point)).await
                {
                    if matches!(dp.quality, Quality::Good) {
                        if write_readback_matches(w.value, dp.value, point.scale) {
                            tracing::info!(
                                point = %point.id,
                                value = w.value,
                                "写结果未知，读回值与写入值一致，判定写入已生效"
                            );
                            audit_result(
                                audit,
                                metrics,
                                &point,
                                w.value,
                                WriteOutcome::Ok,
                                None,
                                ctx,
                            );
                            return ok_result(&point, w.value);
                        }
                        readback = Some(dp.value);
                        message = format!(
                            "{message}；读回值 {} 与写入值 {} 不一致，仍无法确认写入是否生效",
                            dp.value, w.value
                        );
                    }
                }
            }

            audit_result(
                audit,
                metrics,
                &point,
                w.value,
                outcome,
                Some(&message),
                ctx,
            );
            PointWriteResult {
                point_id: point.id.clone(),
                point_name: point.name.clone(),
                device_id: point.device_id.clone(),
                bus: point.bus.clone(),
                value: w.value,
                outcome,
                message: Some(message),
                quality,
                readback,
                reject: None,
            }
        }
    }
}

async fn execute_batch(
    points: &[PointConfig],
    collectors: &HashMap<String, Arc<dyn Collector>>,
    metrics: &Metrics,
    audit: &AuditLog,
    writes: &[WriteRequest],
    ctx: &WriteContext,
) -> Vec<PointWriteResult> {
    let mut results: Vec<Option<PointWriteResult>> = (0..writes.len()).map(|_| None).collect();

    // 按 bus 分组（组内保持入参下标顺序），同总线点位一次 write_multi 下发
    let mut by_bus: BTreeMap<String, Vec<(usize, PointConfig)>> = BTreeMap::new();
    for (i, w) in writes.iter().enumerate() {
        match points.iter().find(|p| p.id == w.id) {
            Some(p) if p.writable => {
                if let Some(message) = p.write_range_error(w.value) {
                    audit_reject(
                        audit,
                        metrics,
                        &p.id,
                        Some(p),
                        w.value,
                        WriteOutcome::Failed,
                        &message,
                        ctx,
                    );
                    results[i] = Some(PointWriteResult::rejected(
                        &p.id,
                        p.name.clone(),
                        p.device_id.clone(),
                        p.bus.clone(),
                        w.value,
                        RejectReason::OutOfRange,
                        message,
                    ));
                } else {
                    by_bus
                        .entry(p.bus.clone())
                        .or_default()
                        .push((i, p.clone()));
                }
            }
            // S11: 写入白名单——未标记 writable 的点位记为该项失败，不放行写入
            Some(p) => {
                let message = whitelist_message(p);
                audit_reject(
                    audit,
                    metrics,
                    &p.id,
                    Some(p),
                    w.value,
                    WriteOutcome::Failed,
                    &message,
                    ctx,
                );
                results[i] = Some(PointWriteResult::rejected(
                    &p.id,
                    p.name.clone(),
                    p.device_id.clone(),
                    p.bus.clone(),
                    w.value,
                    RejectReason::NotWritable,
                    message,
                ));
            }
            None => {
                let message = format!("point {} not found", w.id);
                audit_reject(
                    audit,
                    metrics,
                    &w.id,
                    None,
                    w.value,
                    WriteOutcome::Failed,
                    &message,
                    ctx,
                );
                results[i] = Some(PointWriteResult::rejected(
                    &w.id,
                    None,
                    None,
                    String::new(),
                    w.value,
                    RejectReason::NotFound,
                    message,
                ));
            }
        }
    }

    for (bus_name, group) in by_bus {
        let Some(collector) = collectors.get(&bus_name) else {
            let message = format!("collector for bus {bus_name} not found");
            for (i, p) in &group {
                audit_reject(
                    audit,
                    metrics,
                    &p.id,
                    Some(p),
                    writes[*i].value,
                    WriteOutcome::Failed,
                    &message,
                    ctx,
                );
                results[*i] = Some(PointWriteResult::rejected(
                    &p.id,
                    p.name.clone(),
                    p.device_id.clone(),
                    p.bus.clone(),
                    writes[*i].value,
                    RejectReason::NoCollector,
                    message.clone(),
                ));
            }
            continue;
        };

        let pairs: Vec<(&PointConfig, f64)> =
            group.iter().map(|(i, p)| (p, writes[*i].value)).collect();
        let outcomes = collector.write_multi(&pairs).await;

        // 结果未知的条目先挂起，等本轮写完再统一读回一次（批量读，少占总线时隙）。
        let mut pending_uncertain: Vec<(usize, PointConfig, f64, PointWriteResult)> = Vec::new();

        for ((i, p), outcome) in group.iter().zip(outcomes) {
            let value = writes[*i].value;
            match outcome {
                Ok(()) => {
                    audit_result(audit, metrics, p, value, WriteOutcome::Ok, None, ctx);
                    results[*i] = Some(ok_result(p, value));
                }
                Err(e) => {
                    let write_outcome = e.write_outcome();
                    let message = write_error_message(&e.to_string(), write_outcome);
                    let item = PointWriteResult {
                        point_id: p.id.clone(),
                        point_name: p.name.clone(),
                        device_id: p.device_id.clone(),
                        bus: p.bus.clone(),
                        value,
                        outcome: write_outcome,
                        message: Some(message),
                        quality: Some(e.to_quality()),
                        readback: None,
                        reject: None,
                    };
                    if write_outcome.is_uncertain() {
                        pending_uncertain.push((*i, p.clone(), value, item));
                    } else {
                        audit_result(
                            audit,
                            metrics,
                            p,
                            value,
                            write_outcome,
                            item.message.as_deref(),
                            ctx,
                        );
                        results[*i] = Some(item);
                    }
                }
            }
        }

        // G-03：写结果未知的条目读回核实。
        if !pending_uncertain.is_empty() {
            let probe_points: Vec<PointConfig> = pending_uncertain
                .iter()
                .map(|(_, p, _, _)| p.clone())
                .collect();
            let budget = collector.batch_timeout_budget(probe_points.len());
            let readbacks = tokio::time::timeout(budget, collector.read_batch(&probe_points)).await;

            let mut values: Vec<Option<f64>> = vec![None; pending_uncertain.len()];
            if let Ok(list) = readbacks {
                for (idx, r) in list.into_iter().enumerate() {
                    if let Ok(dp) = r {
                        if matches!(dp.quality, Quality::Good) && idx < values.len() {
                            values[idx] = Some(dp.value);
                        }
                    }
                }
            }

            for (idx, (i, p, value, mut item)) in pending_uncertain.into_iter().enumerate() {
                match values[idx] {
                    Some(rb) if write_readback_matches(value, rb, p.scale) => {
                        tracing::info!(
                            point = %p.id,
                            value,
                            readback = rb,
                            "写结果未知，读回值与写入值一致，判定写入已生效"
                        );
                        audit_result(audit, metrics, &p, value, WriteOutcome::Ok, None, ctx);
                        results[i] = Some(ok_result(&p, value));
                    }
                    other => {
                        if let Some(rb) = other {
                            item.readback = Some(rb);
                            let base = item.message.take().unwrap_or_default();
                            item.message = Some(format!(
                                "{base}；读回值 {} 与写入值 {} 不一致，仍无法确认写入是否生效",
                                rb, value
                            ));
                        }
                        audit_result(
                            audit,
                            metrics,
                            &p,
                            value,
                            item.outcome,
                            item.message.as_deref(),
                            ctx,
                        );
                        results[i] = Some(item);
                    }
                }
            }
        }
    }

    // 每个入参下标都会在三个分支之一填入结果；万一遗漏也只记日志跳过，不 panic。
    results
        .into_iter()
        .enumerate()
        .filter_map(|(i, r)| {
            if r.is_none() {
                tracing::error!("write_service 漏填结果下标 i={}", i);
            }
            r
        })
        .collect()
}

fn ok_result(point: &PointConfig, value: f64) -> PointWriteResult {
    PointWriteResult {
        point_id: point.id.clone(),
        point_name: point.name.clone(),
        device_id: point.device_id.clone(),
        bus: point.bus.clone(),
        value,
        outcome: WriteOutcome::Ok,
        message: None,
        quality: None,
        readback: None,
        reject: None,
    }
}

fn whitelist_message(point: &PointConfig) -> String {
    format!(
        "point {} 不在写入白名单中：需在点位配置中将 writable 置为 true 后方可写入",
        point.id
    )
}

/// 写审计 detail 的统一格式 + U-4 配置代次后缀。
///
/// 代次只追加在末尾（` [cfg:<n>]`），解析侧（[`crate::engine::write::parse_write_detail`]）
/// 会先剥离再解析，不改变既有三态格式。
fn audit_detail_with_generation(
    name: &str,
    value: f64,
    outcome: WriteOutcome,
    message: Option<&str>,
    ctx: &WriteContext,
) -> String {
    let detail = write_audit_detail(name, value, outcome, message);
    match ctx.config_generation {
        Some(generation) => format!("{detail} [cfg:{generation}]"),
        None => detail,
    }
}

/// 落一条「成功 / 失败 / 未知」审计 + 写指标。
#[allow(clippy::too_many_arguments)]
fn audit_result(
    audit: &AuditLog,
    metrics: &Metrics,
    point: &PointConfig,
    value: f64,
    outcome: WriteOutcome,
    message: Option<&str>,
    ctx: &WriteContext,
) {
    metrics.record_point_write(matches!(outcome, WriteOutcome::Ok) && message.is_none());
    let name = point.name.clone().unwrap_or_else(|| point.id.clone());
    let detail = audit_detail_with_generation(&name, value, outcome, message, ctx);
    let target = point.device_id.clone().unwrap_or_else(|| point.bus.clone());
    audit.log("point.write", &target, &detail, ctx.audit_source());
}

/// 落一条「被拒（未下发）」审计 + 失败计数（DD-33）。
#[allow(clippy::too_many_arguments)]
fn audit_reject(
    audit: &AuditLog,
    metrics: &Metrics,
    point_id: &str,
    point: Option<&PointConfig>,
    value: f64,
    outcome: WriteOutcome,
    message: &str,
    ctx: &WriteContext,
) {
    metrics.record_point_write(false);
    let name = point
        .and_then(|p| p.name.clone())
        .unwrap_or_else(|| point_id.to_string());
    let detail = audit_detail_with_generation(&name, value, outcome, Some(message), ctx);
    let target = point
        .and_then(|p| p.device_id.clone().or_else(|| Some(p.bus.clone())))
        .unwrap_or_else(|| point_id.to_string());
    audit.log("point.write", &target, &detail, ctx.audit_source());
}
