//! T2a 负载 / soak 测试入口。
//!
//! 复用 chaos harness 的注入 seam（`BusDeps` / 内存队列），并驱动**真实**运行时链路：
//! 节流串口桩（`throttle`）→ 真实 BusManager → ProtocolRegistry → 真实 Scheduler
//! → ProcessingPipeline → DataQueue + RealtimeBus → ConversionManager → 计数式北向桩。
//!
//! 全部场景 `#[ignore]`，默认 `cargo test` 不执行；手动运行：
//!   cargo test --test load -- --ignored --test-threads=1
//! 规模可用环境变量覆盖（LOAD_BUSES / LOAD_POINTS_PER_BUS / LOAD_BAUD / LOAD_SOAK_SECS）。

#![allow(dead_code)]
// 负载农场全部走 Modbus RTU（`registry.create("modbus", ...)`）：
// 协议裁剪构建里没有 modbus 采集器，整个测试二进制无从运行，整体剥离。
#![cfg(feature = "modbus")]

#[path = "chaos/harness.rs"]
mod chaos_harness;
#[path = "load/farm.rs"]
mod farm;
#[path = "load/north_stub.rs"]
mod north_stub;
#[path = "load/report.rs"]
mod report;
#[path = "load/scenarios.rs"]
mod scenarios;
#[path = "load/throttle.rs"]
mod throttle;
