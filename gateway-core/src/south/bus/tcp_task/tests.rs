//! D4: Modbus TCP 帧/映射纯逻辑单测（RTU 请求解析 / RTU 响应封装 / 错误映射）。
//! 上述纯函数是线上帧格式正确性的关键（Bug 60 曾因 pdu 含 fc 字节导致设备返回 Malformed）。
//!
//! 修复(审查#B5) 另附端到端用例 `stale_disconnect_does_not_clear_new_pool`：
//! 由本地假 Modbus TCP 服务端驱动真实 `bus_task_tcp` 主循环，复现「旧连接的迟到
//! 错误在新连接建立后才回写、误杀新 pool」的时序。

use super::*;
use crate::south::protocol::modbus::transaction::TransactionError;

#[test]
fn parse_rtu_request_rejects_too_short() {
    // 少于 4 字节无法容纳 slave+fc+crc
    let err = match parse_rtu_request(&[0x01, 0x03, 0x00], Duration::from_millis(10)) {
        Err(e) => e,
        Ok(_) => panic!("长度不足应返回 Err"),
    };
    assert!(
        matches!(err, CollectorError::InvalidFrame(_)),
        "预期 InvalidFrame，实际 {err:?}"
    );
}

/// Bug 60 回归：解析出的 pdu 必须从 rtu[3..] 起（不含 function_code 字节）。
#[test]
fn parse_rtu_request_pdu_excludes_function_code() {
    // [slave=01][fc=03][data=00 00 00 01][crc=84 0A]
    let rtu = [0x01, 0x03, 0x00, 0x00, 0x00, 0x01, 0x84, 0x0A];
    let req = parse_rtu_request(&rtu, Duration::from_millis(50)).expect("合法帧应解析成功");
    assert_eq!(req.slave_id, 0x01);
    assert_eq!(req.function_code, 0x03);
    assert_eq!(
        req.pdu,
        vec![0x00, 0x00, 0x00, 0x01],
        "pdu 不得包含 function_code 字节（否则 wire 上 fc 重复）"
    );
    assert_eq!(req.deadline, Duration::from_millis(50));
}

/// D4 回归：解析出的 pdu 经 `build_mbap_frame` 封装后必须携带完整
/// [addr_hi, addr_lo, cnt_hi, cnt_lo]（此前 rtu[3..] 会漏掉 addr_hi）。
#[test]
fn parse_then_build_mbap_preserves_register_address_and_count() {
    use crate::south::protocol::modbus::transaction::build_mbap_frame;
    // 读从站 1、地址 0x0010、数量 0x0002（与 build_read_request 同款布局 + 真实 CRC）
    let mut request = vec![0x01, 0x03, 0x00, 0x10, 0x00, 0x02];
    let crc = crc16(&request);
    request.push((crc & 0xFF) as u8);
    request.push((crc >> 8) as u8);

    let req = parse_rtu_request(&request, Duration::from_secs(3)).expect("合法读请求应解析成功");
    let frame = build_mbap_frame(0x1234, &req).expect("构造 MBAP 帧");

    // MBAP(7) + fc(1) + pdu(4) = 12
    assert_eq!(frame.len(), 12, "MBAP 帧长度应为 12");
    assert_eq!(
        u16::from_be_bytes([frame[4], frame[5]]),
        6,
        "length 字段 = unit_id(1)+fc(1)+pdu(4) = 6"
    );
    assert_eq!(frame[6], 0x01, "unit_id 应等于 slave_id");
    assert_eq!(frame[7], 0x03, "function_code 应紧随 unit_id");
    assert_eq!(
        &frame[8..12],
        &[0x00, 0x10, 0x00, 0x02],
        "PDU 必须完整保留寄存器地址与数量（不得漏掉 addr_hi）"
    );
}

/// 响应帧 CRC 必须与协议约定一致（低字节在前，覆盖 slave+pdu）。
#[test]
fn build_rtu_response_appends_valid_crc() {
    let pdu = [0x03, 0x02, 0x00, 0x2A]; // fc=03, byte_count=02, value=42
    let rtu = build_rtu_response(0x01, &pdu);
    assert_eq!(rtu[0], 0x01, "首字节为 slave_id");
    assert_eq!(&rtu[1..rtu.len() - 2], &pdu, "中间应为 pdu 原文");
    let crc = crc16(&rtu[..rtu.len() - 2]);
    assert_eq!(rtu[rtu.len() - 2], (crc & 0xFF) as u8, "CRC 低字节在前");
    assert_eq!(rtu[rtu.len() - 1], (crc >> 8) as u8, "CRC 高字节在后");
}

