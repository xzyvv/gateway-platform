//! 串口管理（§11 启动流程第 5 步）
//!
//! 基于 tokio-serial 打开串口总线，提供 async 读写。
//! 每条总线由 BusManager 通过 mpsc 通道串行化访问。

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_serial::{DataBits, FlowControl, Parity as SerialParity, SerialPortBuilderExt, StopBits};

use super::transport::AsyncSerial;
use crate::config::BusConfig;

/// 串口句柄封装。
///
/// `inner` 为 trait 对象而非具体的 `tokio_serial::SerialStream`，以便测试通过
/// [`SerialHandle::from_stream`] 注入内存桩（见 `transport` 模块 seam 说明）。
pub struct SerialHandle {
    inner: Box<dyn AsyncSerial>,
}

impl SerialHandle {
    /// 从任意异步流构造句柄（**仅供测试注入**，生产请用 [`SerialHandle::open`]）。
    ///
    /// 典型用法：`SerialHandle::from_stream(tokio::io::duplex(64).0)`。
    #[doc(hidden)]
    pub fn from_stream<S: AsyncSerial + 'static>(stream: S) -> Self {
        Self {
            inner: Box::new(stream),
        }
    }

    /// 从 BusConfig 打开串口
    pub fn open(bus: &BusConfig) -> Result<Self, std::io::Error> {
        let parity = match bus.parity.as_str() {
            "even" => SerialParity::Even,
            "odd" => SerialParity::Odd,
            _ => SerialParity::None,
        };
        let data_bits = match bus.data_bits {
            5 => DataBits::Five,
            6 => DataBits::Six,
            7 => DataBits::Seven,
            _ => DataBits::Eight,
        };
        let stop_bits = if bus.stop_bits == 2 {
            StopBits::Two
        } else {
            StopBits::One
        };

        let port = tokio_serial::new(&bus.port_name, bus.baud_rate)
            .data_bits(data_bits)
            .parity(parity)
            .stop_bits(stop_bits)
            .flow_control(FlowControl::None)
            .open_native_async()?;

        Ok(Self {
            inner: Box::new(port),
        })
    }

    /// 写入数据
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, data), fields(n = data.len()), level = "trace"))]
    pub async fn write(&mut self, data: &[u8]) -> Result<(), std::io::Error> {
        self.inner.write_all(data).await?;
        self.inner.flush().await?;
        Ok(())
    }

    /// 读取最多 n 字节
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, buf), level = "trace")
    )]
    pub async fn read(&mut self, buf: &mut [u8]) -> Result<usize, std::io::Error> {
        self.inner.read(buf).await
    }

    /// 清空接收缓冲中的残留字节（Bug C15）。
    ///
    /// 上一事务超时 / 被中断时，设备回包可能仍滞留在 OS 接收缓冲中；若不清空，
    /// 下一事务解析会先读到旧帧，导致帧错位（CRC / 长度校验失败、脏数据），
    /// 甚至让后续多个事务连续失败。事务开始前调用可保证读到的第一帧属于本次请求。
    ///
    /// 返回丢弃的字节数。残留字节已就绪会被立即读出，只有最后一次"确认为空"
    /// 的读取会等待 `IDLE_MS`（约 5ms）。
    pub async fn clear_input(&mut self) -> usize {
        const IDLE_MS: u64 = 5;
        let mut drained = 0usize;
        let mut scratch = [0u8; 256];
        loop {
            match tokio::time::timeout(
                std::time::Duration::from_millis(IDLE_MS),
                self.inner.read(&mut scratch),
            )
            .await
            {
                Ok(Ok(n)) if n > 0 => drained += n,
                // 超时（缓冲已空）或读错误 → 结束
                _ => break,
            }
        }
        drained
    }
}

/// 串口管理器（启动时打开所有总线）
///
/// `opener` 为打开策略的注入点：生产用 `RealSerialOpener` 转调
/// [`SerialHandle::open`]；测试可注入假打开器验证热重载「先关旧口、再开新口」
/// 与「新口失败回退旧口」的顺序语义（U-5 / U-6）。
pub struct SerialManager {
    opener: std::sync::Arc<dyn super::transport::SerialOpener>,
}

impl SerialManager {
    pub fn new() -> Self {
        Self {
            opener: std::sync::Arc::new(super::transport::RealSerialOpener),
        }
    }

    /// 测试专用：注入自定义打开器（**仅供测试**）。
    #[doc(hidden)]
    pub fn with_opener(opener: std::sync::Arc<dyn super::transport::SerialOpener>) -> Self {
        Self { opener }
    }

    /// 打开一条总线
    pub fn open_bus(&self, bus: &BusConfig) -> Result<SerialHandle, std::io::Error> {
        self.opener.open(bus)
    }
}

impl Default for SerialManager {
    fn default() -> Self {
        Self::new()
    }
}
