//! S7Comm 协议测试：共享的报文构造辅助函数与最小从站桩。
//!
//! 各子模块按主题拆分（golden / address_parse / codec / write_codec /
//! multi_read / multi_write / szl / e2e / regression），统一以 `use super::*;`
//! 取得本文件的私有辅助项，以及 s7comm 的类型/常量。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;
use crate::config::PointConfig;

mod address_parse;
mod codec;
mod e2e;
mod golden;
mod multi_read;
mod multi_write;
mod regression;
mod szl;
mod write_codec;

/// 从站对每个连接的处理方式。
#[derive(Clone)]
enum S7StubAction {
    /// 完成握手后正常应答所有读请求（同一连接持续服务）
    Serve,
    /// 完成握手后立即关闭连接（模拟掉线）
    CloseAfterHandshake,
}

/// 读取一个 TPKT 帧的 body（不含 4 字节头）；EOF/错误返回 None
async fn read_tpkt_body(sock: &mut tokio::net::TcpStream) -> Option<Vec<u8>> {
    let mut header = [0u8; 4];
    sock.read_exact(&mut header).await.ok()?;
    if header[0] != 0x03 || header[1] != 0x00 {
        return None;
    }
    let total = u16::from_be_bytes([header[2], header[3]]) as usize;
    if total < 4 {
        return None;
    }
    let mut body = vec![0u8; total - 4];
    sock.read_exact(&mut body).await.ok()?;
    Some(body)
}

/// 加 TPKT 头后写出；失败返回 false
async fn write_tpkt(sock: &mut tokio::net::TcpStream, body: &[u8]) -> bool {
    let total = (body.len() + 4) as u16;
    let mut frame = vec![0x03, 0x00, (total >> 8) as u8, total as u8];
    frame.extend_from_slice(body);
    sock.write_all(&frame).await.is_ok()
}

/// 手工构造 COTP Connect Confirm（0xD0）
fn s7_cotp_cc_body() -> Vec<u8> {
    vec![0x11, 0xD0, 0x00, 0x01, 0x00, 0x01, 0x00]
}

/// 手工构造 S7 Setup Communication Ack（协商 PDU size = 480）
fn s7_setup_ack_body() -> Vec<u8> {
    // COTP DT(3) + S7 header(12 = 10 基础 + Error class/code) + parameter(8)
    let mut b = vec![
        0x02, 0xF0, 0x80, 0x32, PDU_ACK, 0x00, 0x00, 0x00, 0x01, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00,
    ];
    b.extend_from_slice(&[FUNC_SETUP, 0x01, 0x01, 0x00, 0x01, 0x00, 0x01, 0xE0]);
    b
}

/// 多 item Read Var 响应：每个 item 一段 return_code + ts + len + value（决策表 #13 桩）
fn s7_read_ack_body_multi(value: &[u8], transport_size: u8, item_count: usize) -> Vec<u8> {
    let per_item = 4 + value.len();
    let data_len = per_item * item_count;
    let mut b = vec![0x02, 0xF0, 0x80];
    b.push(0x32);
    b.push(PDU_ACK);
    b.extend_from_slice(&[0x00, 0x00]); // Reserved
    b.extend_from_slice(&[0x00, 0x01]); // PDU reference
    b.extend_from_slice(&(2u16).to_be_bytes()); // Parameter length
    b.extend_from_slice(&(data_len as u16).to_be_bytes()); // Data length
    b.extend_from_slice(&[0x00, 0x00]); // Error class + Error code（Ack-Data 头共 12 字节）
    b.push(FUNC_READ);
    b.push(item_count as u8); // item count
    for _ in 0..item_count {
        b.push(RET_OK); // return code
        b.push(transport_size);
        b.extend_from_slice(&(value.len() as u16).to_be_bytes());
        b.extend_from_slice(value);
    }
    b
}

/// 手工构造 S7 多 item Write Var Ack（成功：每项 data 段 return code = 0xFF）
fn s7_write_ack_body_multi(item_count: usize) -> Vec<u8> {
    // COTP DT(3) + S7 header(12 = 10 基础 + Error class/code) + parameter(2) + data(N)
    let mut b = vec![0x02, 0xF0, 0x80];
    b.push(0x32);
    b.push(PDU_ACK);
    b.extend_from_slice(&[0x00, 0x00]); // Reserved
    b.extend_from_slice(&[0x00, 0x01]); // PDU reference
    b.extend_from_slice(&(2u16).to_be_bytes()); // Parameter length
    b.extend_from_slice(&(item_count as u16).to_be_bytes()); // Data length
    b.extend_from_slice(&[0x00, 0x00]); // Error class + Error code
    b.push(FUNC_WRITE);
    b.push(item_count as u8); // item count
    b.extend(std::iter::repeat_n(RET_OK, item_count));
    b
}

