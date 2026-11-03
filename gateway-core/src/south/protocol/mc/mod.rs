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
//! - tests/: 单元测试（按主题拆分为多个子模块）

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::Mutex;

use crate::config::PointConfig;
use crate::engine::model::{DataPoint, ModbusDataType, Quality};
use crate::south::protocol::{
    clone_collector_error, convert_value, Collector, CollectorError, CollectorResult,
};

mod frames;
use frames::{
    build_read_request_at_station, build_request_1e, build_write_bit_request,
    build_write_words_request, check_end_code, decode_bit, decode_words, encode_words,
    parse_address, plan_batches, read_response_1e, read_response_3e, read_response_4e, McDevice,
    McFrameFormat, FC_BATCH_BIT_READ, FC_BATCH_BIT_WRITE, FC_BATCH_WORD_READ, FC_BATCH_WORD_WRITE,
    MAX_BITS_PER_FRAME, MAX_UNITS_PER_FRAME_1E, MAX_WORDS_PER_FRAME,
};

#[cfg(test)]
mod tests;

/// MC 监视定时器（单位 250ms，0x0010 ≈ 4s）
const MC_TIMER: u16 = 0x0010;

/// 批量读合并允许的地址空洞（字元件，单位：字）
const MC_MERGE_GAP_WORDS: u16 = 8;
/// 批量读合并允许的地址空洞（位元件，单位：位）
const MC_MERGE_GAP_BITS: u16 = 16;

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
    /// 1E 帧响应上下文（仅 1E 帧格式使用）
    e1: Option<E1Ctx>,
}

