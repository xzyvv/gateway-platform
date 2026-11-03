//! S7 Comm 客户端采集器（西门子 S7-300/400/1200/1500 PLC 直连）
//!
//! 基于 ISO-on-TCP (RFC 1006) 传输，TCP 端口 102。
//! 实现 TPKT + COTP + S7Comm 三层协议栈：
//!   1. COTP Connect（TSAP 寻址，区分机架/槽位）
//!   2. S7 Setup Communication（协商 PDU 大小）
//!   3. S7 Read Var（读取 I/Q/M/DB 区域数据）
//!   4. S7 Write Var（写 I/Q/M/DB 区域数据，功能码 0x05）
//!
//! 地址格式（Snap7 兼容）：
//!   - "DB1.DBD0"  → DB1, DWord, offset=0   (默认按 REAL 解析)
//!   - "DB1.DBW0"  → DB1, Word,  offset=0   (默认按 INT 解析)
//!   - "DB1.DBB0"  → DB1, Byte,  offset=0
//!   - "DB1.DBX0.0" → DB1, Bit,  offset=0, bit=0
//!   - "MD0"       → M区, DWord, offset=0
//!   - "MW0"       → M区, Word,  offset=0
//!   - "MB0"       → M区, Byte,  offset=0
//!   - "M0.0"      → M区, Bit,   offset=0, bit=0
//!   - "ID0" / "IW0" / "IB0" / "I0.0"  → I区（输入）
//!   - "QD0" / "QW0" / "QB0" / "Q0.0"  → Q区（输出）
//!
//! 配置：
//!   - bus.tcp_addr: "host:102?rack=0&slot=2"（默认 rack=0, slot=2 适用于 S7-300/400）
//!   - point.data_type: 可选，"real"/"dword"/"dint"/"word"/"int"/"byte"/"bit"
//!     未指定时按地址后缀推断（D→REAL, W→INT, B→BYTE, X→BIT）

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::Mutex;

use crate::south::protocol::{Collector, CollectorError, CollectorResult};
// 重导出供 tests 使用；mod.rs 自身不再直接消费
#[allow(unused_imports)]
pub(crate) use crate::south::protocol::s7comm::frames::{
    ItemSpec, AREA_DB, AREA_INPUTS, AREA_MARKERS, AREA_OUTPUTS, FUNC_READ, FUNC_SETUP, FUNC_WRITE,
    PDU_ACK, PDU_JOB, PDU_USERDATA, RET_NOT_FOUND, RET_OK, SZL_PARAM, TS_BIT, TS_BYTE,
};

/// 组件标识 SZL-ID：一个组件的标识（Siemens 文档 W#16#xy1C，Index=1 取本机 CPU）
const SZL_ID_COMPONENT_ID: u16 = 0x0011;

/// 组件标识 SZL-ID：全部组件的标识（较新 CPU，含更完整的组件条目）
const SZL_ID_COMPONENT_ALL: u16 = 0x001C;

/// S7 数据类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum S7DataType {
    Bit,   // 1 位
    Byte,  // 8 位无符号
    Word,  // 16 位无符号
    Int,   // 16 位有符号
    DWord, // 32 位无符号
    DInt,  // 32 位有符号
    Real,  // 32 位 IEEE 浮点
}

impl S7DataType {
    /// 返回该类型占用的字节数（Bit 在 S7 响应中占 1 字节）
    pub(crate) fn byte_size(&self) -> usize {
        match self {
            S7DataType::Bit => 1,
            S7DataType::Byte => 1,
            S7DataType::Word | S7DataType::Int => 2,
            S7DataType::DWord | S7DataType::DInt | S7DataType::Real => 4,
        }
    }

    /// 请求中的传输类型码（Bit 用 TS_BIT，其余用 TS_BYTE + 元素数=字节数）
    pub(crate) fn transport_size(&self) -> u8 {
        match self {
            S7DataType::Bit => TS_BIT,
            _ => TS_BYTE,
        }
    }

    /// 从字符串解析数据类型
    pub(crate) fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "bit" | "bool" => Some(S7DataType::Bit),
            "byte" => Some(S7DataType::Byte),
            "word" | "uint16" => Some(S7DataType::Word),
            "int" | "int16" => Some(S7DataType::Int),
            "dword" | "uint32" => Some(S7DataType::DWord),
            "dint" | "int32" => Some(S7DataType::DInt),
            "real" | "float" | "float32" => Some(S7DataType::Real),
            _ => None,
        }
    }
}

/// S7 解析后的地址
struct S7Address {
    area: u8,
    db_number: u16,
    byte_offset: u32,
    bit_offset: u8,
    data_type: S7DataType,
}

