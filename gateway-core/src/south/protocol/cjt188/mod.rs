//! CJT188 采集器（CJ/T 188-2018，纯 Rust 自研）
//!
//! 帧格式：68 ADDR(7) CTRL LEN DATA CS 16
//! - 地址：7 字节，传输时低字节在前
//! - 数据区每个字节传输前 +0x33，接收后 -0x33
//! - 校验和：从第一个 68 到 CS 前所有字节之和 mod 256
//!
//! 模块结构：
//! - frames.rs: 帧构建 / 解析 / 解码函数
//! - tests.rs: 单元测试

use std::sync::Arc;

use super::{
    convert_value, parse_bcd_address, reverse_convert, Collector, CollectorError, CollectorResult,
};
use crate::config::PointConfig;
use crate::engine::model::DataPoint;
use crate::south::bus::BusManager;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

mod frames;
use crate::south::protocol::common::util::encode_bcd_value;
use frames::{
    build_cjt188_frame, decode_cjt_value, encode_cjt_value, parse_cjt188_data_id,
    parse_cjt188_data_id_v2004, parse_cjt188_response_v, DEFAULT_SER,
};
pub(crate) use frames::{extract_cjt188_frame, Cjt188Version};

#[cfg(test)]
mod tests;

/// CJT188 采集器
///
/// 决策表 #6：`version` 决定方言（帧数据区布局 / 是否 +0x33 偏移）。
/// 默认 [`Cjt188Version::Vendor7`]，与既有实现保持完全一致。
pub struct Cjt188Collector {
    bus: Arc<BusManager>,
    /// 1.2: 注入的指标实例
    metrics: Arc<crate::engine::metrics::Metrics>,
    /// 协议方言（Vendor7 / V2004）
    version: Cjt188Version,
}

impl Cjt188Collector {
    pub fn new(
        _bus_name: &str,
        bus: Arc<BusManager>,
        metrics: Arc<crate::engine::metrics::Metrics>,
    ) -> Self {
        Self {
            bus,
            metrics,
            version: Cjt188Version::Vendor7,
        }
    }

    /// 构造时指定方言版本（由 `bus.variant()` 驱动，见 registry::create_cjt188）。
    pub fn with_version(
        _bus_name: &str,
        bus: Arc<BusManager>,
        metrics: Arc<crate::engine::metrics::Metrics>,
        version: Cjt188Version,
    ) -> Self {
        Self {
            bus,
            metrics,
            version,
        }
    }

    async fn read_data_id(&self, point: &PointConfig) -> CollectorResult<DataPoint> {
        let addr_str = &point.address;
        let data_id = point
            .data_id
            .as_ref()
            .ok_or_else(|| CollectorError::InvalidConfig("cjt188 data_id required".into()))?;

        // 地址：7 字节
        let addr_bytes = parse_bcd_address(addr_str, 7)?;

        // 构建请求数据区（CTRL=0x01 读数据），按方言分派：
        // - Vendor7（内建）：DI 7 字节，低字节在前，数据区 +0x33
        // - V2004（CJ/T 188-2004）：DI 2 字节（配置书写顺序）+ SER(1)，无偏移
        let data = match self.version {
            Cjt188Version::Vendor7 => {
                let mut di = parse_cjt188_data_id(data_id)?;
                di.reverse();
                for b in di.iter_mut() {
                    *b = b.wrapping_add(0x33);
                }
                di
            }
            Cjt188Version::V2004 => {
                let mut di = parse_cjt188_data_id_v2004(data_id)?;
                di.push(DEFAULT_SER);
                di
            }
        };

        let request = build_cjt188_frame(&addr_bytes, 0x01, &data);
        // CJT188 不做 byte_count 对照，传 None
        let response = self.bus.transaction(request, None).await?;

        let resp_data =
            parse_cjt188_response_v(&response, &addr_bytes, &self.metrics, self.version)?;
        let prefix = self.version.response_prefix_len();
        if resp_data.len() < prefix {
            return Err(CollectorError::InvalidFrame(
                "response data too short".into(),
            ));
        }
        let value_bytes = &resp_data[prefix..];
        // 按 point.data_type 分发：配置为 u16/u32/f32 等时走二进制解码（含字节序），
        // 否则（默认 / "bcd" / 未配置）回退到 BCD 路径，向后兼容现有配置。
        let (raw_value, raw_type, quality) = decode_cjt_value(point, value_bytes);
        let value = convert_value(point, raw_value);

        Ok(DataPoint {
            point_id: point.id.clone(),
            device_id: point.device_id.clone().unwrap_or_else(|| point.bus.clone()),
            value,
            raw_value,
            raw_type,
            quality,
            quality_reason: String::new(),
            timestamp: DataPoint::now(),
            source_ts: 0,
            stale_count: 0,
            value_text: None,
        })
    }
}

