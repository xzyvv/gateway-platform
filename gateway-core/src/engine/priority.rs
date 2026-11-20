//! 总线事务优先级（跨层共享契约）。
//!
//! # 为什么放在 engine
//!
//! `BusPriority` 同时被南向总线（`south::bus`：决定走普通通道还是控制通道）
//! 与北向 Modbus 网关桥接（`north::server::modbus_gateway_bridge`：按功能码
//! 判定外部主站的写指令）使用。若类型留在 `south`，north 就必须写
//! `use crate::south::bus::BusPriority`，直接违反 `arch_deps` 的分层门禁
//! （north 不得依赖 south）。
//!
//! 与 `engine::ports`（消除 engine ↔ north 循环）、`engine::bus`
//! （消除 engine ↔ pipeline 循环）的做法一致：**把双方都需要的契约放到中立层，
//! 由依赖方引用**。此处不引入任何业务逻辑，仅承载一个跨层枚举。

/// 总线事务优先级。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BusPriority {
    /// 控制类（写指令 / 反向下发）：进入独立控制通道，总线 task 优先消费。
    ///
    /// 现场「点了下发要等好几秒」的根因之一：写指令与轮询共用一个 FIFO 队列，
    /// 写必须排在前面所有采集事务之后，而单笔采集最坏耗时 `3T+400ms`。
    Control,
    /// 采集类（周期轮询）与调试直发：普通通道。
    Normal,
}
