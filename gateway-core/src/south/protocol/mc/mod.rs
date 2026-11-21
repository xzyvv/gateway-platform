//! 三菱 MC 协议采集器（Melsec Communication，QnA 3E / 4E + A 兼容 1E 二进制帧，TCP）
//!
//! 面向三菱 Q/L/iQ-R/iQ-F 系列（3E/4E）与老款 FX/A 系列（1E），以太网直连
//! （默认端口 5000/6000 视机型而定）。实现最小可用子集：批量读 + 批量写。
//!
//! 帧格式经统一变体字段 `bus.variant` 选择（决策表 #16 / #24）：
//! - 缺省 / `3e`：QnA 兼容 3E 帧
//! - `4e`：QnA 兼容 4E 帧（3E + 2 字节序列号，含回显校验）
//! - `1e`：A 兼容 1E 帧（老款 FX/A 系列，无长度字段）
//!
//! 配置：
//! - `bus.tcp_addr`: 对端地址，如 `"192.168.1.10:6000"`
//! - `bus.variant`: 帧格式 `3e`(默认) / `4e` / `1e`
//! - `point.address`: 软元件地址，如 `"D100"`（字）/ `"M10"`（位）/ `"ZR1000"`
//! - `point.data_type`: `u16`(默认字元件) / `i16` / `u32` / `i32` / `f32` / `u64` / `i64` / `f64`
//! - 位元件（X/Y/M/B 等）批量读按位打包（2 点/字节），写为单点
//!
//! 模块结构：
//! - frames.rs: 帧构建 / 解析 / 解码函数 + 协议常量
//! - batch.rs: 协议级批量读（分组合并 / 预构建事务 / 逐点写共享槽）
//! - tests/: 单元测试（按主题拆分为多个子模块）

use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::Mutex;

use crate::config::PointConfig;
use crate::engine::model::{DataPoint, PointDataType, Quality};
use crate::south::protocol::{
    convert_value, mark_write_uncertain_with, reverse_convert, Collector, CollectorError,
    CollectorResult, ResultSlots,
};

pub(crate) mod frames;
use frames::{
    build_read_request_at_station, build_request_1e, build_write_bit_request,
    build_write_words_request, check_end_code, check_write_end_code, decode_bit, decode_words,
    encode_words, parse_address, read_response_1e, read_response_3e, read_response_4e, McDevice,
    McFrameFormat, FC_BATCH_BIT_READ, FC_BATCH_BIT_WRITE, FC_BATCH_WORD_READ, FC_BATCH_WORD_WRITE,
};

mod batch;

#[cfg(test)]
mod tests;

/// MC 监视定时器（单位 250ms，0x0010 ≈ 4s）
const MC_TIMER: u16 = 0x0010;

/// 1E 响应读取所需的上下文（功能码 / 是否读 / 位或字 / 点数）
#[derive(Debug, Clone, Copy)]
struct E1Ctx {
    func: u8,
    is_read: bool,
    is_bit: bool,
    points: u16,
}

/// 单次事务描述：请求字节 + 4E 序列号 + 1E 响应上下文
struct McTxn {
    bytes: Vec<u8>,
    /// 4E 帧序列号（其它帧格式不使用）
    serial: u16,
    /// 3E/4E 请求的目标站号（ROI-26：响应必须回显同一站号）
    station: u8,
    /// 1E 帧响应上下文（仅 1E 帧格式使用）
    e1: Option<E1Ctx>,
}

/// 由点位配置与原始值构造 DataPoint（单点 read 与批量 read_batch 共用）。
fn build_datapoint(point: &PointConfig, raw_value: f64) -> DataPoint {
    let value = convert_value(point, raw_value);
    // MC 读响应不带时标：原始采集时刻即本次读成功时刻（此前填 0，北向会得到 1970）
    let ts = DataPoint::now();
    DataPoint {
        point_id: point.id.clone(),
        device_id: point.device_id.clone().unwrap_or_default(),
        value,
        raw_value,
        raw_type: "mc".into(),
        quality: Quality::Good,
        quality_reason: String::new(),
        timestamp: ts,
        source_ts: ts,
        stale_count: 0,
        value_text: None,
    }
}

/// 三菱 MC 采集器
pub struct McCollector {
    addr: String,
    stream: Mutex<Option<TcpStream>>,
    bus_name: String,
    connected: AtomicBool,
    /// 单次请求 IO 超时（源自 bus.timeout_ms，统一南向超时来源）
    io_timeout: Duration,
    /// 帧格式（3E / 4E / 1E，经统一变体字段选择）
    format: McFrameFormat,
    /// 4E 序列号自增计数器（用于请求-响应配对，防串包）
    serial: AtomicU16,
    /// G-08: 指标句柄（工厂注入；None 时写不确定只映射不计数）
    metrics: Option<std::sync::Arc<crate::engine::metrics::Metrics>>,
}