/// 每个 TransactionError 变体都应映射到明确的 CollectorError 分类。
#[test]
fn map_txn_err_covers_all_variants() {
    assert!(matches!(
        map_txn_err(TransactionError::Disconnected),
        CollectorError::IoError(_)
    ));
    assert!(matches!(
        map_txn_err(TransactionError::Timeout(Duration::from_millis(1))),
        CollectorError::NoReply
    ));
    assert!(matches!(
        map_txn_err(TransactionError::Malformed("bad".into())),
        CollectorError::InvalidFrame(_)
    ));
    // 第一批#3：Busy 是本地并发池满，归因为 BusBusy 而非「设备不应答」
    assert!(matches!(
        map_txn_err(TransactionError::Busy),
        CollectorError::BusBusy(_)
    ));
    assert!(matches!(
        map_txn_err(TransactionError::Io(std::io::Error::other("io"))),
        CollectorError::IoError(_)
    ));
    assert!(matches!(
        map_txn_err(TransactionError::NotBound),
        CollectorError::IoError(_)
    ));
}

// =========================================================
// 修复(审查#B5) 运行时回归：旧连接的迟到错误不得清掉新连接
// =========================================================
//
// 时序设计（最大并发 = 1，制造「A0 在途 → A1 Busy 退避持有旧 pool P1」）：
//   t0  A0 在 conn#1 上发出并被服务端读走（挂起不应答），A0 注册进 P1
//   t1  A1 提交 → max_concurrent=1 → 立即 Busy → 进入 retry 退避，持有 P1
//   t2  关闭 conn#1 → P1.reader 读到 EOF → abort_all → A0 得到 Disconnected
//       → A0 任务按 ptr_eq 命中本代次 P1 → 正常清空槽位
//   t3  A2 提交 → 槽位为空 → 主循环重连 conn#2（P2）→ A2 成功
//   t4  A1 退避结束重试：旧 socket 已关闭 → 重试写入失败(Io) → 走清理分支。
//       修复前会**无条件** *slot = None，把刚建好的 P2 一并丢弃 → A3 再次重连(conn#3)。
//       修复后 ptr_eq(P1, P2)=false → 不清空 → A3 复用 P2 → 连接数恒为 2。
//
// 观测量：假服务端 accept 次数 conns（修复后必须恰为 2）。

use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

/// Modbus RTU 读从站 1、地址 0x0000、数量 1 的请求帧（含真实 CRC）
const RTU_READ_REQ: [u8; 8] = [0x01, 0x03, 0x00, 0x00, 0x00, 0x01, 0x84, 0x0A];

