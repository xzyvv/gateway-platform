//! 三菱 MC 协议测试：共享报文构造辅助函数与最小从站桩。
//!
//! 各子模块按主题拆分（golden / address_parse / frame_build / codec / e2e /
//! serial_4e / frame_1e / batch），统一以 `use super::*;` 取得本文件的私有辅助项，
//! 以及 mc 模块的类型/常量与 frames 的建帧/解码函数。
//!
//! 桩为独立实现的最小 MC 从站（字节手写，不复用生产 frames 构造响应），
//! 作为交叉验证 oracle，真实走 TcpStream + Collector 生产路径。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::frames::*;
use super::McCollector;
use crate::config::PointConfig;
use crate::engine::model::PointDataType;
use crate::south::protocol::Collector;

mod address_parse;
mod batch;
mod codec;
mod e2e;
mod frame_1e;
mod frame_build;
mod golden;
mod serial_4e;

/// 从站对一次连接的处理方式。
#[derive(Clone)]
enum StubAction {
    /// 回固定响应帧
    Reply(Vec<u8>),
    /// 读取请求后直接断开（模拟无响应/掉线）
    Close,
}

/// 同 `spawn_mc_stub`，额外返回已接受的连接次数（用于验证"批次只建链一次"）。
async fn spawn_mc_stub_counted(actions: Vec<StubAction>) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr").to_string();
    let conn_idx = Arc::new(AtomicUsize::new(0));
    let conn_count = conn_idx.clone();
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => break,
            };
            let idx = conn_idx.fetch_add(1, Ordering::SeqCst);
            let action = actions
                .get(idx)
                .cloned()
                .unwrap_or_else(|| actions.last().cloned().unwrap_or(StubAction::Close));
            tokio::spawn(async move {
                match action {
                    StubAction::Reply(bytes) => {
                        // 单连接内循环处理，模拟 MC 从站持续服务同一连接
                        loop {
                            let mut head = [0u8; 9];
                            if sock.read_exact(&mut head).await.is_err() {
                                return;
                            }
                            let len = u16::from_le_bytes([head[7], head[8]]) as usize;
                            let mut body = vec![0u8; len];
                            if sock.read_exact(&mut body).await.is_err() {
                                return;
                            }
                            if sock.write_all(&bytes).await.is_err() {
                                return;
                            }
                            let _ = sock.flush().await;
                        }
                    }
                    StubAction::Close => {
                        // 读一次请求后直接断开（模拟无响应/掉线）
                        let mut head = [0u8; 9];
                        if sock.read_exact(&mut head).await.is_err() {
                            return;
                        }
                        let len = u16::from_le_bytes([head[7], head[8]]) as usize;
                        let mut body = vec![0u8; len];
                        let _ = sock.read_exact(&mut body).await;
                        drop(sock);
                    }
                }
            });
        }
    });
    (addr, conn_count)
}

/// 手工构造 3E MC 响应帧（独立于生产构造逻辑）。
fn mc_response(end_code: u16, data: &[u8]) -> Vec<u8> {
    let body_len = 2 + data.len();
    let mut f = vec![0xD0, 0x00, 0x00, 0xFF, 0xFF, 0x03, 0x00];
    f.extend_from_slice(&(body_len as u16).to_le_bytes());
    f.extend_from_slice(&end_code.to_le_bytes());
    f.extend_from_slice(data);
    f
}

fn point(id: &str, address: &str, data_type: &str) -> PointConfig {
    PointConfig {
        id: id.to_string(),
        bus: "mc-bus".to_string(),
        protocol: "mc".to_string(),
        address: address.to_string(),
        data_type: Some(data_type.to_string()),
        interval: 1,
        ..Default::default()
    }
}

/// 1E 从站桩：按功能码分派。字读回显「每字 = 首地址 + 序号」，位读回全 1，
/// 字写/位写回状态 0。请求无长度字段，长度由功能码与点数推导。
async fn spawn_mc_1e_stub() -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr").to_string();
    let reqs = Arc::new(AtomicUsize::new(0));
    let reqs_srv = reqs.clone();
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => break,
            };
            let reqs = reqs_srv.clone();
            tokio::spawn(async move {
                loop {
                    // 1E 请求固定部分：功能码(1) PLC号(1) 定时器(2) 地址(4) 软元件码(2) 点数(2) = 12
                    let mut head = [0u8; 12];
                    if sock.read_exact(&mut head).await.is_err() {
                        return;
                    }
                    let func = head[0];
                    let start = u32::from_le_bytes([head[4], head[5], head[6], head[7]]);
                    let points = u16::from_le_bytes([head[10], head[11]]) as u32;
                    reqs.fetch_add(1, Ordering::SeqCst);
                    let resp = match func {
                        0x01 => {
                            // 字读：状态 0 + 每字回显 首地址+序号
                            let mut d = vec![0x81u8, 0x00];
                            for i in 0..points {
                                d.extend_from_slice(&((start + i) as u16).to_le_bytes());
                            }
                            d
                        }
                        0x00 => {
                            // 位读：状态 0 + ceil(points/2) 字节，全 1 位
                            let n = (points as usize).div_ceil(2);
                            let mut d = vec![0x80u8, 0x00];
                            d.extend(vec![0x11u8; n]);
                            d
                        }
                        0x03 => {
                            // 字写：先吞掉写入数据（points*2 字节）
                            let mut w = vec![0u8; points as usize * 2];
                            if sock.read_exact(&mut w).await.is_err() {
                                return;
                            }
                            vec![0x83, 0x00]
                        }
                        0x02 => {
                            // 位写：吞掉 1 字节数据
                            let mut w = [0u8; 1];
                            if sock.read_exact(&mut w).await.is_err() {
                                return;
                            }
                            vec![0x82, 0x00]
                        }
                        _ => return,
                    };
                    if sock.write_all(&resp).await.is_err() {
                        return;
                    }
                    let _ = sock.flush().await;
                }
            });
        }
    });
    (addr, reqs)
}