impl McCollector {
    pub fn new(
        addr: String,
        bus_name: String,
        io_timeout: Duration,
        variant: Option<String>,
    ) -> Self {
        Self {
            addr,
            stream: Mutex::new(None),
            bus_name,
            connected: AtomicBool::new(false),
            io_timeout,
            format: McFrameFormat::from_variant(variant.as_deref()),
            serial: AtomicU16::new(0),
            metrics: None,
        }
    }

    /// 注入指标句柄（生产路径：`registry::factories::create_mc`）。
    pub fn with_metrics(
        mut self,
        metrics: std::sync::Arc<crate::engine::metrics::Metrics>,
    ) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// 分配下一个 4E 序列号。
    fn next_serial(&self) -> u16 {
        self.serial.fetch_add(1, Ordering::Relaxed)
    }

    /// 构造一次批量读事务（按当前帧格式分派）。
    fn read_txn(&self, device: &McDevice, points: u16) -> CollectorResult<McTxn> {
        self.read_txn_at_station(device, points, 0)
    }

    /// 同 [`Self::read_txn`]，但可指定 3E/4E 帧的目标站号（调试面逐点探测用）。
    ///
    /// 1E 帧的 PLC 号字段按 A 兼容约定固定为 0xFF（配置里没有独立站号语义），
    /// 传入的 `station` 在该帧格式下不生效。
    fn read_txn_at_station(
        &self,
        device: &McDevice,
        points: u16,
        station: u8,
    ) -> CollectorResult<McTxn> {
        match self.format {
            McFrameFormat::E3 => Ok(McTxn {
                bytes: build_read_request_at_station(
                    device,
                    points,
                    MC_TIMER,
                    McFrameFormat::E3,
                    0,
                    station,
                ),
                serial: 0,
                station,
                e1: None,
            }),
            McFrameFormat::E4 => {
                let serial = self.next_serial();
                Ok(McTxn {
                    bytes: build_read_request_at_station(
                        device,
                        points,
                        MC_TIMER,
                        McFrameFormat::E4,
                        serial,
                        station,
                    ),
                    serial,
                    station,
                    e1: None,
                })
            }
            McFrameFormat::E1 => {
                let func = if device.is_bit {
                    FC_BATCH_BIT_READ
                } else {
                    FC_BATCH_WORD_READ
                };
                Ok(McTxn {
                    bytes: build_request_1e(func, device, points, MC_TIMER, &[])?,
                    serial: 0,
                    station: 0,
                    e1: Some(E1Ctx {
                        func,
                        is_read: true,
                        is_bit: device.is_bit,
                        points,
                    }),
                })
            }
        }
    }

    /// 构造一次批量写字事务（按当前帧格式分派）。
    fn write_words_txn(&self, device: &McDevice, words: &[u8]) -> CollectorResult<McTxn> {
        let points = (words.len() / 2) as u16;
        match self.format {
            McFrameFormat::E3 => Ok(McTxn {
                bytes: build_write_words_request(device, words, MC_TIMER, McFrameFormat::E3, 0),
                serial: 0,
                station: 0,
                e1: None,
            }),
            McFrameFormat::E4 => {
                let serial = self.next_serial();
                Ok(McTxn {
                    bytes: build_write_words_request(
                        device,
                        words,
                        MC_TIMER,
                        McFrameFormat::E4,
                        serial,
                    ),
                    serial,
                    station: 0,
                    e1: None,
                })
            }
            McFrameFormat::E1 => Ok(McTxn {
                bytes: build_request_1e(FC_BATCH_WORD_WRITE, device, points, MC_TIMER, words)?,
                serial: 0,
                station: 0,
                e1: Some(E1Ctx {
                    func: FC_BATCH_WORD_WRITE,
                    is_read: false,
                    is_bit: false,
                    points,
                }),
            }),
        }
    }

    /// 构造一次位写事务（按当前帧格式分派）。
    fn write_bit_txn(&self, device: &McDevice, on: bool) -> CollectorResult<McTxn> {
        match self.format {
            McFrameFormat::E3 => Ok(McTxn {
                bytes: build_write_bit_request(device, on, MC_TIMER, McFrameFormat::E3, 0),
                serial: 0,
                station: 0,
                e1: None,
            }),
            McFrameFormat::E4 => {
                let serial = self.next_serial();
                Ok(McTxn {
                    bytes: build_write_bit_request(device, on, MC_TIMER, McFrameFormat::E4, serial),
                    serial,
                    station: 0,
                    e1: None,
                })
            }
            McFrameFormat::E1 => {
                // 位写数据区：1 字节，0x10 = ON，0x00 = OFF
                let payload = [if on { 0x10u8 } else { 0x00u8 }];
                Ok(McTxn {
                    bytes: build_request_1e(FC_BATCH_BIT_WRITE, device, 1, MC_TIMER, &payload)?,
                    serial: 0,
                    station: 0,
                    e1: Some(E1Ctx {
                        func: FC_BATCH_BIT_WRITE,
                        is_read: false,
                        is_bit: true,
                        points: 1,
                    }),
                })
            }
        }
    }

