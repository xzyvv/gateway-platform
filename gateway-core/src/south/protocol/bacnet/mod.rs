//! BACnet/IP 客户端采集器
//!
//! 楼宇自动化协议，UDP 传输（默认端口 47808）。
//! 实现子集：ReadProperty / ReadPropertyMultiple（批量读）+ WriteProperty（写 Present_Value）
//! + SubscribeCOV / COVNotification（推送式采集：订阅确认后 read 优先返回推送缓存）。
//!
//! COV 使用独立的订阅 socket 与后台监听任务，避免与请求/响应共用同一条 UDP
//! 接收队列而互相抢包；订阅失败或过期时自动退回主动轮询。
//!
//! 应用层（APDU/NPDU）编解码函数标记为 pub(crate)，供 BACnet MS/TP 串行链路层复用。
//!
//! 配置：
//! - bus.tcp_addr: BACnet 设备 IP:端口，如 "192.168.1.100:47808"
//! - point.address: "object_type:instance"，如 "analog-input:1" 或 "0:1"
//!   object_type: 0=AI, 1=AO, 2=AV, 3=BI, 4=BO, 5=BV
//! - point.data_type: 可选，默认按属性类型解析

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::sync::Mutex;

use crate::config::PointConfig;
use crate::engine::model::DataPoint;
use crate::engine::model::Quality;
use crate::south::protocol::{clone_collector_error, Collector, CollectorError, CollectorResult};

mod collector;
mod consts;
mod cov;
pub(crate) mod frames;

pub(crate) use consts::*;
use cov::{cov_listener, CovLink, CovState};

#[cfg(test)]
use cov::{classify_cov_datagram, CovEvent};
#[cfg(test)]
use std::sync::atomic::Ordering;

/// BACnet/IP 采集器
pub struct BacnetCollector {
    addr: String,
    socket: Mutex<Option<Arc<UdpSocket>>>,
    bus_name: String,
    invoke_id: Mutex<u8>,
    /// 连接状态标志（UDP 无连接，但用于标记 socket 是否已建立）
    connected: AtomicBool,
    /// 单次请求 IO 超时（源自 bus.timeout_ms，统一南向超时来源）
    io_timeout: Duration,
    /// COV 推送缓存与订阅状态
    cov: Arc<Mutex<CovState>>,
    /// COV 监听任务（惰性随 connect 启动，disconnect 时拆除）
    cov_link: Mutex<Option<CovLink>>,
}

impl BacnetCollector {
    pub fn new(addr: String, bus_name: String, io_timeout: Duration) -> Self {
        Self {
            addr,
            socket: Mutex::new(None),
            bus_name,
            invoke_id: Mutex::new(1),
            connected: AtomicBool::new(false),
            io_timeout,
            cov: Arc::new(Mutex::new(CovState::default())),
            cov_link: Mutex::new(None),
        }
    }

    /// 解析对象类型字符串
    pub(crate) fn parse_object_type(s: &str) -> CollectorResult<u16> {
        match s.to_lowercase().as_str() {
            "analog-input" | "ai" | "0" => Ok(OBJECT_ANALOG_INPUT),
            "analog-output" | "ao" | "1" => Ok(OBJECT_ANALOG_OUTPUT),
            "analog-value" | "av" | "2" => Ok(OBJECT_ANALOG_VALUE),
            "binary-input" | "bi" | "3" => Ok(OBJECT_BINARY_INPUT),
            "binary-output" | "bo" | "4" => Ok(OBJECT_BINARY_OUTPUT),
            "binary-value" | "bv" | "5" => Ok(OBJECT_BINARY_VALUE),
            n => n.parse::<u16>().map_err(|_| {
                CollectorError::InvalidConfig(format!("unknown BACnet object type '{}'", s))
            }),
        }
    }

    /// 解析 point.address: "type:instance"
    pub(crate) fn parse_object_address(s: &str) -> CollectorResult<(u16, u32)> {
        let parts: Vec<&str> = s.splitn(2, ':').collect();
        if parts.len() != 2 {
            return Err(CollectorError::InvalidConfig(format!(
                "invalid BACnet address '{}', expected type:instance",
                s
            )));
        }
        let obj_type = Self::parse_object_type(parts[0])?;
        let instance: u32 = parts[1]
            .parse()
            .map_err(|_| CollectorError::InvalidConfig(format!("invalid instance in '{}'", s)))?;
        // BACnet Object ID 的 instance 字段为 22 位，最大 4194303 (0x3FFFFF)
        if instance > 0x3FFFFF {
            return Err(CollectorError::InvalidConfig(format!(
                "instance {} exceeds 22-bit max (4194303)",
                instance
            )));
        }
        Ok((obj_type, instance))
    }