/// 手工构造 S7 Write Var Ack（成功：单 Item，data 段 1 字节 return code = 0xFF）
fn s7_write_ack_body() -> Vec<u8> {
    s7_write_ack_body_multi(1)
}

/// 手工构造 S7 Read SZL Ack（Userdata，单条 34 字节组件标识记录）
///
/// 记录内含可打印订货号（6ES7 315-2AG10-0AB0），供 diag 文本提取断言使用。
fn s7_szl_ack_body() -> Vec<u8> {
    let mut record = b"6ES7 315-2AG10-0AB0".to_vec();
    record.resize(34, 0x00);
    let list_len: u16 = 8 + record.len() as u16; // 部分列表长度（含 8 字节 SZL 头）

    let mut data = Vec::new();
    data.push(RET_OK); // return code
    data.push(0x09); // transport size
    data.extend_from_slice(&(list_len + 8).to_be_bytes()); // 此后数据长度 = 8 + list_len
    data.extend_from_slice(&0x0011u16.to_be_bytes()); // SZL-ID
    data.extend_from_slice(&1u16.to_be_bytes()); // SZL-Index
    data.extend_from_slice(&list_len.to_be_bytes()); // 部分列表长度
    data.extend_from_slice(&1u16.to_be_bytes()); // 记录条数
    data.extend_from_slice(&record);

    // COTP DT(3) + S7 头(10) + Parameter(8) + Data
    // 注：Userdata 报文头与请求同构（10 字节，无 Error class/code 扩展），
    // 不同于 Ack-Data 的 12 字节；现场读数异常时优先核对此处。
    let mut b = vec![0x02, 0xF0, 0x80];
    b.push(0x32);
    b.push(PDU_USERDATA);
    b.extend_from_slice(&[0x00, 0x00]); // Reserved
    b.extend_from_slice(&[0x00, 0x01]); // PDU reference
    b.extend_from_slice(&8u16.to_be_bytes()); // Parameter length
    b.extend_from_slice(&(data.len() as u16).to_be_bytes()); // Data length
    b.extend_from_slice(&SZL_PARAM); // Parameter（8 字节固定）
    b.extend_from_slice(&data);
    b
}

/// 启动极简 S7 从站桩，返回监听地址。
/// 每连接：COTP CR→CC、Setup→Ack，之后按 `actions`（按连接序取用，耗尽用最后一项）处理。
async fn spawn_s7_stub(actions: Vec<S7StubAction>, value: Vec<u8>, transport_size: u8) -> String {
    spawn_s7_stub_counted(actions, value, transport_size)
        .await
        .0
}

/// 同 `spawn_s7_stub`，额外返回（已接受连接数, 已收到写请求数）。
/// 连接数用于验证"批次只建链一次"，写请求数用于验证"批量写合并为一次事务"。
async fn spawn_s7_stub_counted(
    actions: Vec<S7StubAction>,
    value: Vec<u8>,
    transport_size: u8,
) -> (String, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr").to_string();
    let conn_idx = Arc::new(AtomicUsize::new(0));
    let conn_count = conn_idx.clone();
    let write_count = Arc::new(AtomicUsize::new(0));
    let write_hits = write_count.clone();
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => break,
            };
            let idx = conn_idx.fetch_add(1, Ordering::SeqCst);
            let action = actions.get(idx).cloned().unwrap_or_else(|| {
                actions
                    .last()
                    .cloned()
                    .unwrap_or(S7StubAction::CloseAfterHandshake)
            });
            let value = value.clone();
            let write_hits = write_hits.clone();
            tokio::spawn(async move {
                // 握手：COTP CR → CC
                if read_tpkt_body(&mut sock).await.is_none() {
                    return;
                }
                if !write_tpkt(&mut sock, &s7_cotp_cc_body()).await {
                    return;
                }
                // 握手：Setup → Ack
                if read_tpkt_body(&mut sock).await.is_none() {
                    return;
                }
                if !write_tpkt(&mut sock, &s7_setup_ack_body()).await {
                    return;
                }
                match action {
                    S7StubAction::Serve => loop {
                        let body = match read_tpkt_body(&mut sock).await {
                            Some(b) => b,
                            None => return,
                        };
                        // 请求功能码位于 body[13]（COTP 3 + S7 header 10）；
                        // PDU 类型位于 body[4]（COTP 3 + S7 协议 ID 1 之后）
                        let function = body.get(13).copied().unwrap_or(FUNC_READ);
                        let ack = if body.get(4).copied() == Some(PDU_USERDATA) {
                            // Userdata（Read SZL）→ 返回组件标识
                            s7_szl_ack_body()
                        } else if function == FUNC_WRITE {
                            // 按请求 item_count 逐项回 RET_OK（决策表 #13：多 item 写桩）
                            let item_count = body.get(14).copied().unwrap_or(1) as usize;
                            write_hits.fetch_add(1, Ordering::SeqCst);
                            s7_write_ack_body_multi(item_count)
                        } else {
                            // 多 item Read Var：按请求 item_count 重复回应同一 value（决策表 #13 桩）
                            let item_count = body.get(14).copied().unwrap_or(1) as usize;
                            s7_read_ack_body_multi(&value, transport_size, item_count)
                        };
                        if !write_tpkt(&mut sock, &ack).await {
                            return;
                        }
                    },
                    S7StubAction::CloseAfterHandshake => drop(sock),
                }
            });
        }
    });
    (addr, conn_count, write_count)
}