/// 提交一个事务并等待其最终结果（封装 BusTransaction 的 oneshot 往返）
fn submit_txn(tx: &mpsc::Sender<BusTransaction>) -> JoinHandle<Result<Vec<u8>, CollectorError>> {
    let tx = tx.clone();
    tokio::spawn(async move {
        let (resp_tx, resp_rx) = oneshot::channel();
        tx.send(BusTransaction {
            request: RTU_READ_REQ.to_vec(),
            response_tx: resp_tx,
            expected_count: Some(1),
            raw: false,
            timeout_ms: None,
            enqueued_at: std::time::Instant::now(),
            priority: crate::south::bus::BusPriority::Normal,
            retry_allowed: true,
            frame_sent: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
        .await
        .map_err(|_| CollectorError::BusClosed)?;
        resp_rx.await.map_err(|_| CollectorError::BusClosed)?
    })
}

/// 读一个 Modbus TCP 请求：MBAP(7) + PDU。返回 (tx_id, unit_id, function_code)；
/// 返回 None 表示对端已关闭（EOF）或读失败。
async fn read_request(stream: &mut TcpStream) -> Option<(u16, u8, u8)> {
    let mut header = [0u8; 7];
    if stream.read_exact(&mut header).await.is_err() {
        return None;
    }
    let tx_id = u16::from_be_bytes([header[0], header[1]]);
    let length = u16::from_be_bytes([header[4], header[5]]) as usize;
    let unit_id = header[6];
    let rest = length.saturating_sub(1);
    let mut body = vec![0u8; rest];
    if rest > 0 && stream.read_exact(&mut body).await.is_err() {
        return None;
    }
    Some((tx_id, unit_id, body.first().copied().unwrap_or(0)))
}

/// 构造 Modbus TCP 读响应：tx_id + proto(0) + length(5) + unit(1) + [fc=03, cnt=02, val=0x002A]
fn canned_response(tx_id: u16) -> Vec<u8> {
    let mut f = Vec::with_capacity(11);
    f.extend_from_slice(&tx_id.to_be_bytes());
    f.extend_from_slice(&0u16.to_be_bytes());
    f.extend_from_slice(&5u16.to_be_bytes()); // length = unit(1) + pdu(4)
    f.push(0x01); // unit_id
    f.extend_from_slice(&[0x03, 0x02, 0x00, 0x2A]);
    f
}

/// 假 Modbus TCP 服务端：
/// - conn#1：读走 A0 请求后不应答，收到 close 信号即关闭（发 FIN 并 drop → 后续写入触发 RST）
/// - conn#2+：正常应答，直到对端关闭
async fn fake_modbus_server(
    listener: TcpListener,
    conns: Arc<AtomicUsize>,
    req1_tx: oneshot::Sender<()>,
    close_rx: oneshot::Receiver<()>,
) {
    let (mut s1, _) = match listener.accept().await {
        Ok(v) => v,
        Err(_) => return,
    };
    conns.fetch_add(1, Ordering::SeqCst);

    // 读走 A0 的请求（不应答），并通知测试「A0 已在途」
    if read_request(&mut s1).await.is_some() {
        let _ = req1_tx.send(());
    }

    // 等测试放行后关闭旧连接
    let _ = close_rx.await;
    let _ = s1.shutdown().await;
    drop(s1);

    // conn#2+：正常应答
    while let Ok((mut s, _)) = listener.accept().await {
        conns.fetch_add(1, Ordering::SeqCst);
        tokio::spawn(async move {
            while let Some((tx_id, _unit, _fc)) = read_request(&mut s).await {
                if s.write_all(&canned_response(tx_id)).await.is_err() {
                    break;
                }
            }
        });
    }
}

#[tokio::test]
async fn stale_disconnect_does_not_clear_new_pool() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定本地假 Modbus TCP 端口");
    let addr = listener.local_addr().expect("取本地端口").to_string();
    let conns = Arc::new(AtomicUsize::new(0));

    let (req1_tx, req1_rx) = oneshot::channel::<()>();
    let (close_tx, close_rx) = oneshot::channel::<()>();
    let server = tokio::spawn(fake_modbus_server(
        listener,
        Arc::clone(&conns),
        req1_tx,
        close_rx,
    ));

    let bus = BusConfig {
        port_name: "bus_tcp_stale_clear".into(),
        timeout_ms: 250,
        retry_count: Some(4),
        retry_delay_ms: Some(150),
        // 并发上限 1：A0 在途时 A1 必定 Busy 并退避，从而长时间持有旧 pool P1
        tcp_concurrent: Some(1),
        ..Default::default()
    };

    let (tx, rx) = mpsc::channel::<BusTransaction>(64);
    let (_ctrl_tx, ctrl_rx) = mpsc::channel::<BusTransaction>(64);
    let task = tokio::spawn(bus_task_tcp(
        bus,
        addr,
        rx,
        ctrl_rx,
        std::sync::Arc::new(crate::engine::metrics::Metrics::default()),
    ));

    // A0：在 conn#1 上发出并挂起（服务端读走请求不应答）
    let a0 = submit_txn(&tx);
    tokio::time::timeout(Duration::from_secs(2), req1_rx)
        .await
        .expect("A0 应在 2s 内被假服务端读到")
        .expect("屏障通道不应被丢弃");

    // A1：max_concurrent=1 → 立即 Busy 并进入重试退避，持有旧 pool P1
    let a1 = submit_txn(&tx);
    // 让 A1 完成首次 submit（Busy）进入 sleep，避免与服务端关闭发生竞态
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 关闭 conn#1 → A0 收到 Disconnected → 命中本代次 P1 正常清空槽位
    close_tx.send(()).expect("发送关闭信号");
    let e0 = a0
        .await
        .expect("A0 join")
        .expect_err("A0 必须失败（连接被关闭）");
    assert!(
        matches!(e0, CollectorError::IoError(_)),
        "A0 应为断连错误，实际 {e0:?}"
    );

    // A2：槽位已清空 → 主循环重连 conn#2（P2）→ 成功
    let a2 = submit_txn(&tx);
    let r2 = a2.await.expect("A2 join");
    assert!(r2.is_ok(), "重连后 A2 必须成功，实际 {r2:?}");
    assert_eq!(
        conns.load(Ordering::SeqCst),
        2,
        "重连后应恰好建立 2 条连接（P1 + P2）"
    );

    // A1 退避结束重试：旧 socket 已关闭 → 写入失败(Io) → 走旧连接清理分支
    let e1 = a1
        .await
        .expect("A1 join")
        .expect_err("A1 最终必须失败（旧连接已断）");
    assert!(
        matches!(e1, CollectorError::IoError(_)),
        "A1 应命中旧连接清理分支（IoError），实际 {e1:?}"
    );

    // A3：若 P2 被 A1 的迟到错误误杀，这里会再触发一次重连 → conns 变 3
    let a3 = submit_txn(&tx);
    let r3 = a3.await.expect("A3 join");
    assert!(r3.is_ok(), "A3 必须成功（新连接未被误杀），实际 {r3:?}");

    // 给潜在的第三次重连留出观测窗口
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        conns.load(Ordering::SeqCst),
        2,
        "旧连接的迟到错误不得清掉新连接（否则会反复重连，conns > 2）"
    );

    drop(tx);
    let _ = task.await;
    server.abort();
}

