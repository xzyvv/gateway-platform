//! DLT645 采集器（GB/T 19882-63，纯 Rust 自研）
//!
//! 帧格式：68 ADDR(6) 68 CTRL LEN DATA CS 16
//! - 地址字节序与数据区是否 +0x33 由 [`Dlt645Version`] 变体决定
//!   （2007 默认：低字节在前 + 数据 +0x33；1997 国标：高字节在前 + 明文）
//! - 校验和：从第一个 68 到 CS 前所有字节之和 mod 256
//!
//! 模块结构：
//! - frames.rs: 帧构建 / 解析 / 解码函数
//! - tests.rs: 单元测试

use std::sync::Arc;

pub use frames::Dlt645Version;

use super::{convert_value, parse_bcd_address, Collector, CollectorError, CollectorResult};
use crate::config::PointConfig;
use crate::engine::model::DataPoint;
use crate::south::bus::BusManager;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

pub(crate) mod frames;
use crate::south::protocol::common::util::encode_bcd_value;
use frames::{
    build_dlt645_frame, build_dlt645_write_address_data, build_dlt645_write_password_data,
    decode_dlt_value, encode_dlt_value, parse_data_id, parse_dlt645_response,
};

pub(crate) use frames::extract_dlt645_frame;

#[cfg(test)]
mod tests;

/// DLT645 采集器
pub struct Dlt645Collector {
    bus: Arc<BusManager>,
    /// 协议版本（1997 / 2007），决定帧构建时地址字节序与数据 +0x33 行为。
    version: Dlt645Version,
}

impl Dlt645Collector {
    pub fn new(_bus_name: &str, bus: Arc<BusManager>) -> Self {
        // 默认 2007；带版本时调用 `with_version` 替换。
        Self {
            bus,
            version: Dlt645Version::V2007,
        }
    }

    /// 构造时指定协议版本。
    pub fn with_version(_bus_name: &str, bus: Arc<BusManager>, version: Dlt645Version) -> Self {
        Self { bus, version }
    }

    /// 按当前变体构建帧（地址字节序见 `Dlt645Version::addr_be_on_wire`）。
    ///
    /// 数据区的 +0x33 由各调用点按 `data_cipher()` 预处理：
    /// 写通信地址命令的新地址字段必须保持明文，不能在此统一加密。
    fn build_frame(&self, addr: &[u8], ctrl: u8, data: &[u8]) -> Vec<u8> {
        build_dlt645_frame(addr, ctrl, data, self.version)
    }