/// 「读通信地址」命令的 DI 数据区（按方言编码，含 +0x33）。
///
/// 标准命令为 CTRL=0x01 + DI=0xA018；Vendor7 的 7 字节 DI 以低字节在前并整体
/// +0x33，V2004 为 2 字节 DI + SER。
fn build_read_address_di(version: Cjt188Version) -> CollectorResult<Vec<u8>> {
    match version {
        Cjt188Version::Vendor7 => {
            // Vendor7 的 DI 固定 7 字节：0xA0/0x18 为高位，其余补 0
            let mut di = parse_cjt188_data_id("A0180000000000")?;
            di.reverse();
            for b in di.iter_mut() {
                *b = b.wrapping_add(0x33);
            }
            Ok(di)
        }
        Cjt188Version::V2004 => {
            let mut di = parse_cjt188_data_id_v2004("A018")?;
            di.push(DEFAULT_SER);
            Ok(di)
        }
    }
}

/// 从应答数据区解出 7 字节表地址并格式化为 14 位十进制串。
fn format_cjt_address(data: &[u8], prefix: usize) -> Option<String> {
    let bytes = data.get(prefix..prefix + 7)?;
    let mut high_first = bytes.to_vec();
    high_first.reverse();
    Some(high_first.iter().map(|b| format!("{b:02X}")).collect())
}

