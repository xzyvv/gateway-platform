//! 协议异常指标（counter）
//!
//! 当前主要承载 Modbus 异常分类计数（Bug 186 N16），后续 SI/FINS/BACnet
//! 等协议若加类似异常细分时，可在本子结构中扩展独立命名（不需要再新建子结构）。
//!
//! Prom 命名保持向后兼容：
//! - `gateway_modbus_exception_recoverable_total`
//! - `gateway_modbus_exception_config_error_total`
//! - `gateway_modbus_exception_retry_exhausted_total`

use std::sync::atomic::AtomicU64;

#[derive(Default)]
pub struct ProtocolExceptionMetrics {
    /// Bug 186 (N16): Modbus 可恢复异常（从站忙/故障/确认/存储错）。
    pub modbus_recoverable: AtomicU64,
    /// Bug 186 (N16): Modbus 配置异常（非法功能码/地址/值）。
    pub modbus_config_error: AtomicU64,
    /// Bug 186 (N16): Modbus 可恢复异常重试后仍失败。
    pub modbus_retry_exhausted: AtomicU64,
}

impl ProtocolExceptionMetrics {
    pub fn render(&self, buf: &mut String) {
        use std::fmt::Write as _;
        use std::sync::atomic::Ordering;
        let _ = writeln!(
            buf,
            "# HELP gateway_modbus_exception_recoverable_total Modbus recoverable exceptions\n\
             # TYPE gateway_modbus_exception_recoverable_total counter\n\
             gateway_modbus_exception_recoverable_total {}\n\
             # HELP gateway_modbus_exception_config_error_total Modbus config-error exceptions\n\
             # TYPE gateway_modbus_exception_config_error_total counter\n\
             gateway_modbus_exception_config_error_total {}\n\
             # HELP gateway_modbus_exception_retry_exhausted_total Modbus retry exhausted\n\
             # TYPE gateway_modbus_exception_retry_exhausted_total counter\n\
             gateway_modbus_exception_retry_exhausted_total {}",
            self.modbus_recoverable.load(Ordering::Relaxed),
            self.modbus_config_error.load(Ordering::Relaxed),
            self.modbus_retry_exhausted.load(Ordering::Relaxed),
        );
    }
}
