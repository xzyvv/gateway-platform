//! S7Comm 采集器写入实现：单点写入与多 item 批量写入（决策表 #11 / #13）。
use super::*;

use crate::config::PointConfig;
use crate::south::protocol::{clone_collector_error, reverse_convert, Collector};

impl S7CommCollector {
    /// 写入单个点位（决策表 #11：S7 Write Var，功能码 0x05）
    ///
    /// 与 `read` 对称：惰性建链 → 取 PDU 引用 → 构帧 → 带超时收发。
    /// 任何 IO 失败都会清链并标记未连接，交由下一次调用自动重连
    /// （与 read 的自愈策略保持一致，避免出现永久离线）。
    ///
    /// 供 `Collector::write` 委托。
    pub(super) async fn write_point(&self, point: &PointConfig, value: f64) -> CollectorResult<()> {
        let s7_addr = address::parse_address(&point.address, point.data_type.as_deref())?;

        if !self.is_connected() {
            self.connect().await?;
        }

        let mut guard = self.stream.lock().await;
        let stream = guard
            .as_mut()
            .ok_or_else(|| CollectorError::IoError("S7 not connected".into()))?;

        let pdu_ref = {
            let mut r = self.pdu_ref.lock().await;
            let v = *r;
            *r = r.wrapping_add(1);
            if *r == 0 {
                *r = 1;
            }
            v
        };

        let req = frames::build_write_var(
            s7_addr.area,
            s7_addr.db_number,
            s7_addr.byte_offset,
            s7_addr.bit_offset,
            s7_addr.data_type,
            // 北向下发的是工程量（一次侧值），写回前必须按读路径的反向换算还原成原始值，
            // 否则配了 scale/offset/变比的点位会按工程量直接落库，读回来被再乘一次
            reverse_convert(point, value),
            pdu_ref,
        )?;

        match tokio::time::timeout(self.io_timeout, stream.write_all(&req)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                *guard = None;
                self.connected.store(false, Ordering::Relaxed);
                return Err(CollectorError::IoError(format!("S7 send write: {}", e)));
            }
            Err(_) => {
                *guard = None;
                self.connected.store(false, Ordering::Relaxed);
                return Err(CollectorError::NoReply);
            }
        }

        let resp =
            match tokio::time::timeout(self.io_timeout, frames::read_tpkt_frame(stream)).await {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    *guard = None;
                    self.connected.store(false, Ordering::Relaxed);
                    return Err(e);
                }
                Err(_) => {
                    *guard = None;
                    self.connected.store(false, Ordering::Relaxed);
                    return Err(CollectorError::NoReply);
                }
            };

        frames::parse_write_response(&resp)
    }

    /// 批量写入多个点位（决策表 #13：多 item Write Var，一批一次事务）
    ///
    /// 默认实现逐点调用 [`Collector::write`]，每点多一次发送+响应往返；现场
    /// "一屏批量下发几十点" 时延线性叠加。这里合并成单个 Write Var 多 item 请求，
    /// PLC 侧一次执行，往返次数由 N 降到 1。
    ///
    /// 口径与 `read_batch` 保持一致：
    /// 1. 逐点解析地址 + 编码值，坏点单独报错，不拖累同批其它点；
    /// 2. 惰性建链，建链失败整批同因 `IoError`；
    /// 3. 按协商 PDU 分片——写 item 的 data 段还带值字节，不能套用读的 12B/item
    ///    预算（否则点位稍多即整帧超长被设备拒绝）；
    /// 4. 返回与入参**等长且逐位对应**。
    ///
    /// 供 `Collector::write_multi` 委托。
    pub(super) async fn write_multi_points(
        &self,
        points: &[(&PointConfig, f64)],
    ) -> Vec<CollectorResult<()>> {
        let n = points.len();
        let mut slots: Vec<Option<CollectorResult<()>>> = (0..n).map(|_| None).collect();
        if n == 0 {
            return Vec::new();
        }

        // 阶段一：解析地址并编码值；失败的点位就地回填
        struct PendingWrite {
            idx: usize,
            spec: frames::ItemSpec,
            value: Vec<u8>,
        }
        let mut pending: Vec<PendingWrite> = Vec::with_capacity(n);
        for (idx, (p, value)) in points.iter().enumerate() {
            let s7_addr = match address::parse_address(&p.address, p.data_type.as_deref()) {
                Ok(a) => a,
                Err(e) => {
                    slots[idx] = Some(Err(e));
                    continue;
                }
            };
            match frames::encode_value(reverse_convert(p, *value), s7_addr.data_type) {
                Ok(bytes) => pending.push(PendingWrite {
                    idx,
                    spec: frames::ItemSpec {
                        area: s7_addr.area,
                        db_number: s7_addr.db_number,
                        byte_offset: s7_addr.byte_offset,
                        bit_offset: s7_addr.bit_offset,
                        data_type: s7_addr.data_type,
                    },
                    value: bytes,
                }),
                Err(e) => slots[idx] = Some(Err(e)),
            }
        }

        let unfilled = || {
            Err(CollectorError::InvalidFrame(
                "s7 batch internal: point not processed".into(),
            ))
        };

        if pending.is_empty() {
            return slots
                .into_iter()
                .map(|s| s.unwrap_or_else(unfilled))
                .collect();
        }

        // 阶段二：建链；失败整批同因回填
        if !self.is_connected() {
            if let Err(e) = self.connect().await {
                let msg = e.to_string();
                for p in &pending {
                    slots[p.idx] = Some(Err(CollectorError::IoError(msg.clone())));
                }
                return slots
                    .into_iter()
                    .map(|s| s.unwrap_or_else(unfilled))
                    .collect();
            }
        }

        // 阶段三：按协商 PDU 分片写入。帧头约 14B；每个 item 占
        // 12B 参数 + 4B 数据头 + 值字节 + 奇偶对齐 1B。
        let pdu_size = *self.pdu_size.lock().await as usize;
        let max_items = u8::MAX as usize;
        let mut start = 0;
        while start < pending.len() {
            let mut end = start;
            let mut cost = 14usize;
            while end < pending.len() && end - start < max_items {
                let vlen = pending[end].value.len();
                let item_cost = 12 + 4 + vlen + (vlen % 2);
                // 首项无论多大都要发出去：留给设备拒绝，避免原地死循环
                if end > start && cost + item_cost > pdu_size {
                    break;
                }
                cost += item_cost;
                end += 1;
            }
            let chunk = &pending[start..end];
            let items: Vec<frames::WriteItem<'_>> = chunk
                .iter()
                .map(|p| frames::WriteItem {
                    spec: p.spec,
                    value_le: &p.value,
                })
                .collect();

            let pdu_ref = self.next_pdu_ref().await;
            let req = frames::build_write_var_multi(&items, pdu_ref);
            let outcome = match self.transact(&req).await {
                Ok(resp) => frames::parse_write_response_multi(&resp, chunk.len()),
                Err(e) => Err(e),
            };
            match outcome {
                Ok(()) => {
                    for p in chunk {
                        slots[p.idx] = Some(Ok(()));
                    }
                }
                Err(e) => {
                    for p in chunk {
                        slots[p.idx] = Some(Err(clone_collector_error(&e)));
                    }
                }
            }
            start = end;
        }

        slots
            .into_iter()
            .map(|s| s.unwrap_or_else(unfilled))
            .collect()
    }
}
