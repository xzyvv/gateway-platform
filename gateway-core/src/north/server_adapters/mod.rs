//! 北向从站适配器：将 RealtimeBus 数据桥接到 `north::server` 中的 OPC UA / Modbus 从站。
//!
//! 命名说明：`server/` 是**被动**北向服务端（对外暴露点位供外部主站读取）；
//! 本模块的 `*ServerAdapter` 是**数据推送适配器**，把 RealtimeBus 的采集数据喂给对应的从站。
//!
//! 设计：每个适配器自带内部 task + mpsc channel，实现 async fan-out。
//! - NorthDispatcher 收到一批点位 → 立即 fan-out 给每个 adapter.send(points)
//! - adapter.send 非阻塞（try_send），满则丢弃 + warn + 计数 metric
//! - adapter 内部 task 串行消费，避免被自己 update_channel 锁竞争阻塞
//! - apply_points / apply_registers 内部已经是同步操作（写 HashMap），
//!   拆出来隔离后即使慢点也不影响 NorthDispatcher 派发循环。
//!
//! 优雅停机：adapter task 监听 ShutdownToken，shutdown() 后立即 break。
//! NorthDispatcher 的广播停止 + adapter task 退出顺序由调用方保证。

mod base;
mod modbus_adapter;
// 依赖北向 OPC UA Server，故与之同一 feature 开关
#[cfg(feature = "opcua-server")]
mod opcua_adapter;

pub use modbus_adapter::ModbusServerAdapter;
#[cfg(feature = "opcua-server")]
pub use opcua_adapter::OpcUaServerAdapter;

/// 适配器内部 mpsc 通道默认容量
///
/// 与 broadcast Realtime bus buffer 同量级（1024），保证短时突发可缓冲。
/// 满则丢 + warn（避免适配器慢点写阻塞 NorthDispatcher）。
const ADAPTER_CHANNEL_CAPACITY: usize = 1024;

#[cfg(test)]
mod base_tests;
#[cfg(test)]
mod tests;