fn s7_point(id: &str, address: &str, data_type: &str) -> PointConfig {
    PointConfig {
        id: id.to_string(),
        bus: "s7-bus".to_string(),
        protocol: "s7comm".to_string(),
        address: address.to_string(),
        data_type: Some(data_type.to_string()),
        interval: 1,
        ..Default::default()
    }
}

/// Setup Communication Ack 变体：协商 PDU 大小由参数决定（默认变体固定 480）。
fn s7_setup_ack_body_pdu(pdu: u16) -> Vec<u8> {
    let mut b = vec![
        0x02, 0xF0, 0x80, 0x32, PDU_ACK, 0x00, 0x00, 0x00, 0x01, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00,
    ];
    b.extend_from_slice(&[FUNC_SETUP, 0x01, 0x01, 0x00, 0x01, 0x00]);
    b.extend_from_slice(&pdu.to_be_bytes());
    b
}

/// 多 item Read Var 响应：每个 item 返回一个**互不相同**的 4 字节值。
fn s7_read_ack_body_values(values: &[u32], transport_size: u8) -> Vec<u8> {
    let value_len = 4usize;
    let data_len = (4 + value_len) * values.len();
    let mut b = vec![0x02, 0xF0, 0x80, 0x32, PDU_ACK, 0x00, 0x00, 0x00, 0x01];
    b.extend_from_slice(&(2u16).to_be_bytes()); // Parameter length
    b.extend_from_slice(&(data_len as u16).to_be_bytes()); // Data length
    b.extend_from_slice(&[0x00, 0x00]); // Error class + Error code（Ack-Data 头共 12 字节）
    b.push(FUNC_READ);
    b.push(values.len() as u8);
    for v in values {
        b.push(RET_OK);
        b.push(transport_size);
        b.extend_from_slice(&(value_len as u16).to_be_bytes());
        b.extend_from_slice(&v.to_be_bytes());
    }
    b
}

/// 启动协商 PDU=240 的 S7 从站桩：记录每个 Read Var 请求的 item 数，
/// 并按**全局 item 序**为每个 item 返回唯一值 `1000 + index`，用于验证分片后
/// 是否仍按原下标正确回填。
async fn spawn_s7_stub_pdu240() -> (String, Arc<std::sync::Mutex<Vec<usize>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr").to_string();
    let req_items: Arc<std::sync::Mutex<Vec<usize>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let req_items_srv = req_items.clone();
    tokio::spawn(async move {
        let (mut sock, _) = match listener.accept().await {
            Ok(v) => v,
            Err(_) => return,
        };
        let served = Arc::new(AtomicUsize::new(0));
        // 握手的 Setup 响应协商 PDU=240（对应 max_items=(240-14)/12=18）
        if read_tpkt_body(&mut sock).await.is_none() {
            return;
        }
        if !write_tpkt(&mut sock, &s7_cotp_cc_body()).await {
            return;
        }
        if read_tpkt_body(&mut sock).await.is_none() {
            return;
        }
        if !write_tpkt(&mut sock, &s7_setup_ack_body_pdu(240)).await {
            return;
        }
        loop {
            let body = match read_tpkt_body(&mut sock).await {
                Some(b) => b,
                None => return,
            };
            if body.get(13).copied() != Some(FUNC_READ) {
                return;
            }
            let item_count = body.get(14).copied().unwrap_or(0) as usize;
            req_items_srv.lock().unwrap().push(item_count);
            let start = served.fetch_add(item_count, Ordering::SeqCst);
            let values: Vec<u32> = (0..item_count).map(|i| 1000 + (start + i) as u32).collect();
            let ack = s7_read_ack_body_values(&values, TS_BYTE);
            if !write_tpkt(&mut sock, &ack).await {
                return;
            }
        }
    });
    (addr, req_items)
}
