//! `serial_task` 私有事务函数的单元测试
//!
//! 覆盖集成测试无法构造的边界路径（写挂起 / 静默 / DE 时序 / ASCII / 分片），
//! 通过 [`SerialHandle::from_stream`] 注入内存串口桩，
//! 直接驱动真实 [`do_transaction`]。

use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use super::{do_transaction, write_timeout_for, TransactionParams};
use crate::engine::error::GatewayResult;
#[cfg(feature = "modbus")]
use crate::engine::util::crc::crc16;
use crate::south::bus::serial::SerialHandle;
use crate::south::bus::transport::GpioOutput;
#[cfg(feature = "modbus")]
use crate::south::protocol::util::rtu_to_ascii;
use crate::south::protocol::CollectorError;

// ============================================================
// 可编程串口桩
// ============================================================

/// 串口桩的共享状态（写/读两侧都要观测，故用 Arc<Mutex<_>>）。
#[derive(Default)]
struct PortState {
    /// 单次 flush 周期内累积的写字节
    write_acc: Vec<u8>,
    /// 每次 flush 落定的完整请求（按 flush 顺序）
    requests: Vec<Vec<u8>>,
    /// 首次 poll_write 时间戳
    write_started: Option<Instant>,
    /// 最近一次 flush 完成时间戳
    write_done: Option<Instant>,
    /// 首次真正读到响应字节的时间戳
    first_read: Option<Instant>,
    /// Bug C15 测试支撑：上一事务残留（先于本次请求到达接收缓冲）的字节
    stale: Vec<u8>,
    stale_pos: usize,
    /// 待交付的响应字节（本次请求的响应，写出完成后才交付）
    pending: Vec<u8>,
    pending_pos: usize,
    /// 每次 poll_read 最多交付字节数（0 = 一次给完）
    frag: usize,
}

/// 可编程串口桩：支持写挂起 / 永久静默 / 响应分片，并记录关键时序。
#[derive(Clone)]
struct MockPort {
    st: Arc<Mutex<PortState>>,
    /// true = poll_write 永不就绪（模拟流控异常导致的写阻塞）
    write_hang: bool,
    /// true = poll_read 永不就绪（模拟设备静默）
    silent: bool,
}

impl MockPort {
    /// 正常端口：flush 后按 `frag` 分片交付 `response`。
    #[cfg(feature = "modbus")]
    fn responding(response: Vec<u8>, frag: usize) -> Self {
        let st = PortState {
            pending: response,
            frag,
            ..Default::default()
        };
        Self {
            st: Arc::new(Mutex::new(st)),
            write_hang: false,
            silent: false,
        }
    }

    /// Bug C15: 先交付上一事务残留字节，写出完成后再交付本次响应。
    #[cfg(feature = "modbus")]
    fn with_stale(stale: Vec<u8>, response: Vec<u8>, frag: usize) -> Self {
        let st = PortState {
            stale,
            pending: response,
            frag,
            ..Default::default()
        };
        Self {
            st: Arc::new(Mutex::new(st)),
            write_hang: false,
            silent: false,
        }
    }

    /// 写永久挂起（write_all 永不完成）。
    fn hanging_write() -> Self {
        Self {
            st: Arc::new(Mutex::new(PortState::default())),
            write_hang: true,
            silent: false,
        }
    }

