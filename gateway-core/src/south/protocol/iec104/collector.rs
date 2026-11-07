//! IEC 104 采集器 `Collector` trait 实现：建链 / 断连 / 单点与批量读 / 控制写入口。
//!
//! 具体报文交互（总召唤、对时、命令下发、keepalive）分别由 interrogation.rs /
//! command.rs / keepalive.rs 的固有方法实现，trait 方法仅做转调。

use std::sync::atomic::Ordering;

use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use crate::config::PointConfig;
use crate::engine::model::{AddressReadEntry, AddressReadResult, DataPoint, Quality};
use crate::south::protocol::{Collector, CollectorError, CollectorResult};

use super::frames::{build_uframe, encode_command_object, read_frame, STARTDT_ACT};
use super::{command_type_of, Iec104Collector};

impl Iec104Collector {
    /// 由快照值构造 DataPoint（read / read_batch 共用）
    ///
    /// `source_ts` 为 CP56Time2a 时标（决策表 #17），无时标时为 0。
    fn to_datapoint(
        point: &PointConfig,
        value: f64,
        quality: Quality,
        source_ts: u64,
    ) -> DataPoint {
        DataPoint {
            point_id: point.id.clone(),
            device_id: point.device_id.clone().unwrap_or_default(),
            value,
            raw_value: value,
            raw_type: "iec104".into(),
            quality,
            quality_reason: String::new(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            source_ts: source_ts as i64,
            stale_count: 0,
            value_text: None,
        }
    }

    /// 单点读取：解析 IOA → 惰性建链 → 总召唤刷新快照 → 从快照取值
    pub(super) async fn read_point(&self, point: &PointConfig) -> CollectorResult<DataPoint> {
        let ioa: u32 = point.address.parse().map_err(|_| {
            CollectorError::InvalidConfig(format!("invalid IOA '{}'", point.address))
        })?;

        // C-12: 生产路径不调 `Collector::connect`（同 mc 模块背景），
        // read 入口惰性建链：既是首连入口，也是 keepalive 判定断连后的重建入口。
        // 用 is_connected() 短路——已连接时不碰 stream 锁（避免与 keepalive
        // 探测窗口抢锁导致采集路径多等一个探测超时）。
        if !self.is_connected() {
            self.connect().await?;
        }

        // 单点读取语义 = 现场即时值：每次都执行一次总召唤（返回全站快照）。
        // 旧实现在此命中缓存直接返回，导致点位值永久冻结——已移除该捷径。
        self.interrogate().await?;

        let cache = self.data.lock().await;
        match cache.get(&ioa) {
            Some((val, q, ts)) => Ok(Self::to_datapoint(point, *val, *q, *ts)),
            None => Err(CollectorError::NoReply),
        }
    }

    /// 批量读取：整批点位共享**一次**总召唤快照
    ///
    /// IEC 104 总召唤一次即返回全站数据，逐点各发一次总召唤既低效、又会把
    /// N(S)/N(R) 与 server 发送窗口（k ≤ 12）打满；故本协议覆盖默认的"逐点 read"。
    pub(super) async fn read_batch_points(
        &self,
        points: &[PointConfig],
    ) -> Vec<CollectorResult<DataPoint>> {
        let mut results: Vec<CollectorResult<DataPoint>> = Vec::with_capacity(points.len());
        if points.is_empty() {
            return results;
        }

        if !self.is_connected() {
            if let Err(e) = self.connect().await {
                let msg = e.to_string();
                return points
                    .iter()
                    .map(|_| Err(CollectorError::IoError(msg.clone())))
                    .collect();
            }
        }

        // 一次总召唤刷新整批点位；失败时整批返回同因错误（不逐点重复等待）
        if let Err(e) = self.interrogate().await {
            let msg = e.to_string();
            return points
                .iter()
                .map(|_| Err(CollectorError::IoError(msg.clone())))
                .collect();
        }

        let cache = self.data.lock().await;
        for p in points {
            let r = match p.address.parse::<u32>() {
                Ok(ioa) => match cache.get(&ioa) {
                    Some((val, q, ts)) => Ok(Self::to_datapoint(p, *val, *q, *ts)),
                    None => Err(CollectorError::NoReply),
                },
                Err(_) => Err(CollectorError::InvalidConfig(format!(
                    "invalid IOA '{}'",
                    p.address
                ))),
            };
            results.push(r);
        }
        results
    }

    /// 按 IOA 直读（不依赖点位表）
    ///
    /// IEC 104 协议**没有**「读单个 IOA」的请求帧，唯一取数机制是总召唤
    /// （C_IC_NA_1 / ASDU 100）一次返回全站快照。故「按 IOA 直读」= 一次总召唤
    /// → 从本轮快照取目标 IOA。
    ///
    /// 未命中语义：目标 IOA 不在本轮快照里 → `value = None`，
    /// **不返回 0**（0 是合法工程量，静默 0 会把「从站没这个点」误读成「值为 0」）。
    pub(super) async fn read_addresses_by_ioa(
        &self,
        addresses: &[u32],
    ) -> CollectorResult<AddressReadResult> {
        if addresses.is_empty() {
            return Ok(AddressReadResult {
                entries: Vec::new(),
                snapshot: Vec::new(),
            });
        }

        // 惰性建链（与 read_point 同口径）：首连入口 + keepalive 判定断连后的重建入口。
        if !self.is_connected() {
            self.connect().await?;
        }

        // 一次总召唤刷新整站快照（失败时如实返回错误，不返回空快照冒充成功）
        self.interrogate().await?;

        let cache = self.data.lock().await;

        // 命中项：与请求地址一一对应，保持请求顺序
        let entries: Vec<AddressReadEntry> = addresses
            .iter()
            .map(|ioa| match cache.get(ioa) {
                Some((val, q, ts)) => AddressReadEntry {
                    address: *ioa,
                    value: Some(*val),
                    quality: Some(*q),
                    source_ts: *ts as i64,
                },
                None => AddressReadEntry {
                    address: *ioa,
                    value: None,
                    quality: None,
                    source_ts: 0,
                },
            })
            .collect();

        // 全量快照：按 IOA 升序，保证前端渲染稳定
        let mut snapshot: Vec<AddressReadEntry> = cache
            .iter()
            .map(|(ioa, (val, q, ts))| AddressReadEntry {
                address: *ioa,
                value: Some(*val),
                quality: Some(*q),
                source_ts: *ts as i64,
            })
            .collect();
        snapshot.sort_by_key(|e| e.address);

        Ok(AddressReadResult { entries, snapshot })
    }

    /// 决策表 #18：写入 = 下发控制方向命令
    ///
    /// 命令类型由 `point.function_code` 决定：46=双点 / 50=调节步 / 其余（含 45 与未指定）=单点，
    /// 见 [`command_type_of`]。目标值语义：0 → 分/降；非 0 → 合/升。返回 Ok 即代表从站已给出
    /// 肯定确认（COT=激活确认/激活终止），未被确认一律返回错误，绝不静默丢弃控制命令。
    pub(super) async fn write_point(&self, point: &PointConfig, value: f64) -> CollectorResult<()> {
        let ioa: u32 = point.address.parse().map_err(|_| {
            CollectorError::InvalidConfig(format!("invalid IOA '{}'", point.address))
        })?;
        let type_id = command_type_of(point.function_code);
        let obj = encode_command_object(type_id, value);

        if !self.is_connected() {
            self.connect().await?;
        }
        self.send_command(ioa, type_id, obj).await
    }
}

impl Collector for Iec104Collector {
    fn name(&self) -> &'static str {
        "iec104"
    }

