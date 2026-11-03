//! Bug 148 回归：总线 task 死亡后 transaction() 必须快速失败 BusClosed。
//!
//! 该路径无法从 chaos 集成测试触达：BusManager::spawn 总附带存活 task，
//! task 只在全部 sender drop 后退出，而唯一 sender 由 BusManager 持有，
//! 外部构造不出「task 死了 manager 还在」的状态（除非 task panic——
//! 而 panic 无外部触发面）。这里直接构造死通道，等价于 task panic 被
//! supervision 捕获、rx 随 task 销毁后的状态。

use super::*;

#[tokio::test]
async fn transaction_fails_fast_bus_closed_when_task_dead() {
    let (tx, rx) = mpsc::channel::<BusTransaction>(4);
    drop(rx); // 等价于总线 task 退出后 rx 被销毁
    let bm = BusManager {
        name: "bus_dead".to_string(),
        tcp_addr: None,
        timeout_ms: 500,
        max_inflight: 1,
        variant: None,
        asdu_addr_len: 2,
        opcua: None,
        tx,
        metrics: std::sync::Arc::new(Metrics::new()),
        handle: None,
    };
    let start = std::time::Instant::now();
    let err = bm
        .transaction(vec![0x01, 0x03, 0x00, 0x00, 0x00, 0x01, 0x84, 0x0A], None)
        .await
        .expect_err("dead bus task must fail");
    assert!(
        matches!(err, CollectorError::BusClosed),
        "expected BusClosed, got {err:?}"
    );
    assert!(
        start.elapsed() < std::time::Duration::from_millis(500),
        "must fail fast, not hang: {:?}",
        start.elapsed()
    );
}

// ===== Bug 2.2: serial=None (热拔) 状态下必须快速失败，不阻塞业务调用 =====
//
// 修复前：serial=None 时每笔事务都触发 try_reopen_serial 内 2s sleep，
//         调用方 transaction() 等待 ≥2s，channel 拥堵
// 修复后：serial=None 时 run_serial_bus_task 立即返回 BusClosed，不走 sleep 路径
//
// 这里通过 spawn_with + 永远失败的 opener 模拟"open 始终失败"，
// 验证 transaction() 在 500ms 内失败。
use crate::south::bus::transport::SerialOpener;
use std::io;

struct FailingOpener;
impl SerialOpener for FailingOpener {
    fn open(&self, _bus: &BusConfig) -> io::Result<SerialHandle> {
        Err(io::Error::new(io::ErrorKind::NotFound, "device gone"))
    }
}

#[tokio::test]
async fn transaction_fails_fast_when_serial_cannot_be_opened() {
    let bus = BusConfig {
        port_name: "bus_no_serial".into(),
        timeout_ms: 1000,
        retry_delay_ms: Some(200),
        retry_count: Some(0),
        ..Default::default()
    };

    let deps = BusDeps {
        opener: Arc::new(FailingOpener) as Arc<dyn SerialOpener>,
        gpio: crate::south::bus::transport::BusDeps::production().gpio,
    };
    let bm = BusManager::spawn_with(
        &bus,
        None, // serial = None
        Arc::new(crate::engine::metrics::Metrics::new()),
        deps,
    );

    let start = std::time::Instant::now();
    let err = bm
        .transaction(vec![0x01, 0x03, 0x00, 0x00, 0x00, 0x01, 0x84, 0x0A], None)
        .await
        .expect_err("serial=None + failing opener must fail");
    let elapsed = start.elapsed();
    assert!(
        matches!(err, CollectorError::BusClosed),
        "expected BusClosed, got {err:?}"
    );
    // 修复前：≥2s；修复后：<200ms（仅 rx 排队 + 立即 BusClosed 路径）
    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "must fail fast (under 500ms), actual = {:?}",
        elapsed
    );
    drop(bm);
}

// ===== 修复(审查#B3) 运行时回归：总线 task 被 abort 后后台重连必须收尾 =====
//
// 修复前：serial_task 只在主循环正常退出（rx 关闭）后执行 `alive=false`，而
//         `BusManager::drop` 走的是 `JoinHandle::abort()`——abort 直接 drop 本
//         future，跳过循环之后的语句，后台重连任务永远看不到 alive=false，
//         按 1s→2s→4s→…→30s 退避无限 open 串口并占用（旧总线释放后仍霸占
//         /dev/ttyUSBx，新总线再也打不开）。
// 修复后：AliveGuard 在 future drop（含 abort / panic）时置 alive=false，
//         后台重连循环在下一轮入口检查即退出，不再触碰 opener。
//
// 观测点：CountingOpener 的调用次数——abort 之后不得再增长。

