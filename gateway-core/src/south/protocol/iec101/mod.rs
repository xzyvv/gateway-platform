//! IEC 60870-5-101 主站采集器（串行 FT1.2 非平衡链路）
//!
//! 面向电力远动 RTU/测控装置，串行（RS-232/RS-485）连接。
//! 实现最小可用子集：链路复位 + 总召唤（类型 100）+ 轮询二级数据，
//! 解析短浮点 / 归一化 / 标度化 / 单点测量值。
//!
//! 与 IEC104（TCP）共用同一套 ASDU 编解码（见
//! [`crate::south::protocol::common::asdu`]），差异仅在链路层：
//! 本协议用 FT1.2 帧（见 [`frames`]），IEC104 用 APCI。
//!
//! 配置：
//! - `bus.port_name`: 串口设备，如 `"/dev/ttyS1"`
//! - `point.address`: 信息对象地址（IOA，十进制），如 `"100"`
//! - `point.data_type`: 预留（ASDU 自带类型，暂不参与解码分派）
//!
//! 主站流程（每次刷新）：
//! 1. 首次复位远程链路（`10 40 A CS 16`，尽力而为）
//! 2. 发送总召唤变量帧（控制域 C=0x73，携带总召唤 ASDU）
//! 3. 循环轮询二级数据（`10 7B A CS 16`），解析 ASDU 直至激活终止或无更多数据
//! 4. 结果写入本地缓存，按 IOA 提供点位读取
//!
//! 模块结构：
//! - frames.rs: FT1.2 帧构建 / 解析 / 提取 + 协议常量
//! - tests.rs: 单元测试 + 内存串口桩端到端测试

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::config::PointConfig;
use crate::engine::model::DataPoint;
use crate::south::bus::BusManager;
use crate::south::protocol::common::asdu::IoaDataMap;
use crate::south::protocol::common::asdu::{is_interrogation_termination, parse_asdu};
use crate::south::protocol::{
    clone_collector_error, convert_value, Collector, CollectorError, CollectorResult,
};

mod frames;
use frames::{interrogation, parse_frame, request_class2, reset_link, Ft12Frame};

pub(crate) use frames::extract_iec101_frame;

#[cfg(test)]
mod tests;

/// FT1.2 链路地址（1 字节，非平衡链路主站/从站标识）。
///
/// 当前实现使用标准站地址 1；如需多点链路，可后续从总线配置扩展。
const LINK_ADDR: u8 = 1;
/// ASDU 公共地址（应用层站地址）
const ASDU_ADDR: u16 = 1;
/// 单次刷新最多轮询二级数据的次数（防止设备异常时无限轮询）
const MAX_POLLS: usize = 8;

/// IEC 60870-5-101 采集器
pub struct Iec101Collector {
    bus: Arc<BusManager>,
    bus_name: String,
    /// 最近一次总召唤收到的数据：IOA -> (value, quality, source_ts_millis)
    data: Arc<Mutex<IoaDataMap>>,
    /// 链路是否已复位（首次刷新时置位，避免每轮重复复位）
    initialized: AtomicBool,
    /// 帧计数位（FCB），每次主站用户数据/请求交替翻转
    fcb: AtomicBool,
}

impl Iec101Collector {
    pub fn new(bus_name: &str, bus: Arc<BusManager>) -> Self {
        Self {
            bus,
            bus_name: bus_name.to_string(),
            data: Arc::new(Mutex::new(HashMap::new())),
            initialized: AtomicBool::new(false),
            fcb: AtomicBool::new(false),
        }
    }

    /// 翻转并返回当前 FCB 值
    fn toggle_fcb(&self) -> bool {
        self.fcb.fetch_xor(true, Ordering::Relaxed)
    }