    fn bus_name(&self) -> &str {
        &self.bus_name
    }

    fn connect<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = CollectorResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let mut guard = self.stream.lock().await;
            if guard.is_some() {
                return Ok(());
            }

            let stream = TcpStream::connect(&self.addr).await.map_err(|e| {
                CollectorError::IoError(format!("IEC104 connect {} failed: {}", self.addr, e))
            })?;

            let mut stream = stream;

            let frame = build_uframe(STARTDT_ACT);
            stream
                .write_all(&frame)
                .await
                .map_err(|e| CollectorError::IoError(format!("send STARTDT: {}", e)))?;

            let resp = read_frame(&mut stream).await?;
            if resp.len() >= 4 && resp[0] == 0x0B {
                tracing::debug!("IEC104 STARTDT confirmed");
            }

            self.connected.store(true, Ordering::Relaxed);
            // 新建链：IEC 104 的 N(S)/N(R) 从 0 重新开始
            self.seq_send.store(0, Ordering::Relaxed);
            self.seq_recv.store(0, Ordering::Relaxed);

            // 决策表 #19：建链后主动对时（C_CS_NA_1, 103）。
            // 电力从站依赖主站下发时标做 SOE 时标准确，故随建链同步一次；
            // 失败仅告警不影响建链（个别从站不支持对时不应导致整链不可用）。
            // 注意：必须在 stream 移入 guard 之前调用（借用 &mut stream）。
            self.send_clock_sync(&mut stream).await;

