//! `Collector` trait 实现：建链 / 读 / 写 / 批量读 / 诊断。

use std::sync::atomic::Ordering;

use super::*;

impl Collector for BacnetCollector {
    fn name(&self) -> &'static str {
        "bacnet"
    }

    fn bus_name(&self) -> &str {
        &self.bus_name
    }

    fn connect<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = CollectorResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let mut guard = self.socket.lock().await;
            if guard.is_some() {
                return Ok(());
            }
            // 绑定本地端口（任意可用端口）
            let socket = UdpSocket::bind("0.0.0.0:0")
                .await
                .map_err(|e| CollectorError::IoError(format!("BACnet bind failed: {}", e)))?;
            socket.connect(&self.addr).await.map_err(|e| {
                CollectorError::IoError(format!("BACnet connect {} failed: {}", self.addr, e))
            })?;
            *guard = Some(Arc::new(socket));
            self.connected.store(true, Ordering::Relaxed);
            drop(guard);

            // COV 监听独占一条独立 socket：UDP 是单接收队列，与 read/write
            // 共用会把请求响应当成通知吞掉。订阅请求由监听任务自己发出，
            // 设备据此把通知回送到该 socket 的源端口。
            let mut link_guard = self.cov_link.lock().await;
            if link_guard.is_none() {
                match UdpSocket::bind("0.0.0.0:0").await {
                    Ok(cov_socket) => match cov_socket.connect(&self.addr).await {
                        Ok(()) => {
                            let stop = Arc::new(AtomicBool::new(false));
                            let task = tokio::spawn(cov_listener(
                                Arc::new(cov_socket),
                                self.cov.clone(),
                                stop.clone(),
                            ));
                            *link_guard = Some(CovLink { stop, task });
                        }
                        Err(e) => {
                            tracing::warn!("BACnet COV connect {} failed: {}", self.addr, e)
                        }
                    },
                    Err(e) => tracing::warn!("BACnet COV bind failed: {}", e),
                }
            }
            Ok(())
        })
    }

    fn disconnect<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let _ = self.socket.lock().await.take();
            self.connected.store(false, Ordering::Relaxed);
            if let Some(link) = self.cov_link.lock().await.take() {
                link.stop.store(true, Ordering::Relaxed);
                link.task.abort();
                let _ = link.task.await;
            }
            // 重连后必须重新订阅：清空缓存，避免陈旧值被当作推送值复用。
            let mut cov = self.cov.lock().await;
            cov.cache.clear();
            cov.active.clear();
            cov.wanted.clear();
        })
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    fn read<'a>(
        &'a self,
        point: &'a PointConfig,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = CollectorResult<DataPoint>> + Send + 'a>>
    {
        Box::pin(async move {
            let (obj_type, instance) = Self::parse_object_address(&point.address)?;

            // COV 优先：订阅生效后设备主动推送变化值，此处直接返回缓存，
            // 免去一次 ReadProperty 往返（省轮询）。订阅未确认/过期则继续
            // 走下面的主动读取，保证不因 COV 失败而产出陈旧值。
            let oid = Self::object_id(obj_type, instance);
            if let Some(dp) = self.cov_cached(point, oid).await {
                return Ok(dp);
            }
            self.cov_wanted(oid).await;

            // 修复(审查#1)：调度器只调用 read/read_batch，从不显式 connect()，
            // 原先直接取 self.socket 会恒返回 "socket not connected"，导致 BACnet 采集 100% 失败。
            // 改为与其它 TCP 类协议一致：入口惰性建链。
            let socket = self.ensure_connected().await?;

            let mut invoke_guard = self.invoke_id.lock().await;
            let invoke_id = *invoke_guard;
            *invoke_guard = invoke_id.wrapping_add(1);
            if *invoke_guard == 0 {
                *invoke_guard = 1;
            }
            drop(invoke_guard);

            let packet = frames::build_read_property(obj_type, instance, invoke_id);
            // 发送前清空接收缓冲区：UDP 无连接，上一次请求超时后残留的迟到响应
            // 若不清理，会被本次 recv 消费并与当前请求错配（陈旧值当新值）。
            drain_socket(&socket);
            socket
                .send(&packet)
                .await
                .map_err(|e| CollectorError::IoError(format!("BACnet send: {}", e)))?;

            // 接收响应：BACnet/IP 在 UDP 上，响应可能乱序/重复/残留，
            // 必须按 invoke id 归位——丢弃不匹配的数据报，直到取到目标响应或整体超时。
            let deadline = tokio::time::Instant::now() + self.io_timeout;
            let mut buf = vec![0u8; 1500];
            loop {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    return Err(CollectorError::NoReply);
                }
                let recv = tokio::time::timeout(remaining, socket.recv(&mut buf))
                    .await
                    .map_err(|_| CollectorError::NoReply)?
                    .map_err(|e| CollectorError::IoError(format!("BACnet recv: {}", e)))?;

                if recv < 6 {
                    return Err(CollectorError::InvalidFrame(
                        "BACnet response too short".into(),
                    ));
                }

                // 解析 BVLC：type(1) + function(1) + length(2)
                if buf[0] != 0x81 {
                    return Err(CollectorError::InvalidFrame(format!(
                        "invalid BVLC type 0x{:02X}",
                        buf[0]
                    )));
                }
                // BVLC function: 0x0a = Original-Unicast-NPDU, 0x04 = Forwarded-NPDU (路由器)
                // 0x00 = BVLC-Result（错误响应），不应出现在数据响应中
                if buf[1] == 0x00 {
                    let result_code = u16::from_be_bytes([buf[4], buf[5]]);
                    return Err(CollectorError::InvalidFrame(format!(
                        "BACnet BVLC-Result error: code={}",
                        result_code
                    )));
                }
                // NPDU 从 buf[4] 开始，用统一的 skip_npdu 跳过 NPDU 头得到 APDU
                let npdu = &buf[4..recv];
                let apdu = Self::skip_npdu(npdu)?;

                // invoke id 归位：不匹配的响应（迟到/重复/其它请求的残留）丢弃后继续等待
                if frames::apdu_invoke_id(apdu) != Some(invoke_id) {
                    tracing::debug!(
                        "BACnet: dropping mismatched response (invoke id {:?}, expected {})",
                        frames::apdu_invoke_id(apdu),
                        invoke_id
                    );
                    continue;
                }

                let (value, quality) = frames::parse_value_from_apdu(apdu, invoke_id)?;

                return Ok(DataPoint {
                    point_id: point.id.clone(),
                    device_id: point.device_id.clone().unwrap_or_default(),
                    value,
                    raw_value: value,
                    raw_type: "bacnet".into(),
                    quality,
                    quality_reason: String::new(),
                    timestamp: chrono::Utc::now().timestamp_millis(),
                    source_ts: 0,
                    stale_count: 0,
                    value_text: None,
                });
            }
        })
    }

    /// 决策表 #20：写入 = WriteProperty（service 15，Present_Value）
    ///
    /// 值编码按目标对象类型：二进制对象（BI/BO/BV）用 Enumerated(0/1)，模拟量用 REAL。
    /// 写入优先级固定 8（手动操作员）。返回 Ok 仅当收到匹配 invoke id 且服务号为
    /// WriteProperty 的 SimpleACK/ComplexACK；Error/Reject/Abort 一律判失败。
    fn write<'a>(
        &'a self,
        point: &'a PointConfig,
        value: f64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = CollectorResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let (obj_type, instance) = Self::parse_object_address(&point.address)?;
            // 修复(审查#1)：同 read()，入口惰性建链
            let socket = self.ensure_connected().await?;

            let mut ig = self.invoke_id.lock().await;
            let invoke_id = *ig;
            *ig = invoke_id.wrapping_add(1);
            if *ig == 0 {
                *ig = 1;
            }
            drop(ig);

            let apdu = frames::build_write_property_apdu(obj_type, instance, invoke_id, value);
            let packet = frames::wrap_bvlc_npdu(&apdu);
            drain_socket(&socket);
            socket
                .send(&packet)
                .await
                .map_err(|e| CollectorError::IoError(format!("BACnet write send: {}", e)))?;

            let deadline = tokio::time::Instant::now() + self.io_timeout;
            let mut buf = vec![0u8; 1500];
            loop {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    return Err(CollectorError::NoReply);
                }
                let recv = tokio::time::timeout(remaining, socket.recv(&mut buf))
                    .await
                    .map_err(|_| CollectorError::NoReply)?
                    .map_err(|e| CollectorError::IoError(format!("BACnet write recv: {}", e)))?;
                if recv < 6 {
                    return Err(CollectorError::InvalidFrame(
                        "BACnet write response too short".into(),
                    ));
                }
                if buf[0] != 0x81 {
                    return Err(CollectorError::InvalidFrame(format!(
                        "invalid BVLC type 0x{:02X}",
                        buf[0]
                    )));
                }
                if buf[1] == 0x00 {
                    let code = u16::from_be_bytes([buf[4], buf[5]]);
                    return Err(CollectorError::InvalidFrame(format!(
                        "BACnet BVLC-Result error: code={}",
                        code
                    )));
                }
                let npdu = &buf[4..recv];
                let resp_apdu = Self::skip_npdu(npdu)?;
                if frames::apdu_invoke_id(resp_apdu) != Some(invoke_id) {
                    continue; // 迟到/错配响应：丢弃后继续等待
                }
                frames::parse_write_property_ack(resp_apdu, invoke_id)?;
                return Ok(());
            }
        })
    }

    /// 决策表 #20：批量读走 ReadPropertyMultiple（一次事务读多点）
    ///
    /// 全部合法对象用**单个** RPM 请求读取 Present_Value，按 invoke id 归位后，
    /// 把结果按对象 ID 映射回入参顺序；地址非法或响应缺失的点位仅置单项错误，
    /// 不中断整批。批量级失败（发送/超时/解析错误）时整批回填同因错误，
    /// 绝不把失败静默当作 Good 值。
    fn read_batch<'a>(
        &'a self,
        points: &'a [PointConfig],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Vec<CollectorResult<DataPoint>>> + Send + 'a>,
    > {
        Box::pin(async move {
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
                return out;
            }

            // 修复(审查#1)：同一入口惰性建链；失败时整批回填同型错误，保持返回值与入参等长
            let socket = match self.ensure_connected().await {
                Ok(s) => s,
                Err(e) => {
                    return points
                        .iter()
                        .map(|_| Err(clone_collector_error(&e)))
                        .collect::<Vec<CollectorResult<DataPoint>>>();
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
                return points
                    .iter()
                    .map(|_| Err(clone_collector_error(&err)))
                    .collect::<Vec<CollectorResult<DataPoint>>>();
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
                                Some((v, q)) => results.push(Ok(DataPoint {
                                    point_id: p.id.clone(),
                                    device_id: p.device_id.clone().unwrap_or_default(),
                                    value: *v,
                                    raw_value: *v,
                                    raw_type: "bacnet".into(),
                                    quality: *q,
                                    quality_reason: String::new(),
                                    timestamp: chrono::Utc::now().timestamp_millis(),
                                    source_ts: 0,
                                    stale_count: 0,
                                    value_text: None,
                                })),
                                None => results.push(Err(CollectorError::NoReply)),
                            }
                        }
                    },
                }
            }
            results
        })
    }

    /// 决策表 #21：诊断 = 定向 Who-Is 探测，报告设备实例号 / 厂商 ID / 最大 APDU。
    fn diag_supported(&self) -> bool {
        true
    }

    fn diag<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send + 'a>> {
        Box::pin(async move {
            match self.who_is_device().await {
                Ok(info) => format!(
                    "bacnet@{}: ok (device {} / vendor {} / maxAPDU {})",
                    self.bus_name, info.device_instance, info.vendor_id, info.max_apdu
                ),
                Err(e) => format!("bacnet@{}: error ({})", self.bus_name, e),
            }
        })
    }
}
