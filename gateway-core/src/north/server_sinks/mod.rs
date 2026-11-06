//! 北向 server sink 适配器：将 RealtimeBus 数据桥接到 `north::server` 中的 OPC UA / Modbus Server。
//!
//! 命名说明：server/ 是**被动**北向服务端（对外暴露点位供外部主站读取）；
//! 本模块的 *Sink 是**数据推送适配器**，把 RealtimeBus 的采集数据喂给对应的 server。
//!
//! 设计：每个 sink 自带内部 task + mpsc channel，实现 async fan-out。
//! - ConversionManager 收到一批点位 → 立即 fan-out 给每个 sink.send(points)
//! - sink.send 非阻塞（try_send），满则丢弃 + warn + 计数 metric
//! - sink 内部 task 串行消费，避免被自己 update_channel 锁竞争阻塞
//! - apply_points / apply_registers 内部已经是同步操作（写 HashMap），
//!   拆出来隔离后即使慢点也不影响 ConversionManager 派发循环。
//!
//! 优雅停机：sink task 监听 ShutdownToken，shutdown() 后立即 break。
//! ConversionManager 的广播停止 + sink task 退出顺序由调用方保证。

mod base;
mod modbus_sink;
// 依赖北向 OPC UA Server，故与之同一 feature 开关
#[cfg(feature = "opcua-server")]
mod opcua_sink;

pub use modbus_sink::ModbusSink;
#[cfg(feature = "opcua-server")]
pub use opcua_sink::OpcUaSink;

/// sink 内部 mpsc 通道默认容量
///
/// 与 broadcast Realtime bus buffer 同量级（1024），保证短时突发可缓冲。
/// 满则丢 + warn（避免 sink 慢点写阻塞 ConversionManager）。
const SINK_CHANNEL_CAPACITY: usize = 1024;

#[cfg(test)]
mod base_tests;
#[cfg(test)]
mod tests;
