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

// ===== S7comm =====

#[cfg(feature = "s7comm")]
fn s7_data_type(index: u8) -> crate::south::protocol::s7comm::S7DataType {
    use crate::south::protocol::s7comm::S7DataType;
    match index % 7 {
        0 => S7DataType::Bit,
        1 => S7DataType::Byte,
        2 => S7DataType::Word,
        3 => S7DataType::Int,
        4 => S7DataType::DWord,
        5 => S7DataType::DInt,
        _ => S7DataType::Real,
    }
}

#[cfg(feature = "s7comm")]
pub fn s7_parse_read_response(frame: &[u8], data_type_index: u8) -> Result<f64, String> {
    crate::south::protocol::s7comm::frames::parse_read_response(
        frame,
        s7_data_type(data_type_index),
    )
    .map_err(|e| e.to_string())
}

#[cfg(feature = "s7comm")]
pub fn s7_decode_value(data: &[u8], data_type_index: u8) -> Result<f64, String> {
    crate::south::protocol::s7comm::frames::decode_value(data, s7_data_type(data_type_index))
        .map_err(|e| e.to_string())
}

/// 多 item Read Var 响应解析（决策表 #13 的批量路径）。
///
/// item 数为对端不可控的上限值（截到 8），规格固定为 DB1 上连续 Real，
/// 这样 fuzz 只需覆盖「响应字节畸形」这一面，不引入配置组合爆炸。
#[cfg(feature = "s7comm")]
pub fn s7_parse_read_response_multi(
    frame: &[u8],
    item_count: u8,
) -> Result<Vec<Result<Vec<u8>, u8>>, String> {
    use crate::south::protocol::s7comm::frames::spec::ItemSpec;
    use crate::south::protocol::s7comm::S7DataType;

    let items: Vec<ItemSpec> = (0..item_count.min(8))
        .map(|i| ItemSpec {
            area: crate::south::protocol::s7comm::frames::AREA_DB,
            db_number: 1,
            byte_offset: i as u32 * 4,
            bit_offset: 0,
            data_type: S7DataType::Real,
        })
        .collect();
    crate::south::protocol::s7comm::frames::parse_read_response_multi(frame, &items)
        .map_err(|e| e.to_string())
}

/// SZL 响应解析：返回 `(szl_id, szl_index, record_count)` 摘要，避免暴露内部类型。
#[cfg(feature = "s7comm")]
pub fn s7_parse_szl_response(frame: &[u8]) -> Result<(u16, u16, u16), String> {
    crate::south::protocol::s7comm::frames::parse_szl_response(frame)
        .map(|r| (r.szl_id, r.szl_index, r.record_count))
        .map_err(|e| e.to_string())
}

#[cfg(feature = "s7comm")]
pub fn s7_parse_write_response(frame: &[u8]) -> Result<(), String> {
    crate::south::protocol::s7comm::frames::parse_write_response(frame).map_err(|e| e.to_string())
}

// ===== 三菱 MC =====

/// 按索引轮转 `PointDataType`（MC / FINS 的字节解码共用；两者都按规范大端字序列）。
#[cfg(any(feature = "mc", feature = "fins"))]
fn point_data_type(index: u8) -> crate::engine::model::PointDataType {
    use crate::engine::model::PointDataType;
    match index % 8 {
        0 => PointDataType::U16,
        1 => PointDataType::I16,
        2 => PointDataType::U32,
        3 => PointDataType::I32,
        4 => PointDataType::U64,
        5 => PointDataType::I64,
        6 => PointDataType::F32,
        _ => PointDataType::F64,
    }
}

/// 3E 响应头校验 + 帧体长度推导（生产 `read_response_3e` 同一条路径）。
#[cfg(feature = "mc")]
pub fn mc_parse_response_header_3e(head: &[u8]) -> Result<usize, String> {
    crate::south::protocol::mc::frames::parse_response_header_3e(head).map_err(|e| e.to_string())
}

#[cfg(feature = "mc")]
pub fn mc_parse_response_header_4e(head: &[u8], serial: u16) -> Result<usize, String> {
    crate::south::protocol::mc::frames::parse_response_header_4e(head, serial)
        .map_err(|e| e.to_string())
}

#[cfg(feature = "mc")]
pub fn mc_parse_response_header_1e(
    hdr: &[u8],
    func: u8,
    is_read: bool,
    is_bit: bool,
    points: u16,
) -> Result<(u16, usize), String> {
    crate::south::protocol::mc::frames::parse_response_header_1e(hdr, func, is_read, is_bit, points)
        .map_err(|e| e.to_string())
}

#[cfg(feature = "mc")]
pub fn mc_body_len_checked(len: usize) -> Result<usize, String> {
    crate::south::protocol::mc::frames::body_len_checked(len).map_err(|e| e.to_string())
}

#[cfg(feature = "mc")]
pub fn mc_decode_words(data: &[u8], data_type_index: u8) -> Option<f64> {
    crate::south::protocol::mc::frames::decode_words(data, point_data_type(data_type_index))
}

#[cfg(feature = "mc")]
pub fn mc_encode_words(value: f64, data_type_index: u8) -> Vec<u8> {
    crate::south::protocol::mc::frames::encode_words(value, point_data_type(data_type_index))
}

#[cfg(feature = "mc")]
pub fn mc_decode_bit(data: &[u8], index: usize) -> Option<f64> {
    crate::south::protocol::mc::frames::decode_bit(data, index)
}

#[cfg(feature = "mc")]
pub fn mc_check_end_code(code: u16) -> Result<(), String> {
    crate::south::protocol::mc::frames::check_end_code(code).map_err(|e| e.to_string())
}

// ===== 欧姆龙 FINS =====

#[cfg(feature = "fins")]
pub fn fins_parse_read_response(
    frame: &[u8],
    expect_mrc: u8,
    expect_src: u8,
) -> Result<Vec<u8>, String> {
    crate::south::protocol::fins::frames::parse_read_response(frame, expect_mrc, expect_src)
        .map_err(|e| e.to_string())
}

#[cfg(feature = "fins")]
pub fn fins_parse_write_response(
    frame: &[u8],
    expect_mrc: u8,
    expect_src: u8,
) -> Result<(), String> {
    crate::south::protocol::fins::frames::parse_write_response(frame, expect_mrc, expect_src)
        .map_err(|e| e.to_string())
}

#[cfg(feature = "fins")]
pub fn fins_parse_read_end_code(frame: &[u8]) -> Result<u16, String> {
    crate::south::protocol::fins::frames::parse_read_end_code(frame).map_err(|e| e.to_string())
}

#[cfg(feature = "fins")]
pub fn fins_parse_node_addr_response(payload: &[u8]) -> Result<u8, String> {
    crate::south::protocol::fins::frames::parse_node_addr_response(payload)
        .map_err(|e| e.to_string())
}

#[cfg(feature = "fins")]
pub fn fins_decode_words(data: &[u8], data_type_index: u8) -> Option<f64> {
    crate::south::protocol::fins::frames::decode_words(data, point_data_type(data_type_index))
}
