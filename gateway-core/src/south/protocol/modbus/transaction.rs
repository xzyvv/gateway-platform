//! Modbus TCP 并发事务池（T2.1）
//!
//! 目标：在同一 TCP socket 上同时投递多个事务，
//! 利用 Modbus TCP transaction ID 区分响应归属。
//!
//! 性能提升：
//! - 串行模式：N 个事务总耗时 = Σ(t_i)
//! - 并发模式：N 个事务总耗时 = max(t_i) （受 max_concurrent 限制）
//!
//! 用法：
//! ```ignore
//! use gateway_core::south::protocol::modbus::transaction::{TransactionPool, ModbusRequest};
//! use tokio::net::TcpStream;
//! use std::time::Duration;
//! # async fn demo(stream: TcpStream) -> Result<(), Box<dyn std::error::Error>> {
//! let pool = TransactionPool::with_stream(8, stream);
//! let req = ModbusRequest {
//!     slave_id: 1, function_code: 3,
//!     pdu: vec![0x00, 0x00, 0x00, 0x02], deadline: Duration::from_secs(3),
//! };
//! let resp = pool.submit(req).await?;
//! # Ok(()) }
//! ```
//!
//! #8: 完整实现——pool 持有 socket 读写半边：
//! - submit: 分配 tx_id（pool.next_tx_id 原子自增）→ 注册 waiter → 写 MBAP 帧 → 等响应
//! - reader_loop: 独立任务循环读 MBAP 头 + PDU → deliver(tx_id, payload)
//! - 连接断开 → abort_all 广播 Disconnected
//!
//! 原实现缺陷：
//! - dispatch_loop 未回传 tx_id 给调用方写 socket（事实上无人写 socket，submit 永远等不到响应）
//! - rand_tx_id 用 thread_local 而非 pool.next_tx_id，多线程下事务 ID 碰撞
//!   现统一由 pool 持有 writer，submit 内部完成"分配→注册→写→等"全流程。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::{oneshot, Mutex, Notify};
use tokio::task::JoinHandle;
use tokio::time::timeout;

// 子模块（从原文件拆出）：帧编解码 + reader_loop
mod frame;

// 单元测试：写路径超时 → abort_all → Disconnected 的不变式。
// 用例依赖 Linux socket 缓冲语义（tests.rs 内用文件级 `#![cfg(target_os = "linux")]` 剥离），
// 非 Linux 平台上该模块为空，不产生任何用例。
#[cfg(test)]
mod tests;

// 重导出以保持外部引用路径不变
pub(crate) use frame::build_mbap_frame;

// 本模块内部使用（with_stream spawn reader_loop）
use frame::reader_loop;

/// Modbus TCP MBAP 头（7 字节）
#[derive(Debug, Clone, Copy)]
pub struct MbapHeader {
    pub transaction_id: u16,
    pub protocol_id: u16, // 始终 0
    pub length: u16,      // 后续字节数（含 unit_id）
    pub unit_id: u8,
}

/// 单个事务的等待者：发出请求时创建，收到响应时通知
type Waiter = oneshot::Sender<Result<Vec<u8>, TransactionError>>;

/// 事务请求
#[derive(Clone)]
pub struct ModbusRequest {
    pub slave_id: u8,
    pub function_code: u8,
    /// PDU 数据（不含 function_code，由 function_code 字段单独提供）
    pub pdu: Vec<u8>,
    pub deadline: Duration,
}

/// 事务错误
#[derive(Debug, thiserror::Error)]
pub enum TransactionError {
    #[error("connection lost")]
    Disconnected,
    #[error("timeout after {0:?}")]
    Timeout(Duration),
    #[error("response malformed: {0}")]
    Malformed(String),
    #[error("max concurrent limit reached")]
    Busy,
    #[error("io: {0}")]
    Io(std::io::Error),
    #[error("pool not bound to a socket")]
    NotBound,
}

