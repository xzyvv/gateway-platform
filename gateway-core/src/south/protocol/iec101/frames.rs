//! IEC 60870-5-101 链路层（FT1.2）帧构建 / 解析 / 提取
//!
//! FT1.2 非平衡串行链路，三种帧格式：
//! - 单字符帧：`E5`（仅用于链路层确认 ACK）
//! - 固定帧长：`10 C A CS 16`（5 字节；用于复位/请求链路状态/请求一二级数据）
//!   - CS = (C + A) mod 256
//! - 可变帧长：`68 L L 68 C A <ASDU> CS 16`
//!   - L = C + A + ASDU 的字节数（含 C、A），故总长 = 6 + L
//!   - CS = (C + A + ASDU 各字节) 之和 mod 256
//!
//! 控制域 C 的位定义：bit7=保留，bit6=PRM（1=主站），bit5=FCB，bit4=FCV，
//! bit3..0=功能码。
//!
//! ASDU 的构造 / 解析复用公共层 [`crate::south::protocol::common::asdu`]
//! （与 IEC104 共用同一套 ASDU 结构）。

use crate::south::bus::FrameBuffer;
use crate::south::protocol::common::asdu::build_interrogation_asdu;
use crate::south::protocol::common::codec::checksum8;
use crate::south::protocol::{CollectorError, CollectorResult};

/// 固定帧起始字节
pub(crate) const FT12_START_FIXED: u8 = 0x10;
/// 可变帧起始字节
pub(crate) const FT12_START_VAR: u8 = 0x68;
/// 帧结束字节
pub(crate) const FT12_END: u8 = 0x16;
/// 单字符帧（ACK）
pub(crate) const FT12_SINGLE_ACK: u8 = 0xE5;

/// 控制域：PRM 位（主站报文置 1）
pub(crate) const C_PRM: u8 = 0x40;
/// 控制域：FCV 位（帧计数有效）
pub(crate) const C_FCV: u8 = 0x10;
/// 控制域：FCB 位（帧计数位，交替 0/1）
pub(crate) const C_FCB: u8 = 0x20;

/// 主站功能码：复位远程链路
pub(crate) const FUNC_RESET_LINK: u8 = 0;
/// 主站功能码：用户数据（总召唤等 ASDU 走此功能码）
pub(crate) const FUNC_USER_DATA: u8 = 3;
/// 主站功能码：请求二级数据（class 2）
pub(crate) const FUNC_REQUEST_CLASS2: u8 = 11;

/// FT1.2 可变帧 ASDU 最大字节数（L 为 1 字节，需容纳 C + A）
const MAX_ASDU_LEN: usize = 253;

/// 解析后的 FT1.2 帧
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Ft12Frame {
    /// 单字符确认帧 `E5`
    Ack,
    /// 固定帧长：控制域 + 链路地址
    Fixed { control: u8, addr: u8 },
    /// 可变帧长：控制域 + 链路地址 + ASDU
    Variable {
        control: u8,
        addr: u8,
        asdu: Vec<u8>,
    },
}

/// 校验和：字节累加 mod 256（与 DLT645/CJT188 共用 `common::codec::checksum8`）
fn checksum(bytes: &[u8]) -> u8 {
    checksum8(bytes)
}

/// 构建固定帧长报文 `10 C A CS 16`
pub(crate) fn build_fixed_frame(control: u8, addr: u8) -> Vec<u8> {
    let cs = checksum(&[control, addr]);
    vec![FT12_START_FIXED, control, addr, cs, FT12_END]
}

/// 构建可变帧长报文 `68 L L 68 C A <ASDU> CS 16`
///
/// ASDU 超长（> [`MAX_ASDU_LEN`]）时返回空 `Vec`（发送端据此判定构造失败）。
pub(crate) fn build_variable_frame(control: u8, addr: u8, asdu: &[u8]) -> Vec<u8> {
    if asdu.len() > MAX_ASDU_LEN {
        tracing::error!(
            "iec101 variable frame ASDU length {} exceeds max {}",
            asdu.len(),
            MAX_ASDU_LEN
        );
        return Vec::new();
    }
    let l = (2 + asdu.len()) as u8; // C + A + ASDU
    let mut body = Vec::with_capacity(2 + asdu.len());
    body.push(control);
    body.push(addr);
    body.extend_from_slice(asdu);
    let cs = checksum(&body);
    let mut frame = Vec::with_capacity(6 + body.len());
    frame.push(FT12_START_VAR);
    frame.push(l);
    frame.push(l);
    frame.push(FT12_START_VAR);
    frame.extend_from_slice(&body);
    frame.push(cs);
    frame.push(FT12_END);
    frame
}

/// 复位远程链路（功能码 0，PRM=1，FCV=0）
pub(crate) fn reset_link(addr: u8) -> Vec<u8> {
    build_fixed_frame(C_PRM | FUNC_RESET_LINK, addr)
}

/// 请求二级数据（功能码 11，PRM=1，FCV=1，FCB 交替）
pub(crate) fn request_class2(addr: u8, fcb: bool) -> Vec<u8> {
    build_fixed_frame(class2_control(fcb), addr)
}

