//! Modbus TCP MBAP 帧编解码 + reader_loop（从 transaction.rs 拆出）
//!
//! 本模块独立于 TransactionPool 内部状态，只依赖 pub API（deliver/abort_all）。
//! - build_mbap_frame: 构造 MBAP + PDU 帧（pub(crate)，tests 也会用）
//! - read_mbap_response: 解析响应 MBAP 头 + PDU（私有，仅 reader_loop 用）
//! - reader_loop: 独立任务循环读响应并路由到 pool（pub(super)，TransactionPool::with_stream 调用）

use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::sync::Notify;

use super::{ModbusRequest, TransactionError, TransactionPool};

/// 构造 Modbus TCP MBAP + PDU 帧。
/// MBAP: tx_id(2) + protocol_id(2)=0 + length(2) + unit_id(1) + function_code(1) + pdu
/// length = 1 (unit_id) + 1 (function_code) + pdu.len()
pub(crate) fn build_mbap_frame(
    tx_id: u16,
    req: &ModbusRequest,
) -> Result<Vec<u8>, TransactionError> {
    let pdu_len = req.pdu.len();
    // Bug 41: check before cast to prevent silent truncation
    if pdu_len > 253 {
        return Err(TransactionError::Malformed(
            "pdu exceeds Modbus limit (253 bytes)".into(),
        ));
    }
    let length = 2u16
        .checked_add(pdu_len as u16)
        .ok_or_else(|| TransactionError::Malformed("pdu too long".into()))?;
    let mut buf = Vec::with_capacity(7 + 1 + pdu_len);
    buf.extend_from_slice(&tx_id.to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes()); // protocol_id = 0
    buf.extend_from_slice(&length.to_be_bytes());
    buf.push(req.slave_id);
    buf.push(req.function_code);
    buf.extend_from_slice(&req.pdu);
    Ok(buf)
}

/// MBAP length 字段上界。length = unit_id(1) + function_code(1) + pdu，
/// 与 `build_mbap_frame` 的发送侧上限（pdu ≤ 253，故 length ≤ 255）保持同源口径。
/// Modbus TCP 规范本身也把 ADU 限制在 260 字节内，65534 字节的帧不可能是合法响应。
const MAX_MBAP_LENGTH: u16 = 255;

/// 解析 Modbus TCP 响应：读 MBAP 头 + PDU。
/// 返回 (tx_id, pdu)，其中 pdu = function_code + data（即 unit_id 之后的所有字节）。
async fn read_mbap_response<R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> Result<(u16, Vec<u8>), TransactionError> {
    let mut header = [0u8; 7];
    reader
        .read_exact(&mut header)
        .await
        .map_err(TransactionError::Io)?;
    let tx_id = u16::from_be_bytes([header[0], header[1]]);
    let protocol_id = u16::from_be_bytes([header[2], header[3]]);
    if protocol_id != 0 {
        return Err(TransactionError::Malformed(format!(
            "protocol_id != 0: {}",
            protocol_id
        )));
    }
    let length = u16::from_be_bytes([header[4], header[5]]);
    if length < 1 {
        return Err(TransactionError::Malformed(format!(
            "length < 1: {}",
            length
        )));
    }
    // 上界必须在分配前校验：length 是 u16，旧实现可因一个畸形头分配 64KB 并
    // read_exact 等待 65534 字节，直到 30s reader 超时才作废整条 TCP 总线上的
    // 全部在途事务。超限按 Malformed 处理，reader_loop 会立即 abort_all 断链。
    if length > MAX_MBAP_LENGTH {
        return Err(TransactionError::Malformed(format!(
            "length {} exceeds Modbus TCP limit {}",
            length, MAX_MBAP_LENGTH
        )));
    }
    let _unit_id = header[6];
    let pdu_len = (length as usize).saturating_sub(1);
    let mut pdu = vec![0u8; pdu_len];
    if pdu_len > 0 {
        reader
            .read_exact(&mut pdu)
            .await
            .map_err(TransactionError::Io)?;
    }
    Ok((tx_id, pdu))
}

