//! Modbus 采集器固有方法：构造 / 异常分级 / 带重试读 / 批量读
//!
//! `Collector` trait 的实现（单点读写、多点写）见 `trait_impl.rs`；
//! 合并参数自适应状态与写分组辅助见 `merge.rs`。

use std::collections::HashMap;
use std::sync::Arc;

use futures::future::join_all;

use crate::config::PointConfig;
use crate::engine::model::{DataPoint, Quality};

use super::{
    build_read_request, convert_value, merge_batches, parse_coils_response, parse_point_value,
    parse_read_response, CollectorError, CollectorResult, ModbusCollector, ModbusExceptionCode,
};

impl ModbusCollector {
    // Bug 186 (N16): single-point read uses _with_retry variant.
    /// 在 bootstrap 阶段调用；调用后所有 read/write 路径上的 ModbusException
    /// 都会按 is_recoverable / is_config_error 分类上报。
    pub fn with_metrics(mut self, metrics: Arc<crate::engine::metrics::Metrics>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    // Bug 186 (N16): single-point read uses _with_retry variant.
    ///   - 配置类（非法 FC/地址/值）：只 warn + 递增 modbus_exception_config_error_total，不重试
    ///   - 可恢复类（从站忙/故障/确认/存储错）：warn + 递增 modbus_exception_recoverable_total。
    ///     上层调用方应决定是否重试（见 read_registers_with_retry 等）
    ///   - 未知：保守按可恢复处理（不轻易放弃）
    ///
    /// 返回 (建议重试?, 异常枚举, 分类标签 for log)。
    fn classify_exception(&self, code: u8) -> (bool, ModbusExceptionCode, &'static str) {
        let exc = ModbusExceptionCode::from_code(code);
        let label = if exc.is_config_error() {
            "config_error"
        } else if exc.is_recoverable() {
            "recoverable"
        } else {
            "unknown"
        };
        if let Some(m) = &self.metrics {
            if exc.is_config_error() {
                m.protocol_exception
                    .modbus_config_error
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            } else {
                m.protocol_exception
                    .modbus_recoverable
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
        (exc.is_recoverable(), exc, label)
    }

    // Bug 186 (N16): single-point read uses _with_retry variant.
    /// - 配置类异常：直接返回原错误，不重试
    /// - 可恢复异常：sleep backoff 50ms 后重试一次；仍失败则累计 retry_exhausted
    /// - 其它错误（InvalidFrame/ChecksumError 等）：原样透传
    pub(super) async fn read_registers_with_retry(
        &self,
        slave: u8,
        fc: u8,
        start_addr: u16,
        count: u16,
    ) -> CollectorResult<Vec<u16>> {
        let request = build_read_request(slave, fc, start_addr, count);
        // 关键：异常响应是「从站正常应答的一帧」，由 parse_read_response 识别，
        // 而不是 bus.transaction 的 Err。原实现只匹配 `Err(ModbusException)`，
        // 于是异常码永远进不了重试/计数分支（Bug 186 的「可恢复异常重试一次」
        // 实际是死代码）。这里把「事务结果 + 解帧结果」先合并，再统一分类。
        let first = match self.bus.transaction(request.clone(), Some(count)).await {
            Ok(resp) => parse_read_response(slave, fc, count, &resp),
            Err(e) => Err(e),
        };
        let code = match first {
            Ok(regs) => return Ok(regs),
            Err(CollectorError::ModbusException(code)) => code,
            // 超时 / 校验错 / 非法帧不是「从站回了异常码」，原样透传，不重试
            Err(e) => return Err(e),
        };
        {
            let (retryable, exc, label) = self.classify_exception(code);
            if !retryable {
                tracing::warn!(
                    target: "south.modbus",
                    "modbus exception ({}): fc={:#04x} slave={} addr={} count={} -> {}",
                    label, fc, slave, start_addr, count, exc.description()
                );
                return Err(CollectorError::ModbusException(code));
            }
            tracing::warn!(
                target: "south.modbus",
                "modbus exception ({}): fc={:#04x} slave={} addr={} count={} -> {} (will retry once after 50ms backoff)",
                label, fc, slave, start_addr, count, exc.description()
            );
            // Bug 186: 单次重试——现网 90%+ 的 recoverable 都是偶发。
            // 一次重试足够；第二次重试会让通信周期翻倍，无明显收益。
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let second = match self.bus.transaction(request, Some(count)).await {
                Ok(resp) => parse_read_response(slave, fc, count, &resp),
                Err(e) => Err(e),
            };
            match second {
                Ok(regs) => {
                    tracing::info!(
                        target: "south.modbus",
                        "modbus exception ({}): retry succeeded for fc={:#04x} slave={} addr={}",
                        label, fc, slave, start_addr
                    );
                    Ok(regs)
                }
                Err(e) => {
                    if let Some(m) = &self.metrics {
                        m.protocol_exception
                            .modbus_retry_exhausted
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    tracing::error!(
                        target: "south.modbus",
                        "modbus exception ({}): retry exhausted for fc={:#04x} slave={} addr={}: {}",
                        label, fc, slave, start_addr, e
                    );
                    Err(e)
                }
            }
        }
    }

    // Bug 186 (N16): single-point read uses _with_retry variant.
    pub(super) async fn read_coils_with_retry(
        &self,
        slave: u8,
        fc: u8,
        start_addr: u16,
        count: u16,
    ) -> CollectorResult<Vec<bool>> {
        let request = build_read_request(slave, fc, start_addr, count);
        // 同 read_registers_with_retry：异常帧由 parse_coils_response 识别，
        // 必须合并「事务结果 + 解帧结果」后再分类，否则异常码走不到重试。
        let first = match self.bus.transaction(request.clone(), None).await {
            Ok(resp) => parse_coils_response(slave, fc, count, &resp),
            Err(e) => Err(e),
        };
        let code = match first {
            Ok(bits) => return Ok(bits),
            Err(CollectorError::ModbusException(code)) => code,
            Err(e) => return Err(e),
        };
        {
            let (retryable, exc, label) = self.classify_exception(code);
            if !retryable {
                tracing::warn!(
                    target: "south.modbus",
                    "modbus exception ({}): fc={:#04x} slave={} addr={} count={} -> {}",
                    label, fc, slave, start_addr, count, exc.description()
                );
                return Err(CollectorError::ModbusException(code));
            }
            tracing::warn!(
                target: "south.modbus",
                "modbus exception ({}): fc={:#04x} slave={} addr={} count={} -> {} (will retry once after 50ms backoff)",
                label, fc, slave, start_addr, count, exc.description()
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let second = match self.bus.transaction(request, None).await {
                Ok(resp) => parse_coils_response(slave, fc, count, &resp),
                Err(e) => Err(e),
            };
            match second {
                Ok(bits) => {
                    tracing::info!(
                        target: "south.modbus",
                        "modbus exception ({}): retry succeeded for fc={:#04x} slave={} addr={}",
                        label, fc, slave, start_addr
                    );
                    Ok(bits)
                }
                Err(e) => {
                    if let Some(m) = &self.metrics {
                        m.protocol_exception
                            .modbus_retry_exhausted
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    tracing::error!(
                        target: "south.modbus",
                        "modbus exception ({}): retry exhausted for fc={:#04x} slave={} addr={}: {}",
                        label, fc, slave, start_addr, e
                    );
                    Err(e)
                }
            }
        }
    }

    /// 批量读取（§9.1 分组合并算法）
    pub async fn read_batch_points(&self, points: &[PointConfig]) -> Vec<DataPoint> {
        // 决策表 #3：按观测自适应 gap
        // （保守：仅调 gap，不调 max_count；max_count 为 Modbus 协议上限）
        let adaptive_gap = self.merge_stats.lock().next_gap();
        // 按 (slave_id, function_code) 分组
        let mut groups: HashMap<(u8, u8), Vec<&PointConfig>> = HashMap::new();
        for p in points {
            let slave = p.slave_id.unwrap_or(1);
            let fc = p.function_code.unwrap_or(3);
            groups.entry((slave, fc)).or_default().push(p);
        }

        let mut results: HashMap<String, DataPoint> = HashMap::new();

        for ((slave, fc), mut group) in groups {
            group.sort_by_key(|p| p.address_num.unwrap_or(0));

            if fc == 1 || fc == 2 {
                // 线圈/离散输入：按位读取
                // max_count 保持 2000（Modbus 协议位打包上限）
                let batches = merge_batches(&group, adaptive_gap, 2000);
                // Bug 217: 原实现逐帧 `await`，TCP 流水线并发（tcp_concurrent）从未生效。
                // 改为按总线并发上限受控并发提交：Modbus TCP 单 socket 可多事务在途；
                // 串口总线 max_concurrency()==1，信号量退化为 1 个在途，等价于原顺序读取。
                // 保持 RS485 半双工的物理串行化。
                let sem = Arc::new(tokio::sync::Semaphore::new(
                    self.bus.max_concurrency().max(1),
                ));
                let outcomes = join_all(batches.iter().map(|batch| {
                    let sem = Arc::clone(&sem);
                    async move {
                        let _permit = sem.acquire().await;
                        // Bug 186 (N16): use _with_retry variant on batch read path.
                        let res = self
                            .read_coils_with_retry(slave, fc, batch.start_addr, batch.count)
                            .await;
                        (batch, res)
                    }
                }))
                .await;
                for (batch, res) in &outcomes {
                    match res {
                        Ok(bits) => {
                            for p in &batch.points {
                                let idx = (p.address_num.unwrap_or(0) - batch.start_addr) as usize;
                                let bit = bits.get(idx).copied().unwrap_or(false);
                                let raw = if bit { 1.0 } else { 0.0 };
                                let value = convert_value(p, raw);
                                results.insert(
                                    p.id.clone(),
                                    DataPoint {
                                        point_id: p.id.clone(),
                                        device_id: p
                                            .device_id
                                            .clone()
                                            .unwrap_or_else(|| p.bus.clone()),
                                        value,
                                        raw_value: raw,
                                        raw_type: "bool".to_string(),
                                        quality: Quality::Good,
                                        quality_reason: String::new(),
                                        timestamp: DataPoint::now(),
                                        source_ts: DataPoint::now(),
                                        stale_count: 0,
                                        value_text: None,
                                    },
                                );
                            }
                        }
                        Err(e) => {
                            for p in &batch.points {
                                results.insert(
                                    p.id.clone(),
                                    DataPoint::failed(
                                        &p.id,
                                        &p.device_id.clone().unwrap_or_else(|| p.bus.clone()),
                                        None,
                                        0,
                                        e.to_quality(),
                                        "modbus_batch_failed",
                                        0,
                                    ),
                                );
                            }
                        }
                    }
                }
                // 决策表 #3：本 (slave, fc=1|2) 组的成功批次数写入 stats
                let successful_pts: u64 = outcomes
                    .iter()
                    .filter(|(_, res)| res.is_ok())
                    .map(|(b, _)| b.points.len() as u64)
                    .sum();
                if !outcomes.is_empty() {
                    let avg = successful_pts as f64 / outcomes.len() as f64;
                    self.merge_stats.lock().observe(avg);
                }
            } else {
                // 寄存器：FC03/FC04
                // max_count 保持 125（Modbus 协议寄存器读上限）
                let batches = merge_batches(&group, adaptive_gap, 125);
                // Bug 217: 同线圈分支——受控并发提交，让 tcp_concurrent 真正生效。
                let sem = Arc::new(tokio::sync::Semaphore::new(
                    self.bus.max_concurrency().max(1),
                ));
                let outcomes = join_all(batches.iter().map(|batch| {
                    let sem = Arc::clone(&sem);
                    async move {
                        let _permit = sem.acquire().await;
                        // Bug 186 (N16): use _with_retry variant on batch read path.
                        let res = self
                            .read_registers_with_retry(slave, fc, batch.start_addr, batch.count)
                            .await;
                        (batch, res)
                    }
                }))
                .await;
                for (batch, res) in &outcomes {
                    match res {
                        Ok(regs) => {
                            for p in &batch.points {
                                let dp = parse_point_value(p, regs, batch.start_addr);
                                results.insert(p.id.clone(), dp);
                            }
                        }
                        Err(e) => {
                            for p in &batch.points {
                                results.insert(
                                    p.id.clone(),
                                    DataPoint::failed(
                                        &p.id,
                                        &p.device_id.clone().unwrap_or_else(|| p.bus.clone()),
                                        None,
                                        0,
                                        e.to_quality(),
                                        "modbus_read_failed",
                                        0,
                                    ),
                                );
                            }
                        }
                    }
                }
                // 决策表 #3：本 (slave, fc=3|4) 组的成功批次数写入 stats
                let successful_pts: u64 = outcomes
                    .iter()
                    .filter(|(_, res)| res.is_ok())
                    .map(|(b, _)| b.points.len() as u64)
                    .sum();
                if !outcomes.is_empty() {
                    let avg = successful_pts as f64 / outcomes.len() as f64;
                    self.merge_stats.lock().observe(avg);
                }
            }
        }

        points
            .iter()
            .map(|p| {
                results.remove(&p.id).unwrap_or_else(|| {
                    DataPoint::failed(
                        &p.id,
                        &p.device_id.clone().unwrap_or_else(|| p.bus.clone()),
                        None,
                        0,
                        Quality::Bad,
                        "modbus_no_result",
                        0,
                    )
                })
            })
            .collect()
    }
}
