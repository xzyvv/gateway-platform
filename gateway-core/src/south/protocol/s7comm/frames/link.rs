//! 链路层帧：COTP Connect / Setup Communication / TPKT 读取。
use super::*;

use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;

/// COTP CR 中声明的 TPDU 大小参数（class 0x0A = 1024 字节）。
///
/// ISO 8073 的 TPDU 大小参数取值 0x0A 对应 1024；本端在 `build_cotp_connect`
/// 里原样声明该值，对端 CC 只能协商成不大于它的值。
const COTP_TPDU_SIZE_CLASS: u8 = 0x0A;
/// 由 `COTP_TPDU_SIZE_CLASS` 换算出的 TPDU 上限（字节）。
const COTP_TPDU_SIZE: usize = 1024;

/// TPKT 总长业务上限（S7-R2 / NB-07）：TPKT 头 4 字节 + COTP 声明的 TPDU 上限。
///
/// 长度字段本身是 u16（65535 只是位宽），不能直接当作读取预算——否则一个
/// 异常/恶意的对端声明 65535 就会让本端一次分配约 64KB 缓冲区。取「本端在
/// COTP CR 里已声明的 1024 字节 TPDU」作为业务上限，是最紧且与本端协商口径
/// 一致的值：协商 PDU ≤960 的合法 S7 帧（4 + 3 + 960 = 967）都在此范围内。
pub(crate) const MAX_TPKT_LEN: usize = 4 + COTP_TPDU_SIZE;

/// 构建 COTP Connect (TSAP 寻址)
///
/// S7-200/300/400/1200/1500 共用 dst TSAP 编码：0x01<<8 | (rack<<5 | slot)，
/// 与 Snap7 S7Client::SetConnectionParams 行为一致。frame 总长 22 字节。
pub(crate) fn build_cotp_connect(rack: u8, slot: u8) -> Vec<u8> {
    let src_tsap: u16 = 0x0100;
    let dst_tsap: u16 = 0x0100 | (((rack as u16) & 0x07) << 5) | (slot as u16 & 0x1F);
    let mut frame = Vec::with_capacity(22);
    // TPKT (4)
    frame.extend_from_slice(&[0x03, 0x00, 0x00, 0x16]);
    // COTP CR (17 字节)
    frame.push(0x11); // PDU length (后续 17 字节)
    frame.push(0xE0); // PDU type: CR
    frame.extend_from_slice(&[0x00, 0x00]); // DST reference
    frame.extend_from_slice(&[0x00, 0x00]); // SRC reference
    frame.push(0x00); // Class option
                      // TPDU size parameter (3 字节)
    frame.extend_from_slice(&[0xC0, 0x01, COTP_TPDU_SIZE_CLASS]); // TPDU size = 1024
                                                                  // Src TSAP parameter (4 字节)
    frame.extend_from_slice(&[0xC1, 0x02]);
    frame.extend_from_slice(&src_tsap.to_be_bytes());
    // Dst TSAP parameter (4 字节)
    frame.extend_from_slice(&[0xC2, 0x02]);
    frame.extend_from_slice(&dst_tsap.to_be_bytes());
    frame
}

/// 构建 S7 Setup Communication 请求
pub(crate) fn build_setup_comm() -> Vec<u8> {
    let mut frame = Vec::with_capacity(25);
    frame.extend_from_slice(&[0x03, 0x00, 0x00, 0x19]);
    frame.extend_from_slice(&[0x02, 0xF0, 0x80]);
    frame.push(0x32);
    frame.push(PDU_JOB);
    frame.extend_from_slice(&[0x00, 0x00]);
    frame.extend_from_slice(&[0x00, 0x01]);
    frame.extend_from_slice(&[0x00, 0x08]);
    frame.extend_from_slice(&[0x00, 0x00]);
    frame.push(FUNC_SETUP);
    frame.push(0x01);
    frame.extend_from_slice(&[0x01, 0x00]);
    frame.extend_from_slice(&[0x01, 0x00]);
    frame.extend_from_slice(&[0x01, 0xE0]);
    frame
}

/// 读取 TPKT 帧（4 字节头 + body）
pub(crate) async fn read_tpkt_frame(stream: &mut TcpStream) -> CollectorResult<Vec<u8>> {
    let mut header = [0u8; 4];
    stream
        .read_exact(&mut header)
        .await
        .map_err(|e| CollectorError::IoError(format!("S7 read TPKT header: {}", e)))?;
    if header[0] != 0x03 || header[1] != 0x00 {
        return Err(CollectorError::InvalidFrame(format!(
            "invalid TPKT header: 0x{:02X} 0x{:02X}",
            header[0], header[1]
        )));
    }
    let total_len = u16::from_be_bytes([header[2], header[3]]) as usize;
    if total_len < 4 {
        return Err(CollectorError::InvalidFrame(format!(
            "TPKT length too short: {}",
            total_len
        )));
    }
    // S7-R2 / NB-07：超过本端声明的 TPDU 上限即视为异常帧，不再按对端声明
    // 的长度分配读取预算（避免一个坏帧就让本端分配近 64KB）。
    if total_len > MAX_TPKT_LEN {
        return Err(CollectorError::InvalidFrame(format!(
            "TPKT length exceeds limit: {} > {}",
            total_len, MAX_TPKT_LEN
        )));
    }
    let body_len = total_len - 4;
    let mut body = vec![0u8; body_len];
    stream
        .read_exact(&mut body)
        .await
        .map_err(|e| CollectorError::IoError(format!("S7 read TPKT body: {}", e)))?;
    Ok(body)
}