/// Reader loop read timeout: prevents partial-header server hang from blocking forever
const READER_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// #8: reader loop——独立任务循环读响应并路由。
/// - 正常：读 MBAP 头 + PDU → pool.deliver(tx_id, pdu)
/// - 读到 EOF/错误：pool.abort_all(Disconnected) 并退出
/// - shutdown notify：退出
/// - Bug 40: read timeout prevents server sending partial header from hanging reader
pub(super) async fn reader_loop(
    mut reader: tokio::net::tcp::OwnedReadHalf,
    pool: Arc<TransactionPool>,
    shutdown: Arc<Notify>,
) {
    loop {
        tokio::select! {
            biased;
            _ = shutdown.notified() => {
                break;
            }
            res = tokio::time::timeout(READER_READ_TIMEOUT, read_mbap_response(&mut reader)) => {
                match res {
                    Ok(Ok((tx_id, pdu))) => {
                        // 路由到 waiter；若 tx_id 不在 active（超时已移除/晚到），静默丢弃
                        pool.deliver(tx_id, pdu).await;
                    }
                    Ok(Err(_)) => {
                        // 读失败通常是连接断开；Malformed 则连接可疑，统一中断
                        pool.abort_all().await;
                        break;
                    }
                    Err(_) => {
                        // Bug 40: read timeout — server sent partial header and hung
                        tracing::warn!(
                            "modbus reader_loop: read timeout after {}s, aborting connection",
                            READER_READ_TIMEOUT.as_secs()
                        );
                        pool.abort_all().await;
                        break;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{read_mbap_response, MAX_MBAP_LENGTH};
    use crate::south::protocol::modbus::transaction::TransactionError;

    fn header_with_length(tx_id: u16, length: u16) -> Vec<u8> {
        let mut buf = Vec::with_capacity(7);
        buf.extend_from_slice(&tx_id.to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes());
        buf.extend_from_slice(&length.to_be_bytes());
        buf.push(1); // unit_id
        buf
    }

    /// 畸形头声称 65534 字节 PDU 却只发头：必须在分配/等待 body 之前判 Malformed。
    /// 旧实现会走到 read_exact 并因 EOF 返回 Io，或真有后续数据时挂到 30s 超时。
    #[tokio::test]
    async fn oversized_mbap_length_is_rejected_before_reading_body() {
        let bytes = header_with_length(0x1234, u16::MAX);
        let mut reader: &[u8] = &bytes;
        let started = std::time::Instant::now();

        match read_mbap_response(&mut reader).await {
            Err(TransactionError::Malformed(msg)) => assert!(msg.contains("exceeds"), "{msg}"),
            other => panic!("expected Malformed for oversized length, got {other:?}"),
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "超长帧必须立即拒绝，不得等待 body 填满"
        );
    }

    #[tokio::test]
    async fn mbap_length_one_over_limit_is_rejected() {
        let mut bytes = header_with_length(1, MAX_MBAP_LENGTH + 1);
        bytes.resize(7 + (MAX_MBAP_LENGTH + 1) as usize, 0);
        let mut reader: &[u8] = &bytes;

        match read_mbap_response(&mut reader).await {
            Err(TransactionError::Malformed(msg)) => assert!(msg.contains("exceeds"), "{msg}"),
            other => panic!("expected Malformed for length at limit + 1, got {other:?}"),
        }
    }

    /// 边界内最大帧（PDU = function_code 1 + data 253）仍须正常解析，防止误杀合法大帧。
    #[tokio::test]
    async fn maximum_mbap_length_is_accepted() {
        let tx_id = 0x1234u16;
        let body_len = (MAX_MBAP_LENGTH - 1) as usize;
        let mut bytes = header_with_length(tx_id, MAX_MBAP_LENGTH);
        bytes.push(3); // function code
        bytes.resize(7 + body_len, 0xAB);
        let mut reader: &[u8] = &bytes;

        let (got_tx_id, pdu) = match read_mbap_response(&mut reader).await {
            Ok(v) => v,
            Err(e) => panic!("maximum legal frame must parse, got {e:?}"),
        };
        assert_eq!(got_tx_id, tx_id);
        assert_eq!(pdu.len(), body_len);
        assert_eq!(pdu[0], 3);
        assert_eq!(pdu[body_len - 1], 0xAB);
    }
}
