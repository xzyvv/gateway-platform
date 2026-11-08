//! GPIO 协议采集器（§3.10 Collector trait 实现）
//!
//! 通过 `platform::gpio` 读取/写入 GPIO 引脚，实现 `Collector` trait，
//! 纳入统一采集调度。
//!
//! 与 Modbus/DLT645/CJT188 不同：GPIO 不需要 BusManager，
//! 直接通过 `platform::gpio` 访问 gpiochip。bootstrap 在 `init_collectors`
//! 中按 bus_name 单独构造注册（复用 `Collector::write` 后，点位写 API /
//! `/api/point-tools/write` 按总线名查到的就是本采集器，DO 写由此闭环）。
//!
//! # 地址约定
//!
//! `point.address` 解析为**绝对线号**，支持：
//! - 裸整数 `"5"`：线号 5（兼容旧行为）
//! - `"di:<n>"`：数字输入线 n → 线号 n
//! - `"do:<n>"`：数字输出 DO n（n 属于 0..4）→ 线号 n + 4（与 Web `/api/io/:index` 一致）
//!
//! 写操作仅接受落在 DO 段 `4..=7` 的线号（否则返回错误）。
//!
//! # DI 信号处理（边沿计数 / 防抖）
//!
//! `point.data_type` 决定 DI 处理模式（见 [`filter::DiMode`]）：
//! `debounced` 输出两扫描确认后的稳定电平；`counter`/`counter_rising`/
//! `counter_falling`/`counter_both` 输出累计边沿数；缺省或其它值保持原始
//! 电平直通（与旧行为兼容）。仅对稳定保持 ≥2 个扫描周期的信号可靠，
//! 详见 [`filter`] 模块文档。

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use parking_lot::Mutex;

use crate::config::PointConfig;
use crate::engine::model::DataPoint;
use crate::south::protocol::{Collector, CollectorError, CollectorResult};

mod filter;
use filter::{DiMode, DiState};

/// DO 输出绝对线号区间（序号 = 线号 - 4，与 Web `/api/io/:index` 对齐）
const DO_PIN_RANGE: std::ops::RangeInclusive<u32> = 4..=7;

/// 解析 GPIO 点位地址为**绝对线号**
///
/// 支持裸整数、`di:<n>`、`do:<n>`（详见模块文档）。`do:<n>` 的 n 限定
/// `0..4`，越界返回 [`CollectorError::InvalidConfig`]。
pub(crate) fn parse_gpio_pin(address: &str) -> CollectorResult<u32> {
    let s = address.trim();
    let lower = s.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("do:") {
        let idx: u32 = rest.trim().parse().map_err(|_| {
            CollectorError::InvalidConfig(format!(
                "invalid GPIO DO index '{}', expected do:<0..3>",
                address
            ))
        })?;
        if idx >= 4 {
            return Err(CollectorError::InvalidConfig(format!(
                "GPIO DO index {} out of range 0..=3",
                idx
            )));
        }
        Ok(idx + 4)
    } else if let Some(rest) = lower.strip_prefix("di:") {
        rest.trim().parse::<u32>().map_err(|_| {
            CollectorError::InvalidConfig(format!(
                "invalid GPIO DI index '{}', expected di:<n>",
                address
            ))
        })
    } else {
        s.parse::<u32>().map_err(|e| {
            CollectorError::InvalidConfig(format!("invalid GPIO address '{}': {}", address, e))
        })
    }
}

/// GPIO 采集器
///
/// 一个 `GpioCollector` 实例对应一个 bus_name（通常为 "gpio"），
/// 服务该 bus 下所有 GPIO 点位（每个点位 address 字段为引脚编号）。
pub struct GpioCollector {
    bus_name: String,
    /// 点位 id → DI 处理状态（仅 counter/debounced 模式的点位会写入）
    states: Mutex<HashMap<String, DiState>>,
}

impl GpioCollector {
    pub fn new(bus_name: impl Into<String>) -> Self {
        Self {
            bus_name: bus_name.into(),
            states: Mutex::new(HashMap::new()),
        }
    }

    /// 按点位模式取出（或创建/重置）DI 状态，应用一次扫描
    ///
    /// `data_type` 变更时重置状态，避免沿用旧模式的计数或基线。
    fn apply_di_filter(&self, point_id: &str, mode: DiMode, raw: bool) -> filter::DiOutput {
        let mut states = self.states.lock();
        let state = states
            .entry(point_id.to_string())
            .or_insert_with(|| DiState::new(mode));
        if state.mode() != mode {
            *state = DiState::new(mode);
        }
        state.update(raw)
    }
}

impl Collector for GpioCollector {
    fn name(&self) -> &'static str {
        "gpio"
    }

    fn bus_name(&self) -> &str {
        &self.bus_name
    }

    /// 读取单个 GPIO 点位
    ///
    /// 先按地址解析线号并读取原始电平，再按 `data_type` 决定是否做
    /// 边沿计数/防抖处理（见模块文档）。读取失败返回 [`CollectorError::IoError`]，
    /// 由上层退化为失败数据点，**不**推进滤波状态（避免失败样本污染计数）。
    fn read<'a>(
        &'a self,
        point: &'a PointConfig,
    ) -> Pin<Box<dyn Future<Output = CollectorResult<DataPoint>> + Send + 'a>> {
        Box::pin(async move {
            let pin = parse_gpio_pin(&point.address)?;
            let raw = crate::platform::gpio::read_gpio_line(pin)
                .map_err(|e| CollectorError::IoError(e.to_string()))?
                != 0;

            let mode = DiMode::from_data_type(point.data_type.as_deref());
            let (value, raw_value) = if mode.needs_state() {
                let out = self.apply_di_filter(&point.id, mode, raw);
                let level = if out.level { 1.0 } else { 0.0 };
                if mode.is_counter() {
                    (out.count as f64, level)
                } else {
                    (level, level)
                }
            } else {
                let level = if raw { 1.0 } else { 0.0 };
                (level, level)
            };

            let device_id = point.device_id.clone().unwrap_or_else(|| point.bus.clone());
            Ok(DataPoint::good(
                &point.id, &device_id, value, raw_value, "gpio",
            ))
        })
    }

    /// 诊断信息
    fn diag_supported(&self) -> bool {
        true
    }

    /// 诊断信息
    fn diag<'a>(&'a self) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> {
        Box::pin(async move { format!("gpio@{}: /dev/gpiochip0", self.bus_name) })
    }

    /// 写入 DO 输出线（点位级协议，经统一点位写路径下发）
    ///
    /// 地址解析出绝对线号后，仅接受 DO 段 `4..=7`；`value != 0` 视为置位。
    /// 硬件驱动失败返回 [`CollectorError::IoError`]，且不更新 DO 状态位，
    /// 保证 `/api/io/status` 与实际输出一致。
    fn write<'a>(
        &'a self,
        point: &'a PointConfig,
        value: f64,
    ) -> Pin<Box<dyn Future<Output = CollectorResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let pin = parse_gpio_pin(&point.address)?;
            if !DO_PIN_RANGE.contains(&pin) {
                return Err(CollectorError::InvalidConfig(format!(
                    "GPIO DO pin {} out of range 4..=7 (write only supports DO lines)",
                    pin
                )));
            }
            crate::platform::gpio::write_do(pin, value != 0.0)
                .map_err(|e| CollectorError::IoError(e.to_string()))?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests;
