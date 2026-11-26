//! 「一帧应答代表什么」的单一判定口径（DD-32）。
//!
//! 调试台有两类完全不同的提问，过去各自在 handler 里写了一版，容易读成
//! 「同一件事两个答案」：
//!
//! - **设备在线吗？**（从站扫描）——协议层只要有应答就证明设备物理在线，
//!   **异常帧也算**；CRC 错/截断才是线路噪声，不算应答。
//! - **这个地址存在吗？**（寄存器扫描）——只有正常帧才证明目标地址可读；
//!   异常帧（非法数据地址 02）恰恰说明该地址不存在。
//!
//! 两处必须调用这里的同一份实现，避免以后有人只改一侧。

use crate::web::debug_bus::FrameCheck;

/// 设备在协议层是否有应答：正常帧与异常帧都算在线。
pub(crate) fn device_online(check: &FrameCheck) -> bool {
    matches!(check, FrameCheck::Ok | FrameCheck::Exception { .. })
}

/// 目标地址是否存在/可读：只有正常帧才算。
pub(crate) fn address_exists(check: &FrameCheck) -> bool {
    matches!(check, FrameCheck::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok() -> FrameCheck {
        FrameCheck::Ok
    }

    fn exception() -> FrameCheck {
        FrameCheck::Exception {
            code: 2,
            text: "非法数据地址".into(),
        }
    }

    fn crc() -> FrameCheck {
        FrameCheck::Crc
    }

    #[test]
    fn exception_frame_means_online_but_address_missing() {
        let check = exception();
        assert!(device_online(&check), "异常帧证明设备物理在线");
        assert!(!address_exists(&check), "异常帧说明目标地址不存在");
    }

    #[test]
    fn normal_frame_means_online_and_address_exists() {
        let check = ok();
        assert!(device_online(&check));
        assert!(address_exists(&check));
    }

    #[test]
    fn crc_error_is_neither_online_nor_address() {
        let check = crc();
        assert!(!device_online(&check), "CRC 错是线路噪声，不能当在线证据");
        assert!(!address_exists(&check));
    }
}
