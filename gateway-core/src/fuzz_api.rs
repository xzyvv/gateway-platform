//! fuzz / 变异测试专用入口（`feature = "fuzz"`，生产构建完全不编译）。
//!
//! 各协议的解帧/解析函数原本是 `pub(crate)`，仓库外的 `fuzz` crate 无法直接调用。
//! 本模块只做「薄转发」：固定签名、错误转 `String`，**不复制任何协议逻辑**，
//! 保证 fuzz 命中的与生产运行的是同一份代码。
//!
//! 稳定性不做承诺：仅供 `fuzz/` 下的 harness 使用，随 fuzz 需要增删。

#[cfg(feature = "modbus")]
pub fn modbus_parse_read_response(
    slave: u8,
    fc: u8,
    expected_count: u16,
    response: &[u8],
) -> Result<Vec<u16>, String> {
    crate::south::protocol::modbus::frames::parse_read_response(slave, fc, expected_count, response)
        .map_err(|e| e.to_string())
}

#[cfg(feature = "modbus")]
pub fn modbus_parse_coils_response(
    slave: u8,
    fc: u8,
    expected_count: u16,
    response: &[u8],
) -> Result<Vec<bool>, String> {
    crate::south::protocol::modbus::frames::parse_coils_response(
        slave,
        fc,
        expected_count,
        response,
    )
    .map_err(|e| e.to_string())
}

#[cfg(feature = "modbus")]
pub fn modbus_extract_frame(
    fb: &mut crate::south::bus::FrameBuffer,
    expected_count: Option<u16>,
) -> Option<Vec<u8>> {
    crate::south::protocol::modbus::frames::extract_modbus_frame(fb, expected_count)
}

#[cfg(feature = "dlt645")]
fn dlt645_version(index: u8) -> crate::south::protocol::dlt645::frames::Dlt645Version {
    use crate::south::protocol::dlt645::frames::Dlt645Version;
    match index % 5 {
        0 => Dlt645Version::V2007,
        1 => Dlt645Version::V1997BePlain,
        2 => Dlt645Version::V1997LePlain,
        3 => Dlt645Version::V1997BeCipher,
        _ => Dlt645Version::V1997LeCipher,
    }
}

#[cfg(feature = "dlt645")]
pub fn dlt645_build_frame(addr: &[u8], ctrl: u8, data: &[u8], version_index: u8) -> Vec<u8> {
    crate::south::protocol::dlt645::frames::build_dlt645_frame(
        addr,
        ctrl,
        data,
        dlt645_version(version_index),
    )
}

#[cfg(feature = "dlt645")]
pub fn dlt645_parse_response(
    response: &[u8],
    expected_addr: &[u8],
    version_index: u8,
) -> Result<Vec<u8>, String> {
    crate::south::protocol::dlt645::frames::parse_dlt645_response(
        response,
        expected_addr,
        dlt645_version(version_index),
    )
    .map_err(|e| e.to_string())
}

/// 该变体的数据区是否逐字节 -0x33（round-trip 断言需要据此换算期望值）。
#[cfg(feature = "dlt645")]
pub fn dlt645_data_cipher(version_index: u8) -> bool {
    dlt645_version(version_index).data_cipher()
}

#[cfg(feature = "dlt645")]
pub fn dlt645_extract_frame(
    fb: &mut crate::south::bus::FrameBuffer,
    expected_count: Option<u16>,
) -> Option<Vec<u8>> {
    crate::south::protocol::dlt645::frames::extract_dlt645_frame(fb, expected_count)
}

#[cfg(feature = "bacnet-mstp")]
pub fn mstp_parse_frame(
    frame: &[u8],
    metrics: &crate::engine::metrics::Metrics,
) -> Result<(u8, u8, u8, Vec<u8>), String> {
    crate::south::protocol::bacnet_mstp::frames::parse_mstp_frame(frame, metrics)
        .map_err(|e| e.to_string())
}

#[cfg(feature = "bacnet-mstp")]
pub fn mstp_build_frame(frame_type: u8, dest: u8, src: u8, data: &[u8]) -> Vec<u8> {
    crate::south::protocol::bacnet_mstp::frames::build_mstp_frame(frame_type, dest, src, data)
}

/// 单帧数据区上限：超过该长度的帧解析侧会拒绝，round-trip 需要提前截断。
#[cfg(feature = "bacnet-mstp")]
pub fn mstp_max_data_len() -> usize {
    crate::south::protocol::bacnet_mstp::frames::MAX_DATA_LEN
}

#[cfg(feature = "bacnet-mstp")]
pub fn mstp_extract_frame(
    fb: &mut crate::south::bus::FrameBuffer,
    expected_count: Option<u16>,
) -> Option<Vec<u8>> {
    crate::south::protocol::bacnet_mstp::frames::extract_mstp_frame(fb, expected_count)
}