impl Collector for Cjt188Collector {
    fn name(&self) -> &'static str {
        "cjt188"
    }

    fn bus_name(&self) -> &str {
        self.bus.name()
    }

    fn diag_supported(&self) -> bool {
        true
    }

    /// B3：广播「读通信地址」（CTRL=0x01 / DI=0xA018），用表地址回帧验证链路。
    ///
    /// 与 DLT645 同思路：不依赖配置表号、不依赖厂家实现了哪个数据标识，
    /// 在线表都应答并能报出实际表地址。
    fn diag<'a>(&'a self) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
        Box::pin(async move {
            let di = match build_read_address_di(self.version) {
                Ok(d) => d,
                Err(e) => {
                    return format!(
                        "cjt188 总线 {}：读通信地址命令构造失败（{e}）",
                        self.bus.name()
                    )
                }
            };
            let broadcast = [0xAAu8; 7];
            let request = build_cjt188_frame(&broadcast, 0x01, &di);
            let response = match self.bus.transaction(request, None).await {
                Ok(r) => r,
                Err(e) => {
                    return format!(
                        "cjt188 总线 {}：广播读通信地址无响应（{e}）",
                        self.bus.name()
                    )
                }
            };
            if response.len() < 10 {
                return format!(
                    "cjt188 总线 {}：回帧过短（{} 字节），无法解析",
                    self.bus.name(),
                    response.len()
                );
            }
            // 应答地址即表地址，用回帧自身地址通过校验（线上低字节在前）
            let expected = response[1..8].to_vec();
            let data =
                match parse_cjt188_response_v(&response, &expected, &self.metrics, self.version) {
                    Ok(d) => d,
                    Err(e) => {
                        return format!("cjt188 总线 {}：回帧校验失败（{e}）", self.bus.name())
                    }
                };
            let ctrl = response[8];
            if ctrl != 0x81 {
                return format!(
                    "cjt188 总线 {}：回帧控制码 0x{ctrl:02X} 非预期的 0x81",
                    self.bus.name()
                );
            }
            match format_cjt_address(&data, self.version.response_prefix_len()) {
                Some(addr) => format!(
                    "cjt188 总线 {}：读通信地址成功，表地址={addr}",
                    self.bus.name()
                ),
                None => format!(
                    "cjt188 总线 {}：应答数据不足（实收 {} 字节，前缀 {}）",
                    self.bus.name(),
                    data.len(),
                    self.version.response_prefix_len()
                ),
            }
        })
    }

    /// 修复(审查#B2)：CJT188 逐点串行读取（每点一个总线事务），组级超时必须按
    /// 「点位数 × 单点总线超时」估算，否则 10+ 点位的组每个采集周期都会被 5s
    /// 硬编码超时误判为整组失败。按最坏情况 `io_timeout × 3`（首试 + 2 次重试）
    /// 估算，钳制在 [5s, 120s]。
    fn batch_timeout_budget(&self, n_points: usize) -> Duration {
        let per_point = self.bus.io_timeout().saturating_mul(3);
        let ms = (per_point.as_millis() as u64)
            .saturating_mul(n_points.max(1) as u64)
            .clamp(5_000, 120_000);
        Duration::from_millis(ms)
    }

    fn read<'a>(
        &'a self,
        point: &'a PointConfig,
    ) -> Pin<Box<dyn Future<Output = CollectorResult<DataPoint>> + Send + 'a>> {
        Box::pin(async move { self.read_data_id(point).await })
    }

    /// 决策表 #6：CJT188 写命令。
    ///
    /// `point.address` 为当前设备地址；`point.function_code` 决定命令：
    /// - `Some(0x15)` → 写通信地址（仅 V2004 标准方言支持）
    /// - 其它（含 None）/ `Some(0x04)` → 写数据（控制码 0x04）
    fn write<'a>(
        &'a self,
        point: &'a PointConfig,
        value: f64,
    ) -> Pin<Box<dyn Future<Output = CollectorResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let addr_bytes = parse_bcd_address(&point.address, 7)?;
            match point.function_code {
                Some(0x15) => self.do_write_address(&addr_bytes, value).await,
                _ => self.do_write_data(point, &addr_bytes, value).await,
            }
        })
    }

    fn read_batch<'a>(
        &'a self,
        points: &'a [PointConfig],
    ) -> Pin<Box<dyn Future<Output = Vec<CollectorResult<DataPoint>>> + Send + 'a>> {
        Box::pin(async move {
            // Bug 224: 原实现把多个 DI 拼进一个 0x01 读帧（仅 Vendor7 方言），
            // 存在两类问题：
            // 1) 多 DI 拼接是厂家方言的非标扩展，不支持的表只应答首个 DI 或直接报错，
            //    前者会让后续点位在响应里搜不到自己的 DI，整批退化并刷 warn 日志；
            // 2) 响应切分依赖在数据区搜索下一个 DI 的字节模式（find_subsequence），
            //    当某个数据值恰好包含该模式时会误切分——把错误数值以 Good 质量上报，
            //    属静默数据错误，现场极难定位。
            // 稳定性优先：统一改为逐点标准单 DI 读取（每点一个事务），两种方言行为一致，
            // 各点位地址天然独立，不再依赖任何帧内搜索。
            let mut results: Vec<CollectorResult<DataPoint>> = Vec::with_capacity(points.len());
            for p in points {
                results.push(self.read(p).await);
            }
            results
        })
    }
}

