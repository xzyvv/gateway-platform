//! BACnet `ReadPropertyMultiple` 单事务批量读（逐点结果写共享槽）。
//!
//! 从 `collector.rs` 拆出，只为控制单文件行数（C24 行数预算），行为不变。

use crate::south::protocol::{convert_value, ResultSlots};

use super::super::*;

impl BacnetCollector {
    /// 缺陷 #7：与 `read_batch` 相同的 ReadPropertyMultiple 单事务批量语义，但把
    /// 每一批产出的结果同步写入共享槽 `slots`。所有结果都在最后一次网络 await
    /// 之后的无 await 循环里生成，故在此统一回填与逐结果写槽等价，超时不丢已完成点。
    pub(crate) async fn read_batch_points_with_slots(
        &self,
        points: &[PointConfig],
        slots: ResultSlots,
    ) -> Vec<CollectorResult<DataPoint>> {
        let s = slots.clone();
        let finish = |out: Vec<CollectorResult<DataPoint>>| {
            let mut guard = s.lock().unwrap_or_else(|e| e.into_inner());
            for (i, r) in out.iter().enumerate() {
                guard[i] = Some(match r {
                    Ok(dp) => Ok(dp.clone()),
                    Err(e) => Err(clone_collector_error(e)),
                });
            }
            out
        };
        if points.is_empty() {
            return Vec::new();
        }
        // 1) 解析地址：命中 COV 缓存的点直接产出结果，不再进入批量读
        let mut parsed: Vec<CollectorResult<(u16, u32)>> = Vec::with_capacity(points.len());
        let mut cached: Vec<Option<DataPoint>> = Vec::with_capacity(points.len());
        let mut objects: Vec<(u16, u32)> = Vec::new();
        for p in points {
            match Self::parse_object_address(&p.address) {
                Ok((ot, inst)) => {
                    let oid = Self::object_id(ot, inst);
                    let hit = self.cov_cached(p, oid).await;
                    if hit.is_none() {
                        self.cov_wanted(oid).await;
                        objects.push((ot, inst));
                    }
                    cached.push(hit);
                    parsed.push(Ok((ot, inst)));
                }
                Err(e) => {
                    cached.push(None);
                    parsed.push(Err(e));
                }
            }
        }
        // 2) 无待主动读取的对象：命中缓存直接返回，其余按各自错误返回
        if objects.is_empty() {
            let mut out: Vec<CollectorResult<DataPoint>> = Vec::with_capacity(points.len());
            for (i, c) in cached.into_iter().enumerate() {
                out.push(match c {
                    Some(dp) => Ok(dp),
                    None => Err(match &parsed[i] {
                        Err(e) => clone_collector_error(e),
                        Ok(_) => CollectorError::InvalidConfig("no BACnet object".into()),
                    }),
                });
            }
            return finish(out);
        }

        // 修复(审查#1)：同一入口惰性建链；失败时整批回填同型错误，保持返回值与入参等长
        let socket = match self.ensure_connected().await {
            Ok(s) => s,
            Err(e) => {
                return finish(
                    points
                        .iter()
                        .map(|_| Err(clone_collector_error(&e)))
                        .collect::<Vec<CollectorResult<DataPoint>>>(),
                );
            }
        };

        let mut ig = self.invoke_id.lock().await;
        let invoke_id = *ig;
        *ig = invoke_id.wrapping_add(1);
        if *ig == 0 {
            *ig = 1;
        }
        drop(ig);

        let apdu = frames::build_rpm_request_apdu(&objects, invoke_id);
        let packet = frames::wrap_bvlc_npdu(&apdu);
        drain_socket(&socket);
        if let Err(e) = socket.send(&packet).await {
            let err = CollectorError::IoError(format!("BACnet RPM send: {}", e));
            return finish(
                points
                    .iter()
                    .map(|_| Err(clone_collector_error(&err)))
                    .collect::<Vec<CollectorResult<DataPoint>>>(),
            );
        }

        let deadline = tokio::time::Instant::now() + self.io_timeout;
        let mut buf = vec![0u8; 4096];
        let batch: CollectorResult<HashMap<u32, (f64, Quality)>> = loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break Err(CollectorError::NoReply);
            }
            let recv = match tokio::time::timeout(remaining, socket.recv(&mut buf)).await {
                Err(_) => break Err(CollectorError::NoReply),
                Ok(Err(e)) => {
                    break Err(CollectorError::IoError(format!("BACnet RPM recv: {}", e)))
                }
                Ok(Ok(n)) => n,
            };
            if recv < 6 {
                break Err(CollectorError::InvalidFrame(
                    "BACnet RPM response too short".into(),
                ));
            }
            if buf[0] != 0x81 {
                break Err(CollectorError::InvalidFrame(format!(
                    "invalid BVLC type 0x{:02X}",
                    buf[0]
                )));
            }
            if buf[1] == 0x00 {
                let code = u16::from_be_bytes([buf[4], buf[5]]);
                break Err(CollectorError::InvalidFrame(format!(
                    "BACnet BVLC-Result error: code={}",
                    code
                )));
            }
            let npdu = &buf[4..recv];
            let resp_apdu = match Self::skip_npdu(npdu) {
                Ok(a) => a,
                Err(e) => break Err(e),
            };
            if frames::apdu_invoke_id(resp_apdu) != Some(invoke_id) {
                continue; // 丢弃错配响应
            }
            match frames::parse_rpm_ack(resp_apdu, invoke_id) {
                Ok(list) => {
                    let mut m: HashMap<u32, (f64, Quality)> = HashMap::new();
                    for (oid, v, q) in list {
                        m.insert(oid, (v, q));
                    }
                    break Ok(m);
                }
                Err(e) => break Err(e),
            }
        };

        // 3) 按入参顺序回填（缓存命中优先，其余取 RPM 结果）
        let mut results: Vec<CollectorResult<DataPoint>> = Vec::with_capacity(points.len());
        for (i, p) in points.iter().enumerate() {
            if let Some(dp) = cached[i].take() {
                results.push(Ok(dp));
                continue;
            }
            match &parsed[i] {
                Err(e) => results.push(Err(clone_collector_error(e))),
                Ok((ot, inst)) => match &batch {
                    Err(e) => results.push(Err(clone_collector_error(e))),
                    Ok(map) => {
                        let oid = ((*ot as u32) << 22) | (*inst & 0x3FFFFF);
                        match map.get(&oid) {
                            Some((v, q)) => {
                                // 批量响应同样不带时标：取本批解析时刻
                                let ts = DataPoint::now();
                                results.push(Ok(DataPoint {
                                    point_id: p.id.clone(),
                                    device_id: p.device_id.clone().unwrap_or_default(),
                                    // 批量路径与单点路径同一套换算，避免两条路径数值不一致
                                    value: convert_value(p, *v),
                                    raw_value: *v,
                                    raw_type: "bacnet".into(),
                                    quality: *q,
                                    quality_reason: String::new(),
                                    timestamp: ts,
                                    source_ts: ts,
                                    stale_count: 0,
                                    value_text: None,
                                }));
                            }
                            None => results.push(Err(CollectorError::NoReply)),
                        }
                    }
                },
            }
        }
        finish(results)
    }
}
