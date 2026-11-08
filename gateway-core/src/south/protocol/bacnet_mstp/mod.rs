//! BACnet MS/TP 客户端采集器（串行链路层）
//!
//! BACnet 楼宇自动化协议的串行版本，基于 RS-485 UART 传输。
//! 实现数据链路层帧编解码（Preamble + Header + CRC8 + Data + CRC16），
//! 应用层复用 bacnet.rs 的 NPDU/APDU 编解码函数。
//!
//! 简化为单主模式：网关作为唯一主站，直接发送 DataExpectingReply 帧，
//! 等待从站返回 DataNotExpectingReply 响应。不实现令牌传递和主站轮询。
//!
//! 配置：
//!   - bus.port_name: 串口设备路径，如 "/dev/ttyS1" 或 "COM3"
//!   - bus.baud_rate: 波特率（常见 9600/38400/76800/115200）
//!   - bus.tcp_addr: 可选，"master=0&max_master=127"（默认 master=0）
//!   - point.slave_id: 目标 MS/TP 设备地址（0-127，默认 0）
//!   - point.address: "object_type:instance"，如 "analog-input:1"（与 BACnet/IP 相同）
//!   - point.data_type: 可选

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::config::PointConfig;
use crate::engine::model::DataPoint;
use crate::south::bus::BusManager;
use crate::south::protocol::bacnet::BacnetCollector;
use crate::south::protocol::{Collector, CollectorError, CollectorResult};

use frames::*;

/// MS/TP 前导码（仅测试通过 `super::*` 引用，生产帧编解码在 frames.rs）
#[cfg(test)]
const PREAMBLE: [u8; 2] = [0x55, 0xFF];
/// MS/TP Token 帧类型（仅测试引用；生产侧收到即视为控制帧）
#[cfg(test)]
const FRAME_TOKEN: u8 = 0x00;

// ===== MS/TP 帧类型（ASHRAE 135 表 9-1，不可随意改）=====
// 表 9-1 全表：0x00 Token / 0x01 Poll For Master / 0x02 Reply To Poll For
// Master / 0x03 Test Request / 0x04 Test Response / 0x05 Data Expecting
// Reply / 0x06 Data Not Expecting Reply / 0x07 Reply Postponed。
// 曾把 DataExpectingReply 写成 0x00（实为 Token）、DataNotExpectingReply 写成
// 0x01（实为 Poll For Master）：网关发出的「请求数据」成了 Token 帧，从站不会
// 应答，现场表现就是「总线通、一帧数据都没有」。
const FRAME_DATA_EXPECTING_REPLY: u8 = 0x05;
const FRAME_DATA_NOT_EXPECTING_REPLY: u8 = 0x06;
const FRAME_REPLY_POSTPONED: u8 = 0x07;

/// 最大数据长度
const MAX_DATA_LEN: usize = 512;

// ===== CRC 函数 =====

/// MS/TP 头 CRC-8（多项式 0x99，初始值 0xFF）
mod frames;
pub use frames::*;

pub struct BacnetMstpCollector {
    bus: Arc<BusManager>,
    bus_name: String,
    master_addr: u8,
    invoke_id: Mutex<u8>,
    /// 1.2: 注入的指标实例
    metrics: Arc<crate::engine::metrics::Metrics>,
}

impl BacnetMstpCollector {
    pub fn new(
        bus_name: &str,
        bus: Arc<BusManager>,
        master_addr: u8,
        metrics: Arc<crate::engine::metrics::Metrics>,
    ) -> Self {
        Self {
            bus,
            bus_name: bus_name.to_string(),
            master_addr,
            invoke_id: Mutex::new(1),
            metrics,
        }
    }

    /// 从配置字符串解析 master 地址
    /// "master=0&max_master=127" → 0
    pub fn parse_master_addr(config: &str) -> u8 {
        for pair in config.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                if k.trim().eq_ignore_ascii_case("master") {
                    if let Ok(n) = v.trim().parse() {
                        return n;
                    }
                }
            }
        }
        0
    }
}

