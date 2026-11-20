//! `Collector` trait 实现：建链 / 读 / 写 / 批量读 / 诊断。

use std::sync::atomic::Ordering;

use super::*;

use crate::south::protocol::{
    convert_value, mark_write_uncertain_with, reverse_convert, ResultSlots,
};

mod batch;

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

                let (raw_value, quality) = frames::parse_value_from_apdu(apdu, invoke_id)?;
                // BACnet 读响应不带时标：原始采集时刻即本次读成功时刻
                let ts = DataPoint::now();

                return Ok(DataPoint {
                    point_id: point.id.clone(),
                    device_id: point.device_id.clone().unwrap_or_default(),
                    // BACnet 解析出的只是原始值，scale/offset/formula 与 CT/PT 变比在 common 层统一换算
                    value: convert_value(point, raw_value),
                    raw_value,
                    raw_type: "bacnet".into(),
                    quality,
                    quality_reason: String::new(),
                    timestamp: ts,
                    source_ts: ts,
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

            // 北向下发的是工程量，写回前按读路径反向还原，保证读写对称
            let apdu = frames::build_write_property_apdu(
                obj_type,
                instance,
                invoke_id,
                reverse_convert(point, value),
            );
            let packet = frames::wrap_bvlc_npdu(&apdu);
            drain_socket(&socket);
            // 请求已发出：UDP 发送失败也无法证明设备未收到 → 结果未知
            socket.send(&packet).await.map_err(|e| {
                mark_write_uncertain_with(
                    self.metrics.as_deref(),
                    CollectorError::IoError(format!("BACnet write send: {}", e)),
                )
            })?;

            let deadline = tokio::time::Instant::now() + self.io_timeout;
            let mut buf = vec![0u8; 1500];
            loop {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    return Err(mark_write_uncertain_with(
                        self.metrics.as_deref(),
                        CollectorError::NoReply,
                    ));
                }
                let recv = tokio::time::timeout(remaining, socket.recv(&mut buf))
                    .await
                    .map_err(|_| {
                        mark_write_uncertain_with(self.metrics.as_deref(), CollectorError::NoReply)
                    })?
                    .map_err(|e| {
                        mark_write_uncertain_with(
                            self.metrics.as_deref(),
                            CollectorError::IoError(format!("BACnet write recv: {}", e)),
                        )
                    })?;
                if recv < 6 {
                    return Err(mark_write_uncertain_with(
                        self.metrics.as_deref(),
                        CollectorError::InvalidFrame("BACnet write response too short".into()),
                    ));
                }
                if buf[0] != 0x81 {
                    return Err(mark_write_uncertain_with(
                        self.metrics.as_deref(),
                        CollectorError::InvalidFrame(format!("invalid BVLC type 0x{:02X}", buf[0])),
                    ));
                }
                if buf[1] == 0x00 {
                    // BVLC-Result 非 0：设备明确回了错误码 → 写确定未生效
                    let code = u16::from_be_bytes([buf[4], buf[5]]);
                    return Err(CollectorError::DeviceRejected(format!(
                        "BACnet BVLC-Result error: code={}",
                        code
                    )));
                }
                let npdu = &buf[4..recv];
                let resp_apdu = Self::skip_npdu(npdu)
                    .map_err(|e| mark_write_uncertain_with(self.metrics.as_deref(), e))?;
                if frames::apdu_invoke_id(resp_apdu) != Some(invoke_id) {
                    continue; // 迟到/错配响应：丢弃后继续等待
                }
                // Error/Reject/Abort → DeviceRejected（明确失败）；其余解析失败 → 结果未知
                frames::parse_write_property_ack(resp_apdu, invoke_id)
                    .map_err(|e| mark_write_uncertain_with(self.metrics.as_deref(), e))?;
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
            let slots: ResultSlots = Arc::new(std::sync::Mutex::new(
                (0..points.len()).map(|_| None).collect(),
            ));
            self.read_batch_points_with_slots(points, slots).await
        })
    }

    /// 缺陷 #7：沿用 `read_batch` 的 ReadPropertyMultiple 单事务批量语义，每个点位
    /// 结果产生即写共享槽，组超时 drop future 时已完成结果不丢失。
    #[allow(private_interfaces)]
    fn read_batch_with_slots<'a>(
        &'a self,
        points: &'a [PointConfig],
        slots: ResultSlots,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let _ = self.read_batch_points_with_slots(points, slots).await;
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