    /// 组合 BACnet Object Identifier（type 10 位 + instance 22 位）
    fn object_id(obj_type: u16, instance: u32) -> u32 {
        ((obj_type as u32) << 22) | (instance & 0x3F_FFFF)
    }

    /// 取 COV 推送缓存中该对象的最新值
    ///
    /// 仅当订阅仍在有效期内（设备确认时间未超过生命周期）才返回缓存；
    /// 订阅未确认/已过期时返回 None，调用方回退到主动轮询——COV 失败绝不
    /// 让采集静默沿用陈旧值。
    async fn cov_cached(&self, point: &PointConfig, oid: u32) -> Option<DataPoint> {
        let guard = self.cov.lock().await;
        let confirmed_ms = *guard.active.get(&oid)?;
        let entry = guard.cache.get(&oid)?;
        if chrono::Utc::now().timestamp_millis() - confirmed_ms >= COV_LIFETIME_MS {
            return None;
        }
        Some(DataPoint {
            point_id: point.id.clone(),
            device_id: point.device_id.clone().unwrap_or_default(),
            value: entry.value,
            raw_value: entry.value,
            raw_type: "bacnet".into(),
            quality: entry.quality,
            quality_reason: String::new(),
            timestamp: entry.received_ms,
            source_ts: 0,
            stale_count: 0,
            value_text: None,
        })
    }

    /// 登记需要 COV 订阅的对象（后台监听任务下一拍消费）
    async fn cov_wanted(&self, oid: u32) {
        self.cov.lock().await.wanted.insert(oid);
    }

    /// 编码 BACnet 无符号整数（变长）
    pub(crate) fn encode_unsigned(value: u32) -> Vec<u8> {
        if value <= 0xFF {
            vec![value as u8]
        } else if value <= 0xFFFF {
            vec![(value >> 8) as u8, value as u8]
        } else if value <= 0xFF_FFFF {
            vec![(value >> 16) as u8, (value >> 8) as u8, value as u8]
        } else {
            vec![
                (value >> 24) as u8,
                (value >> 16) as u8,
                (value >> 8) as u8,
                value as u8,
            ]
        }
    }

    /// 编码上下文标签
    pub(crate) fn encode_context_tag(tag: u8, data: &[u8]) -> Vec<u8> {
        let mut result = Vec::new();
        let len = data.len();
        // BACnet 标准 tag 字节布局：bits7-4 = tag 编号，bit3 = context 标志，bits2-0 = 长度
        if len < 5 {
            // 长度 0-4：直接编码在 bits2-0
            result.push(((tag & 0x0F) << 4) | 0x08 | (len as u8));
        } else if len < 0xFE {
            // 长度 5-253：扩展长度（1 字节）
            result.push(((tag & 0x0F) << 4) | 0x08 | 0x05);
            result.push(len as u8);
        } else {
            // 长度 254-65535：扩展长度（2 字节）
            result.push(((tag & 0x0F) << 4) | 0x08 | 0x05);
            result.push(0xFE);
            result.extend_from_slice(&(len as u16).to_be_bytes());
        }
        result.extend_from_slice(data);
        result
    }

    /// 构建 ReadProperty 请求的 APDU 部分（不含 BVLC/NPDU）
    ///
    /// 供 BACnet MS/TP 串行链路层复用：MS/TP 帧自带数据链路层寻址，
    /// NPDU 头与 APDU 由本函数拼装后作为 MS/TP Data 域负载。
    pub(crate) fn build_read_property_apdu(obj_type: u16, instance: u32, invoke_id: u8) -> Vec<u8> {
        // Confirmed-Request PDU: 0x00; Max segments=0 (unspecified); Max APDU length=2 (1024 octets);
        // Invoke ID; Service choice: ReadProperty (12)
        let mut apdu = vec![0x00, 0x02, invoke_id, SERVICE_READ_PROPERTY];

        // Object ID: context tag 0
        let mut obj_id = Vec::new();
        // object type (10 bits) + instance (22 bits)
        let type_bits = (obj_type as u32) << 22;
        let id_val = type_bits | (instance & 0x3FFFFF);
        obj_id.extend_from_slice(&id_val.to_be_bytes());
        apdu.extend(Self::encode_context_tag(0, &obj_id));

        // Property ID: context tag 1 (enumerated)
        let prop_data = Self::encode_unsigned(PROP_PRESENT_VALUE);
        apdu.extend(Self::encode_context_tag(1, &prop_data));

        apdu
    }

    /// 构建 NPDU 头（无 DNET，单播直连场景）
    ///
    /// version(1) + control(1, 0x00 表示无网络层消息、无 DNET)
    pub(crate) fn build_npdu_header() -> [u8; 2] {
        [0x01, 0x00]
    }