    /// 读取单个数据标识
    async fn read_data_id(&self, point: &PointConfig) -> CollectorResult<DataPoint> {
        let addr_str = &point.address;
        let data_id = point
            .data_id
            .as_ref()
            .ok_or_else(|| CollectorError::InvalidConfig("dlt645 data_id required".into()))?;

        let addr_bytes = parse_bcd_address(addr_str, 6)?;
        let id_bytes = parse_data_id(data_id)?;

        let mut data = id_bytes;
        data.reverse();
        // 数据区 +0x33 由变体决定（2007 与 1997 厂家 cipher 变体加密，1997 国标不加密）
        if self.version.data_cipher() {
            for b in data.iter_mut() {
                *b = b.wrapping_add(0x33);
            }
        }

        let request = self.build_frame(&addr_bytes, 0x01, &data);
        let response = self.bus.transaction(request, None).await?;

        let resp_data = parse_dlt645_response(&response, &addr_bytes, self.version)?;
        if resp_data.len() < 4 {
            return Err(CollectorError::InvalidFrame(
                "response data too short".into(),
            ));
        }
        let value_bytes = &resp_data[4..];
        let (raw_value, raw_type, quality) = decode_dlt_value(point, value_bytes);
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

/// BCD 表地址格式化为 12 位十进制串（线上低字节在前时先反转）。
fn format_meter_address(mut bytes: Vec<u8>, addr_be_on_wire: bool) -> String {
    if !addr_be_on_wire {
        bytes.reverse();
    }
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

/// 解析「读通信地址」回帧并生成诊断文本（无总线依赖，便于单测）。
///
/// 广播应答携带的是表地址，不能用配置地址做校验——这里用回帧自身地址通过
/// 校验，再核对控制码与数据长度。
fn describe_read_address_response(
    bus_name: &str,
    version: Dlt645Version,
    response: &[u8],
) -> String {
    if response.len() < 9 {
        return format!(
            "dlt645 总线 {bus_name}：回帧过短（{} 字节），无法解析",
            response.len()
        );
    }
    let mut expected = response[1..7].to_vec();
    if version.addr_be_on_wire() {
        expected.reverse();
    }
    let data = match parse_dlt645_response(response, &expected, version) {
        Ok(d) => d,
        Err(e) => return format!("dlt645 总线 {bus_name}：回帧校验失败（{e}）"),
    };
    let ctrl = response[8];
    if ctrl != 0x93 {
        return format!("dlt645 总线 {bus_name}：回帧控制码 0x{ctrl:02X} 非预期的 0x93");
    }
    if data.len() < 6 {
        return format!(
            "dlt645 总线 {bus_name}：读通信地址应答数据不足 6 字节（实收 {}）",
            data.len()
        );
    }
    let addr = format_meter_address(data[..6].to_vec(), version.addr_be_on_wire());
    format!("dlt645 总线 {bus_name}：读通信地址成功，表地址={addr}")
}

impl Collector for Dlt645Collector {
    fn name(&self) -> &'static str {
        "dlt645"
    }

    fn bus_name(&self) -> &str {
        self.bus.name()
    }

    fn diag_supported(&self) -> bool {
        true
    }

    /// B3：广播「读通信地址」（CTRL=0x13），用表地址回帧验证链路。
    ///
    /// 选这个命令而不是读某个数据标识：它不依赖表号配置、也不依赖具体 DI 是否被
    /// 厂家实现，任何一台在线表都应答，且能顺便报出实际表地址。
    fn diag<'a>(&'a self) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
        Box::pin(async move {
            let broadcast = [0xAAu8; 6];
            let request = self.build_frame(&broadcast, 0x13, &[]);
            let response = match self.bus.transaction(request, None).await {
                Ok(r) => r,
                Err(e) => {
                    return format!(
                        "dlt645 总线 {}：广播读通信地址无响应（{e}）",
                        self.bus.name()
                    )
                }
            };

            describe_read_address_response(self.bus.name(), self.version, &response)
        })
    }

    /// 修复(审查#B2)：DLT645 逐点串行读取（每点一个总线事务），组级超时必须按
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

    fn read_batch<'a>(
        &'a self,
        points: &'a [PointConfig],
    ) -> Pin<Box<dyn Future<Output = Vec<CollectorResult<DataPoint>>> + Send + 'a>> {
        Box::pin(async move {
            // Bug 223: 原实现把多个 DI 拼进一个 0x01 读帧——DLT645-2007 标准读命令
            // 仅支持单 DI，多 DI 拼接属非标扩展，不支持的表会报错或只应答首个 DI；
            // 且响应切分依赖在数据区搜索下一 DI 字节模式（find_subsequence），
            // 数据值恰好包含该模式时会误切分、读错值。稳定性优先：
            // 改为逐点标准单 DI 读取（每点一个事务），各点位地址天然独立。
            let mut results: Vec<CollectorResult<DataPoint>> = Vec::with_capacity(points.len());
            for p in points {
                results.push(self.read(p).await);
            }
            results
        })
    }

    /// 写入单个点位（控制码由 `point.function_code` 决定）
    ///
    /// 三种命令路由（DLT645-2007）：
    /// - `function_code = 0x04`（或 None）：写数据
    ///   请求：68 ADDR 68 04 LEN [DI(4)+0x33×4] [VALUE(n)+0x33×n] CS 16
    ///   响应：68 ADDR 68 84 LEN [DI(4)+0x33×4] [STATUS(1)+0x33] CS 16
    /// - `function_code = 0x0C`：写通信地址（新地址由 value 整数化为 6 字节 BCD）
    ///   请求：68 ADDR 68 0C LEN [新地址 6 字节明文] [旧密码(4)+0x33] [新密码(4)+0x33] CS 16
    ///   响应：68 ADDR 68 8C LEN [STATUS(1)+0x33] CS 16
    /// - `function_code = 0x0F`：写密码（新密码由 value 整数化）
    ///   请求：68 ADDR 68 0F LEN [旧密码(4)+0x33] [新密码(4)+0x33] CS 16
    ///   响应：68 ADDR 68 8F LEN [STATUS(1)+0x33] CS 16
    ///
    /// `address` 字段含义统一为目标设备地址（方案 B 约定）。
    /// 密码字段默认 0x00000000（无密码场景）。
    fn write<'a>(
        &'a self,
        point: &'a PointConfig,
        value: f64,
    ) -> Pin<Box<dyn Future<Output = CollectorResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let addr_str = &point.address;
            let addr_bytes = parse_bcd_address(addr_str, 6)?;

            // 路由：根据 point.function_code 决定写命令
            match point.function_code {
                Some(0x0C) => self.do_write_address(&addr_bytes, value).await,
                Some(0x0F) => self.do_write_password(&addr_bytes, value).await,
                _ => self.do_write_data(point, &addr_bytes, value).await,
            }
        })
    }
}