// =========================================================
// 修复(C-1) 运行时回归：连接不可用 → 作废 pool → 重连
// =========================================================
//
// 场景：服务端 accept 后读走客户端的一个请求，随即**断开连接**
//       （模拟设备掉电、中间设备半开、网线松动——TCP 连接已不可用，
//       但客户端未必立刻察觉）。
// 客户端 reader_loop 读到 EOF/ECONNRESET → abort_all 作废全部在途事务 →
// 事务返回 Disconnected → tcp_task 清空 pool_slot → 下一笔事务触发重连。
//
// 修复前：连接已死但 pool 仍被认为健康，在途事务永远等不到响应（也不返回
//         Disconnected/Io），主循环的「清空 pool_slot 触发重连」逻辑不会执行，
//         采集对该总线全线 stall，只能重启进程。
// 修复后：断连 → pool 作废 → 重连（且只重连一次，不得出现重连风暴）。
//
// 观测量：io_errors（至少一笔断连）与 conns（重连恰好一次，不得风暴）。
//
// 为什么不用「内核发送缓冲被填满 → 写超时」作为触发条件：
// 客户端 socket 由 bus_task_tcp 内部创建，测试无法设置其 SO_SNDBUF；而 Linux
// 会对发送缓冲做自动调优（上限 tcp_wmem[2]，默认 4MB），几百 KB 的写突发
// 根本填不满缓冲 → 写永不阻塞 → 写超时路径在 Linux 上不会发生。
// 「写路径超时 → abort_all」这条不变式由 transaction 层的单测覆盖
// （south/protocol/modbus/transaction/tests.rs），那里可以直接控制客户端 socket。

/// 单次写最大化的合法请求：fc=0x10，PDU 恰好 253 字节（Modbus 上限）。
/// MBAP 帧 = 7 + 1 + 253 = 261 字节。
fn max_size_valid_request() -> Vec<u8> {
    let mut f = Vec::with_capacity(257);
    f.push(0x01); // slave
    f.push(0x10); // fc
    f.extend_from_slice(&[0x00u8; 253]); // pdu
    f.push(0x00); // crc_lo（parse_rtu_request 只做切片，不校验 CRC）
    f.push(0x00); // crc_hi
    f
}