    /// 跳过 NPDU 头，返回 APDU 切片
    ///
    /// NPDU 头格式：
    /// - version(1) + control(1)
    /// - [+ DNET(2) + DLEN(1) + DADR(DLEN) + HopCount(1)] if DNET present (control bit 6)
    /// - [+ SNET(2) + SLEN(1) + SADR(SLEN)] if SNET present (control bit 5)
    ///
    /// control bit 7 (0x80) = 网络层消息标志（1=网络层消息，非 APDU）
    pub(crate) fn skip_npdu(data: &[u8]) -> CollectorResult<&[u8]> {
        if data.len() < 2 {
            return Err(CollectorError::InvalidFrame("NPDU too short".into()));
        }
        let control = data[1];

        // bit 7: 网络层消息标志（响应不应该是网络层消息）
        if (control & 0x80) != 0 {
            return Err(CollectorError::InvalidFrame(
                "NPDU is network layer message, not APDU".into(),
            ));
        }

        let mut offset = 2usize; // version(1) + control(1)

        // bit 6 (0x40): DNET 存在标志
        if (control & 0x40) != 0 {
            // DNET(2) + DLEN(1) + DADR(DLEN) + HopCount(1)
            if data.len() < offset + 3 {
                return Err(CollectorError::InvalidFrame("truncated DNET/DLEN".into()));
            }
            let dlen = data[offset + 2] as usize;
            let dnet_total = 2 + 1 + dlen + 1; // DNET + DLEN + DADR + HopCount
            if data.len() < offset + dnet_total {
                return Err(CollectorError::InvalidFrame(
                    "truncated DADR/HopCount".into(),
                ));
            }
            offset += dnet_total;
        }

        // bit 5 (0x20): SNET 存在标志
        if (control & 0x20) != 0 {
            // SNET(2) + SLEN(1) + SADR(SLEN)
            if data.len() < offset + 3 {
                return Err(CollectorError::InvalidFrame("truncated SNET/SLEN".into()));
            }
            let slen = data[offset + 2] as usize;
            let snet_total = 2 + 1 + slen; // SNET + SLEN + SADR
            if data.len() < offset + snet_total {
                return Err(CollectorError::InvalidFrame("truncated SADR".into()));
            }
            offset += snet_total;
        }

        if data.len() < offset {
            return Err(CollectorError::InvalidFrame("no APDU after NPDU".into()));
        }
        Ok(&data[offset..])
    }

    /// 决策表 #21：确保 UDP socket 已建立（惰性连接），返回其句柄。
    async fn ensure_connected(&self) -> CollectorResult<Arc<UdpSocket>> {
        self.connect().await?;
        let guard = self.socket.lock().await;
        guard
            .as_ref()
            .cloned()
            .ok_or_else(|| CollectorError::IoError("BACnet socket not connected".into()))
    }