impl Collector for BacnetMstpCollector {
    fn name(&self) -> &'static str {
        "bacnet_mstp"
    }

    fn bus_name(&self) -> &str {
        &self.bus_name
    }

    fn diag_supported(&self) -> bool {
        true
    }

    /// 链路诊断：向 MS/TP 广播地址 0xFF 发 Who-Is，取第一条 I-Am 应答。
    ///
    /// 本协议按单主模式实现（不参与令牌轮询），因此广播帧之后只等待一帧响应，
    /// 只用于证明「总线上至少有一台设备在应答」；**不能**据此枚举全网设备。
    /// 无应答时要如实区分「链路无设备 / 波特率不匹配 / 总线未接」，绝不伪造成功。
    fn diag<'a>(&'a self) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
        Box::pin(async move {
            let bus_name = &self.bus_name;
            let apdu = super::bacnet::frames::build_who_is_apdu(None, None);
            let npdu_header = BacnetCollector::build_npdu_header();
            let mut payload = Vec::with_capacity(npdu_header.len() + apdu.len());
            payload.extend_from_slice(&npdu_header);
            payload.extend_from_slice(&apdu);
            if payload.len() > MAX_DATA_LEN {
                return format!(
                    "BACnet MS/TP {bus_name}：Who-Is 报文超长（{} > {}），未发送",
                    payload.len(),
                    MAX_DATA_LEN
                );
            }
            let frame =
                build_mstp_frame(FRAME_DATA_EXPECTING_REPLY, 0xFF, self.master_addr, &payload);

            let response =
                match tokio::time::timeout(self.bus.io_timeout(), self.bus.transaction(frame, None))
                    .await
                {
                    Ok(Ok(r)) => r,
                    // 超时与「总线报错」在现场都表现为收不到 I-Am：统一按无应答如实上报
                    Ok(Err(_)) | Err(_) => {
                        return format!(
                            "BACnet MS/TP {bus_name}：Who-Is 无应答（链路可能无设备、波特率不匹配或总线未接）"
                        )
                    }
                };

            let (_frame_type, _dest, _src, resp_data) =
                match parse_mstp_frame(&response, &self.metrics) {
                    Ok(v) => v,
                    Err(e) => return format!("BACnet MS/TP {bus_name}：收到应答但解析失败（{e}）"),
                };
            let apdu = match BacnetCollector::skip_npdu(&resp_data) {
                Ok(a) => a,
                Err(e) => return format!("BACnet MS/TP {bus_name}：收到应答但解析失败（{e}）"),
            };
            match super::bacnet::frames::parse_i_am_apdu(apdu) {
                Ok(info) => format!(
                    "BACnet MS/TP {bus_name}：收到 I-Am，设备实例 {}（厂商 {}）",
                    info.device_instance, info.vendor_id
                ),
                Err(e) => format!("BACnet MS/TP {bus_name}：收到应答但解析失败（{e}）"),
            }
        })
    }

    fn read<'a>(
        &'a self,
        point: &'a PointConfig,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = CollectorResult<DataPoint>> + Send + 'a>>
    {
        Box::pin(async move {
            // 1. 解析 BACnet 对象地址
            let (obj_type, instance) = BacnetCollector::parse_object_address(&point.address)?;

            // 2. 获取目标设备地址
            let dest_addr = point.slave_id.unwrap_or(0);
            if dest_addr > 127 {
                return Err(CollectorError::InvalidConfig(format!(
                    "MS/TP device address {} exceeds 127",
                    dest_addr
                )));
            }
            // master_addr（本机地址）也必须 <= 127
            if self.master_addr > 127 {
                return Err(CollectorError::InvalidConfig(format!(
                    "MS/TP master address {} exceeds 127",
                    self.master_addr
                )));
            }

            // 3. 获取 invoke ID
            let invoke_id = {
                let mut guard = self.invoke_id.lock().await;
                let id = *guard;
                *guard = id.wrapping_add(1);
                if *guard == 0 {
                    *guard = 1;
                }
                id
            };

            // 4. 构建 NPDU + APDU
            let apdu = BacnetCollector::build_read_property_apdu(obj_type, instance, invoke_id);
            let npdu_header = BacnetCollector::build_npdu_header();
            let mut npdu_apdu = Vec::with_capacity(npdu_header.len() + apdu.len());
            npdu_apdu.extend_from_slice(&npdu_header);
            npdu_apdu.extend_from_slice(&apdu);

            // 5. 构建 MS/TP DataExpectingReply 帧
            // 防御性检查：MS/TP 数据长度不得超过 MAX_DATA_LEN (512)
            if npdu_apdu.len() > MAX_DATA_LEN {
                return Err(CollectorError::InvalidFrame(format!(
                    "MS/TP NPDU+APDU too large: {} > {}",
                    npdu_apdu.len(),
                    MAX_DATA_LEN
                )));
            }
            let frame = build_mstp_frame(
                FRAME_DATA_EXPECTING_REPLY,
                dest_addr,
                self.master_addr,
                &npdu_apdu,
            );

            // 6. 发送并接收响应
            let response =
                tokio::time::timeout(self.bus.io_timeout(), self.bus.transaction(frame, None))
                    .await
                    .map_err(|_| CollectorError::NoReply)??;

            // 7. 解析 MS/TP 响应帧
            let (resp_type, _resp_dest, _resp_src, resp_data) =
                parse_mstp_frame(&response, &self.metrics)?;

            // 只接受数据帧：Data Expecting Reply / Data Not Expecting Reply /
            // Reply Postponed。Token / Poll For Master / Test Request /
            // Test Response 都是链路层控制帧，说明从站没给出数据，按无应答处理。
            if !matches!(
                resp_type,
                FRAME_DATA_EXPECTING_REPLY | FRAME_DATA_NOT_EXPECTING_REPLY | FRAME_REPLY_POSTPONED
            ) {
                return Err(CollectorError::NoReply);
            }

            if resp_data.is_empty() {
                return Err(CollectorError::InvalidFrame(
                    "MS/TP response has no data".into(),
                ));
            }

            // 8. 跳过 NPDU 头，提取 APDU
            let apdu_data = BacnetCollector::skip_npdu(&resp_data)?;

            // 9. 解析 APDU 值
            let (value, quality) = super::bacnet::parse_value_from_apdu(apdu_data, invoke_id)?;

            Ok(DataPoint {
                point_id: point.id.clone(),
                device_id: point.device_id.clone().unwrap_or_default(),
                value,
                raw_value: value,
                raw_type: "bacnet_mstp".into(),
                quality,
                quality_reason: String::new(),
                timestamp: chrono::Utc::now().timestamp_millis(),
                source_ts: 0,
                stale_count: 0,
                value_text: None,
            })
        })
    }
}

#[cfg(test)]
mod tests;