    /// 惰性建链：已连接则直接返回；否则建立 TCP 连接。
    ///
    /// 生产路径下没有外部 connect() 调用，故 read/write 内部自动补建链。
    async fn ensure_connected(&self) -> CollectorResult<()> {
        let mut guard = self.stream.lock().await;
        if guard.is_some() {
            self.connected.store(true, Ordering::Relaxed);
            return Ok(());
        }
        let stream = TcpStream::connect(&self.addr).await.map_err(|e| {
            CollectorError::IoError(format!("mc connect {} failed: {e}", self.addr))
        })?;
        stream.set_nodelay(true).ok();
        *guard = Some(stream);
        self.connected.store(true, Ordering::Relaxed);
        tracing::info!("mc connected to {}", self.addr);
        Ok(())
    }

    /// 发送请求并按帧格式读取响应；IO 失败时清理连接以触发下次重连。
    async fn transact(&self, txn: &McTxn) -> CollectorResult<(u16, Vec<u8>)> {
        self.ensure_connected().await?;
        let mut guard = self.stream.lock().await;
        let stream = guard
            .as_mut()
            .ok_or_else(|| CollectorError::IoError("mc not connected".into()))?;

        match tokio::time::timeout(self.io_timeout, stream.write_all(&txn.bytes)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                *guard = None;
                self.connected.store(false, Ordering::Relaxed);
                return Err(CollectorError::IoError(format!("mc send: {e}")));
            }
            Err(_) => {
                *guard = None;
                self.connected.store(false, Ordering::Relaxed);
                return Err(CollectorError::NoReply);
            }
        }

        let read = match self.format {
            McFrameFormat::E3 => {
                tokio::time::timeout(self.io_timeout, read_response_3e(stream, txn.station)).await
            }
            McFrameFormat::E4 => {
                tokio::time::timeout(
                    self.io_timeout,
                    read_response_4e(stream, txn.serial, txn.station),
                )
                .await
            }
            McFrameFormat::E1 => match txn.e1 {
                Some(c) => {
                    tokio::time::timeout(
                        self.io_timeout,
                        read_response_1e(stream, c.func, c.is_read, c.is_bit, c.points),
                    )
                    .await
                }
                None => {
                    return Err(CollectorError::IoError(
                        "mc 1E transaction missing response context".into(),
                    ))
                }
            },
        };

        match read {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => {
                *guard = None;
                self.connected.store(false, Ordering::Relaxed);
                Err(e)
            }
            Err(_) => {
                *guard = None;
                self.connected.store(false, Ordering::Relaxed);
                Err(CollectorError::NoReply)
            }
        }
    }

    /// 按「站号 + 软元件区 + 地址」单点探测一个字（寄存器扫描工具复用）。
    ///
    /// 与 `read` 走同一套建帧 / 结束码校验 / 解码逻辑（位元件返回 bit0）。
    /// 结束码非 0 不当作异常上抛，而是随 `(结束码, None)` 返回，让调用方
    /// 如实判定「该地址不存在/被拒」，而不是把它误报成链路故障。
    ///
    /// 跨 crate 可见性：`gateway-api` 的寄存器扫描需要直接调用本方法，
    /// 故为 `pub`（非 `pub(crate)`）。
    pub async fn probe_word(
        &self,
        station: u8,
        area: &str,
        addr: u32,
    ) -> CollectorResult<(u16, Option<i32>)> {
        let device = parse_address(&format!("{area}{addr}"))?;
        let txn = self.read_txn_at_station(&device, 1, station)?;
        let (end_code, data) = self.transact(&txn).await?;
        if check_end_code(end_code).is_err() {
            return Ok((end_code, None));
        }
        let value = if device.is_bit {
            decode_bit(&data, 0)
        } else {
            decode_words(&data, PointDataType::U16)
        };
        Ok((end_code, value.map(|v| v as i32)))
    }
}

