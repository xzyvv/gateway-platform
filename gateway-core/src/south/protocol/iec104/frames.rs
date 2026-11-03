//! IEC 60870-5-104 帧处理（构建 / 解析 / 解码）
//!
//! 电力系统远动协议，TCP 传输（默认端口 2404）。
//! 实现最小子集：STARTDT 握手 + 总召唤 + 解析浮点量测值。

use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;

use crate::south::protocol::{CollectorError, CollectorResult};

// ASDU 构造 / 解析已上提到公共层 `common::asdu`，供 iec104 与 iec101 复用。
// 生产路径需要的项在此 re-export；仅测试使用的解析助手在 cfg(test) 下 re-export，
// 避免非测试构建产生 unused_imports 告警。
pub(crate) use crate::south::protocol::common::asdu::{
    build_clock_sync_asdu, build_command_asdu, build_interrogation_asdu, encode_command_object,
    is_interrogation_termination, parse_asdu, parse_command_confirmation,
    COT_ACTIVATION_CONFIRMATION, COT_ACTIVATION_TERMINATION, COT_UNKNOWN_CA, COT_UNKNOWN_COT,
    COT_UNKNOWN_IOA, COT_UNKNOWN_TYPE_ID, TYPE_C_DC_NA_1, TYPE_C_RC_NA_1, TYPE_C_SC_NA_1,
};
#[cfg(test)]
pub(crate) use crate::south::protocol::common::asdu::{
    ca_len, decode_cp56time2a_millis, encode_ca, encode_cp56time2a, parse_float, parse_normalized,
    TYPE_C_CS_NA_1, TYPE_M_ME_TF_1,
};

/// IEC 104 APFI 起始字节
pub(crate) const IEC104_START: u8 = 0x68;

/// ASDU 最大长度（IEC 60870-5-104 §6.1：255 - 起始 1 - 长度 1 - 控制域 4）
pub(crate) const MAX_ASDU_LEN: usize = 249;

/// U-frame 控制域：STARTDT act
pub(crate) const STARTDT_ACT: [u8; 4] = [0x07, 0x00, 0x00, 0x00];

/// 构建 I-frame
///
/// 控制域 4 字节：低 2 位为 0（I 格式），字节 0/1 = 发送序号 N(S)，
/// 字节 2/3 = 接收序号 N(R)（对已收 I-frame 的确认，捎带在下一个 I-frame 上）。
///
/// 修复（缓存失效）：原实现把 N(S)/N(R) 硬编码为 0，只在"整个进程生命周期只
/// 发一次总召唤"的旧语义下侥幸可用；改为每轮总召唤刷新数据后，重复发同一个
/// N(S)=0、N(R)=0 的 I-frame 会被 server 判为序号错乱/未确认，server 发送窗口
/// （k=12）也会因收不到确认而打满停发。故必须由调用方维护并传入两个计数器。
///
/// A3 修复（APCI 长度溢出）：旧实现 `(asdu.len() + 4) as u8`——asdu.len() > 252 时
/// 强转 u8 绕回小值（如 253→1），对端 APCI length 字段判非法整帧拒收。
///
/// C-4 修复（超长 ASDU 被当成发送成功）：原实现在超长时 `return Vec::new()`，
/// 调用方 `stream.write_all(&[])` 得到 `Ok(0)` 并判定"发送成功"，报文实际一字节
/// 都没上线；日志只有一条 warn，且无法关联到具体事务。现场表现为"大点表总召唤 /
/// 突发 SOE 时部分点位永不上报、且无任何错误计数"，只能抓包才能发现。
/// 现改为返回 `Err(CollectorError::InvalidFrame(..))`，由调用方显式失败并计入错误。
///
/// 上限同时按规范收到 249：APCI 长度域本身能容纳 251（`251 + 4 = 255` 不溢出），
/// 但 IEC 60870-5-104 §6.1 规定 ASDU ≤ 255 - 起始(1) - 长度(1) - 控制域(4) = 249，
/// 超出即为非法帧，不该发出去让对端拒收。
pub(crate) fn build_iframe(seq_send: u16, seq_recv: u16, asdu: &[u8]) -> CollectorResult<Vec<u8>> {
    if asdu.len() > MAX_ASDU_LEN {
        return Err(CollectorError::InvalidFrame(format!(
            "asdu too long ({} bytes > {})",
            asdu.len(),
            MAX_ASDU_LEN
        )));
    }
    // A4：编码前统一收敛到 15-bit 半窗口。N(S)/N(R) 都是模 32768 计数器，
    // 计数器若涨到 >= 32768 会使 cf[1]/cf[3] 的 bit7（sign 位）置 1，被对端
    // 误判为 S/U 帧。这里作为唯一编码出口做兜底掩码，与调用方计数逻辑解耦。
    let seq_send = seq_send & 0x7FFF;
    let seq_recv = seq_recv & 0x7FFF;
    let mut cf = [0u8; 4];
    cf[0] = (seq_send << 1) as u8;
    cf[1] = (seq_send >> 7) as u8;
    cf[2] = (seq_recv << 1) as u8;
    cf[3] = (seq_recv >> 7) as u8;
    let mut frame = vec![IEC104_START, (asdu.len() + 4) as u8];
    frame.extend_from_slice(&cf);
    frame.extend_from_slice(asdu);
    Ok(frame)
}