/// 总召唤（用户数据帧，功能码 3，携带总召唤 ASDU）
///
/// `addr_len` 为 ASDU 公共地址字节数（1 或 2，决策表 #28）。
pub(crate) fn interrogation(addr: u8, asdu_addr: u16, fcb: bool, addr_len: u8) -> Vec<u8> {
    let control = C_PRM | C_FCV | FUNC_USER_DATA | if fcb { C_FCB } else { 0 };
    build_variable_frame(
        control,
        addr,
        &build_interrogation_asdu(asdu_addr, addr_len),
    )
}

/// 请求二级数据的控制域字节
fn class2_control(fcb: bool) -> u8 {
    C_PRM | C_FCV | FUNC_REQUEST_CLASS2 | if fcb { C_FCB } else { 0 }
}

/// 解析一帧完整 FT1.2 报文（校验长度与校验和）
pub(crate) fn parse_frame(frame: &[u8]) -> CollectorResult<Ft12Frame> {
    match frame.first().copied() {
        Some(FT12_SINGLE_ACK) => {
            if frame.len() != 1 {
                return Err(CollectorError::InvalidFrame(format!(
                    "iec101 single-char frame must be 1 byte, got {}",
                    frame.len()
                )));
            }
            Ok(Ft12Frame::Ack)
        }
        Some(FT12_START_FIXED) => {
            if frame.len() != 5 {
                return Err(CollectorError::InvalidFrame(format!(
                    "iec101 fixed frame must be 5 bytes, got {}",
                    frame.len()
                )));
            }
            if frame[4] != FT12_END {
                return Err(CollectorError::InvalidFrame(
                    "iec101 fixed frame missing end byte 0x16".into(),
                ));
            }
            if checksum(&frame[1..3]) != frame[3] {
                return Err(CollectorError::ChecksumError);
            }
            Ok(Ft12Frame::Fixed {
                control: frame[1],
                addr: frame[2],
            })
        }
        Some(FT12_START_VAR) => {
            if frame.len() < 4 {
                return Err(CollectorError::InvalidFrame(
                    "iec101 variable frame too short".into(),
                ));
            }
            let l = frame[1] as usize;
            if frame[2] as usize != l {
                return Err(CollectorError::InvalidFrame(
                    "iec101 variable frame length bytes mismatch".into(),
                ));
            }
            if frame[3] != FT12_START_VAR {
                return Err(CollectorError::InvalidFrame(
                    "iec101 variable frame missing second start byte".into(),
                ));
            }
            if frame.len() != 6 + l {
                return Err(CollectorError::InvalidFrame(format!(
                    "iec101 variable frame length mismatch: expect {}, got {}",
                    6 + l,
                    frame.len()
                )));
            }
            if frame[frame.len() - 1] != FT12_END {
                return Err(CollectorError::InvalidFrame(
                    "iec101 variable frame missing end byte 0x16".into(),
                ));
            }
            let cs = checksum(&frame[4..4 + l]);
            if cs != frame[4 + l] {
                return Err(CollectorError::ChecksumError);
            }
            Ok(Ft12Frame::Variable {
                control: frame[4],
                addr: frame[5],
                asdu: frame[6..4 + l].to_vec(),
            })
        }
        _ => Err(CollectorError::InvalidFrame(format!(
            "iec101 unknown frame start byte 0x{:02X}",
            frame.first().copied().unwrap_or(0)
        ))),
    }
}

/// 帧提取器：从 [`FrameBuffer`] 中切出一帧完整 FT1.2 报文。
///
/// 内部循环丢弃前导噪声/非法字节，直到切出**校验和正确**的完整帧；
/// 数据不足以构成完整帧时返回 `None`（等待总线继续收字节）。
pub(crate) fn extract_iec101_frame(
    fb: &mut FrameBuffer,
    _expected_count: Option<u16>,
) -> Option<Vec<u8>> {
    loop {
        let data = fb.peek();
        let first = *data.first()?;
        match first {
            FT12_SINGLE_ACK => {
                fb.pop(1);
                return Some(vec![FT12_SINGLE_ACK]);
            }
            FT12_START_FIXED => {
                if data.len() < 5 {
                    return None;
                }
                let valid = data[4] == FT12_END && checksum(&data[1..3]) == data[3];
                if !valid {
                    fb.pop(1);
                    continue;
                }
                let frame = data[..5].to_vec();
                fb.pop(5);
                return Some(frame);
            }
            FT12_START_VAR => {
                if data.len() < 4 {
                    return None;
                }
                let l = data[1] as usize;
                if l < 2 {
                    // 非法长度，丢弃起始字节继续
                    fb.pop(1);
                    continue;
                }
                let total = 6 + l;
                if data.len() < total {
                    return None;
                }
                let valid = data[2] as usize == l
                    && data[3] == FT12_START_VAR
                    && data[total - 1] == FT12_END
                    && checksum(&data[4..4 + l]) == data[4 + l];
                if !valid {
                    fb.pop(1);
                    continue;
                }
                let frame = data[..total].to_vec();
                fb.pop(total);
                return Some(frame);
            }
            _ => {
                // 前导噪声字节，丢弃
                fb.pop(1);
                continue;
            }
        }
    }
}