impl Collector for McCollector {
    fn name(&self) -> &'static str {
        "mc"
    }

    fn bus_name(&self) -> &str {
        &self.bus_name
    }

    fn diag_supported(&self) -> bool {
        true
    }

    /// B3：建链 + 读 D0 一个字，报告实际回读值。
    ///
    /// 用 D0 作为探针：所有 MC 兼容 PLC 都有 D 软元件，且单字读最便宜。
    fn diag<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send + 'a>> {
        Box::pin(async move {
            let device = match parse_address("D0") {
                Ok(d) => d,
                Err(e) => return format!("MC {}：D0 地址解析失败（{e}）", self.bus_name),
            };
            let txn = match self.read_txn(&device, 1) {
                Ok(t) => t,
                Err(e) => return format!("MC {}：D0 读帧构造失败（{e}）", self.bus_name),
            };
            let (end_code, data) = match self.transact(&txn).await {
                Ok(v) => v,
                Err(e) => return format!("MC {}：建链/读取失败（{e}）", self.bus_name),
            };
            if let Err(e) = check_end_code(end_code) {
                return format!(
                    "MC {}：建链成功，但 D0 读取返回结束代码 0x{end_code:04X}（{e}）",
                    self.bus_name
                );
            }
            match decode_words(&data, PointDataType::U16) {
                Some(v) => format!("MC {}：建链成功，D0 回读值={v}", self.bus_name),
                None => format!(
                    "MC {}：建链成功，但 D0 应答数据无法按 U16 解码（{} 字节）",
                    self.bus_name,
                    data.len()
                ),
            }
        })
    }

    fn connect<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = CollectorResult<()>> + Send + 'a>> {
        Box::pin(async move { self.ensure_connected().await })
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

    fn read<'a>(
        &'a self,
        point: &'a PointConfig,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = CollectorResult<DataPoint>> + Send + 'a>>
    {
        Box::pin(async move {
            let device = parse_address(&point.address)?;
            let dt = point.data_type_enum().unwrap_or(PointDataType::U16);
            let points = if device.is_bit {
                1
            } else {
                dt.register_count()
            };

            let txn = self.read_txn(&device, points)?;
            let (end_code, data) = self.transact(&txn).await?;
            check_end_code(end_code)?;

            let raw_value = if device.is_bit {
                decode_bit(&data, 0)
                    .ok_or_else(|| CollectorError::InvalidFrame("mc bit data empty".into()))?
            } else {
                decode_words(&data, dt).ok_or_else(|| {
                    CollectorError::InvalidFrame(format!(
                        "mc word decode failed ({} bytes for {:?})",
                        data.len(),
                        dt
                    ))
                })?
            };

            Ok(build_datapoint(point, raw_value))
        })
    }

    /// 批量读取（决策表 #7：协议级批量读 + 超帧上限自动分片）
    ///
    /// 不再是逐点 read，而是：
    /// 1. 先按（软元件码, 字/位）分组、组内按地址升序合并为连续区间并预构建全部事务；建帧失败
    ///    （如 1E 帧下 A 系列不可寻址软元件）就地逐点回填同因错误，**不建立连接、不发请求**
    /// 2. 仅当存在待发事务时才建链（整批一次）；设备不可达时整批返回同因错误，不再逐点重复等待连接超时
    /// 3. 片段单元数超过单帧上限（3E/4E：字 MAX_WORDS_PER_FRAME / 位 MAX_BITS_PER_FRAME；
    ///    1E：MAX_UNITS_PER_FRAME_1E）时自动切分为多帧顺序下发（超限回退）
    ///
    /// 同一 TCP 连接由 transact 内的流锁串行化，故分片帧天然顺序执行，不会交叉串包。
    fn read_batch<'a>(
        &'a self,
        points: &'a [PointConfig],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Vec<CollectorResult<DataPoint>>> + Send + 'a>,
    > {
        Box::pin(async move {
            let slots: ResultSlots = std::sync::Arc::new(std::sync::Mutex::new(
                (0..points.len()).map(|_| None).collect(),
            ));
            self.read_batch_points_with_slots(points, slots).await
        })
    }

    /// 缺陷 #7：沿用 `read_batch` 的分组合并 / 预构建事务 / 惰性建链语义，每个点位
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

    fn write<'a>(
        &'a self,
        point: &'a PointConfig,
        value: f64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = CollectorResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let device = parse_address(&point.address)?;
            let txn = if device.is_bit {
                self.write_bit_txn(&device, value != 0.0)?
            } else {
                let dt = point.data_type_enum().unwrap_or(PointDataType::U16);
                // 写值与读路径对偶：北向下发的是一次侧值，需先除回变比与 scale/offset
                let words = encode_words(reverse_convert(point, value), dt);
                self.write_words_txn(&device, &words)?
            };
            let (end_code, _) = self
                .transact(&txn)
                .await
                .map_err(|e| mark_write_uncertain_with(self.metrics.as_deref(), e))?;
            check_write_end_code(end_code)
        })
    }
}