/// 构建 S-frame（确认帧）
///
/// 控制域：cf[0] = 0x01（bit0=1, bit1=0 → S 格式），cf[1] = 0x00，
/// cf[2]/cf[3] = 接收序号 N(R)。用于在收到若干 I-frame 后立即释放 server 发送窗口，
/// 不等下一个 I-frame 捎带确认。
pub(crate) fn build_sframe(seq_recv: u16) -> Vec<u8> {
    // A4 防御：build_sframe 也强制 15-bit 半窗口，与 I-frame N(R) 语义一致。
    // seq_recv >= 32768 时 cf[3] bit7 = 1，触发对端 S-frame 误判为 I/U 帧。
    // 注意：N(R)=0 是合法值（表明确认到序号 0 之前），不能像 N(S) 那样跳过 0。
    let seq_recv = seq_recv & 0x7FFF;
    vec![
        IEC104_START,
        0x04,
        0x01,
        0x00,
        (seq_recv << 1) as u8,
        (seq_recv >> 7) as u8,
    ]
}

/// 构建 U-frame
pub(crate) fn build_uframe(cf: [u8; 4]) -> Vec<u8> {
    vec![IEC104_START, 0x04, cf[0], cf[1], cf[2], cf[3]]
}

/// 读取一帧，返回帧数据（不含起始字节和长度）
pub(crate) async fn read_frame(stream: &mut TcpStream) -> CollectorResult<Vec<u8>> {
    let mut start = [0u8; 1];
    stream
        .read_exact(&mut start)
        .await
        .map_err(|e| CollectorError::IoError(format!("read start: {}", e)))?;
    if start[0] != IEC104_START {
        return Err(CollectorError::InvalidFrame(format!(
            "invalid start byte 0x{:02X}",
            start[0]
        )));
    }
    let mut len_buf = [0u8; 1];
    stream
        .read_exact(&mut len_buf)
        .await
        .map_err(|e| CollectorError::IoError(format!("read len: {}", e)))?;
    let len = len_buf[0] as usize;
    // 修复(C-4)：长度域必须落在 [4, 253]。
    //
    // 控制域固定 4 字节，IEC 60870-5-104 单 APDU 上限 255 字节（含起始字节与长度
    // 字节），故长度域合法范围是 4..=253（MAX_ASDU_LEN 249 + 控制域 4）。
    //
    // 修复前直接 `vec![0u8; len]`：
    // - len = 0（或 1..3）时返回 Ok(空 body)，**看起来像一次成功的收帧**。
    //   各调用点虽有 `body.len() < 4 { continue }` 兜底，但那只是"跳过并继续读"，
    //   于是对端只要持续发 `68 00`，主站就会在 deadline 内空转忙轮询；
    //   更严重的是 STARTDT 握手处（read_frame(...)? 后仅判 len>=4）会把这种
    //   垃圾帧当成"对端已应答"，在**链路实际未建立**的情况下置 connected=true。
    // - len 过大时同理，会先分配再阻塞读满，白占一个 t1 周期。
    // 现在 fail-closed：帧长非法立即返回 InvalidFrame，由调用方按断链处理。
    const MIN_APDU_LEN: usize = 4;
    const MAX_APDU_LEN: usize = MAX_ASDU_LEN + 4;
    if !(MIN_APDU_LEN..=MAX_APDU_LEN).contains(&len) {
        return Err(CollectorError::InvalidFrame(format!(
            "invalid APDU length {} (expected {}..={})",
            len, MIN_APDU_LEN, MAX_APDU_LEN
        )));
    }
    let mut body = vec![0u8; len];
    stream
        .read_exact(&mut body)
        .await
        .map_err(|e| CollectorError::IoError(format!("read body: {}", e)))?;
    Ok(body)
}
