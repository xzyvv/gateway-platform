//! 按 device_id 从配置解析设备 / 总线 / 从站地址

use crate::web::api::protocol_tools::DebugError;
use crate::web::debug_bus::BusErrorKind;
use crate::web::types::AppState;
use gateway_core::config::DeviceConfig;

// 协议能力路由（是否支持参数探测）由调用方按注册表能力位判断，
// 本函数只负责「设备 → 总线 + 从站地址」解析，不感知协议名。

/// 从配置中按 device_id 查找设备，返回 (DeviceConfig, bus_name, slave_id)
///
/// 失败只回「类别 + 文案」，响应由调用点用 `error_response` 组装
/// （避免把体积 ≥128 字节的 `Response` 当作 Err 变体）。
pub(super) fn resolve_device(
    state: &AppState,
    device_id: &str,
) -> Result<(DeviceConfig, String, u8), DebugError> {
    let cfg = state.config.load_full();
    let devices = cfg.devices.clone().unwrap_or_default();
    let device = devices
        .into_iter()
        .find(|d| d.id == device_id)
        .ok_or_else(|| DebugError {
            kind: BusErrorKind::BusNotFound,
            detail: format!("设备不存在: {device_id}"),
        })?;

    // 从站地址：优先 slave_id，否则从 address 解析
    let slave_id = match device.slave_id {
        Some(sid) => sid,
        None => device.address.parse::<u8>().map_err(|_| DebugError {
            kind: BusErrorKind::InvalidFrame,
            detail: format!("设备 {} 的从站地址无效: {}", device_id, device.address),
        })?,
    };

    let bus_name = device.bus.clone();
    if bus_name.is_empty() {
        return Err(DebugError {
            kind: BusErrorKind::MissingBus,
            detail: format!("设备 {} 未配置总线", device_id),
        });
    }

    Ok((device, bus_name, slave_id))
}
