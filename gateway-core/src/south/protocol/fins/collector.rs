//! FINS 采集器的 `Collector` trait 实现（连接 / 读写 / 批量读 / 诊断）。
//!
//! 从 `mod.rs` 拆出，只为控制单文件行数（C24 行数预算），行为不变。

use std::sync::atomic::Ordering;

use crate::config::PointConfig;
use crate::engine::model::{DataPoint, PointDataType, Quality};
use crate::south::protocol::{
    convert_value, mark_write_uncertain, reverse_convert, Collector, CollectorError, CollectorResult,
    ResultSlots,
};

use super::frames;
use super::frames::{
    build_cpu_control_frame, build_read_frame, build_write_frame, encode_words, parse_address,
    parse_cpu_control, parse_read_response, parse_write_response, MRC_MEMORY_READ,
    MRC_MEMORY_WRITE, SRC_MEMORY_READ, SRC_MEMORY_WRITE,
};
use super::{FinsCollector, CLIENT_NODE};

impl Collector for FinsCollector {
    fn name(&self) -> &'static str {
        "fins"
    }

    fn bus_name(&self) -> &str {
        &self.bus_name
    }

    fn diag_supported(&self) -> bool {
        true
    }

    /// B3：节点地址握手 + 读 DM0 一个字，报告实际回读值。
    ///
    /// FINS 的建链本身包含节点地址交换（TCP），读 DM0 再验证一次真实数据面。
    fn diag<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send + 'a>> {
        Box::pin(async move {
            let area = match parse_address("D0") {
                Ok(a) => a,
                Err(e) => return format!("FINS {}：D0 地址解析失败（{e}）", self.bus_name),
            };
            if let Err(e) = self.ensure_connected().await {
                return format!("FINS {}：建链/节点握手失败（{e}）", self.bus_name);
            }
            let server_node = self.server_node.load(Ordering::Relaxed);
            let frame = build_read_frame(server_node, CLIENT_NODE, area.word_code, area.address, 1);
            let payload = match self.transact(&frame).await {
                Ok(p) => p,
                Err(e) => return format!("FINS {}：建链成功，但读 DM0 失败（{e}）", self.bus_name),
            };
            let data = match parse_read_response(&payload, MRC_MEMORY_READ, SRC_MEMORY_READ) {
                Ok(d) => d,
                Err(e) => {
                    return format!("FINS {}：建链成功，但 DM0 应答不合规（{e}）", self.bus_name)
                }
            };
            match frames::decode_words(&data, PointDataType::U16) {
                Some(v) => format!(
                    "FINS {}：建链成功，DM0 回读值={v}（server_node=0x{server_node:02X}）",
                    self.bus_name
                ),
                None => format!(
                    "FINS {}：建链成功，但 DM0 应答无法按 U16 解码（{} 字节）",
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
            let _ = self.udp.lock().await.take();
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
            let area = parse_address(&point.address)?;
            let is_bit = matches!(point.data_type.as_deref(), Some("bit") | Some("bool"));
            let (area_code, count, dt) = if is_bit {
                (area.bit_code, 1u16, None)
            } else {
                let dt = point.data_type_enum().unwrap_or(PointDataType::U16);
                (area.word_code, dt.register_count(), Some(dt))
            };

            let server_node = {
                // 先确保握手完成，server_node 才是有效的目标节点地址
                self.ensure_connected().await?;
                self.server_node.load(Ordering::Relaxed)
            };
            let frame = build_read_frame(server_node, CLIENT_NODE, area_code, area.address, count);
            let payload = self.transact(&frame).await?;
            let data = parse_read_response(&payload, MRC_MEMORY_READ, SRC_MEMORY_READ)?;

            let (raw_value, raw_type) = match dt {
                None => {
                    let byte = *data.first().ok_or_else(|| {
                        CollectorError::InvalidFrame("fins bit data empty".into())
                    })?;
                    ((byte & 0x01) as f64, "bit".to_string())
                }
                Some(dt) => {
                    let v = frames::decode_words(&data, dt).ok_or_else(|| {
                        CollectorError::InvalidFrame(format!(
                            "fins word decode failed ({} bytes for {:?})",
                            data.len(),
                            dt
                        ))
                    })?;
                    (v, "fins".to_string())
                }
            };

            let value = convert_value(point, raw_value);
            // FINS 读响应不带时标：原始采集时刻即本次读成功时刻（此前填 0 → 北向 1970）
            let ts = DataPoint::now();
            Ok(DataPoint {
                point_id: point.id.clone(),
                device_id: point.device_id.clone().unwrap_or_default(),
                value,
                raw_value,
                raw_type,
                quality: Quality::Good,
                quality_reason: String::new(),
                timestamp: ts,
                source_ts: ts,
                stale_count: 0,
                value_text: None,
            })
        })
    }

    /// 批量读取：整批共享一次惰性建链（对齐 IEC 104）；默认逐点 read 在设备
    /// 不可达时叠加 N 倍 io_timeout，这里先建链一次：失败整批同因返回、成功则短路。
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
            self.read_batch_points_with_slots(points, slots.clone())
                .await;
            let out = slots
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter_mut()
                .map(|s| s.take().unwrap_or_else(|| Err(CollectorError::NoReply)))
                .collect();
            out
        })
    }

    /// 缺陷 #7：沿用 `read_batch` 的建链 + 逐点 read 语义，结果即写共享槽，组超时 drop 不丢。
    #[allow(private_interfaces)]
    fn read_batch_with_slots<'a>(
        &'a self,
        points: &'a [PointConfig],
        slots: ResultSlots,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.read_batch_points_with_slots(points, slots).await;
        })
    }

    fn write<'a>(
        &'a self,
        point: &'a PointConfig,
        value: f64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = CollectorResult<()>> + Send + 'a>> {
        Box::pin(async move {
            // CPU 单元运行控制：地址为 CPU:RUN / CPU:STOP / RUN / STOP 时，
            // 地址决定动作（value 忽略），改发 0401/0402 命令而非存储区写。
            if let Some(ctrl) = parse_cpu_control(&point.address) {
                self.ensure_connected().await?;
                let server_node = self.server_node.load(Ordering::Relaxed);
                let frame = build_cpu_control_frame(server_node, CLIENT_NODE, ctrl);
                // 请求已下发：传输失败无法证明写未生效 → 结果未知
                let payload = self.transact(&frame).await.map_err(mark_write_uncertain)?;
                let (mrc, src) = ctrl.command_code();
                return parse_write_response(&payload, mrc, src).map_err(mark_write_uncertain);
            }
            let area = parse_address(&point.address)?;
            // 先建链完成节点地址握手，server_node 才有效
            self.ensure_connected().await?;
            let server_node = self.server_node.load(Ordering::Relaxed);

            let frame = if matches!(point.data_type.as_deref(), Some("bit") | Some("bool")) {
                let data = [if value != 0.0 { 0x01u8 } else { 0x00u8 }];
                build_write_frame(
                    server_node,
                    CLIENT_NODE,
                    area.bit_code,
                    area.address,
                    1,
                    &data,
                )
            } else {
                let dt = point.data_type_enum().unwrap_or(PointDataType::U16);
                // 写值与读路径对偶：北向下发的是一次侧值，需先除回变比与 scale/offset
                let data = encode_words(reverse_convert(point, value), dt);
                build_write_frame(
                    server_node,
                    CLIENT_NODE,
                    area.word_code,
                    area.address,
                    dt.register_count(),
                    &data,
                )
            };
            let payload = self.transact(&frame).await.map_err(mark_write_uncertain)?;
            parse_write_response(&payload, MRC_MEMORY_WRITE, SRC_MEMORY_WRITE)
                .map_err(mark_write_uncertain)
        })
    }
}
