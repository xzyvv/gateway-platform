//! 可切换行为的 Modbus RTU 从站桩（浸泡专用）。
//!
//! 与 `load/throttle.rs` 的区别：本桩**支持运行时切换行为**，用于故障注入时间线：
//! - `silence_slave(id, on)`：单个从站静默（设备级故障，端口仍健康）
//! - `set_bus_down(on)`：整条总线静默（端口级故障）
//! - `set_slow(ms)`：应答延迟叠加（超时边界）
//! - `set_exception(on)`：返回 Modbus 异常码（合法帧 → 证明端口健康）
//!
//! 一条总线一个桩、多个从站号（请求帧第 1 字节即从站地址），因此能真实复现
//! “同一 485 总线上某台表坏、其他表正常”的场景——这正是自检帧要区分的边界。

use std::collections::{HashMap, HashSet};
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use gateway_core::config::BusConfig;
use gateway_core::south::bus::{SerialHandle, SerialOpener};

/// 从站桩的可编程行为。
pub struct SoakControl {
    silent_slaves: Mutex<HashSet<u8>>,
    bus_down: AtomicBool,
    slow_ms: AtomicU64,
    exception: AtomicBool,
    /// 每从站累计 (请求数, 应答数)
    stats: Mutex<HashMap<u8, (u64, u64)>>,
}

impl Default for SoakControl {
    fn default() -> Self {
        Self::new()
    }
}

impl SoakControl {
    pub fn new() -> Self {
        Self {
            silent_slaves: Mutex::new(HashSet::new()),
            bus_down: AtomicBool::new(false),
            slow_ms: AtomicU64::new(0),
            exception: AtomicBool::new(false),
            stats: Mutex::new(HashMap::new()),
        }
    }

    /// 单个从站静默开关（设备级故障注入）。
    pub fn silence_slave(&self, slave: u8, on: bool) {
        let mut s = self.silent_slaves.lock();
        if on {
            s.insert(slave);
        } else {
            s.remove(&slave);
        }
    }

    /// 整条总线静默开关（端口级故障注入）。
    pub fn set_bus_down(&self, on: bool) {
        self.bus_down.store(on, Ordering::Relaxed);
    }

    /// 叠加应答延迟（毫秒）。
    pub fn set_slow(&self, ms: u64) {
        self.slow_ms.store(ms, Ordering::Relaxed);
    }

    /// 当前是否处于慢响应模式（时间线据此配对进入/退出故障状态）。
    pub fn slow_ms_is_active(&self) -> bool {
        self.slow_ms.load(Ordering::Relaxed) > 0
    }

    /// 是否返回 Modbus 异常码（0x83 / 02 非法数据地址）。
    pub fn set_exception(&self, on: bool) {
        self.exception.store(on, Ordering::Relaxed);
    }

    /// 每从站累计 (从站号, 请求数, 应答数)。
    pub fn snapshot(&self) -> Vec<(u8, u64, u64)> {
        let mut v: Vec<(u8, u64, u64)> = self
            .stats
            .lock()
            .iter()
            .map(|(id, (req, ans))| (*id, *req, *ans))
            .collect();
        v.sort_by_key(|(id, _, _)| *id);
        v
    }

    fn record(&self, slave: u8, answered: bool) {
        let mut s = self.stats.lock();
        let e = s.entry(slave).or_insert((0, 0));
        e.0 += 1;
        if answered {
            e.1 += 1;
        }
    }

    fn is_silent(&self, slave: u8) -> bool {
        self.bus_down.load(Ordering::Relaxed) || self.silent_slaves.lock().contains(&slave)
    }
}

/// 串口时序参数（与 load/throttle.rs 保持一致）。
#[derive(Clone, Copy, Debug)]
pub struct PortTiming {
    pub baud: u32,
    pub bits_per_byte: u8,
    pub turnaround: Duration,
}

impl PortTiming {
    pub fn new(baud: u32) -> Self {
        Self {
            baud,
            bits_per_byte: 10,
            turnaround: Duration::from_millis(2),
        }
    }

    pub fn byte_time(&self, bytes: usize) -> Duration {
        if self.baud == 0 {
            return Duration::ZERO;
        }
        let ns = (bytes as u64) * (self.bits_per_byte as u64) * 1_000_000_000 / (self.baud as u64);
        Duration::from_nanos(ns)
    }
}

fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= b as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xA001
            } else {
                crc >> 1
            };
        }
    }
    crc
}

#[derive(Default)]
struct PortState {
    write_acc: Vec<u8>,
    pending: Vec<u8>,
    pending_pos: usize,
    read_due: Option<Instant>,
    timer_armed: bool,
    silent: bool,
}

/// 可切换 Modbus RTU 从站桩（FC03 读取返回 42）。
#[derive(Clone)]
pub struct SoakPort {
    st: Arc<Mutex<PortState>>,
    timing: PortTiming,
    ctl: Arc<SoakControl>,
}