/// 事务池：管理所有在途事务，并持有 TCP socket 的读写半边。
///
/// #8: 完整实现 submit→write→wait→deliver 全流程。
/// - `new(max)`: 仅创建 waiter 注册表，不绑定 socket（submit 返回 NotBound）。
///   用于诊断/占位；真正采集应使用 `with_stream`。
/// - `with_stream(max, stream)`: 绑定 TCP 流，split 出读写半边，
///   writer 用 Mutex 串行化整帧写，reader 单独 spawn 循环读。
pub struct TransactionPool {
    max_concurrent: usize,
    next_tx_id: AtomicU16,
    active: Arc<Mutex<HashMap<u16, Waiter>>>,
    /// 写半边。None 表示未绑定 socket（new 构造）。
    /// 所有 submit 共用此 writer，Mutex 保证一次写一整帧 MBAP，避免半帧交错。
    writer: Option<Arc<Mutex<tokio::net::tcp::OwnedWriteHalf>>>,
    /// reader 任务句柄，shutdown 时 abort
    reader_handle: parking_lot::Mutex<Option<JoinHandle<()>>>,
    /// reader 退出信号
    shutdown: Arc<Notify>,
}

impl TransactionPool {
    /// 仅创建 waiter 注册表，不绑定 socket。
    /// 适用于初始化/测试；submit 会立即返回 NotBound。
    pub fn new(max_concurrent: usize) -> Arc<Self> {
        Arc::new(Self {
            max_concurrent,
            next_tx_id: AtomicU16::new(1),
            active: Arc::new(Mutex::new(HashMap::new())),
            writer: None,
            reader_handle: parking_lot::Mutex::new(None),
            shutdown: Arc::new(Notify::new()),
        })
    }

    /// 绑定 TCP 流构造事务池：split 读写半边，spawn reader_loop。
    pub fn with_stream(max_concurrent: usize, stream: TcpStream) -> Arc<Self> {
        let (read_half, write_half) = stream.into_split();
        let active: Arc<Mutex<HashMap<u16, Waiter>>> = Arc::new(Mutex::new(HashMap::new()));
        let shutdown = Arc::new(Notify::new());

        let pool = Arc::new(Self {
            max_concurrent,
            next_tx_id: AtomicU16::new(1),
            active: active.clone(),
            writer: Some(Arc::new(Mutex::new(write_half))),
            reader_handle: parking_lot::Mutex::new(None),
            shutdown: shutdown.clone(),
        });

        // 启动 reader loop：读到响应按 tx_id 路由，断开则 abort_all
        let pool_for_reader = pool.clone();
        let handle =
            crate::engine::supervision::spawn_catch_unwind("modbus_reader_loop", async move {
                reader_loop(read_half, pool_for_reader, shutdown).await;
            });
        *pool.reader_handle.lock() = Some(handle);

        pool
    }

    pub fn max_concurrent(&self) -> usize {
        self.max_concurrent
    }

