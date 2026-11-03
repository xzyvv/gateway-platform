//! 协议骨架模板（参考用，不注册到 ProtocolRegistry）
//!
//! 复制本目录为你的协议名（如 `your_protocol/`），然后：
//! 1. 重命名 `SkeletonCollector` → `YourProtocolCollector`
//! 2. 实现 `read` / `extract_frame`
//! 3. 在 `registry.rs` 的 `PROTOCOL_ENTRIES` 追加注册项
//!
//! 本文件刻意保持最简，展示 Collector trait 的最小可用实现。

// 有意保留：本模块为「复制即用」的协议模板，不注册进 ProtocolRegistry，
// 部分示例方法/字段不会被生产路径引用，故整文件豁免 dead_code；
// 复制为新协议并注册后，真实实现自然消除该豁免。
#![allow(dead_code)]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::config::PointConfig;
use crate::engine::model::DataPoint;
use crate::south::bus::{BusManager, FrameBuffer};
use crate::south::protocol::{Collector, CollectorError, CollectorResult};

/// 骨架采集器：串行 RS485 协议示例
pub struct SkeletonCollector {
    port_name: String,
    bus: Arc<BusManager>,
}

impl SkeletonCollector {
    pub fn new(port_name: &str, bus: Arc<BusManager>) -> Self {
        Self {
            port_name: port_name.to_string(),
            bus,
        }
    }
}

impl Collector for SkeletonCollector {
    fn name(&self) -> &'static str {
        "skeleton"
    }

    fn bus_name(&self) -> &str {
        &self.port_name
    }

    /// 读取单个点位
    ///
    /// 必须用手写 Pin<Box<dyn Future>>，不能用 async-trait（保持对象安全）。
    fn read<'a>(
        &'a self,
        point: &'a PointConfig,
    ) -> Pin<Box<dyn Future<Output = CollectorResult<DataPoint>> + Send + 'a>> {
        Box::pin(async move {
            // 1. 构造请求帧
            let req = build_request(point);

            // 2. 发送并等待响应（transaction 返回 Result<Vec<u8>, CollectorError>）
            let resp = self.bus.transaction(req, None).await?;

            // 3. 解析响应
            let value = parse_response(&resp)
                .ok_or_else(|| CollectorError::InvalidFrame("parse failed".into()))?;

            // 4. 构造数据点
            let device_id = point.device_id.as_deref().unwrap_or(&point.bus);
            Ok(DataPoint::good(&point.id, device_id, value, value, "u16"))
        })
    }
}

/// 构造请求帧（按你的协议格式实现）
fn build_request(point: &PointConfig) -> Vec<u8> {
    // 示例：从站号 + 功能码 + 寄存器地址（大端）
    let slave = point.slave_id.unwrap_or(0);
    let addr = point.address.parse::<u16>().unwrap_or(0);
    vec![slave, 0x03, (addr >> 8) as u8, addr as u8]
}

/// 解析响应帧（按你的协议格式实现）
fn parse_response(resp: &[u8]) -> Option<f64> {
    if resp.len() < 2 {
        return None;
    }
    // 示例：取最后两个字节作为大端 u16
    let val = u16::from_be_bytes([resp[resp.len() - 2], resp[resp.len() - 1]]);
    Some(val as f64)
}

/// 帧提取器：从 FrameBuffer 中提取一帧完整报文
///
/// 仅串行协议需要。TCP/UDP 协议返回 None 即可。
pub fn extract_skeleton_frame(
    buf: &mut FrameBuffer,
    _expected_len: Option<u16>,
) -> Option<Vec<u8>> {
    // 示例：以 0x68 开头、0x16 结尾的帧
    let data = buf.peek();
    let start = data.iter().position(|&b| b == 0x68)?;
    let end = data[start..].iter().position(|&b| b == 0x16)?;
    let frame = data[start..=start + end].to_vec();
    buf.pop(start + end + 1);
    Some(frame)
}

#[cfg(test)]
mod tests;
