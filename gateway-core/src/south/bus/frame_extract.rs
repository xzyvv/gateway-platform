//! 串口帧提取分发：按协议从 FrameBuffer 中切出完整帧。
//!
//! 各协议的帧提取实现位于协议自身模块（modbus/frames.rs、dlt645/frames.rs 等），
//! 本模块通过 [`ProtocolRegistry`] 查找协议对应的帧提取器，避免字符串 match 硬编码。
//!
//! 设计说明：早期帧提取逻辑集中在本文件用字符串 match 分发，已改为通过协议注册表
//! 查找。新增串行协议只需在 `PROTOCOL_ENTRIES` 注册 `frame_extractor`，此处自动生效，
//! 无需修改本文件——这与 `docs/adding-a-protocol.md` 文档描述的"单一注册点"对齐。

use once_cell::sync::Lazy;

use super::frame_buffer::FrameBuffer;
use crate::south::protocol::ProtocolRegistry;

/// 全局协议注册表单例：首次访问时构建一次，本模块通过它查找帧提取器。
///
/// 使用全局单例而非让 `BusManager` 持有 registry 引用，是为了保持
/// `BusManager::spawn` 签名不变、避免侵入式改造 serial_task 调用链。
/// `ProtocolRegistry` 内部为 `HashMap<String, fn 指针>`，构建后只读访问，线程安全。
/// 若未来需要运行时动态注册协议，可改为 `BusManager` 持有 `Arc<ProtocolRegistry>`。
static REGISTRY: Lazy<ProtocolRegistry> = Lazy::new(ProtocolRegistry::new);

/// 帧提取分发：通过协议注册表查找对应提取器并调用。
///
/// 各协议的帧提取器实现于协议自身模块（如 modbus/frames.rs 的 extract_modbus_frame），
/// 本函数仅做 registry 查找分发。未注册协议（含 TCP/UDP 协议与未在
/// `PROTOCOL_ENTRIES` 登记的新协议）返回 `None`，调用方据此判定帧不完整。
pub(super) fn extract_frame(
    fb: &mut FrameBuffer,
    protocol: &str,
    expected_count: Option<u16>,
) -> Option<Vec<u8>> {
    REGISTRY
        .frame_extractor(protocol)
        .and_then(|extractor| extractor(fb, expected_count))
}

#[cfg(test)]
mod tests;