impl SoakPort {
    pub fn new(baud: u32, ctl: Arc<SoakControl>) -> Self {
        Self {
            st: Arc::new(Mutex::new(PortState::default())),
            timing: PortTiming::new(baud),
            ctl,
        }
    }
}

impl AsyncWrite for SoakPort {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.st.lock().write_acc.extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let req = {
            let mut st = self.st.lock();
            std::mem::take(&mut st.write_acc)
        };

        if req.len() < 8 {
            // 帧还没收全（或不是 Modbus RTU），不产生应答
            let mut st = self.st.lock();
            st.silent = true;
            st.pending.clear();
            st.pending_pos = 0;
            st.read_due = None;
            return Poll::Ready(Ok(()));
        }

        let slave = req[0];
        let fc = req[1];
        let silent = self.ctl.is_silent(slave);
        let reply = if silent {
            Vec::new()
        } else if fc != 0x03 {
            // 非 FC03 用异常码回应：合法帧，证明端口健康
            exception_frame(slave, fc | 0x80, 0x01)
        } else if self.ctl.exception.load(Ordering::Relaxed) {
            exception_frame(slave, 0x83, 0x02)
        } else {
            let count = u16::from_be_bytes([req[4], req[5]]).clamp(1, 125) as usize;
            normal_fc03(slave, count)
        };

        self.ctl.record(slave, !silent);

        let mut st = self.st.lock();
        st.silent = silent;
        st.pending = reply;
        st.pending_pos = 0;
        let slow = Duration::from_millis(self.ctl.slow_ms.load(Ordering::Relaxed));
        let rtt = self.timing.byte_time(req.len())
            + self.timing.turnaround
            + slow
            + self.timing.byte_time(st.pending.len());
        st.read_due = Some(Instant::now() + rtt);
        st.timer_armed = false;
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for SoakPort {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let (silent, remaining, due) = {
            let st = self.st.lock();
            (
                st.silent,
                st.pending.len().saturating_sub(st.pending_pos),
                st.read_due,
            )
        };
        if silent || remaining == 0 {
            return Poll::Pending;
        }
        if let Some(d) = due {
            if Instant::now() < d {
                let need_arm = {
                    let mut st = self.st.lock();
                    if st.timer_armed {
                        false
                    } else {
                        st.timer_armed = true;
                        true
                    }
                };
                if need_arm {
                    let st2 = self.st.clone();
                    let waker = cx.waker().clone();
                    tokio::spawn(async move {
                        tokio::time::sleep_until(tokio::time::Instant::from_std(d)).await;
                        st2.lock().timer_armed = false;
                        waker.wake();
                    });
                }
                return Poll::Pending;
            }
        }
        let mut st = self.st.lock();
        let n = remaining.min(buf.remaining());
        let start = st.pending_pos;
        buf.put_slice(&st.pending[start..start + n]);
        st.pending_pos += n;
        Poll::Ready(Ok(()))
    }
}

fn normal_fc03(slave: u8, count: usize) -> Vec<u8> {
    let mut frame = Vec::with_capacity(3 + count * 2 + 2);
    frame.push(slave);
    frame.push(0x03);
    frame.push((count * 2) as u8);
    for _ in 0..count {
        frame.extend_from_slice(&42u16.to_be_bytes());
    }
    let crc = crc16(&frame);
    frame.push((crc & 0xFF) as u8);
    frame.push((crc >> 8) as u8);
    frame
}

fn exception_frame(slave: u8, fc: u8, code: u8) -> Vec<u8> {
    let mut frame = vec![slave, fc, code];
    let crc = crc16(&frame);
    frame.push((crc & 0xFF) as u8);
    frame.push((crc >> 8) as u8);
    frame
}

/// 浸泡测试专用串口打开器。
///
/// 与 chaos 测试的 `MockOpener`（基于预填充 `VecDeque<MockPort>`）不同，
/// `SoakOpener` 采用工厂模式：每次 `open()` 都创建一个**共享同一 `SoakControl`**
/// 的新 `SoakPort`。
///
/// 这保证了自检帧触发的后台重开（`ensure_background_reconnect`）能拿到新端口，
/// 且新端口仍受故障注入时间线（`set_bus_down` / `silence_slave` / `set_slow` /
/// `set_exception`）控制，使「故障窗口结束后总线自愈」路径可被真实验证。
/// 否则重开永远因 `MockOpener` 端口队列为空而失败，总线永久静默。
pub struct SoakOpener {
    baud: u32,
    ctl: Arc<SoakControl>,
}

impl SoakOpener {
    pub fn new(baud: u32, ctl: Arc<SoakControl>) -> Self {
        Self { baud, ctl }
    }
}

impl SerialOpener for SoakOpener {
    fn open(&self, _bus: &BusConfig) -> io::Result<SerialHandle> {
        let port = SoakPort::new(self.baud, self.ctl.clone());
        Ok(SerialHandle::from_stream(port))
    }
}