    /// 决策表 #21：向已连接设备发送定向 Who-Is，等待 I-Am 响应并解析设备信息。
    ///
    /// UDP 无连接，接收循环按 APDU 类型分派：非 I-Am 报文（无关/迟到）跳过后继续等待，
    /// 直到取到 I-Am 或整体超时。
    async fn who_is_device(&self) -> CollectorResult<frames::IAmInfo> {
        let socket = self.ensure_connected().await?;
        let packet = frames::wrap_bvlc_npdu(&frames::build_who_is_apdu(None, None));
        drain_socket(&socket);
        socket
            .send(&packet)
            .await
            .map_err(|e| CollectorError::IoError(format!("BACnet Who-Is send: {}", e)))?;

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
            let apdu = Self::skip_npdu(&buf[4..recv])?;
            match frames::parse_i_am_apdu(apdu) {
                Ok(info) => return Ok(info),
                Err(_) => continue, // 非 I-Am（无关报文），丢弃后继续等待
            }
        }
    }

    /// 决策表 #21：广播 Who-Is 并收集窗口期内的 I-Am，返回网段内发现的设备列表。
    ///
    /// 使用独立的广播 socket（BACnet/IP 默认端口 47808），按 device instance 去重。
    /// 无生产调用方：供后续 Web/运维侧「网段设备扫描」入口接入，当前预留。
    // X4 (fix)：生产侧没有调用方——上一个 use site 还在 `subscribe_cov` 内部，
    // 但 subscribe_cov 整体本身就是 cfg(test)。[cfg(test)] 后 Windows
    // 服务端生产构建自动剔除，避免携带未用到的 discovery socket 路径。
    #[cfg(test)]
    pub(crate) async fn discover_devices(
        &self,
        low: Option<u32>,
        high: Option<u32>,
        window: Duration,
    ) -> CollectorResult<Vec<frames::IAmInfo>> {
        let socket = UdpSocket::bind("0.0.0.0:0")
            .await
            .map_err(|e| CollectorError::IoError(format!("BACnet discovery bind: {}", e)))?;
        socket
            .set_broadcast(true)
            .map_err(|e| CollectorError::IoError(format!("BACnet set_broadcast: {}", e)))?;

        let apdu = frames::build_who_is_apdu(low, high);
        // BVLC Original-Broadcast-NPDU(0x0b) + NPDU(0x01,0x00) + APDU
        let npdu = Self::build_npdu_header();
        let mut packet = vec![0x81, 0x0b];
        packet.extend_from_slice(&((npdu.len() + apdu.len() + 4) as u16).to_be_bytes());
        packet.extend_from_slice(&npdu);
        packet.extend_from_slice(&apdu);
        socket
            .send_to(&packet, "255.255.255.255:47808")
            .await
            .map_err(|e| CollectorError::IoError(format!("BACnet Who-Is broadcast: {}", e)))?;

        let deadline = tokio::time::Instant::now() + window;
        let mut buf = vec![0u8; 1500];
        let mut found: Vec<frames::IAmInfo> = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, socket.recv_from(&mut buf)).await {
                Err(_) => break,
                Ok(Err(_)) => break,
                Ok(Ok((recv, _from))) => {
                    if recv < 6 || buf[0] != 0x81 || buf[1] == 0x00 {
                        continue;
                    }
                    if let Ok(apdu) = Self::skip_npdu(&buf[4..recv]) {
                        if let Ok(info) = frames::parse_i_am_apdu(apdu) {
                            // X4：去重逻辑抽到 frames::dedup_i_am（纯函数、可单测），
                            // 收集阶段先全量收，返回前统一去重——行为与原内联版一致。
                            found.push(info);
                        }
                    }
                }
            }
        }
        Ok(frames::dedup_i_am(found))
    }

    /// 决策表 #21：对指定监视对象订阅 COV（SubscribeCOV，service 27）。
    ///
    /// `issue_confirmed` 为 true 时要求设备回 SimpleACK；仅在收到匹配 invoke id
    /// 且服务号为 SubscribeCOV 的 ACK 时返回 Ok，否则判失败。
    ///
    /// 生产路径不再调用本方法——COV 订阅由后台监听任务在独立 socket 上完成
    /// （见 `cov_listener`）。本方法保留给端到端测试，直接验证 ACK 归位契约。
    #[cfg(test)]
    pub(crate) async fn subscribe_cov(
        &self,
        object_type: u16,
        instance: u32,
        subscriber_process_id: u32,
        issue_confirmed: bool,
        lifetime_secs: Option<u32>,
    ) -> CollectorResult<()> {
        let socket = self.ensure_connected().await?;

        let mut invoke_guard = self.invoke_id.lock().await;
        let invoke_id = *invoke_guard;
        *invoke_guard = invoke_id.wrapping_add(1);
        if *invoke_guard == 0 {
            *invoke_guard = 1;
        }
        drop(invoke_guard);

        let apdu = frames::build_subscribe_cov_apdu(
            object_type,
            instance,
            invoke_id,
            subscriber_process_id,
            issue_confirmed,
            lifetime_secs,
        );
        let packet = frames::wrap_bvlc_npdu(&apdu);
        drain_socket(&socket);
        socket
            .send(&packet)
            .await
            .map_err(|e| CollectorError::IoError(format!("BACnet SubscribeCOV send: {}", e)))?;

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
            let apdu = Self::skip_npdu(&buf[4..recv])?;
            if frames::apdu_invoke_id(apdu) != Some(invoke_id) {
                continue; // 丢弃错配响应
            }
            return frames::parse_subscribe_cov_ack(apdu, invoke_id);
        }
    }
}

/// 仅 MS/TP 链路需要复用 BACnet/IP 的取值解析（`bacnet-mstp` 依赖 `bacnet`）。
///
/// 只编 `bacnet`（不含 `bacnet-mstp`）时无调用方，re-export 会触发
/// `unused_imports` 告警（`RUSTFLAGS=-D warnings` 下即构建失败）。
#[cfg(feature = "bacnet-mstp")]
pub(crate) use frames::parse_value_from_apdu;

/// 非阻塞清空 UDP 接收缓冲区中的残留数据报。
///
/// BACnet/IP 无连接，上一次请求超时后对端的迟到回包会残留在缓冲区；发送新请求前
/// 清理，避免本次 recv 消费到陈旧响应。最多清 16 个，防止异常情况下死循环。
fn drain_socket(socket: &UdpSocket) {
    let mut buf = [0u8; 1500];
    for _ in 0..16 {
        if socket.try_recv(&mut buf).is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests;
