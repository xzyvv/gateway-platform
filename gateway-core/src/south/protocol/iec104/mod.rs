//! IEC 60870-5-104 客户端采集器
//!
//! 电力系统远动协议，TCP 传输（默认端口 2404）。
//! 实现最小子集：STARTDT 握手 + 总召唤 + 解析浮点量测值。
//!
//! 配置：
//! - bus.tcp_addr: 对端地址，如 "192.168.1.100:2404"
//! - point.address: 信息对象地址（IOA），如 "100"
//! - point.data_type: "float"(默认) / "normalized" / "scaled" / "single"
//!
//! 模块结构：
//! - frames.rs: 帧构建 / 解析 / 解码函数 + 协议常量
//! - params.rs: 协议时序参数（keepalive / 确认阈值 / 对时超时）与功能码映射
//! - keepalive.rs: 长连接 keepalive supervisor（探测与链路维护）
//! - interrogation.rs: 总召唤采集与建链对时
//! - command.rs: 控制方向命令下发
//! - collector.rs: `impl Collector`（建链 / 断连 / 读写入口，转调上述固有方法）
//! - tests/: 单元测试（按主题拆分，共享辅助函数在 tests/mod.rs）
//!
//! C-12 修复：长连接 keepalive（IEC 104 标准 t0/t1/t2/t3 计时器）
//!
//! 旧实现完全没有 keepalive 机制——server 端按 IEC 104 规范会定期发
//! `TESTFR_ACT` 探测 client，client 必须在 t1 时间（默认 15s）内回
//! `TESTFR_CON`；超时未响应 server 主动关连接。旧代码既不主动发探测
//! 也不响应 server 探测，导致：
//! - 现场网络抖动/中间设备超时丢包时 server 早已单方面关连接
//! - 本地 `connected=true` 标志永远不变，read 一直报 `NoReply`
//! - 无人触发重连
//!
//! 修复策略：
//! 1. `read` 入口惰性建链（生产路径不调 `Collector::connect`，与 mc 模块同一背景），
//!    建链成功后启动独立 keepalive supervisor task（spawn_catch_unwind 包装）
//! 2. supervisor 每 10s 主动发 `TESTFR_ACT`（0x43）探测，并在
//!    KEEPALIVE_RESPONSE_TIMEOUT 内等 `TESTFR_CON`（0x83）
//! 3. server 主动发来的 `TESTFR_ACT` → 立即回 `TESTFR_CON`
//!    （t1 = 15s 内不回，server 主动关连接）
//! 4. supervisor 与 read 共享同一把 `tokio::sync::Mutex`——用 `try_lock`
//!    抢锁（不阻塞 read 路径；抢不到说明 read 在用，链路活跃，跳过本次）
//! 5. 连续探测失败达 KEEPALIVE_MAX_CONSECUTIVE_FAILURES 次（网络抖动容忍）
//!    → 清空 stream + 标记 `connected=false`，下次 read 惰性建链时重连
//! 6. `disconnect` 关闭 supervisor + 清理 handle
//!
//! 注意：TESTFR act / con 是**不同**字节（0x43 / 0x83）——U-frame 控制域
//! b0/b1 恒为 1（用于与 I/S 格式区分），功能位 one-hot：
//! b6 = TESTFR act，b7 = TESTFR con。
//!
//! 本设计假设 bus.timeout_ms ≫ keepalive 周期（周期 10s，read timeout 通常 2-5s），
//! 因此 try_lock 大概率抢到。
//!
//! 修复（数据缓存失效）：
//! 旧实现 `read` 只要命中 `data` 缓存就直接返回，而缓存只在"未命中"时由一次
//! 总召唤填充、**从不失效** → 点位首次读取后永远返回同一个值（时间戳却每次刷新）。
//! 现改为：
//! 1. 每轮采集 `read_batch` 整批点位共享**一次**总召唤快照（IEC 104 总召唤天然
//!    返回全站数据，适合批量刷新）；`read` 单点调用执行一次总召唤；
//! 2. 收到 ASDU 类型 100 / COT=10（总召唤激活终止）即提前结束本轮，不再干等满
//!    io_timeout，缩短采集周期占用；
//! 3. 用本轮快照**整体替换**缓存——IOA 未出现在本轮响应中 → 返回 NoReply
//!    （宁可为坏质量，也不上报陈旧 Good 值）；
//! 4. 配套维护 N(S)/N(R) 序号 + 每 `ACK_EVERY_N_FRAMES` 个 I-frame 回一次 S-frame：
//!    重复发总召唤必须遵守序号与确认规则，否则 server 会因序号错乱或发送窗口
//!    （k=12）打满而停发/断链。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::south::protocol::common::asdu::IoaDataMap;