/// S7 Comm 采集器
pub struct S7CommCollector {
    addr: String,
    rack: u8,
    slot: u8,
    stream: Mutex<Option<TcpStream>>,
    bus_name: String,
    /// PDU 引用计数器（递增，用于请求-响应匹配）
    pdu_ref: Mutex<u16>,
    /// 协商后的 PDU 大小
    pdu_size: Mutex<u16>,
    /// 连接状态标志（用于 is_connected 快速检查）
    connected: AtomicBool,
    /// 单次请求 IO 超时（源自 bus.timeout_ms，统一南向超时来源）
    io_timeout: Duration,
}

impl S7CommCollector {
    pub fn new(addr: String, bus_name: String, io_timeout: Duration) -> Self {
        let (host_addr, rack, slot) = address::parse_addr(&addr);
        Self {
            addr: host_addr,
            rack,
            slot,
            stream: Mutex::new(None),
            bus_name,
            pdu_ref: Mutex::new(1),
            pdu_size: Mutex::new(480),
            connected: AtomicBool::new(false),
            io_timeout,
        }
    }

    /// 读取 SZL 系统状态列表（决策表 #12：S7 诊断入口）
    ///
    /// 惰性建链 → 取 PDU 引用 → 构 Userdata 帧 → 带超时收发；任何 IO 失败都会
    /// 清链并标记未连接，交由下一次调用自动重连（与 read/write 自愈策略一致）。
    pub(crate) async fn read_szl(
        &self,
        szl_id: u16,
        index: u16,
    ) -> CollectorResult<frames::SzlResponse> {
        if !self.is_connected() {
            self.connect().await?;
        }

        let mut guard = self.stream.lock().await;
        let stream = guard
            .as_mut()
            .ok_or_else(|| CollectorError::IoError("S7 not connected".into()))?;

        let pdu_ref = {
            let mut r = self.pdu_ref.lock().await;
            let v = *r;
            *r = r.wrapping_add(1);
            if *r == 0 {
                *r = 1;
            }
            v
        };

        let req = frames::build_szl_request(szl_id, index, pdu_ref);

        match tokio::time::timeout(self.io_timeout, stream.write_all(&req)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                *guard = None;
                self.connected.store(false, Ordering::Relaxed);
                return Err(CollectorError::IoError(format!("S7 send SZL: {}", e)));
            }
            Err(_) => {
                *guard = None;
                self.connected.store(false, Ordering::Relaxed);
                return Err(CollectorError::NoReply);
            }
        }

        let resp =
            match tokio::time::timeout(self.io_timeout, frames::read_tpkt_frame(stream)).await {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    *guard = None;
                    self.connected.store(false, Ordering::Relaxed);
                    return Err(e);
                }
                Err(_) => {
                    *guard = None;
                    self.connected.store(false, Ordering::Relaxed);
                    return Err(CollectorError::NoReply);
                }
            };

        frames::parse_szl_response(&resp)
    }
}

/// 分配下一个 PDU Reference（S7 协议层请求-响应配对用）
impl S7CommCollector {
    async fn next_pdu_ref(&self) -> u16 {
        let mut r = self.pdu_ref.lock().await;
        let v = *r;
        *r = r.wrapping_add(1);
        if *r == 0 {
            *r = 1;
        }
        v
    }

    /// 通用事务：取流锁 → 带超时发送 → 带超时读 TPKT 响应；任何 IO 失败
    /// 清链并标记未连接（与 read/write 自愈策略一致）。
    async fn transact(&self, req: &[u8]) -> CollectorResult<Vec<u8>> {
        let mut guard = self.stream.lock().await;
        let stream = guard
            .as_mut()
            .ok_or_else(|| CollectorError::IoError("S7 not connected".into()))?;
        match tokio::time::timeout(self.io_timeout, stream.write_all(req)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                *guard = None;
                self.connected.store(false, Ordering::Relaxed);
                return Err(CollectorError::IoError(format!("S7 send: {}", e)));
            }
            Err(_) => {
                *guard = None;
                self.connected.store(false, Ordering::Relaxed);
                return Err(CollectorError::NoReply);
            }
        }
        match tokio::time::timeout(self.io_timeout, frames::read_tpkt_frame(stream)).await {
            Ok(Ok(r)) => Ok(r),
            Ok(Err(e)) => {
                *guard = None;
                self.connected.store(false, Ordering::Relaxed);
                Err(e)
            }
            Err(_) => {
                *guard = None;
                self.connected.store(false, Ordering::Relaxed);
                Err(CollectorError::NoReply)
            }
        }
    }

    /// 多 item 读取事务：构多 item Read Var 帧 → 通用收发（决策表 #13）
    async fn transact_multi(&self, items: &[frames::ItemSpec]) -> CollectorResult<Vec<u8>> {
        let pdu_ref = self.next_pdu_ref().await;
        let req = frames::build_read_var_multi(items, pdu_ref);
        self.transact(&req).await
    }
}

mod address;
mod collector;
mod frames;
#[cfg(test)]
mod tests;