impl Cjt188Collector {
    /// 写数据（控制码 0x04，响应控制码 0x84）。
    ///
    /// 数据区：DI + [SER] + 写值。写值长度由 `point.data_type` 推导
    ///（`register_count()*2` 字节，未配置时默认 4 字节 BCD）。
    async fn do_write_data(
        &self,
        point: &PointConfig,
        addr_bytes: &[u8],
        value: f64,
    ) -> CollectorResult<()> {
        let data_id = point
            .data_id
            .as_ref()
            .ok_or_else(|| CollectorError::InvalidConfig("cjt188 data_id required".into()))?;
        let out_len = point
            .data_type_enum()
            .map(|dt| dt.register_count() as usize * 2)
            .unwrap_or(4);
        // 写数据走「一次侧值」语义：北向下发的是一次侧值，
        // 必须先反算回原始寄存器值（÷CT×PT、÷scale、-offset），
        // 否则与读路径不对称——读乘变比、写不还原。
        // 写通信地址（0x15）传的是地址整数，不能走这里，故只在写数据分支反算。
        let raw_value = reverse_convert(point, value);
        let value_bytes = encode_cjt_value(point, raw_value, out_len).ok_or_else(|| {
            CollectorError::InvalidConfig(format!(
                "cjt188 write encode failed for point {} (value={}, data_type={:?})",
                point.id, value, point.data_type
            ))
        })?;

        let mut data = match self.version {
            Cjt188Version::Vendor7 => {
                let mut di = parse_cjt188_data_id(data_id)?;
                di.reverse();
                for b in di.iter_mut() {
                    *b = b.wrapping_add(0x33);
                }
                di
            }
            Cjt188Version::V2004 => {
                let mut di = parse_cjt188_data_id_v2004(data_id)?;
                di.push(DEFAULT_SER);
                di
            }
        };
        match self.version {
            Cjt188Version::Vendor7 => {
                for b in value_bytes.iter() {
                    data.push(b.wrapping_add(0x33));
                }
            }
            Cjt188Version::V2004 => data.extend_from_slice(&value_bytes),
        }

        let request = build_cjt188_frame(addr_bytes, 0x04, &data);
        let response = self.bus.transaction(request, None).await?;
        self.parse_write_response(&response, addr_bytes, 0x04)
    }

    /// 写通信地址（控制码 0x15，响应控制码 0x95），仅 V2004 标准方言。
    ///
    /// 数据区：DI=0xA0 0x18（固定）+ SER(0) + 新地址（7 字节 BCD）。
    /// `new_addr_value` 必须为非负整数且可用 7 字节 BCD 表示（< 10^14）。
    async fn do_write_address(
        &self,
        addr_bytes: &[u8],
        new_addr_value: f64,
    ) -> CollectorResult<()> {
        if self.version != Cjt188Version::V2004 {
            return Err(CollectorError::InvalidConfig(
                "cjt188 write address requires the 2004 variant".into(),
            ));
        }
        if !new_addr_value.is_finite()
            || new_addr_value < 0.0
            || new_addr_value >= 1e14
            || new_addr_value.fract() != 0.0
        {
            return Err(CollectorError::InvalidConfig(format!(
                "cjt188 write address: new addr must be non-negative integer < 1e14, got {}",
                new_addr_value
            )));
        }
        let new_addr = encode_bcd_value(new_addr_value, 7).ok_or_else(|| {
            CollectorError::InvalidConfig(format!(
                "cjt188 write address: cannot encode {} as 7-byte BCD",
                new_addr_value
            ))
        })?;

        let mut data = Vec::with_capacity(10);
        data.push(0xA0); // DI 高字节（配置书写顺序）
        data.push(0x18); // DI 低字节
        data.push(DEFAULT_SER);
        data.extend_from_slice(&new_addr);

        let request = build_cjt188_frame(addr_bytes, 0x15, &data);
        let response = self.bus.transaction(request, None).await?;
        self.parse_write_response(&response, addr_bytes, 0x15)
    }

    /// 通用写响应解析：校验响应控制码（0x80 | request_ctrl）与 STATUS 字节。
    ///
    /// 响应数据区前缀长度由方言决定（Vendor7=DI(7)，V2004=DI(2)+SER(1)），
    /// STATUS 紧跟前缀。
    fn parse_write_response(
        &self,
        response: &[u8],
        addr_bytes: &[u8],
        request_ctrl: u8,
    ) -> CollectorResult<()> {
        let resp_data = parse_cjt188_response_v(response, addr_bytes, &self.metrics, self.version)?;
        let prefix = self.version.response_prefix_len();
        if resp_data.len() < prefix + 1 {
            return Err(CollectorError::InvalidFrame(
                "cjt188 write response too short (expected prefix+STATUS)".into(),
            ));
        }
        let expect_ctrl = request_ctrl | 0x80;
        let resp_ctrl = response[8];
        if resp_ctrl != expect_ctrl {
            return Err(CollectorError::InvalidFrame(format!(
                "cjt188 write response ctrl mismatch: expected 0x{:02X}, got 0x{:02X}",
                expect_ctrl, resp_ctrl
            )));
        }
        let status = resp_data[prefix];
        if status != 0x00 {
            return Err(CollectorError::InvalidFrame(format!(
                "cjt188 write rejected: status=0x{:02X}",
                status
            )));
        }
        Ok(())
    }
}