    /// 设备静默：写成功但读永不返回数据。
    fn silent() -> Self {
        Self {
            st: Arc::new(Mutex::new(PortState::default())),
            write_hang: false,
            silent: true,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PortState> {
        self.st.lock().expect("port mutex poisoned")
    }
}

impl AsyncWrite for MockPort {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let mut st = self.lock();
        if st.write_started.is_none() {
            st.write_started = Some(Instant::now());
        }
        if self.write_hang {
            return Poll::Pending;
        }
        st.write_acc.extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        let mut st = self.lock();
        if !st.write_acc.is_empty() {
            let req = std::mem::take(&mut st.write_acc);
            st.requests.push(req);
        }
        st.write_done = Some(Instant::now());
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for MockPort {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let mut st = self.lock();
        if self.silent {
            return Poll::Pending;
        }
        // Bug C15: 先交付「上一事务残留字节」——物理上它们先于本次请求到达接收缓冲。
        let stale_remaining = st.stale.len().saturating_sub(st.stale_pos);
        if stale_remaining > 0 {
            let n = stale_remaining.min(buf.remaining());
            let start = st.stale_pos;
            buf.put_slice(&st.stale[start..start + n]);
            st.stale_pos += n;
            if st.first_read.is_none() {
                st.first_read = Some(Instant::now());
            }
            return Poll::Ready(Ok(()));
        }
        // 半双工真实语义：请求写出完成前设备不会有响应到达。
        // 返回「0 字节」（等价于接收缓冲为空）而非 Pending：这样 `clear_input()`
        // （写前清缓冲）能立即判定缓冲为空并退出，不再依赖真实 5ms 计时器唤醒，
        // 使测试在各种负载 / 不同机器上都确定（R4：消除计时敏感导致的偶发 flaky）。
        if st.write_done.is_none() {
            return Poll::Ready(Ok(()));
        }
        let remaining = st.pending.len().saturating_sub(st.pending_pos);
        if remaining == 0 {
            return Poll::Pending;
        }
        let per_call = if st.frag > 0 { st.frag } else { remaining };
        let n = remaining.min(per_call).min(buf.remaining());
        let start = st.pending_pos;
        buf.put_slice(&st.pending[start..start + n]);
        st.pending_pos += n;
        if st.first_read.is_none() {
            st.first_read = Some(Instant::now());
        }
        Poll::Ready(Ok(()))
    }
}

// ============================================================
// 记录型 GPIO
// ============================================================

/// 记录每次 DE 电平变化的 GPIO 桩。
struct RecordingGpio {
    events: Arc<Mutex<Vec<(u32, bool, Instant)>>>,
}

impl RecordingGpio {
    fn new() -> Self {
        Self {
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn events(&self) -> Vec<(u32, bool, Instant)> {
        self.events.lock().expect("gpio mutex poisoned").clone()
    }
}

impl GpioOutput for RecordingGpio {
    fn set(&self, pin: u32, on: bool) -> GatewayResult<()> {
        self.events
            .lock()
            .expect("gpio mutex poisoned")
            .push((pin, on, Instant::now()));
        Ok(())
    }
}

// ============================================================
// 帧辅助
// ============================================================

/// 构造合法 Modbus RTU FC03 单寄存器响应（值为 `value`）。
#[cfg(feature = "modbus")]
fn modbus_fc03_response(value: u16) -> Vec<u8> {
    let mut frame = vec![0x01, 0x03, 0x02];
    frame.extend_from_slice(&value.to_be_bytes());
    let crc = crc16(&frame);
    frame.push((crc & 0xFF) as u8);
    frame.push((crc >> 8) as u8);
    frame
}

const DE_PIN: u32 = 7;

// ============================================================
// B3-1: 写挂起 → 短写超时（100ms）→ IoError，且 DE 必须回落
// ============================================================

/// 现场复盘：USB 转串口被拔掉后，驱动 write 永久阻塞；若不加写超时，
/// 总线 task 会被这一笔请求永久占住，后续所有点位采集全部挂死。
/// 修复后写使用自适应短超时（短帧仍取 100ms 下限，Bug 225 后按帧长/波特率放宽），
/// 且失败路径必须把 DE 拉回接收态（否则下一次发送前总线方向仍处于发送态）。
#[tokio::test]
async fn write_hang_hits_short_write_timeout_and_releases_de() {
    let port = MockPort::hanging_write();
    let mut handle = SerialHandle::from_stream(port.clone());
    let gpio = RecordingGpio::new();

    let t0 = Instant::now();
    let res = do_transaction(
        &mut handle,
        TransactionParams {
            request: &[0x01, 0x03, 0x00, 0x00, 0x00, 0x01],
            timeout: Duration::from_millis(500),
            protocol: "modbus",
            expected_count: Some(1),
            de_pin: Some(DE_PIN),
            modbus_mode: "rtu",
            baud: 9600,
        },
        &gpio,
    )
    .await;
    let elapsed = t0.elapsed();

    match res {
        Err(CollectorError::IoError(m)) => assert_eq!(
            m, "serial write timeout",
            "写挂起必须归因为短写超时，而非其它 IO 错误"
        ),
        other => panic!("写挂起应以 IoError 失败，实际 {other:?}"),
    }
    // 500ms 的读超时被写超时（6 字节 @9600 → 100ms 下限）截断：
    // 耗时应在 100ms 附近，而不是 500ms
    assert!(
        (Duration::from_millis(90)..Duration::from_millis(400)).contains(&elapsed),
        "写超时应截断到 ~100ms（完整读超时 500ms），实际 {elapsed:?}"
    );
    // DE 已回落：失败路径不得把总线留在发送态
    let ev = gpio.events();
    assert_eq!(
        ev.last().map(|(p, on, _)| (*p, *on)),
        Some((DE_PIN, false)),
        "写失败后 DE 必须拉低，实际事件序列 {:?}",
        ev.iter().map(|(p, on, _)| (*p, *on)).collect::<Vec<_>>()
    );
}

// ============================================================
// Bug 225: 写超时按帧长 + 波特率自适应
// ============================================================

/// 现场复盘：9600bps 总线上用 FC16 批量写 100 个寄存器（帧长约 211 字节），
/// 原固定 100ms 写超时只够传约 96 字节 —— 波特率与硬件完全正常，写却被判超时，
/// 事务失败并重试，重试同样必然超时，表现为"批量写永远失败、单点写正常"。
#[test]
fn write_timeout_scales_with_frame_len_and_baud() {
    let total = Duration::from_millis(1000);

    // 短帧（FC03 读请求 6 字节）@9600：线上约 7ms → 预算 71ms → 取 100ms 下限
    assert_eq!(
        write_timeout_for(6, 9600, total),
        Duration::from_millis(100),
        "短帧应仍使用 100ms 下限，保持原有防挂死语义"
    );

    // 长帧（FC16 批量写 211 字节）@9600：线上约 220ms → 预算约 710ms，必须显著放宽
    let long_frame = write_timeout_for(211, 9600, total);
    assert!(
        long_frame > Duration::from_millis(500) && long_frame <= total,
        "211 字节 @9600 的写超时应放宽到数百毫秒且不超总超时，实际 {long_frame:?}"
    );

    // 同样长帧 @115200：线上约 19ms → 预算约 107ms（远小于 9600 场景）
    let fast_bus = write_timeout_for(211, 115200, total);
    assert!(
        fast_bus < long_frame && fast_bus <= Duration::from_millis(200),
        "高速总线应回到接近 100ms 的短超时，实际 {fast_bus:?}"
    );

    // 总超时更小时必须以总超时为上限（不得突破事务预算）
    assert_eq!(
        write_timeout_for(211, 1200, Duration::from_millis(300)),
        Duration::from_millis(300),
        "预算不得突破事务总超时"
    );

    // baud 未配置（0）不得除零 / panic，退化为受总超时约束
    assert_eq!(
        write_timeout_for(211, 0, total),
        total,
        "非法波特率应保守退化为总超时上限"
    );
}

// ============================================================
// B3-2: 设备静默 → 必须走满完整读超时才判定 NoReply
// ============================================================

/// 现场复盘：表计掉电但 RS485 收发器仍在线（总线无数据）。若静默被
/// 误判为"立即无响应"，会提前空闲总线，掩盖设备的真实超时特征，
/// 也会让上层退避策略误判。此处钉住"必须等满完整 timeout"。
#[tokio::test]
async fn silent_device_waits_full_timeout_before_no_reply() {
    let port = MockPort::silent();
    let mut handle = SerialHandle::from_stream(port.clone());
    let gpio = RecordingGpio::new();
    let timeout = Duration::from_millis(200);

    let t0 = Instant::now();
    let res = do_transaction(
        &mut handle,
        TransactionParams {
            request: &[0x01, 0x03, 0x00, 0x00, 0x00, 0x01],
            timeout,
            protocol: "modbus",
            expected_count: Some(1),
            de_pin: Some(DE_PIN),
            modbus_mode: "rtu",
            baud: 9600,
        },
        &gpio,
    )
    .await;
    let elapsed = t0.elapsed();

    assert!(
        matches!(res, Err(CollectorError::NoReply)),
        "静默设备应判 NoReply，实际 {res:?}"
    );
    assert!(
        elapsed >= Duration::from_millis(190),
        "静默必须走满完整读超时（200ms）而非提前返回，实际 {elapsed:?}"
    );
    // 静默路径下 DE 也应在读取前回落
    let ev = gpio.events();
    assert_eq!(
        ev.last().map(|(p, on, _)| (*p, *on)),
        Some((DE_PIN, false)),
        "静默路径 DE 必须拉低"
    );
}

// ============================================================
// B3-3: DE 时序——高在首次写之前、低在首次读之前
// ============================================================

/// 现场复盘：RS485 是半双工，DE 拉高期间收发器处于发送态，此时读到
/// 的任何字节都是自身回显。若 DE 拉低晚于首次 poll_read，会把回显
/// 当作设备响应解析，产生 CRC 错误/错帧。此处钉住严格时序。
///
/// 桩帧是 Modbus RTU 帧，协议裁剪构建里没有 modbus 帧提取器可解析，故随 feature 门控。
#[cfg(feature = "modbus")]
#[tokio::test]
async fn de_goes_high_before_write_and_low_before_read() {
    let response = modbus_fc03_response(0x002A);
    let port = MockPort::responding(response.clone(), 0);
    let mut handle = SerialHandle::from_stream(port.clone());
    let gpio = RecordingGpio::new();

    let res = do_transaction(
        &mut handle,
        TransactionParams {
            request: &[0x01, 0x03, 0x00, 0x00, 0x00, 0x01],
            timeout: Duration::from_millis(500),
            protocol: "modbus",
            expected_count: Some(1),
            de_pin: Some(DE_PIN),
            modbus_mode: "rtu",
            baud: 9600,
        },
        &gpio,
    )
    .await;

    assert_eq!(res.expect("正常端口应返回完整响应帧"), response);

    let ev = gpio.events();
    assert_eq!(ev.len(), 2, "一次事务应恰好产生 DE 拉高/拉低两次操作");
    let (hi_pin, hi_on, hi_ts) = ev[0];
    let (lo_pin, lo_on, lo_ts) = ev[1];
    assert_eq!((hi_pin, hi_on), (DE_PIN, true), "首次操作必须是 DE 拉高");
    assert_eq!((lo_pin, lo_on), (DE_PIN, false), "第二次操作必须是 DE 拉低");

    let st = port.lock();
    let write_started = st.write_started.expect("必须发生过写");
    let write_done = st.write_done.expect("必须完成过 flush");
    let first_read = st.first_read.expect("必须发生过读");
    drop(st);

    assert!(
        hi_ts <= write_started,
        "DE 拉高必须早于首次写（hi={hi_ts:?}, write_started={write_started:?}）"
    );
    assert!(
        lo_ts >= write_done,
        "DE 拉低必须在写完成之后（lo={lo_ts:?}, write_done={write_done:?}）"
    );
    assert!(
        lo_ts <= first_read,
        "DE 拉低必须早于首次读（lo={lo_ts:?}, first_read={first_read:?}）"
    );
}

// ============================================================
// B3-4: Modbus ASCII 模式——请求转换 + 响应分片
// ============================================================

/// 现场复盘：ASCII 模式请求必须转成 `:..LRC\r\n`；响应可能逐字节到达，
/// 早期实现对分片 ASCII 逐字节转换失败，导致完整响应永远拼不出来（Bug 80）。
#[cfg(feature = "modbus")]
#[tokio::test]
async fn ascii_mode_converts_request_and_handles_fragmented_response() {
    let rtu_request = vec![0x01, 0x03, 0x00, 0x00, 0x00, 0x01, 0x84, 0x0A];
    let rtu_response = modbus_fc03_response(0x002A);
    // 设备以 ASCII 帧回复，且每片只交付 1 字节
    let ascii_response = rtu_to_ascii(&rtu_response);
    let port = MockPort::responding(ascii_response, 1);
    let mut handle = SerialHandle::from_stream(port.clone());
    let gpio = RecordingGpio::new();

    let res = do_transaction(
        &mut handle,
        TransactionParams {
            request: &rtu_request,
            timeout: Duration::from_millis(500),
            protocol: "modbus",
            expected_count: Some(1),
            de_pin: None,
            modbus_mode: "ascii",
            baud: 9600,
        },
        &gpio,
    )
    .await;

    // 响应侧：分片 ASCII 必须被拼回并还原成 RTU 帧
    assert_eq!(
        res.expect("分片 ASCII 响应应拼接成功"),
        rtu_response,
        "ASCII 响应应还原为原始 RTU 帧"
    );
    // 请求侧：写出的字节必须是 ASCII 帧（RTU 去掉 CRC 转 hex + LRC）
    let st = port.lock();
    let written = st.requests.first().cloned().unwrap_or_default();
    drop(st);
    assert_eq!(
        written,
        rtu_to_ascii(&rtu_request),
        "ASCII 模式请求必须以 ASCII 帧发送"
    );
    assert_eq!(written.first(), Some(&b':'), "ASCII 帧必须以 ':' 起始");
    assert_eq!(
        &written[written.len() - 2..],
        b"\r\n",
        "ASCII 帧必须以 CRLF 结束"
    );
}

// ============================================================
// B3-5: 响应分片（1 字节/片）仍能拼出完整帧
// ============================================================

/// 现场复盘：低波特率 + 中断驱动的串口驱动常把一帧拆成多片交付。
/// 若帧缓冲不跨 read 累积，会出现"帧不全 → 偶发 NoReply"的现场疑难。
#[cfg(feature = "modbus")]
#[tokio::test]
async fn fragmented_rtu_response_is_reassembled() {
    let response = modbus_fc03_response(0x1234);
    let port = MockPort::responding(response.clone(), 1);
    let mut handle = SerialHandle::from_stream(port.clone());
    let gpio = RecordingGpio::new();

    let res = do_transaction(
        &mut handle,
        TransactionParams {
            request: &[0x01, 0x03, 0x00, 0x00, 0x00, 0x01, 0x84, 0x0A],
            timeout: Duration::from_millis(500),
            protocol: "modbus",
            expected_count: Some(1),
            de_pin: None,
            modbus_mode: "rtu",
            baud: 9600,
        },
        &gpio,
    )
    .await;

    assert_eq!(res.expect("逐字节分片的 RTU 响应应被完整拼出"), response);
}

// ============================================================
// Bug C15: 事务前清空接收缓冲——残留字节不得污染本次响应解析
// ============================================================

/// 现场复盘：上一事务读超时后，设备迟到的回包仍滞留在串口接收缓冲；
/// 若下一事务前不清理，本次解析会先读到旧帧，得到的是**上一个请求的数值**
/// （甚至 CRC 错位导致连续 NoReply）。此处钉住"写前必须清缓冲"。
#[cfg(feature = "modbus")]
#[tokio::test]
async fn stale_input_is_drained_before_transaction() {
    // 残留 = 上一事务的完整响应（值 0x1111）
    let stale = modbus_fc03_response(0x1111);
    // 本次真实响应（值 0x002A）
    let response = modbus_fc03_response(0x002A);
    let port = MockPort::with_stale(stale, response.clone(), 0);
    let mut handle = SerialHandle::from_stream(port.clone());
    let gpio = RecordingGpio::new();

    let res = do_transaction(
        &mut handle,
        TransactionParams {
            request: &[0x01, 0x03, 0x00, 0x00, 0x00, 0x01],
            timeout: Duration::from_millis(500),
            protocol: "modbus",
            expected_count: Some(1),
            de_pin: Some(DE_PIN),
            modbus_mode: "rtu",
            baud: 9600,
        },
        &gpio,
    )
    .await;

    assert_eq!(
        res.expect("清缓冲后应解析出本次响应"),
        response,
        "必须返回本次请求的响应，而不是上一事务的残留帧"
    );
}
