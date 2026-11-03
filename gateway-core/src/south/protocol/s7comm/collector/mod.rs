//! S7Comm 采集器 trait 实现之一：建链/握手、连接状态与 SZL 诊断入口。
use super::*;

use crate::config::PointConfig;
use crate::engine::model::DataPoint;
use crate::south::protocol::Collector;

mod read;
mod write;

impl Collector for S7CommCollector {
    fn name(&self) -> &'static str {
        "s7comm"
    }

    fn bus_name(&self) -> &str {
        &self.bus_name
    }

    fn connect<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = CollectorResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let mut guard = self.stream.lock().await;
            if guard.is_some() {
                self.connected.store(true, Ordering::Relaxed);
                return Ok(());
            }

            let stream = TcpStream::connect(&self.addr).await.map_err(|e| {
                CollectorError::IoError(format!("S7 connect {} failed: {}", self.addr, e))
            })?;
            let mut stream = stream;
            stream.set_nodelay(true).ok();

            // Step 1: COTP Connect
            let cotp_req = frames::build_cotp_connect(self.rack, self.slot);
            tokio::time::timeout(self.io_timeout, stream.write_all(&cotp_req))
                .await
                .map_err(|_| {
                    CollectorError::IoError(format!(
                        "S7 send COTP connect timeout ({} ms)",
                        self.io_timeout.as_millis()
                    ))
                })?
                .map_err(|e| CollectorError::IoError(format!("S7 send COTP connect: {}", e)))?;
            // 等待 Connect Confirm，校验响应类型
            let cotp_resp = frames::read_tpkt_frame(&mut stream).await?;
            if cotp_resp.len() < 2 {
                return Err(CollectorError::InvalidFrame(
                    "COTP Connect Confirm too short".into(),
                ));
            }
            // COTP PDU type: 0xD0 = Connect Confirm, 0xD3 = Reject
            let cotp_pdu_type = cotp_resp[1] & 0xF0;
            if cotp_pdu_type != 0xD0 {
                return Err(CollectorError::IoError(format!(
                    "S7 COTP connect rejected (PDU type: 0x{:02X})",
                    cotp_resp[1]
                )));
            }

            // Step 2: S7 Setup Communication
            let setup_req = frames::build_setup_comm();
            tokio::time::timeout(self.io_timeout, stream.write_all(&setup_req))
                .await
                .map_err(|_| {
                    CollectorError::IoError(format!(
                        "S7 send setup comm timeout ({} ms)",
                        self.io_timeout.as_millis()
                    ))
                })?
                .map_err(|e| CollectorError::IoError(format!("S7 send setup comm: {}", e)))?;
            let setup_resp = frames::read_tpkt_frame(&mut stream).await?;
            // 验证 Setup Communication 响应的 PDU type
            // setup_resp 结构: COTP(3) + S7 Header(12 = 10 基础头 + Error class/code) + Parameter(8)
            // setup_resp[3] = Protocol ID (0x32), setup_resp[4] = PDU type
            if setup_resp.len() >= 5 && setup_resp[3] == 0x32 {
                let setup_pdu_type = setup_resp[4];
                if (setup_pdu_type & 0x80) != 0 {
                    // 错误响应：Setup Communication 被拒绝
                    // Error class/code 紧随 10 字节基础头（s7[10]/s7[11]）→ 帧内偏移 [13]/[14]
                    let err_class = if setup_resp.len() > 13 {
                        setup_resp[13]
                    } else {
                        0
                    };
                    let err_code = if setup_resp.len() > 14 {
                        setup_resp[14]
                    } else {
                        0
                    };
                    return Err(CollectorError::IoError(format!(
                        "S7 Setup Communication rejected: PDU type=0x{:02X}, class=0x{:02X} code=0x{:02X}",
                        setup_pdu_type, err_class, err_code
                    )));
                }
                if setup_pdu_type != PDU_ACK {
                    return Err(CollectorError::IoError(format!(
                        "S7 Setup Communication unexpected PDU type: 0x{:02X}",
                        setup_pdu_type
                    )));
                }
            }
            // 解析协商的 PDU 大小（仅在响应有效时）
            // Bug: Setup Communication 响应是 Ack-Data，头为 12 字节（含 Error class/code），
            // 参数段自 3+12 起，PDU 大小位于参数偏移 +6。旧按 10 取值会读到 MaxAmQ 字段
            // （典型 0x0001），使分片大小退化为 1 字节、请求近乎全失败。
            if setup_resp.len() >= 3 + 12 + 8 && setup_resp[3] == 0x32 && setup_resp[4] == PDU_ACK {
                let pdu = u16::from_be_bytes([setup_resp[3 + 12 + 6], setup_resp[3 + 12 + 7]]);
                if pdu > 0 {
                    *self.pdu_size.lock().await = pdu;
                    tracing::debug!("S7 negotiated PDU size: {}", pdu);
                }
            }

            *guard = Some(stream);
            self.connected.store(true, Ordering::Relaxed);
            tracing::info!(
                "S7 connected to {} (rack={}, slot={})",
                self.addr,
                self.rack,
                self.slot
            );
            Ok(())
        })
    }

    fn disconnect<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let _ = self.stream.lock().await.take();
            self.connected.store(false, Ordering::Relaxed);
        })
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// 诊断入口（决策表 #12）：读取组件标识 SZL（0x0011 / 0x001C，Index=1），
    /// 汇总订货号 / 模块类型 / 序列号等可读字符串供上位诊断展示。
    ///
    /// best-effort：任一 SZL 读失败不影响整体，仅记录失败原因；全部无文本时
    /// 返回可读的失败摘要而非 panic，保证诊断路径永不中断采集主链路。
    fn diag_supported(&self) -> bool {
        true
    }

    fn diag<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send + 'a>> {
        Box::pin(async move {
            let mut tags: Vec<String> = Vec::new();
            let mut errors: Vec<String> = Vec::new();
            for (id, label) in [
                (SZL_ID_COMPONENT_ID, "SZL 0x0011"),
                (SZL_ID_COMPONENT_ALL, "SZL 0x001C"),
            ] {
                match self.read_szl(id, 1).await {
                    Ok(resp) => {
                        let strs = frames::extract_ascii_strings(&resp.records);
                        if strs.is_empty() {
                            errors.push(format!(
                                "{}: id=0x{:04X} idx=0x{:04X} list_len={} records={} (no text)",
                                label,
                                resp.szl_id,
                                resp.szl_index,
                                resp.list_len,
                                resp.record_count
                            ));
                        } else {
                            for s in strs {
                                if !tags.contains(&s) {
                                    tags.push(s);
                                }
                            }
                        }
                    }
                    Err(e) => errors.push(format!("{}: {}", label, e)),
                }
            }
            if !tags.is_empty() {
                format!("{}@{}: {}", self.name(), self.bus_name(), tags.join(" | "))
            } else {
                format!(
                    "{}@{}: SZL unavailable ({})",
                    self.name(),
                    self.bus_name(),
                    errors.join("; ")
                )
            }
        })
    }

    fn read<'a>(
        &'a self,
        point: &'a PointConfig,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = CollectorResult<DataPoint>> + Send + 'a>>
    {
        Box::pin(self.read_point(point))
    }

    fn read_batch<'a>(
        &'a self,
        points: &'a [PointConfig],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Vec<CollectorResult<DataPoint>>> + Send + 'a>,
    > {
        Box::pin(self.read_batch_points(points))
    }

    fn write<'a>(
        &'a self,
        point: &'a PointConfig,
        value: f64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = CollectorResult<()>> + Send + 'a>> {
        Box::pin(self.write_point(point, value))
    }

    fn write_multi<'a>(
        &'a self,
        points: &'a [(&'a PointConfig, f64)],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<CollectorResult<()>>> + Send + 'a>>
    {
        Box::pin(self.write_multi_points(points))
    }
}
