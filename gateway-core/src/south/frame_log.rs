//! 总线帧环形日志（抓包 A 档：本网关收发帧记录）
//!
//! 在 [`crate::south::bus::BusManager::transaction`] 与 `raw_transaction` 单点挂钩——
//! 所有协议、串口/TCP、采集与调试直发流量都经此入口，记录完整请求/响应帧。
//! 不镜像第三方主站流量（串口旁路 sniff 需额外硬件，属 B 档，暂不做）。
//!
//! 容量 500 帧（超出淘汰最旧）；进程内存态，重启清空。读取方：
//! `diagnostics/capture.rs`（配对展示）与 `protocol_tools/packet_capture.rs`。

use std::collections::VecDeque;

use once_cell::sync::Lazy;
use parking_lot::Mutex;

/// 单帧记录
#[derive(Debug, Clone)]
pub struct FrameRecord {
    /// 记录时刻（Unix 毫秒）
    pub ts_ms: i64,
    /// 总线标识（BusConfig.port_name）
    pub bus: String,
    /// true = 网关下发（tx）；false = 设备响应（rx）
    pub is_tx: bool,
    /// 完整帧字节（Modbus RTU 含 CRC；TCP 含 MBAP 头）
    pub bytes: Vec<u8>,
    /// 仅 rx：对应请求的往返耗时（含排队）
    pub latency_ms: Option<i32>,
}

/// 环形容量：500 帧 ≈ 9600bps 总线约 1-2 分钟流量，够现场「出问题后回头看」
const CAPACITY: usize = 500;

static FRAMES: Lazy<Mutex<VecDeque<FrameRecord>>> =
    Lazy::new(|| Mutex::new(VecDeque::with_capacity(CAPACITY)));

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 记录一帧。锁内仅 push/pop，O(1)，不阻塞采集热路径。
pub fn record(bus: &str, is_tx: bool, bytes: &[u8], latency_ms: Option<i32>) {
    if bytes.is_empty() {
        return;
    }
    let mut q = FRAMES.lock();
    if q.len() >= CAPACITY {
        q.pop_front();
    }
    q.push_back(FrameRecord {
        ts_ms: now_ms(),
        bus: bus.to_string(),
        is_tx,
        bytes: bytes.to_vec(),
        latency_ms,
    });
}

/// 快照读取：按 bus 过滤（None = 全部），新的在前，最多 limit 条。
pub fn snapshot(bus: Option<&str>, limit: usize) -> Vec<FrameRecord> {
    let q = FRAMES.lock();
    q.iter()
        .rev()
        .filter(|f| bus.map(|b| f.bus == b).unwrap_or(true))
        .take(limit)
        .cloned()
        .collect()
}

/// 清空全部帧（前端「清空」按钮语义：当前快照为用户可见真帧，清空必须真清）。
pub fn clear() {
    FRAMES.lock().clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_filters_by_bus_newest_first() {
        record("test-bus-fl", true, &[1, 2], None);
        record("test-bus-fl", false, &[3, 4], Some(12));
        record("other-bus-fl", true, &[9], None);
        let all = snapshot(Some("test-bus-fl"), 10);
        assert_eq!(all.len(), 2);
        assert!(!all[0].is_tx, "新的在前");
        assert_eq!(all[0].latency_ms, Some(12));
        assert_eq!(snapshot(Some("other-bus-fl"), 10).len(), 1);
    }

    #[test]
    fn empty_frame_ignored() {
        let before = snapshot(Some("test-bus-fl2"), 10).len();
        record("test-bus-fl2", true, &[], None);
        assert_eq!(snapshot(Some("test-bus-fl2"), 10).len(), before);
    }
}
