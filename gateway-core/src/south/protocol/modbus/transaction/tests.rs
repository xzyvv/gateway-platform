//! TransactionPool 写路径不变式的单元测试。
//!
//! 覆盖修复(C-1)：写路径超时 → `abort_all()` 作废全部在途事务 → 返回 Disconnected。
//!
//! 触发方式：客户端 socket 显式设置较小的 `SO_SNDBUF`，服务端 accept 后
//! **永不读取**也**绝不关闭连接**。
//! - 显式设置 `SO_SNDBUF` 会关闭 Linux 发送缓冲自动调优（内核置 `SOCK_SNDBUF_LOCK`），
//!   缓冲大小固定为约 8KB；否则自动调优会把缓冲扩到 `tcp_wmem[2]`（默认 4MB），
//!   几百 KB 的写突发根本填不满 → 写永不阻塞 → 写超时路径在 Linux 上不会发生。
//! - 服务端不读 → 数据堆在服务端接收缓冲（约 128KB）里，接收窗口归零，
//!   客户端的写最终阻塞在 socket 上 → 触发 `submit()` 的写超时分支。
//! - 服务端绝不能关闭连接：一旦关闭，客户端写会收到 RST 走 `Io(e)` 分支，
//!   那就不是「写超时」这条路径了。
//!
//! 为什么这一层能确定性地测、而 bus 层测不了：
//! `bus_task_tcp` 内部自己创建客户端 socket，测试无法设置它的 `SO_SNDBUF`，
//! 因此 bus 层拿不到「写被卡住」这个前提，只能改用「对端断连」作为触发源。
//! 本模块的测试自己建 socket，可以直接关掉自动调优。
//!
//! 平台：仅 Linux。写阻塞依赖 Linux 的 socket 缓冲语义（显式 setsockopt 关闭
//! `tcp_sndbuf_expand` 自动调优 + 服务端不读导致接收窗口归零），其它平台的缓冲
//! 策略不同，用例不作数——用文件级 `#![cfg(target_os = "linux")]` 整体剥离，
//! 而不是留下一个在别的平台上假绿/假红的用例。

#![cfg(target_os = "linux")]

use std::sync::Arc;
use std::time::Duration;

use tokio::net::{TcpListener, TcpSocket};

use super::{ModbusRequest, TransactionError, TransactionPool};

/// 客户端发送缓冲上限（字节）。显式设置会关闭 Linux 发送缓冲自动调优，
/// 使「写满缓冲」这件事变得可预期（内核实际会翻倍，下限约 4.6KB）。
const CLIENT_SND_BUF: u32 = 4096;

/// 并发在途事务数。每帧 261 字节。
///
/// 可被吸收的数据量 ≈ 客户端发送缓冲（约 8KB，`SO_SNDBUF` 已被钉死）
/// 与服务端接收缓冲（默认约 128KB，服务端不读 → 接收窗口归零）之和，
/// 约 540 帧。本数远大于该阈值（写入尝试量约 1MB），确保写路径必然阻塞。
const BURST: usize = 4096;

/// 单笔事务的用户 deadline。写超时口径 = deadline + 50ms 网络开销。
const DEADLINE: Duration = Duration::from_millis(200);

/// 构造一个 PDU 恰好 253 字节（Modbus 上限）的请求，使每帧尽可能大：
/// MBAP(7) + unit_id(1) + fc(1) + pdu(253) = 261 字节。
fn max_size_request() -> ModbusRequest {
    ModbusRequest {
        slave_id: 1,
        function_code: 0x10,
        pdu: vec![0u8; 253],
        deadline: DEADLINE,
    }
}

#[tokio::test]
async fn write_timeout_aborts_pool_and_returns_disconnected() {
    // 服务端：accept 后永不读取，也绝不关闭连接。
    // 客户端的写只能阻塞在发送缓冲上（而不是收到 RST 变成写错误）。
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定本地端口");
    let addr = listener.local_addr().expect("取本地端口");
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("接受客户端连接");
        // 连接必须保持存活：一旦关闭，客户端写会收到 RST 变成写错误，
        // 那就不是「写超时」这条路径了。
        let _keep_alive = stream;
        std::future::pending::<()>().await;
    });

    // 客户端：显式设置发送缓冲 → 关闭 Linux 自动调优 → 缓冲固定且很小
    let socket = TcpSocket::new_v4().expect("创建客户端 socket");
    socket
        .set_send_buffer_size(CLIENT_SND_BUF)
        .expect("设置发送缓冲大小");
    let stream = socket.connect(addr).await.expect("连接服务端");

    let pool = TransactionPool::with_stream(BURST * 2, stream);

    // 并发灌入：前若干笔写成功（填缓冲），其后必然有一笔卡在写上直到超时。
    let mut handles = Vec::with_capacity(BURST);
    for _ in 0..BURST {
        let pool = Arc::clone(&pool);
        handles.push(tokio::spawn(async move { pool.submit(max_size_request()).await }));
    }

    let mut disconnected = 0usize;
    let mut unexpected: Vec<String> = Vec::new();
    for h in handles {
        let res = tokio::time::timeout(Duration::from_secs(20), h)
            .await
            .expect("submit 必须在 20s 内返回：写超时后不得永久挂起")
            .expect("join");
        match res {
            Err(TransactionError::Disconnected) => disconnected += 1,
            // 写被卡住期间，先行写入成功的在途事务等不到响应 → 超时收场
            Err(TransactionError::Timeout(_)) => {}
            other => {
                if unexpected.len() < 4 {
                    unexpected.push(format!("{other:?}"));
                }
            }
        }
    }

    assert!(
        disconnected > 0,
        "写超时必须作废 pool 并返回 Disconnected（至少一笔）；实际样本: {unexpected:?}"
    );
    assert!(
        unexpected.is_empty(),
        "写超时路径只允许 Disconnected / Timeout 两种结果，实际样本: {unexpected:?}"
    );
    assert_eq!(
        pool.active_count().await,
        0,
        "写超时后 pool 必须清空全部在途事务（abort_all），不得残留槽位"
    );

    server.abort();
}
