//! 平台抽象层：硬件相关

pub mod cellular;
pub mod gpio;
pub mod hardware_watchdog;
pub mod ntp;
pub mod ota;
pub mod watchdog_flush;

pub use cellular::CellularManager;
pub use gpio::GpioController;
#[cfg(target_os = "linux")]
pub use hardware_watchdog::Watchdog;
pub use ntp::NtpManager;
pub use ntp::NtpStatus;
pub use ota::OtaManager;