impl Dlt645Collector {
    /// 写数据（控制码 0x04）
    async fn do_write_data(
        &self,
        point: &PointConfig,
        addr_bytes: &[u8],
        value: f64,
    ) -> CollectorResult<()> {
        let data_id_str = point
            .data_id
            .as_ref()
            .ok_or_else(|| CollectorError::InvalidConfig("dlt645 data_id required".into()))?;
        let id_bytes = parse_data_id(data_id_str)?;

        // DI(4 字节) → 在数据区按变体编码（cipher 变体 +0x33，1997 国标不加密）
        let mut data = Vec::with_capacity(4);
        let mut di_le = id_bytes.clone();
        di_le.reverse();
        if self.version.data_cipher() {
            for b in di_le.iter_mut() {
                *b = b.wrapping_add(0x33);
            }
        }
        data.extend_from_slice(&di_le);

        // 写值编码：按 point.data_type 分派，输出长度 = DI 字段的实际字节数。
        // 工业现场电能量/需量/瞬时量等不同 DI 字段长度不同（U16/I16=2 字节、
        // U32/I32/F32=4 字节、U64/I64/F64=8 字节）。
        //
        // Bug P0-B1 (fix): 旧实现硬编码 4 字节，U16/I16 类型被 resize(4, 0) 补零
        // 后写入——表端收到 [hi, lo, 0x00, 0x00] 期望 [hi, lo]，寄存器错位。
        // 改为按 data_type.register_count() * 2 推断，与 read_data_id 路径的
        // decode_dlt_value 用同一来源（point.data_type_enum().register_count()）。
        // 保留 4 字节作为最低兜底（兼容未配 data_type 的历史点位）。
        let write_value_len: usize = point
            .data_type_enum()
            .map(|d| d.register_count() as usize * 2)
            .unwrap_or(4);
        let raw_bytes = encode_dlt_value(point, value, write_value_len).ok_or_else(|| {
            CollectorError::InvalidConfig(format!(
                "dlt645 write encode failed for point {} (value={}, data_type={:?})",
                point.id, value, point.data_type
            ))
        })?;
        // 写值同样按变体处理：cipher 变体 +0x33，plain 变体原样
        if self.version.data_cipher() {
            for b in raw_bytes.iter() {
                data.push(b.wrapping_add(0x33));
            }
        } else {
            data.extend_from_slice(&raw_bytes);
        }

        // 控制码 0x04 = 写数据
        let request = self.build_frame(addr_bytes, 0x04, &data);
        let response = self.bus.transaction(request, None).await?;

        self.parse_write_status_response(&response, addr_bytes, 0x84, true)
    }

    /// 写通信地址（控制码 0x0C）
    ///
    /// `value` 为非负整数（< 1e12）按 6 字节 BCD 编码为新地址。
    /// 不允许负数 / 越界。
    async fn do_write_address(
        &self,
        addr_bytes: &[u8],
        new_addr_value: f64,
    ) -> CollectorResult<()> {
        if !new_addr_value.is_finite()
            || new_addr_value < 0.0
            || new_addr_value >= 1e12
            || new_addr_value.fract() != 0.0
        {
            return Err(CollectorError::InvalidConfig(format!(
                "dlt645 write address: new addr must be non-negative integer < 1e12, got {}",
                new_addr_value
            )));
        }
        // 6 字节 BCD = 12 位十进制数字（< 1e12 留 1 位缓冲，10^12 = 1 000 000 000 000）
        let new_addr_bytes_vec = encode_bcd_value(new_addr_value, 6).ok_or_else(|| {
            CollectorError::InvalidConfig(format!(
                "dlt645 write address: cannot encode {} as 6-byte BCD",
                new_addr_value
            ))
        })?;
        let new_addr_bytes_arr: [u8; 6] = new_addr_bytes_vec.try_into().map_err(|_| {
            CollectorError::InvalidConfig("dlt645 write address BCD length != 6".into())
        })?;
        // 0x0C 默认无密码（现场 99% 不设密码）
        let data =
            build_dlt645_write_address_data(&new_addr_bytes_arr, 0, 0, self.version.data_cipher())
                .ok_or_else(|| {
                    CollectorError::InvalidConfig("dlt645 write address data build".into())
                })?;

        let request = self.build_frame(addr_bytes, 0x0C, &data);
        let response = self.bus.transaction(request, None).await?;
        // 0x0C 响应只有 1 字节 STATUS（无 DI）
        self.parse_write_status_response(&response, addr_bytes, 0x8C, false)
    }