/// 提交任意请求帧并等待最终结果。
fn submit_custom(
    tx: &mpsc::Sender<BusTransaction>,
    request: Vec<u8>,
) -> JoinHandle<Result<Vec<u8>, CollectorError>> {
    let tx = tx.clone();
    tokio::spawn(async move {
        let (resp_tx, resp_rx) = oneshot::channel();
        tx.send(BusTransaction {
            request,
            response_tx: resp_tx,
            expected_count: None,
            raw: false,
            timeout_ms: None,
            enqueued_at: std::time::Instant::now(),
            priority: crate::south::bus::BusPriority::Normal,
            retry_allowed: true,
            frame_sent: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
        .await
        .map_err(|_| CollectorError::BusClosed)?;
        resp_rx.await.map_err(|_| CollectorError::BusClosed)?
    })
}

/// 并发灌入的请求数：覆盖「大量在途事务同时被断连作废」的路径。
const DISCONNECT_BURST: usize = 64;

#[tokio::test]
async fn disconnect_aborts_pool_and_triggers_reconnect() {
    let socket = tokio::net::TcpSocket::new_v4().expect("创建 TcpSocket");
    socket
        .bind("127.0.0.1:0".parse().expect("本地地址"))
        .expect("绑定本地端口");
    let listener = socket.listen(1024).expect("监听");
    let addr = listener.local_addr().expect("取本地端口").to_string();

    let conns = Arc::new(AtomicUsize::new(0));
    let accept_task = tokio::spawn({
        let conns = Arc::clone(&conns);
        async move {
            let mut first = true;
            while let Ok((mut s, _)) = listener.accept().await {
                conns.fetch_add(1, Ordering::SeqCst);
                if first {
                    first = false;
                    // conn#1：读走客户端的一个请求后立刻断开（设备掉电/半开）。
                    // 客户端 reader_loop 随即读到 EOF → abort_all → 在途事务全部作废。
                    tokio::spawn(async move {
                        let mut buf = [0u8; 1];
                        let _ = s.read(&mut buf).await;
                        drop(s);
                    });
                } else {
                    // conn#2+：保持连接但不应答（客户端等到自身超时），
                    // 避免二次断连引发重连风暴，干扰「只重连一次」的断言。
                    tokio::spawn(async move {
                        let _held = s;
                        tokio::time::sleep(Duration::from_secs(30)).await;
                    });
                }
            }
        }
    });

    let bus = BusConfig {
        port_name: "bus_tcp_disconnect".into(),
        timeout_ms: 200,
        retry_count: Some(0),
        retry_delay_ms: Some(10),
        // 并发上限足够大：避免 Busy 掩盖断连（Busy 会被映射成 NoReply）
        tcp_concurrent: Some(DISCONNECT_BURST * 2),
        ..Default::default()
    };

    let (tx, rx) = mpsc::channel::<BusTransaction>(DISCONNECT_BURST * 2);
    let (_ctrl_tx, ctrl_rx) = mpsc::channel::<BusTransaction>(DISCONNECT_BURST * 2);
    let task = tokio::spawn(bus_task_tcp(
        bus,
        addr,
        rx,
        ctrl_rx,
        std::sync::Arc::new(crate::engine::metrics::Metrics::default()),
    ));

    // 并发灌入：这些事务都将在 conn#1 上发出，随后被对端断开一并作废
    let req = max_size_valid_request();
    let mut handles = Vec::with_capacity(DISCONNECT_BURST);
    for _ in 0..DISCONNECT_BURST {
        handles.push(submit_custom(&tx, req.clone()));
    }

    let mut io_errors = 0usize;
    let mut unexpected: Vec<String> = Vec::new();
    for h in handles {
        match h.await.expect("join") {
            Err(CollectorError::IoError(_)) => io_errors += 1,
            other => {
                if unexpected.len() < 4 {
                    unexpected.push(format!("{other:?}"));
                }
            }
        }
    }
    assert!(
        io_errors > 0,
        "连接断开后必须至少有一笔事务被判为断连（Disconnected→IoError）；实际样本: {unexpected:?}"
    );

    // 断连必须作废 pool：再提交一笔 → 主循环重连 → 恰好建立第 2 条连接
    let probe = submit_txn(&tx);
    let _ = probe.await.expect("join");
    assert_eq!(
        conns.load(Ordering::SeqCst),
        2,
        "断连后 pool 必须被清空并重连（且只重连一次，不得出现重连风暴）"
    );

    drop(tx);
    let _ = task.await;
    accept_task.abort();
}

/// r15-SD4 回归：退避只在"真正通信成功"后清零；单纯 TCP 建连成功（连上即断）
/// 不得把累加的重连次数冲掉，否则设备半开时永远停在 1s 退避。
/// DD-09：没有成功通信的失败周期必须让档位 +1，而不是原值不动。
#[test]
fn reconnect_backoff_resets_only_after_real_communication() {
    assert_eq!(advance_reconnect_backoff(0, false), 1);
    assert_eq!(advance_reconnect_backoff(1, false), 2);
    assert_eq!(advance_reconnect_backoff(5, false), 6);
    assert_eq!(advance_reconnect_backoff(5, true), 0);
}

// =========================================================
// G-T1a TCP-04 / TCP-08：写通道串行额度
// =========================================================

/// TCP-04：多个写事务并发提交时，同一 socket 上同时下发的写最多 1 笔。
#[tokio::test]
async fn write_gate_serializes_concurrent_writes() {
    let gate = Arc::new(write_gate());
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();
    for _ in 0..4 {
        let gate = Arc::clone(&gate);
        let active = Arc::clone(&active);
        let peak = Arc::clone(&peak);
        handles.push(tokio::spawn(async move {
            let _permit = gate.acquire().await.expect("acquire");
            let n = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(n, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(10)).await;
            active.fetch_sub(1, Ordering::SeqCst);
        }));
    }
    for h in handles {
        h.await.expect("join");
    }
    assert_eq!(
        peak.load(Ordering::SeqCst),
        1,
        "写与写不得并发下发到同一 socket（否则同一设备的写会互相覆盖）"
    );
}

/// TCP-08：写通道与采集共享同一 socket，写额度固定为 1 个槽位。
#[test]
fn write_gate_exposes_single_shared_slot() {
    let gate = write_gate();
    assert_eq!(gate.available_permits(), 1, "写通道只允许 1 个在途写");
}

/// 文本契约：`Ok(s)` 建连成功分支不得再直接动退避计数，清零必须经上述判定函数。
#[test]
fn connect_success_does_not_clear_backoff() {
    let src = include_str!("../tcp_task.rs");
    let connect_ok = src.find("Ok(s) => {").expect("建连成功分支必须存在");
    let err_branch = connect_ok
        + src[connect_ok..]
            .find("Err(e) => {")
            .expect("建连失败分支必须存在");
    let block = &src[connect_ok..err_branch];
    assert!(
        !block.contains("reconnect_failures"),
        "建连成功分支不得直接改退避计数，否则连上即断会清零累加值"
    );
    assert!(
        src.contains("advance_reconnect_backoff("),
        "重连判定必须经过 had_successful_comm 判定函数"
    );
}

// =========================================================
// U-3：reload 与 TCP 在途写并发——旧事务终态 / 旧连接存活 / 不重复下发
// =========================================================
//
// reload 重建总线会 abort 主循环。TCP 的在途事务由独立 task 提交到 pool，
// 主循环 abort 时这些 task 只是被 detach（不是 abort），因此：
//   - 旧事务仍会在旧 socket 上收到应答并正常结束（不会退化成 BusClosed）；
//   - 旧连接一直存活到该事务结束；
//   - 写请求只下发一次（不会因 reload 被重复投递到新连接）。

/// 构造合法 Modbus RTU FC06 单寄存器写请求（含真实 CRC）。
fn modbus_fc06_write_local(slave: u8, addr: u16, value: u16) -> Vec<u8> {
    let mut frame = vec![slave, 0x06];
    frame.extend_from_slice(&addr.to_be_bytes());
    frame.extend_from_slice(&value.to_be_bytes());
    let crc = crc16(&frame);
    frame.push((crc & 0xFF) as u8);
    frame.push((crc >> 8) as u8);
    frame
}

/// 构造 FC06 写响应：tx_id + proto(0) + length(6) + unit(1) + [fc=06, addr, value]
fn fc06_response(tx_id: u16, addr: u16, value: u16) -> Vec<u8> {
    let mut f = Vec::with_capacity(12);
    f.extend_from_slice(&tx_id.to_be_bytes());
    f.extend_from_slice(&0u16.to_be_bytes());
    f.extend_from_slice(&6u16.to_be_bytes()); // unit(1) + pdu(5)
    f.push(0x01);
    f.push(0x06);
    f.extend_from_slice(&addr.to_be_bytes());
    f.extend_from_slice(&value.to_be_bytes());
    f
}

#[tokio::test]
async fn reload_does_not_duplicate_inflight_tcp_write() {
    use std::sync::atomic::AtomicBool;

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定本地假 Modbus TCP 端口");
    let addr = listener.local_addr().expect("取本地端口").to_string();
    let requests = Arc::new(AtomicUsize::new(0));
    let closed_before_reply = Arc::new(AtomicBool::new(false));
    let (seen_tx, seen_rx) = oneshot::channel::<()>();

    let requests_srv = Arc::clone(&requests);
    let closed_srv = Arc::clone(&closed_before_reply);
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.expect("accept");
        let (tx_id, _unit, _fc) = match read_request(&mut s).await {
            Some(v) => v,
            None => return,
        };
        requests_srv.fetch_add(1, Ordering::SeqCst);
        let _ = seen_tx.send(());

        // 应答前观察客户端是否已关闭（旧连接若被 reload 立即掐断，会读到 EOF/RST）。
        let mut one = [0u8; 1];
        match tokio::time::timeout(Duration::from_millis(300), s.read(&mut one)).await {
            Ok(Ok(0)) | Ok(Err(_)) => closed_srv.store(true, Ordering::SeqCst),
            _ => {}
        }

        let _ = s.write_all(&fc06_response(tx_id, 0x0001, 0x1234)).await;
        // 保住连接一小段时间，供测试确认事务在旧连接上完成
        tokio::time::sleep(Duration::from_millis(150)).await;
    });

    let bus = BusConfig {
        port_name: "bus_tcp_reload_write".into(),
        timeout_ms: 1000,
        retry_count: Some(0),
        retry_delay_ms: Some(10),
        tcp_concurrent: Some(2),
        ..Default::default()
    };
    let (tx, rx) = mpsc::channel::<BusTransaction>(16);
    let (_ctrl_tx, ctrl_rx) = mpsc::channel::<BusTransaction>(16);
    let task = tokio::spawn(bus_task_tcp(
        bus,
        addr,
        rx,
        ctrl_rx,
        std::sync::Arc::new(crate::engine::metrics::Metrics::default()),
    ));

    // 提交一笔写事务（不重发），等待服务端真正读到写帧
    let write_req = modbus_fc06_write_local(1, 0x0001, 0x1234);
    let mut caller = {
        let tx = tx.clone();
        tokio::spawn(async move {
            let (resp_tx, resp_rx) = oneshot::channel();
            tx.send(BusTransaction {
                request: write_req,
                response_tx: resp_tx,
                expected_count: None,
                raw: false,
                timeout_ms: None,
                enqueued_at: std::time::Instant::now(),
                priority: crate::south::bus::BusPriority::Control,
                retry_allowed: false,
                frame_sent: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            })
            .await
            .map_err(|_| CollectorError::BusClosed)?;
            resp_rx.await.map_err(|_| CollectorError::BusClosed)?
        })
    };

    tokio::time::timeout(Duration::from_secs(2), seen_rx)
        .await
        .expect("写请求应在 2s 内到达假服务端")
        .expect("屏障通道不应被丢弃");

    // 模拟 reload：abort 总线主循环（等价于 close_task 的 abort）
    task.abort();

    let result = tokio::time::timeout(Duration::from_secs(3), &mut caller)
        .await
        .expect("取消后旧事务仍应在超时内结束")
        .expect("写事务任务不应 panic");
    assert!(
        result.is_ok(),
        "在途写必须在旧连接上正常结束，不得退化成 BusClosed/WriteUncertain，实际 {result:?}"
    );

    drop(tx);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        requests.load(Ordering::SeqCst),
        1,
        "写请求只应下发一次（reload 不得对同一设备重复下发）"
    );
    assert!(
        !closed_before_reply.load(Ordering::SeqCst),
        "旧连接必须存活到在途事务结束（应答前不得被掐断）"
    );

    server.abort();
}