    /// 提交一个事务：分配 tx_id → 注册 waiter → 写 MBAP 帧 → 等响应。
    ///
    /// #8: 这是原 dispatch_loop + rand_tx_id 缺失逻辑的完整实现：
    /// - tx_id 由 pool.next_tx_id 原子自增（跨线程唯一，不再 thread_local 碰撞）
    /// - 写 socket 由 pool 持有的 writer 完成（原实现无人写 socket）
    /// - 响应由独立 reader_loop 经 deliver 路由回来
    pub async fn submit(&self, request: ModbusRequest) -> Result<Vec<u8>, TransactionError> {
        let writer = self
            .writer
            .as_ref()
            .ok_or(TransactionError::NotBound)?
            .clone();

        // 1. 容量检查 + 分配 tx_id + 注册 waiter（持锁内完成，避免超卖）
        let (tx_id, reply_rx) = {
            let mut active = self.active.lock().await;
            if active.len() >= self.max_concurrent {
                return Err(TransactionError::Busy);
            }
            let tx_id = self.next_tx_id(); // 跳过 0
            let (reply_tx, reply_rx) = oneshot::channel();
            active.insert(tx_id, reply_tx);
            (tx_id, reply_rx)
        };

        // 2. 构造 MBAP 帧
        // C20: 帧构造可能失败（PDU > 253 字节），原实现用 `?` 直接返回，但此时
        // waiter 已插入 active 表且从未被移除——容量槽位永久泄漏。重复出现后
        // active.len() 长期 >= max_concurrent，pool 对所有后续事务恒返回 Busy，
        // 该 TCP 总线采集全线失效且不可自愈。失败时必须回收刚登记的 waiter。
        let frame = match build_mbap_frame(tx_id, &request) {
            Ok(f) => f,
            Err(e) => {
                self.active.lock().await.remove(&tx_id);
                return Err(e);
            }
        };

        // 用户 deadline + 网络开销，写与等响应共用同一口径
        let total = request_modbus_deadline(request.deadline);

        // 3. 写 socket（整帧原子写，Mutex 串行化）
        // 修复(C-1)：写路径原先完全没有超时。对端不读（设备掉电但仍持有连接、
        // 中间设备半开、内核发送缓冲被填满）时 `write_all` 会永久 pending，并且
        // 会一直占着 writer Mutex——同 pool 上后续所有 submit 都卡在
        // `writer.lock().await`；此时又不会返回 Disconnected/Io，总线主循环的
        // "清空 pool_slot 触发重连" 逻辑也不会执行，连接假装健康、永不重连，
        // 最终整组采集卡死且只能重启进程恢复。
        // 现给写路径补超时：写不出去说明这条 socket 已不可用，作废全部在途事务
        // 并返回 Disconnected，交由 tcp_task 置空 pool 并重连。
        {
            let mut w = writer.lock().await;
            // 拿到写锁后先确认本事务仍有效：若前面的写超时已 abort_all() 作废了
            // 全部在途事务，这里必须立刻退出，不能再往一个已知坏掉的 socket 写。
            // 否则每个排队者都会各自再等一个 `total`，尾延迟 = 队列长度 × timeout
            // （对端不读时，数百个在途事务会让整条总线停摆数十秒）。
            if !self.active.lock().await.contains_key(&tx_id) {
                return Err(TransactionError::Disconnected);
            }
            match timeout(total, w.write_all(&frame)).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    // 写失败：移除 waiter 避免泄漏槽位
                    self.active.lock().await.remove(&tx_id);
                    return Err(TransactionError::Io(e));
                }
                Err(_) => {
                    self.active.lock().await.remove(&tx_id);
                    self.abort_all().await;
                    return Err(TransactionError::Disconnected);
                }
            }
        }

        // 4. 等响应（用户 deadline + 网络开销）
        match timeout(total, reply_rx).await {
            Ok(Ok(resp)) => resp,
            Ok(Err(_)) => {
                // waiter 被 abort_all 撤销（连接断开）→ 视为断连
                Err(TransactionError::Disconnected)
            }
            Err(_) => {
                // 超时：移除 waiter 防止后续晚到响应顶替他人
                self.active.lock().await.remove(&tx_id);
                Err(TransactionError::Timeout(request.deadline))
            }
        }
    }

    /// 分配下一个 transaction ID（原子自增，跨线程唯一）。
    /// #8: 替代原 thread_local rand_tx_id。跳过 0（部分实现把 0 当无事务）。
    pub fn next_tx_id(&self) -> u16 {
        loop {
            let v = self.next_tx_id.fetch_add(1, Ordering::Relaxed);
            if v != 0 {
                return v;
            }
        }
    }

    /// 把收到的响应路由到对应的 waiter。reader_loop 调用。
    pub async fn deliver(&self, tx_id: u16, payload: Vec<u8>) {
        if let Some(reply) = self.active.lock().await.remove(&tx_id) {
            let _ = reply.send(Ok(payload));
        }
    }

    /// 把错误广播给所有 waiter（连接断开时调用）。
    pub async fn abort_all(&self) {
        let mut active = self.active.lock().await;
        let waiters: Vec<_> = active.drain().collect();
        drop(active);
        for (_, reply) in waiters {
            let _ = reply.send(Err(TransactionError::Disconnected));
        }
    }

    /// 当前在途事务数
    pub async fn active_count(&self) -> usize {
        self.active.lock().await.len()
    }

    /// 停止 reader loop（abort handle + notify）。
    pub fn shutdown(&self) {
        if let Some(h) = self.reader_handle.lock().take() {
            h.abort();
        }
        self.shutdown.notify_waiters();
    }
}

impl Drop for TransactionPool {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 给定 deadline + 网络开销，得到总超时
fn request_modbus_deadline(user_deadline: Duration) -> Duration {
    user_deadline + Duration::from_millis(50)
}
