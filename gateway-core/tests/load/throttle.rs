//! 自包含的“节流串口桩”（T2a 保真）。
//!
//! 不依赖 chaos harness 的任何改动：实现一个真实 `AsyncRead + AsyncWrite` 的
//! Modbus RTU 从站桩，按波特率推迟应答可见时刻，并叠加 RS485 半双工 turn-around，
//! 使负载测试的吞吐真实受波特率约束。经 `SerialHandle::from_stream` 注入
//! 真实 `BusManager`，因此 `do_transaction` / 帧提取 / 协议解析全部走生产代码。

use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// 串口时序参数。
#[derive(Clone, Copy, Debug)]
pub struct PortTiming {
    pub baud: u32,
    /// 单字节位数（8N1 = 10）
    pub bits_per_byte: u8,
    /// 半双工 turn-around：请求发完到从站开始应答的固定间隔
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

    /// `bytes` 个字节在线路上的传输耗时。
    pub fn byte_time(&self, bytes: usize) -> Duration {
        if self.baud == 0 {
            return Duration::ZERO;
        }
        let ns = (bytes as u64) * (self.bits_per_byte as u64) * 1_000_000_000 / (self.baud as u64);
        Duration::from_nanos(ns)
    }
}

/// Modbus RTU CRC16（独立实现，避免复用生产实现）。
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
    flush_count: usize,
}

/// 节流 Modbus RTU 从站桩（FC03 恒返回 42）。
#[derive(Clone)]
pub struct ThrottledModbusPort {
    st: Arc<Mutex<PortState>>,
    timing: PortTiming,
    slave_id: u8,
    /// 开头静默的事务次数（L3 恢复场景用）
    silent_first: usize,
    calls: Arc<AtomicUsize>,
}

impl ThrottledModbusPort {
    pub fn new(baud: u32, slave_id: u8, silent_first: usize) -> Self {
        Self {
            st: Arc::new(Mutex::new(PortState::default())),
            timing: PortTiming::new(baud),
            slave_id,
            silent_first,
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// 已完成的事务次数（= 收到的完整请求数）。
    pub fn flush_count(&self) -> usize {
        self.st.lock().flush_count
    }
}

impl AsyncWrite for ThrottledModbusPort {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.st.lock().write_acc.extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        // 取出本次请求并释放锁，再构造应答
        let req = {
            let mut st = self.st.lock();
            std::mem::take(&mut st.write_acc)
        };
        let n = self.calls.fetch_add(1, Ordering::Relaxed);

        // 请求帧：[slave, fc, addr_hi, addr_lo, cnt_hi, cnt_lo, crc_lo, crc_hi]
        let (reply, silent) = if n < self.silent_first || req.len() < 8 {
            (Vec::new(), true)
        } else {
            let count = u16::from_be_bytes([req[4], req[5]]).clamp(1, 125) as usize;
            let mut frame = vec![self.slave_id, 0x03, (count * 2) as u8];
            for _ in 0..count {
                frame.extend_from_slice(&42u16.to_be_bytes());
            }
            let crc = crc16(&frame);
            frame.push((crc & 0xFF) as u8);
            frame.push((crc >> 8) as u8);
            (frame, false)
        };

        let mut st = self.st.lock();
        st.flush_count += 1;
        st.silent = silent;
        st.pending = reply;
        st.pending_pos = 0;
        // 半双工时序：请求发完 + turn-around + 应答发完
        let rtt = self.timing.byte_time(req.len())
            + self.timing.turnaround
            + self.timing.byte_time(st.pending.len());
        st.read_due = Some(Instant::now() + rtt);
        st.timer_armed = false;
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for ThrottledModbusPort {
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
        // 应答尚未“到达线路”时挂起，并装配一次性唤醒定时器
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