    /// 写密码（控制码 0x0F）
    ///
    /// `value` 为新密码（0 ~ 10^8 非负整数），旧密码默认 0。
    async fn do_write_password(
        &self,
        addr_bytes: &[u8],
        new_password_value: f64,
    ) -> CollectorResult<()> {
        if !new_password_value.is_finite()
            || new_password_value < 0.0
            || new_password_value >= 1e8
            || new_password_value.fract() != 0.0
        {
            return Err(CollectorError::InvalidConfig(format!(
                "dlt645 write password: new password must be non-negative integer < 1e8, got {}",
                new_password_value
            )));
        }
        let new_pwd = new_password_value as u32;
        let data = build_dlt645_write_password_data(0, new_pwd, self.version.data_cipher())
            .ok_or_else(|| {
                CollectorError::InvalidConfig("dlt645 write password data build".into())
            })?;

        let request = self.build_frame(addr_bytes, 0x0F, &data);
        let response = self.bus.transaction(request, None).await?;
        // 0x0F 响应只有 1 字节 STATUS（无密码回显）
        self.parse_write_status_response(&response, addr_bytes, 0x8F, false)
    }

    /// 通用写响应解析：验证响应 ctrl 与 STATUS 字节。
    ///
    /// `expect_ctrl_high` = 0x80 | ctrl
    /// `has_di_echo` = true: 响应前 4 字节为 DI 回显（写数据 0x84），
    ///                  false: 响应只有 1 字节 STATUS（写通信地址 0x8C、写密码 0x8F）
    fn parse_write_status_response(
        &self,
        response: &[u8],
        addr_bytes: &[u8],
        expect_ctrl: u8,
        has_di_echo: bool,
    ) -> CollectorResult<()> {
        let resp_data = parse_dlt645_response(response, addr_bytes, self.version)?;
        let (status, min_len) = if has_di_echo {
            if resp_data.len() < 5 {
                return Err(CollectorError::InvalidFrame(
                    "dlt645 write response too short (expected DI+STATUS)".into(),
                ));
            }
            // 验证响应控制域为期望值
            let resp_ctrl = response[8];
            if resp_ctrl != (expect_ctrl | 0x80) {
                return Err(CollectorError::InvalidFrame(format!(
                    "dlt645 write response ctrl mismatch: expected 0x{:02X}, got 0x{:02X}",
                    expect_ctrl | 0x80,
                    resp_ctrl
                )));
            }
            (resp_data[4], 5)
        } else {
            if resp_data.is_empty() {
                return Err(CollectorError::InvalidFrame(
                    "dlt645 write response too short (expected STATUS)".into(),
                ));
            }
            // 验证响应控制域（response[8] 是原始字节，期望 = expect_ctrl | 0x80）
            let resp_ctrl = response[8];
            if resp_ctrl != (expect_ctrl | 0x80) {
                return Err(CollectorError::InvalidFrame(format!(
                    "dlt645 write response ctrl mismatch: expected 0x{:02X}, got 0x{:02X}",
                    expect_ctrl | 0x80,
                    resp_ctrl
                )));
            }
            (resp_data[0], 1)
        };
        let _ = min_len; // 已通过 min_len 校验
        if status != 0x00 {
            return Err(CollectorError::InvalidFrame(format!(
                "dlt645 write rejected: status=0x{:02X}",
                status
            )));
        }
        Ok(())
    }
}
