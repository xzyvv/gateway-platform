//! 总线外部依赖注入 seam（**仅供测试注入**）
//!
//! 串口总线任务循环（[`super::serial_task`]）在生产环境直接依赖两个"物理世界"
//! 入口：`tokio_serial` 打开串口、`gpiod` 控制 RS485 DE/RE 引脚。二者在 CI /
//! Windows 上不可用，导致 `do_transaction` 的重试、写/读超时分离、DE 时序、
//! 分片读取等核心稳定性逻辑长期零覆盖。
//!
//! 本模块把这两个入口收敛为可注入的 trait 对象，使故障注入测试能用
//! `tokio::io::duplex()` 或纯内存桩驱动**真实**的总线任务循环：
//!
//! - [`SerialOpener`]：串口重开注入点（`try_reopen_serial` 使用）
//! - [`GpioOutput`]：DE/RE 方向控制注入点（`do_transaction` / `de_low` 使用）
//! - [`BusDeps`]：上述依赖的聚合，随 [`super::BusManager::spawn_with`] 传入
//!
//! 生产路径行为零变化：[`BusDeps::production`] 返回转调真实实现的默认依赖。
//!
//! 关于 `#[doc(hidden)]`：这些类型属于测试 seam，不作为对外 API 承诺，
//! 故不纳入文档，但必须是 `pub`（integration test 位于独立 crate，只能看见
//! `pub` 项）。刻意不用 feature gate——那会迫使 `cargo test --test chaos`
//! 额外加 flag，隐患大于收益。

use std::io;
use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncWrite};

use crate::config::BusConfig;
use crate::engine::error::GatewayResult;

use super::serial::SerialHandle;

/// 抽象异步串口流。
///
/// 不自定义 `async fn` trait（`async fn in trait` 对 dyn 不友好），直接复用
/// tokio 的 [`AsyncRead`] / [`AsyncWrite`]，并为所有满足条件的类型提供 blanket
/// impl，`tokio::io::duplex()` 的 `DuplexStream` 与 `tokio_serial::SerialStream`
/// 均自动满足。
pub trait AsyncSerial: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T: AsyncRead + AsyncWrite + Send + Unpin> AsyncSerial for T {}

/// 串口打开器（重开路径注入点）。
///
/// 生产实现 [`RealSerialOpener`] 转调 [`SerialHandle::open`]。
#[doc(hidden)]
pub trait SerialOpener: Send + Sync + 'static {
    fn open(&self, bus: &BusConfig) -> io::Result<SerialHandle>;
}

/// 生产串口打开器：转调真实 [`SerialHandle::open`]。
#[doc(hidden)]
pub struct RealSerialOpener;

impl SerialOpener for RealSerialOpener {
    fn open(&self, bus: &BusConfig) -> io::Result<SerialHandle> {
        SerialHandle::open(bus)
    }
}

/// GPIO 输出（DE/RE 方向控制注入点）。
///
/// 生产实现 [`RealGpioOutput`] 转调 [`crate::platform::gpio::set_gpio_output`]。
#[doc(hidden)]
pub trait GpioOutput: Send + Sync + 'static {
    fn set(&self, pin: u32, on: bool) -> GatewayResult<()>;
}

/// 生产 GPIO 输出：转调真实 `set_gpio_output`（非 Linux 平台返回明确错误）。
#[doc(hidden)]
pub struct RealGpioOutput;

impl GpioOutput for RealGpioOutput {
    fn set(&self, pin: u32, on: bool) -> GatewayResult<()> {
        crate::platform::gpio::set_gpio_output(pin, on)
    }
}

/// 总线外部依赖聚合，随总线任务创建传入。
#[derive(Clone)]
#[doc(hidden)]
pub struct BusDeps {
    pub opener: Arc<dyn SerialOpener>,
    pub gpio: Arc<dyn GpioOutput>,
}

impl BusDeps {
    /// 生产默认依赖：真实串口打开器 + 真实 GPIO 输出。
    pub fn production() -> Self {
        Self {
            opener: Arc::new(RealSerialOpener),
            gpio: Arc::new(RealGpioOutput),
        }
    }
}
