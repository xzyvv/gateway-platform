//! 运行时组件聚合
//!
//! `GatewayRuntime` 持有热更新所需的全部引擎组件（总线 / MQTT / NTP / 调度器 /
//! 队列 / 北向分发器），是配置 reload 时统一替换组件的入口。

mod gateway;

pub use gateway::{GatewayComponents, GatewayRuntime};

#[cfg(test)]
pub use gateway::gateway_runtime_without_buses_for_test;