            *guard = Some(stream);
            drop(guard); // 显式释放锁，spawn 之前

            // C-12: 连接成功后启动 keepalive supervisor
            self.spawn_keepalive_supervisor();

            Ok(())
        })
    }

    fn disconnect<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            // C-12: 先停 supervisor（让 supervisor 退出循环），再 drop stream
            if let Ok(mut h) = self.keepalive_handle.try_lock() {
                if let Some(handle) = h.take() {
                    handle.abort();
                }
            }
            let _ = self.stream.lock().await.take();
            self.connected.store(false, Ordering::Relaxed);
        })
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    fn diag_supported(&self) -> bool {
        true
    }

    /// P4：IEC104 连通性诊断 = TCP 建链 + STARTDT 握手 + 一次总召唤。
    ///
    /// 只看「TCP 连上」不足以证明远动链路可用：现场常见 TCP 通但 STARTDT 未被
    /// 从站确认、或公共地址/IOA 配置错误导致总召唤返回空。因此这里把建链与
    /// 总召唤都走一遍，并报告本轮拿到的 IOA 数量；调用方（/api/protocols/:name/diag）
    /// 已先暂停该总线，诊断与采集不会争用同一链路。
    fn diag<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send + 'a>> {
        Box::pin(async move {
            if !self.is_connected() {
                if let Err(e) = self.connect().await {
                    return format!("iec104@{}: 建链失败（{}）", self.bus_name, e);
                }
            }
            match self.interrogate().await {
                Ok(()) => {
                    let ioa_count = self.data.lock().await.len();
                    format!(
                        "iec104@{}: TCP 已连接，STARTDT 已确认，总召唤返回 {} 个 IOA",
                        self.bus_name, ioa_count
                    )
                }
                Err(CollectorError::NoReply) => format!(
                    "iec104@{}: TCP 已连接，但总召唤未返回任何数据（检查公共地址 / 总召唤是否被从站接受）",
                    self.bus_name
                ),
                Err(e) => format!(
                    "iec104@{}: TCP 已连接，但总召唤失败（{}）",
                    self.bus_name, e
                ),
            }
        })
    }

    fn read<'a>(
        &'a self,
        point: &'a PointConfig,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = CollectorResult<DataPoint>> + Send + 'a>>
    {
        Box::pin(self.read_point(point))
    }

    fn read_batch<'a>(
        &'a self,
        points: &'a [PointConfig],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Vec<CollectorResult<DataPoint>>> + Send + 'a>,
    > {
        Box::pin(self.read_batch_points(points))
    }

    fn read_addresses<'a>(
        &'a self,
        addresses: &'a [u32],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = CollectorResult<AddressReadResult>> + Send + 'a>,
    > {
        Box::pin(self.read_addresses_by_ioa(addresses))
    }

    fn write<'a>(
        &'a self,
        point: &'a PointConfig,
        value: f64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = CollectorResult<()>> + Send + 'a>> {
        Box::pin(self.write_point(point, value))
    }
}
