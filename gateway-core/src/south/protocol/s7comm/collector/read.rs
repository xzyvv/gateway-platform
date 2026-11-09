//! S7Comm 采集器读取实现：单点读取与多 item 批量读取（决策表 #13）。
use super::*;

use crate::config::PointConfig;
use crate::engine::model::DataPoint;
use crate::engine::model::Quality;
use crate::south::protocol::{clone_collector_error, convert_value, Collector};

impl S7CommCollector {
    /// 单点读取（供 `Collector::read` 委托）；惰性建链 + 单 item Read Var。
    pub(super) async fn read_point(&self, point: &PointConfig) -> CollectorResult<DataPoint> {
        let s7_addr = address::parse_address(&point.address, point.data_type.as_deref())?;

        // C-13: 生产路径（Scheduler::spawn_collector_task）只调 read_batch/read，从不调
        // Collector::connect；故 read 入口惰性建链：既是首连建链入口，也是 IO 失败
        // 清链（*guard = None）后的自动重建入口。用 is_connected() 短路——已连接时
        // 不碰 stream 锁，避免与热路径锁竞争。
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

        let req = frames::build_read_var(
            s7_addr.area,
            s7_addr.db_number,
            s7_addr.byte_offset,
            s7_addr.bit_offset,
            s7_addr.data_type,
            pdu_ref,
        );

        // 写超时保护：防止半开 TCP 连接无限阻塞
        let write_result = tokio::time::timeout(self.io_timeout, stream.write_all(&req)).await;
        match write_result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                // IO 错误：清理断线，触发后续重连
                *guard = None;
                self.connected.store(false, Ordering::Relaxed);
                return Err(CollectorError::IoError(format!("S7 send read: {}", e)));
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

        let raw_value = frames::parse_read_response(&resp, s7_addr.data_type)?;
        // 与 Modbus/DLT645 对齐：scale/offset/formula + CT/PT 变比统一在 common 层换算。
        // 此前 S7 直接把原始值当工程量上送，点位配了 scale/offset 也不生效。
        let value = convert_value(point, raw_value);

        Ok(DataPoint {
            point_id: point.id.clone(),
            device_id: point.device_id.clone().unwrap_or_default(),
            value,
            raw_value,
            raw_type: "s7comm".into(),
            quality: Quality::Good,
            quality_reason: String::new(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            source_ts: 0,
            stale_count: 0,
            value_text: None,
        })
    }

    /// 批量读取（决策表 #13：S7-1200/1500 优化块 = 多 item Read Var）
    ///
    /// 单次 S7 PDU 携带 N 个 VariableSpecification（N ≤ 255），任一 item 失败
    /// 仅该点位回填 `InvalidFrame`，不影响其它点位。
    ///
    /// 行为契约：
    /// 1. 先解析全部点位（地址错误 → 逐点同因 `InvalidConfig`，不建链）
    /// 2. 仅当存在待发请求时才建链（设备不可达时整批同因 `IoError`）
    /// 3. 单次多 item Read Var（沿用 S7 Put/Get 优化语义）
    ///
    /// 供 `Collector::read_batch` 委托。
    pub(super) async fn read_batch_points(
        &self,
        points: &[PointConfig],
    ) -> Vec<CollectorResult<DataPoint>> {
        let n = points.len();
        let mut slots: Vec<Option<CollectorResult<DataPoint>>> = (0..n).map(|_| None).collect();
        if n == 0 {
            return Vec::new();
        }

        // 阶段一：解析所有点位；建帧失败（地址错误/类型不支持）就地回填
        struct Pending {
            idx: usize,
            item: frames::ItemSpec,
            data_type: S7DataType,
        }
        let mut pending: Vec<Pending> = Vec::with_capacity(n);
        for (idx, p) in points.iter().enumerate() {
            match address::parse_address(&p.address, p.data_type.as_deref()) {
                Ok(s7_addr) => pending.push(Pending {
                    idx,
                    item: frames::ItemSpec {
                        area: s7_addr.area,
                        db_number: s7_addr.db_number,
                        byte_offset: s7_addr.byte_offset,
                        bit_offset: s7_addr.bit_offset,
                        data_type: s7_addr.data_type,
                    },
                    data_type: s7_addr.data_type,
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

        // 阶段三：单次多 item Read Var（沿用 S7 Put/Get 优化语义）
        // 修复(审查#4)：原实现固定按 255 分片，远超协商 PDU（S7-300 典型 240、
        //   S7-400/1200 典型 480）。单个 Read Var item 请求参数固定 12 字节、
        //   帧头约 14 字节，故点位稍多（>18/38）即整批被设备拒绝。
        //   现按协商 PDU 推导 item 上限，并保留 item_count 单字节（255）上限约束。
        let pdu_size = *self.pdu_size.lock().await as usize;
        let max_items = ((pdu_size.saturating_sub(14)) / 12).clamp(1, u8::MAX as usize);
        for chunk in pending.chunks(max_items) {
            let items: Vec<frames::ItemSpec> = chunk.iter().map(|p| p.item).collect();
            let resp = match self.transact_multi(&items).await {
                Ok(r) => r,
                Err(e) => {
                    for p in chunk {
                        slots[p.idx] = Some(Err(clone_collector_error(&e)));
                    }
                    continue;
                }
            };
            let parsed = match frames::parse_read_response_multi(&resp, &items) {
                Ok(v) => v,
                Err(e) => {
                    for p in chunk {
                        slots[p.idx] = Some(Err(clone_collector_error(&e)));
                    }
                    continue;
                }
            };
            for (i, item_result) in parsed.into_iter().enumerate() {
                let p = &chunk[i];
                let point_cfg = &points[p.idx];
                slots[p.idx] = Some(match item_result {
                    Err(rc) => Err(CollectorError::InvalidFrame(format!(
                        "S7 multi-read item {i} failed (return code 0x{rc:02X})"
                    ))),
                    Ok(value_bytes) => match frames::decode_value(&value_bytes, p.data_type) {
                        Ok(raw) => Ok(DataPoint {
                            point_id: point_cfg.id.clone(),
                            device_id: point_cfg.device_id.clone().unwrap_or_default(),
                            // 批量路径与单点路径同一套换算，避免两条路径数值不一致
                            value: convert_value(point_cfg, raw),
                            raw_value: raw,
                            raw_type: "s7comm".into(),
                            quality: Quality::Good,
                            quality_reason: String::new(),
                            timestamp: chrono::Utc::now().timestamp_millis(),
                            source_ts: 0,
                            stale_count: 0,
                            value_text: None,
                        }),
                        Err(e) => Err(e),
                    },
                });
            }
        }

        slots
            .into_iter()
            .map(|s| s.unwrap_or_else(unfilled))
            .collect()
    }
}
