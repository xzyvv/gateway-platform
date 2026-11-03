//! IEC 104 协议测试：共享报文构造辅助函数与最小从站桩。
//!
//! 各子模块按主题拆分（golden / codec / keepalive / time_tag / clock_sync /
//! command / ca_len / regression），统一以 `use super::*;` 取得本文件的私有
//! 辅助项，以及 iec104 的类型与常量。

use super::frames::read_frame;
use super::frames::{
    build_clock_sync_asdu, build_command_asdu, build_iframe, build_interrogation_asdu,
    build_sframe, build_uframe, ca_len, decode_cp56time2a_millis, encode_ca, encode_command_object,
    encode_cp56time2a, parse_asdu, parse_command_confirmation, parse_float, parse_normalized,
    COT_ACTIVATION_CONFIRMATION, COT_ACTIVATION_TERMINATION, COT_UNKNOWN_CA, COT_UNKNOWN_COT,
    COT_UNKNOWN_IOA, COT_UNKNOWN_TYPE_ID, STARTDT_ACT, TYPE_C_CS_NA_1, TYPE_C_DC_NA_1,
    TYPE_C_RC_NA_1, TYPE_C_SC_NA_1, TYPE_M_ME_TF_1,
};
use super::*;
use crate::south::protocol::Collector;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

mod ca_len;
mod clock_sync;
mod codec;
mod command;
mod diag;
mod golden;
mod keepalive;
mod regression;
mod time_tag;

/// 从 I/S-frame 控制域还原 N(S) / N(R)
/// IEC 104：值 = (cf[低] >> 1) | (cf[高] << 7)
fn decode_seq(cf_low: u8, cf_high: u8) -> u16 {
    ((cf_low >> 1) as u16) | ((cf_high as u16) << 7)
}

/// 假 IEC104 从站：STARTDT 握手 + 回一帧对时确认；随后阻塞读，读到 EOF / 出错
/// 即表示「主站已释放 TCP」，通过 `closed_tx` 通知测试。
async fn fake_iec104_slave(
    listener: tokio::net::TcpListener,
    closed_tx: tokio::sync::oneshot::Sender<()>,
) {
    let Ok((mut stream, _)) = listener.accept().await else {
        return;
    };

    // 1) STARTDT act → STARTDT con（connect 的握手判据是 resp[0] == 0x0B）
    if read_frame(&mut stream).await.is_err() {
        return;
    }
    if stream
        .write_all(&build_uframe([0x0B, 0x00, 0x00, 0x00]))
        .await
        .is_err()
    {
        return;
    }

    // 2) 对时 I 帧 → 回一帧 I 帧确认，避免 send_clock_sync 白等 CLOCK_SYNC_TIMEOUT
    if read_frame(&mut stream).await.is_err() {
        return;
    }
    let ack = build_iframe(0, 1, &[0x64, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00])
        .expect("I-frame 构造失败");
    if stream.write_all(&ack).await.is_err() {
        return;
    }

    // 3) 等对端关闭：读到 EOF / 出错即「TCP 已释放」
    let mut buf = [0u8; 64];
    loop {
        match stream.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
    let _ = closed_tx.send(());
}

/// 用真实 TCP 连接先送入 `bytes`，再调用 read_frame，返回其错误。
async fn read_frame_err_with(bytes: &[u8]) -> crate::south::protocol::CollectorError {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定本地端口");
    let addr = listener.local_addr().expect("取本地端口");
    // 复制成 owned：spawn 要求 'static，借用切片无法逃逸出本函数
    let bytes = bytes.to_vec();
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.expect("accept");
        s.write_all(&bytes).await.expect("write");
        // 故意不关闭连接：若实现仍去读超长 body，测试会卡到超时而非误通过
        tokio::time::sleep(Duration::from_secs(5)).await;
    });
    let mut client = tokio::net::TcpStream::connect(addr).await.expect("connect");
    let r = read_frame(&mut client)
        .await
        .expect_err("非法 APDU 长度必须返回错误");
    server.abort();
    r
}
