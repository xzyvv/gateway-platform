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