    /// 刷新一次：复位链路（首次） + 总召唤 + 轮询二级数据，结果并入缓存
    async fn refresh(&self) -> CollectorResult<()> {
        // 1. 首次复位远程链路（尽力而为：部分设备不需要显式复位，失败不阻断）
        if !self.initialized.swap(true, Ordering::Relaxed) {
            match self.bus.transaction(reset_link(LINK_ADDR), None).await {
                Ok(resp) => {
                    if let Err(e) = parse_frame(&resp) {
                        tracing::debug!("iec101 reset link response invalid (ignored): {e}");
                    }
                }
                Err(e) => tracing::debug!("iec101 reset link failed (ignored): {e}"),
            }
        }

        // 2. 总召唤
        let fcb = self.toggle_fcb();
        let mut collected: IoaDataMap = HashMap::new();
        let resp = self
            .bus
            .transaction(
                interrogation(LINK_ADDR, ASDU_ADDR, fcb, self.bus.asdu_addr_len()),
                None,
            )
            .await?;
        // 部分设备对总召唤直接回数据帧（而非先 ACK），此处一并解析避免丢数据
        if let Ok(Ft12Frame::Variable { asdu, .. }) = parse_frame(&resp) {
            if !is_interrogation_termination(&asdu) {
                parse_asdu(&asdu, &mut collected, self.bus.asdu_addr_len());
            }
        }

        // 3. 轮询二级数据直至激活终止或无更多数据
        let mut got_data = !collected.is_empty();
        for _ in 0..MAX_POLLS {
            let fcb = self.toggle_fcb();
            let resp = match self
                .bus
                .transaction(request_class2(LINK_ADDR, fcb), None)
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::debug!("iec101 class2 poll failed: {e}");
                    break;
                }
            };
            match parse_frame(&resp)? {
                Ft12Frame::Variable { asdu, .. } => {
                    if is_interrogation_termination(&asdu) {
                        break;
                    }
                    parse_asdu(&asdu, &mut collected, self.bus.asdu_addr_len());
                    got_data = true;
                }
                Ft12Frame::Ack => {
                    // 已收到数据后遇到空确认 → 本帧为数据结束；否则再试一次
                    if got_data {
                        break;
                    }
                }
                Ft12Frame::Fixed { .. } => break,
            }
        }

        if collected.is_empty() {
            return Err(CollectorError::NoReply);
        }

        let mut cache = self.data.lock().await;
        for (k, v) in collected {
            cache.insert(k, v);
        }
        Ok(())
    }

    /// 从缓存构造数据点（不含刷新）
    async fn cached_point(&self, point: &PointConfig, ioa: u32) -> CollectorResult<DataPoint> {
        let cache = self.data.lock().await;
        match cache.get(&ioa) {
            Some((val, q, ts)) => {
                let value = convert_value(point, *val);
                Ok(DataPoint {
                    point_id: point.id.clone(),
                    device_id: point.device_id.clone().unwrap_or_default(),
                    value,
                    raw_value: *val,
                    raw_type: "iec101".into(),
                    quality: *q,
                    quality_reason: String::new(),
                    timestamp: DataPoint::now(),
                    source_ts: *ts as i64,
                    stale_count: 0,
                    value_text: None,
                })
            }
            None => Err(CollectorError::NoReply),
        }
    }
}

/// 解析点位地址为 IOA（十进制）
fn parse_ioa(addr: &str) -> CollectorResult<u32> {
    addr.trim()
        .parse::<u32>()
        .map_err(|_| CollectorError::InvalidConfig(format!("invalid IOA '{addr}'")))
}

impl Collector for Iec101Collector {
    fn name(&self) -> &'static str {
        "iec101"
    }

    fn bus_name(&self) -> &str {
        &self.bus_name
    }

    fn diag_supported(&self) -> bool {
        true
    }

    /// B3：链路复位（显式验证）+ 总召唤 + 轮询，报告实际收到的 IOA 数量。
    ///
    /// 与 `refresh` 不同：复位失败在这里不再「尽力而为静默」，而是写进诊断文本；
    /// 总召唤无数据也如实报告，供现场判断是链路问题还是从站无点。
    fn diag<'a>(&'a self) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
        Box::pin(async move {
            let reset_note = match self.bus.transaction(reset_link(LINK_ADDR), None).await {
                Ok(resp) => match parse_frame(&resp) {
                    Ok(_) => {
                        // 复位成功等于已完成一次链路握手，后续 refresh 不必再复位
                        self.initialized.store(true, Ordering::Relaxed);
                        "链路复位成功".to_string()
                    }
                    Err(e) => format!("链路复位响应无效（{e}）"),
                },
                Err(e) => format!("链路复位失败（{e}）"),
            };

            match self.refresh().await {
                Ok(()) => {
                    let n = self.data.lock().await.len();
                    format!(
                        "iec101 总线 {}：{reset_note}；总召唤返回 {n} 个 IOA",
                        self.bus_name
                    )
                }
                Err(CollectorError::NoReply) => format!(
                    "iec101 总线 {}：{reset_note}；总召唤未返回任何数据（检查链路地址 / 公共地址 / 从站是否在线）",
                    self.bus_name
                ),
                Err(e) => format!(
                    "iec101 总线 {}：{reset_note}；总召唤失败（{e}）",
                    self.bus_name
                ),
            }
        })
    }

    fn read<'a>(
        &'a self,
        point: &'a PointConfig,
    ) -> Pin<Box<dyn Future<Output = CollectorResult<DataPoint>> + Send + 'a>> {
        Box::pin(async move {
            let ioa = parse_ioa(&point.address)?;
            self.refresh().await?;
            self.cached_point(point, ioa).await
        })
    }

    fn read_batch<'a>(
        &'a self,
        points: &'a [PointConfig],
    ) -> Pin<Box<dyn Future<Output = Vec<CollectorResult<DataPoint>>> + Send + 'a>> {
        Box::pin(async move {
            if points.is_empty() {
                return Vec::new();
            }
            // 一次总召唤 + 轮询服务本批全部点位，避免逐点重复召唤
            let refresh_result = self.refresh().await;
            let mut results = Vec::with_capacity(points.len());
            for p in points {
                match (parse_ioa(&p.address), &refresh_result) {
                    (Err(e), _) => results.push(Err(e)),
                    (Ok(_), Err(e)) => results.push(Err(clone_collector_error(e))),
                    (Ok(ioa), Ok(())) => results.push(self.cached_point(p, ioa).await),
                }
            }
            results
        })
    }
}
