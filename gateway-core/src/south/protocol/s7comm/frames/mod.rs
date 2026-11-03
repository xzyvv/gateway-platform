//! S7 协议帧编/解码（Read Var / Write Var / Setup Comm / SZL 等）
//!
//! 决策表 #11：FUNC_WRITE 0x05
//! 决策表 #12：SZL 诊断（Userdata PDU 0x07）
//! 决策表 #13：S7-1200/1500 优化块 = 多 item Read/Write Var
//!
//! 按职责拆分：link（COTP/TPKT/Setup）、decode（值编解码）、read（读侧帧）、
//! write（写侧帧）、szl（系统状态列表）、spec（多 item 复用单元）。
use super::*;

mod decode;
mod link;
mod read;
mod spec;
mod szl;
mod write;

// ===== 区域标识（Snap7 约定）=====
pub(crate) const AREA_INPUTS: u8 = 0x81; // I 区
pub(crate) const AREA_OUTPUTS: u8 = 0x82; // Q 区
pub(crate) const AREA_MARKERS: u8 = 0x83; // M 区（中间存储器）
pub(crate) const AREA_DB: u8 = 0x84; // DB 区（数据块）

// ===== 传输类型（Transport Size, S7ANY 语法）=====
pub(crate) const TS_BIT: u8 = 0x03; // 位
pub(crate) const TS_BYTE: u8 = 0x04; // 字节（所有字节型数据统一使用此值，元素数=字节数）

// ===== S7 PDU 类型 =====
pub(crate) const PDU_JOB: u8 = 0x01; // 请求（Job）
pub(crate) const PDU_ACK: u8 = 0x03; // 确认响应（Ack-Data）

/// Userdata（决策表 #12：SZL 诊断）
pub(crate) const PDU_USERDATA: u8 = 0x07;

/// SZL 读请求的 Parameter 段（固定 8 字节）
pub(crate) const SZL_PARAM: [u8; 8] = [0x00, 0x01, 0x12, 0x04, 0x11, 0x44, 0x01, 0x00];

// ===== S7 功能码 =====
pub(crate) const FUNC_SETUP: u8 = 0xF0; // Setup Communication function code
pub(crate) const FUNC_READ: u8 = 0x04; // Read Var
pub(crate) const FUNC_WRITE: u8 = 0x05; // Write Var（决策表 #11）

// ===== 返回码 =====
pub(crate) const RET_OK: u8 = 0xFF; // 读取成功
pub(crate) const RET_NOT_FOUND: u8 = 0x05; // 地址不存在

pub(crate) use decode::{decode_value, encode_value};
pub(crate) use link::{build_cotp_connect, build_setup_comm, read_tpkt_frame};
pub(crate) use read::{
    build_read_var, build_read_var_multi, parse_read_response, parse_read_response_multi,
};
pub(crate) use spec::{ItemSpec, WriteItem};
pub(crate) use szl::{build_szl_request, extract_ascii_strings, parse_szl_response, SzlResponse};
pub(crate) use write::{
    build_write_var, build_write_var_multi, parse_write_response, parse_write_response_multi,
};