use std::sync::atomic::{AtomicUsize, Ordering};

/// 只计数、永远失败的串口 opener（不触碰真实 /dev/ttyUSB*）
struct CountingOpener {
    calls: Arc<AtomicUsize>,
}

impl SerialOpener for CountingOpener {
    fn open(&self, _bus: &BusConfig) -> io::Result<SerialHandle> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(io::Error::new(io::ErrorKind::NotFound, "device gone"))
    }
}

#[tokio::test]
async fn aborting_bus_task_stops_background_reconnect() {
    let calls = Arc::new(AtomicUsize::new(0));
    let bus = BusConfig {
        port_name: "bus_reconnect_leak".into(),
        timeout_ms: 200,
        retry_count: Some(0),
        retry_delay_ms: Some(20),
        ..Default::default()
    };

    let deps = BusDeps {
        opener: Arc::new(CountingOpener {
            calls: Arc::clone(&calls),
        }) as Arc<dyn SerialOpener>,
        gpio: crate::south::bus::transport::BusDeps::production().gpio,
    };
    let bm = BusManager::spawn_with(
        &bus,
        None, // serial=None → 事务快速失败，同时拉起后台重连任务
        Arc::new(crate::engine::metrics::Metrics::new()),
        deps,
    );

    let err = bm
        .transaction(vec![0x01, 0x03, 0x00, 0x00, 0x00, 0x01, 0x84, 0x0A], None)
        .await
        .expect_err("serial=None 必须快速失败");
    assert!(
        matches!(err, CollectorError::BusClosed),
        "expected BusClosed, got {err:?}"
    );

    // 等后台重连任务真正跑起来（第一次 open 已被调用）
    let started = std::time::Instant::now();
    while calls.load(Ordering::SeqCst) == 0 {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "后台重连任务未启动"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    // 等价停机 / 热重载整体释放总线：abort 总线 task（不走正常退出路径）
    drop(bm);

    let before = calls.load(Ordering::SeqCst);
    // 覆盖后台退避的 1s / 2s 两轮：修复后循环在下一轮 alive 检查即退出
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    let after = calls.load(Ordering::SeqCst);
    assert!(
        after <= before + 1,
        "总线 task 被 abort 后后台重连仍在反复 open 串口（任务泄漏 / 占用串口）: {} -> {}",
        before,
        after
    );
}

// ===== Bug 217: 读路径并发上限（max_concurrency）=====
//
// TCP 单 socket 支持多事务并发；串口半双工必须串行。采集器据此决定并发提交帧的上限。

#[tokio::test]
async fn max_concurrency_serial_is_one_and_tcp_follows_config() {
    let metrics = || Arc::new(crate::engine::metrics::Metrics::new());

    // 1) 串口总线（无 tcp_addr）：即使配置了 tcp_concurrent 也必须为 1
    let serial = BusConfig {
        port_name: "bus_serial_conc".into(),
        tcp_addr: None,
        tcp_concurrent: Some(64),
        ..Default::default()
    };
    let bm_serial = BusManager::spawn_with(&serial, None, metrics(), BusDeps::production());
    assert_eq!(bm_serial.max_concurrency(), 1, "串口半双工必须串行，恒为 1");
    drop(bm_serial);

    // 2) TCP 总线：取 tcp_concurrent
    let tcp = BusConfig {
        port_name: "bus_tcp_conc".into(),
        tcp_addr: Some("127.0.0.1:1".into()),
        tcp_concurrent: Some(16),
        ..Default::default()
    };
    let bm_tcp = BusManager::spawn_with(&tcp, None, metrics(), BusDeps::production());
    assert_eq!(bm_tcp.max_concurrency(), 16, "TCP 应取 tcp_concurrent");
    drop(bm_tcp);

    // 3) TCP 未配置 tcp_concurrent：默认 8（与 tcp_task 内部默认一致）
    let tcp_def = BusConfig {
        port_name: "bus_tcp_default_conc".into(),
        tcp_addr: Some("127.0.0.1:1".into()),
        tcp_concurrent: None,
        ..Default::default()
    };
    let bm_tcp_def = BusManager::spawn_with(&tcp_def, None, metrics(), BusDeps::production());
    assert_eq!(bm_tcp_def.max_concurrency(), 8, "未配置时应默认 8");
    drop(bm_tcp_def);
}

// ===== C-2: 串口总线自检帧（bus 级集成回归）=====
//
// 这几条测试走**完整总线链路**：BusManager::spawn_with → mpsc → run_serial_bus_task
// → do_transaction → 可编程串口桩，观测点是「串口重开次数」与「实际发出的帧序列」。
//
// 修复前的行为：NoReply/ChecksumError 连续 10 次即丢掉串口句柄、关闭整条 RS485 端口
// 重开。一根总线上只要有一个从站掉线，同总线所有正常设备都会被反复掐断。
// 修复后：先向「最近一次成功通信的从站」发一帧 FC03 最小读做自检，据结果分级处置。
//
// 本节全部依赖 Modbus RTU 帧（FC03 请求 / 响应 + CRC 校验），而 Modbus 是 feature
// gate 协议——协议裁剪构建（`--no-default-features --features ...`）里既没有 modbus
// 采集器，也没有 modbus 帧提取器，桩帧无法被解析成合法响应。故整节随 feature 门控。

#[cfg(feature = "modbus")]
use crate::engine::util::crc::crc16;
#[cfg(feature = "modbus")]
use crate::south::bus::serial::SerialHandle;
#[cfg(feature = "modbus")]
use std::collections::HashSet;
#[cfg(feature = "modbus")]
use std::sync::Mutex;
#[cfg(feature = "modbus")]
use std::task::{Context, Poll};
#[cfg(feature = "modbus")]
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// 可编程串口桩：按请求里的从站号决定是否响应，支持运行时把某个从站改成静默。
#[cfg(feature = "modbus")]
#[derive(Clone)]
struct SwitchablePort {
    st: Arc<Mutex<SwitchState>>,
}

#[cfg(feature = "modbus")]
#[derive(Default)]
struct SwitchState {
    /// 单次 flush 周期内累积的写字节
    write_acc: Vec<u8>,
    /// 每次 flush 落定的完整请求（按写入顺序）
    requests: Vec<Vec<u8>>,
    /// 是否已经完成过一次写（半双工：写出前不会有响应）
    write_done: bool,
    /// 待交付的响应字节
    pending: Vec<u8>,
    pending_pos: usize,
    /// 当前"在线"的从站号集合（不在集合内 = 静默）
    alive: HashSet<u8>,
}

#[cfg(feature = "modbus")]
impl SwitchablePort {
    fn new(alive: &[u8]) -> Self {
        Self {
            st: Arc::new(Mutex::new(SwitchState {
                alive: alive.iter().copied().collect(),
                ..Default::default()
            })),
        }
    }

    /// 调整在线从站集合，用于模拟"某个设备掉线"或"全部掉线"。
    fn set_alive(&self, alive: &[u8]) {
        let mut st = self.st.lock().expect("port mutex poisoned");
        st.alive = alive.iter().copied().collect();
    }

    /// 已落定的请求帧序列（用于断言自检帧是否发出、发给了谁）。
    fn requests(&self) -> Vec<Vec<u8>> {
        self.st
            .lock()
            .expect("port mutex poisoned")
            .requests
            .clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SwitchState> {
        self.st.lock().expect("port mutex poisoned")
    }
}

#[cfg(feature = "modbus")]
impl AsyncWrite for SwitchablePort {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.lock().write_acc.extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        let mut st = self.lock();
        if !st.write_acc.is_empty() {
            let req = std::mem::take(&mut st.write_acc);
            if let Some(&slave) = req.first() {
                if st.alive.contains(&slave) && req.get(1) == Some(&0x03) {
                    // 读取请求：回 FC03 单寄存器响应（值固定，内容与自检无关）
                    st.pending = modbus_fc03_response(slave, 0x1234);
                    st.pending_pos = 0;
                } else {
                    // 从站静默：不交付任何字节，由读超时归因为 NoReply
                    st.pending.clear();
                    st.pending_pos = 0;
                }
            }
            st.requests.push(req);
        }
        st.write_done = true;
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[cfg(feature = "modbus")]
impl AsyncRead for SwitchablePort {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let mut st = self.lock();
        // 半双工语义：请求尚未写出时接收缓冲为空。
        // 返回 0 字节（而非 Pending），让写前清缓冲的 clear_input 立即结束。
        if !st.write_done {
            return Poll::Ready(Ok(()));
        }
        let remaining = st.pending.len().saturating_sub(st.pending_pos);
        if remaining == 0 {
            return Poll::Pending;
        }
        let n = remaining.min(buf.remaining());
        let start = st.pending_pos;
        buf.put_slice(&st.pending[start..start + n]);
        st.pending_pos += n;
        Poll::Ready(Ok(()))
    }
}

/// 构造合法 Modbus RTU FC03 单寄存器响应。
#[cfg(feature = "modbus")]
fn modbus_fc03_response(slave: u8, value: u16) -> Vec<u8> {
    let mut frame = vec![slave, 0x03, 0x02];
    frame.extend_from_slice(&value.to_be_bytes());
    let crc = crc16(&frame);
    frame.push((crc & 0xFF) as u8);
    frame.push((crc >> 8) as u8);
    frame
}

/// FC03 单寄存器读请求（与 `build_read_request(slave, 3, 0, 1)` 等价）。
#[cfg(feature = "modbus")]
fn read_one(slave: u8) -> Vec<u8> {
    let mut frame = vec![slave, 0x03, 0x00, 0x00, 0x00, 0x01];
    let crc = crc16(&frame);
    frame.push((crc & 0xFF) as u8);
    frame.push((crc >> 8) as u8);
    frame
}

/// 轮询等待条件成立（避免依赖固定 sleep 造成的偶发 flaky）。
#[cfg(feature = "modbus")]
async fn wait_until(mut cond: impl FnMut() -> bool, budget_ms: u64, what: &str) {
    let start = std::time::Instant::now();
    while !cond() {
        assert!(
            start.elapsed() < std::time::Duration::from_millis(budget_ms),
            "等待超时：{what}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

/// 构造一条挂了「可编程串口桩」的串口总线（故意压低超时以缩短用例耗时）。
#[cfg(feature = "modbus")]
fn spawn_serial_bus(
    port: &SwitchablePort,
    opener: Arc<dyn SerialOpener>,
) -> (BusManager, Arc<crate::engine::metrics::Metrics>) {
    let bus = BusConfig {
        port_name: "bus_selfcheck".into(),
        protocol: "modbus".into(),
        modbus_mode: "rtu".into(),
        de_pin: None,
        // 静默设备每笔事务 = 5ms 清缓冲 + timeout，压低到 40ms 让 10 笔用例约 0.5s
        timeout_ms: 40,
        retry_count: Some(0),
        retry_delay_ms: Some(10),
        ..Default::default()
    };

    let deps = BusDeps {
        opener,
        gpio: crate::south::bus::transport::BusDeps::production().gpio,
    };
    // 自检结论的可观测性：返回 metrics 供用例断言计数器确实被写入
    let metrics = Arc::new(crate::engine::metrics::Metrics::new());
    let bm = BusManager::spawn_with(
        &bus,
        Some(SerialHandle::from_stream(port.clone())),
        metrics.clone(),
        deps,
    );
    (bm, metrics)
}

/// 场景 1（核心）：一个从站掉线，自检帧打到在线从站 → 判定设备级故障，**不得重开端口**，
/// 且同总线上的正常设备持续可读。
#[cfg(feature = "modbus")]
#[tokio::test]
async fn self_check_passes_so_silent_device_does_not_reopen_port() {
    let port = SwitchablePort::new(&[1, 2]);
    let calls = Arc::new(AtomicUsize::new(0));
    let (bm, metrics) = spawn_serial_bus(
        &port,
        Arc::new(CountingOpener {
            calls: Arc::clone(&calls),
        }) as Arc<dyn SerialOpener>,
    );

    // 1) 先成功读一笔 slave 1，使 last_good_slave 记录为 1
    let ok = bm
        .transaction(read_one(1), Some(1))
        .await
        .expect("slave 1 在线，首笔事务必须成功");
    assert_eq!(ok.len(), 7, "FC03 单寄存器响应应为 7 字节");

    // 2) slave 2 掉线，连续 10 笔无响应 → 触发自检
    port.set_alive(&[1]);
    for i in 0..10 {
        let err = bm
            .transaction(read_one(2), Some(1))
            .await
            .expect_err("静默从站必须失败");
        assert!(
            !matches!(err, CollectorError::BusClosed),
            "第 {} 笔：静默不应导致句柄被丢弃（{err:?}）",
            i + 1
        );
    }

    // 3) 自检帧必须已发往在线从站（slave 1），而非被跳过的静默从站。
    //    步骤 1 已产生 1 帧 read_one(1)，因此出现第 2 帧即证明自检帧确实发出。
    let probe = read_one(1);
    wait_until(
        || port.requests().iter().filter(|r| *r == &probe).count() >= 2,
        2000,
        "自检帧未发出",
    )
    .await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "自检通过说明端口健康，不得丢句柄重开串口"
    );

    // 自检结论必须落到指标上：现场靠这两个计数区分"设备坏"与"端口坏"，
    // 只打 tracing 日志等于不可见。
    assert!(
        metrics.bus.self_check_ok.load(Ordering::Relaxed) >= 1,
        "自检成功应累加 gateway_bus_self_check_ok_total"
    );
    assert_eq!(
        metrics
            .bus
            .port_reopen_by_self_check
            .load(Ordering::Relaxed),
        0,
        "端口健康时不得累加重开计数"
    );

    // 4) 正常设备不受影响，仍可继续采集
    let ok2 = bm
        .transaction(read_one(1), Some(1))
        .await
        .expect("同总线正常设备必须持续可读");
    assert_eq!(ok2.len(), 7);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "静默设备不应把整条总线拖入重连循环"
    );

    drop(bm);
}

/// 场景 2：总线全部静默 → 自检帧同样收不到响应 → 判定端口级故障，丢句柄交由后台重开。
#[cfg(feature = "modbus")]
#[tokio::test]
async fn self_check_fails_when_all_slaves_silent_and_reopens_port() {
    let port = SwitchablePort::new(&[1]);
    let calls = Arc::new(AtomicUsize::new(0));
    let (bm, metrics) = spawn_serial_bus(
        &port,
        Arc::new(CountingOpener {
            calls: Arc::clone(&calls),
        }) as Arc<dyn SerialOpener>,
    );

    // 先建立"曾经成功过"的前提，使自检帧有明确探测目标
    bm.transaction(read_one(1), Some(1))
        .await
        .expect("slave 1 在线");

    // 整条总线静默（等价于串口半开：句柄活着但收不到任何数据）
    port.set_alive(&[]);
    for _ in 0..10 {
        let err = bm
            .transaction(read_one(1), Some(1))
            .await
            .expect_err("全静默必须失败");
        assert!(!matches!(err, CollectorError::BusClosed), "{err:?}");
    }

    // 自检帧也失败 → 端口判定为损坏 → 触发后台重开（opener 被调用）
    wait_until(
        || calls.load(Ordering::SeqCst) >= 1,
        3000,
        "自检失败后应丢句柄并触发串口重开",
    )
    .await;

    // 归因指标：自检失败 + 实际重开各计一次（冷却期外的首次重开）
    assert!(
        metrics.bus.self_check_fail.load(Ordering::Relaxed) >= 1,
        "自检失败应累加 gateway_bus_self_check_fail_total"
    );
    assert!(
        metrics
            .bus
            .port_reopen_by_self_check
            .load(Ordering::Relaxed)
            >= 1,
        "自检失败且冷却已过时应累加 gateway_bus_port_reopen_by_self_check_total"
    );

    drop(bm);
}

/// 场景 3：从未有过成功通信（无探测目标）→ 无法区分设备级/端口级，保守不动端口。
///
/// 这是防止"盲判"的关键约束：宁可等待后续 IO 错误判定，也不要在毫无依据时
/// 关闭整条总线把其它设备一起拖下水。
#[cfg(feature = "modbus")]
#[tokio::test]
async fn self_check_skipped_without_known_good_slave_keeps_handle() {
    let port = SwitchablePort::new(&[]); // 全程静默，从未成功
    let calls = Arc::new(AtomicUsize::new(0));
    let (bm, metrics) = spawn_serial_bus(
        &port,
        Arc::new(CountingOpener {
            calls: Arc::clone(&calls),
        }) as Arc<dyn SerialOpener>,
    );

    for _ in 0..10 {
        let err = bm
            .transaction(read_one(1), Some(1))
            .await
            .expect_err("静默从站必须失败");
        assert!(!matches!(err, CollectorError::BusClosed), "{err:?}");
    }

    // 留出自检判定窗口：若无探测目标，代码会直接跳过
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "无历史成功从站时不得盲判端口损坏"
    );
    // 跳过也必须可见：运维看到 skipped 增长就知道"系统还无法归因"，
    // 而不是误以为自检一直在正常工作。
    assert!(
        metrics.bus.self_check_skipped.load(Ordering::Relaxed) >= 1,
        "无已知从站时应累加 gateway_bus_self_check_skipped_total"
    );
    assert_eq!(
        metrics
            .bus
            .port_reopen_by_self_check
            .load(Ordering::Relaxed),
        0,
        "跳过自检时不得累加重开计数"
    );

    // 句柄仍在：继续返回设备级失败，而不是 BusClosed
    let err = bm
        .transaction(read_one(1), Some(1))
        .await
        .expect_err("静默从站必须失败");
    assert!(
        matches!(
            err,
            CollectorError::NoReply | CollectorError::ChecksumError | CollectorError::Timeout(_)
        ),
        "句柄应被保留（设备级失败），实际 {err:?}"
    );

    drop(bm);
}