/// 由点位配置与原始值构造 DataPoint（单点 read 与批量 read_batch 共用）。
fn build_datapoint(point: &PointConfig, raw_value: f64) -> DataPoint {
    let value = convert_value(point, raw_value);
    DataPoint {
        point_id: point.id.clone(),
        device_id: point.device_id.clone().unwrap_or_default(),
        value,
        raw_value,
        raw_type: "mc".into(),
        quality: Quality::Good,
        quality_reason: String::new(),
        timestamp: DataPoint::now(),
        source_ts: 0,
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
        }
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
                    e1: None,
                })
            }
            McFrameFormat::E1 => Ok(McTxn {
                bytes: build_request_1e(FC_BATCH_WORD_WRITE, device, points, MC_TIMER, words)?,
                serial: 0,
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
                e1: None,
            }),
            McFrameFormat::E4 => {
                let serial = self.next_serial();
                Ok(McTxn {
                    bytes: build_write_bit_request(device, on, MC_TIMER, McFrameFormat::E4, serial),
                    serial,
                    e1: None,
                })
            }
            McFrameFormat::E1 => {
                // 位写数据区：1 字节，0x10 = ON，0x00 = OFF
                let payload = [if on { 0x10u8 } else { 0x00u8 }];
                Ok(McTxn {
                    bytes: build_request_1e(FC_BATCH_BIT_WRITE, device, 1, MC_TIMER, &payload)?,
                    serial: 0,
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
                tokio::time::timeout(self.io_timeout, read_response_3e(stream)).await
            }
            McFrameFormat::E4 => {
                tokio::time::timeout(self.io_timeout, read_response_4e(stream, txn.serial)).await
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
            decode_words(&data, ModbusDataType::U16)
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
            match decode_words(&data, ModbusDataType::U16) {
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
            let dt = point.data_type_enum().unwrap_or(ModbusDataType::U16);
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
            let n = points.len();
            if n == 0 {
                return Vec::new();
            }

            // 点位预解析结果（下标保持为输入顺序，最终按 idx 回填）
            #[derive(Clone)]
            struct Item {
                idx: usize,
                head: u32,
                size: u16,
                is_bit: bool,
                dt: ModbusDataType,
            }

            let mut slots: Vec<Option<CollectorResult<DataPoint>>> = (0..n).map(|_| None).collect();
            // 按（软元件码, 是否位元件）分组：同一组的字元件即使 data_type 不同也可共享一帧
            let mut groups: BTreeMap<(u8, bool), Vec<Item>> = BTreeMap::new();

            for (idx, p) in points.iter().enumerate() {
                let device = match parse_address(&p.address) {
                    Ok(d) => d,
                    Err(e) => {
                        slots[idx] = Some(Err(e));
                        continue;
                    }
                };
                let dt = p.data_type_enum().unwrap_or(ModbusDataType::U16);
                let size = if device.is_bit {
                    1
                } else {
                    dt.register_count()
                };
                groups
                    .entry((device.code, device.is_bit))
                    .or_default()
                    .push(Item {
                        idx,
                        head: device.head,
                        size,
                        is_bit: device.is_bit,
                        dt,
                    });
            }

            // 阶段一：预构建全部批量事务。建帧失败（如 1E 帧下 A 系列不可寻址软元件）就地
            // 逐点回填同因错误，不建立连接、不发请求。
            struct Pending {
                txn: McTxn,
                start: u32,
                members: Vec<Item>,
            }
            let mut pending: Vec<Pending> = Vec::new();

            for ((code, is_bit), mut items) in groups {
                items.sort_by_key(|it| it.head);
                let keys: Vec<(u32, u16)> = items.iter().map(|it| (it.head, it.size)).collect();
                let (max_units, gap) = if self.format == McFrameFormat::E1 {
                    (
                        MAX_UNITS_PER_FRAME_1E,
                        if is_bit {
                            MC_MERGE_GAP_BITS
                        } else {
                            MC_MERGE_GAP_WORDS
                        },
                    )
                } else if is_bit {
                    (MAX_BITS_PER_FRAME, MC_MERGE_GAP_BITS)
                } else {
                    (MAX_WORDS_PER_FRAME, MC_MERGE_GAP_WORDS)
                };

                for plan in plan_batches(&keys, max_units, gap) {
                    let members = &items[plan.range.clone()];
                    let device = McDevice {
                        code,
                        head: plan.start,
                        is_bit,
                    };
                    // 1E 帧格式对部分软元件（A 系列不可寻址）会在建帧时报错 → 该片段逐点回填同因错误
                    match self.read_txn(&device, plan.units) {
                        Ok(txn) => pending.push(Pending {
                            txn,
                            start: plan.start,
                            members: members.to_vec(),
                        }),
                        Err(e) => {
                            for it in members {
                                slots[it.idx] = Some(Err(clone_collector_error(&e)));
                            }
                        }
                    }
                }
            }

            // 阶段二：存在待发事务时才建链；无事务时直接返回，不产生无谓连接。
            // 建链失败仍复用整批同因错误语义（整批仅一次尝试，避免逐帧重复等待）。
            if !pending.is_empty() && !self.is_connected() {
                if let Err(e) = self.connect().await {
                    let msg = e.to_string();
                    for p in &pending {
                        for it in &p.members {
                            slots[it.idx] = Some(Err(CollectorError::IoError(msg.clone())));
                        }
                    }
                    return slots
                        .into_iter()
                        .map(|s| {
                            s.unwrap_or_else(|| {
                                Err(CollectorError::InvalidFrame(
                                    "mc batch internal: point not processed".into(),
                                ))
                            })
                        })
                        .collect();
                }
            }

            for p in &pending {
                match self.transact(&p.txn).await {
                    Ok((end_code, data)) => {
                        if let Err(e) = check_end_code(end_code) {
                            let msg = e.to_string();
                            for it in &p.members {
                                slots[it.idx] =
                                    Some(Err(CollectorError::InvalidFrame(msg.clone())));
                            }
                            continue;
                        }
                        for it in &p.members {
                            let offset = (it.head - p.start) as usize;
                            let raw = if it.is_bit {
                                decode_bit(&data, offset)
                            } else {
                                let b = offset * 2;
                                let e = b + it.size as usize * 2;
                                if e <= data.len() {
                                    decode_words(&data[b..e], it.dt)
                                } else {
                                    None
                                }
                            };
                            slots[it.idx] = Some(match raw {
                                Some(rv) => Ok(build_datapoint(&points[it.idx], rv)),
                                None => Err(CollectorError::InvalidFrame(format!(
                                    "mc batch decode failed for {} ({} bytes)",
                                    points[it.idx].address,
                                    data.len()
                                ))),
                            });
                        }
                    }
                    Err(e) => {
                        // 该片段整体失败：逐点回填同因错误，并保留原始错误变体
                        for it in &p.members {
                            slots[it.idx] = Some(Err(clone_collector_error(&e)));
                        }
                    }
                }
            }

            slots
                .into_iter()
                .map(|s| {
                    s.unwrap_or_else(|| {
                        Err(CollectorError::InvalidFrame(
                            "mc batch internal: point not processed".into(),
                        ))
                    })
                })
                .collect()
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
                let dt = point.data_type_enum().unwrap_or(ModbusDataType::U16);
                let words = encode_words(value, dt);
                self.write_words_txn(&device, &words)?
            };
            let (end_code, _) = self.transact(&txn).await?;
            check_end_code(end_code)
        })
    }
}