mod collector;
mod command;
mod frames;
mod interrogation;
mod keepalive;
mod params;

#[cfg(test)]
mod tests;

pub(crate) use params::{
    command_type_of, ACK_EVERY_N_FRAMES, CLOCK_SYNC_TIMEOUT, KEEPALIVE_INTERVAL,
    KEEPALIVE_MAX_CONSECUTIVE_FAILURES, KEEPALIVE_RESPONSE_TIMEOUT, TESTFR_ACT_CF, TESTFR_CON_CF,
};

/// IEC 104 采集器
pub struct Iec104Collector {
    addr: String,
    /// C-12: stream 用 Arc<Mutex<...>> 包装，让 supervisor 与 read 共享所有权
    stream: Arc<Mutex<Option<TcpStream>>>,
    /// C-12: keepalive supervisor task 句柄
    keepalive_handle: Mutex<Option<JoinHandle<()>>>,
    bus_name: String,
    /// 最近一次总召唤快照：IOA -> (value, quality, source_ts_millis)。每轮总召唤
    /// **整体替换**，不再"一次填充、永久命中"（旧实现导致点位值被冻结）。
    /// 第三元为 CP56Time2a 时标（决策表 #17），0 表示该点位无时标。
    data: Arc<Mutex<IoaDataMap>>,
    connected: Arc<AtomicBool>,
    /// 发送序号 N(S)：下一个发出的 I-frame 序号（建链后从 0 开始，每发一帧 +1）
    ///
    /// A4 修复（15-bit 半窗口）：IEC 104 规定 N(S)/N(R) 占用 15 位（0..0x7FFF），
    /// 最高位是 sign bit。`AtomicU16` 直接累加 → seq >= 32768 后 cf[1].bit7 = 1，
    /// 对端按「bit0/bit1 = 0」判定 I-frame 时会把 bit7 当 sign，把 I-frame 误判为
    /// S/U 帧 → 协议违规断链。累加处统一 & 0x7FFF 强制 15-bit 半窗口；
    /// 32767 后按 IEC 104 语义自然回绕到 0（不跳过 0，否则对端会观察到序号跳变）。
    seq_send: AtomicU16,
    /// 接收序号 N(R)：下一个期望收到的 I-frame 序号 = 已收 I-frame 数。
    ///
    /// S09: 用 `Arc` 包装，让 keepalive supervisor 在被 try_lock 选中、代为读取
    /// I-frame 时也能累加 N(R)，避免"supervisor 偷帧却不计数"导致接收序号错乱。
    seq_recv: Arc<AtomicU16>,
    /// 总召唤采集窗口 / 单次请求 IO 超时（源自 bus.timeout_ms，统一南向超时来源）
    io_timeout: Duration,
    /// 决策表 #28：公共地址（CA）字节数（1 或 2），源自 `bus.asdu_addr_len()`。
    asdu_addr_len: u8,
}

impl Iec104Collector {
    pub fn new(addr: String, bus_name: String, io_timeout: Duration, asdu_addr_len: u8) -> Self {
        Self {
            addr,
            stream: Arc::new(Mutex::new(None)),
            keepalive_handle: Mutex::new(None),
            bus_name,
            data: Arc::new(Mutex::new(HashMap::new())),
            connected: Arc::new(AtomicBool::new(false)),
            seq_send: AtomicU16::new(0),
            seq_recv: Arc::new(AtomicU16::new(0)),
            io_timeout,
            asdu_addr_len,
        }
    }
}

/// 修复(审查#B4)：热重载整体替换 `collectors` 时不会调用 `disconnect()`，
/// 若不在此收尾，keepalive supervisor 会继续运行、TCP 流也一直保持——旧连接
/// 不释放且任务泄漏（每次热重载都堆积一份，并可能互相偷帧）。
/// Drop 中不能 await：直接 abort supervisor 句柄，并 take 出 TcpStream 由 drop 关闭。
impl Drop for Iec104Collector {
    fn drop(&mut self) {
        if let Ok(mut h) = self.keepalive_handle.try_lock() {
            if let Some(handle) = h.take() {
                handle.abort();
            }
        }
        if let Ok(mut guard) = self.stream.try_lock() {
            let _ = guard.take();
        }
        self.connected.store(false, Ordering::Relaxed);
    }
}
