//! 浸泡测试骨架。
//!
//! 分层与职责：
//! - `spec`     规模 / 时长 / 采样 / 输出目录（全部可由环境变量覆盖）
//! - `port`     可切换行为的 Modbus RTU 从站桩（按从站静默 / 整总线静默 / 慢响应 / 异常码）
//! - `farm`     真实 BusManager + ProtocolRegistry + Scheduler + ProcessingPipeline 装配
//! - `timeline` 故障注入时间线（单设备静默 → 整总线静默 → 慢响应 → 异常码）
//! - `sampler`  周期采样指标并写 JSONL
//! - `verdict`  阈值判定 + 报告生成
//! - `runner`   长跑主循环与两个 `#[ignore]` 测试入口
//!
//! 设计取舍：**不使用 `cargo test` 做 24h 计时**，而是由 PowerShell 驱动脚本
//! 采样进程级 RSS / 句柄数；Rust 侧只负责跑真实运行时链路与打点。这样
//! 进程级指标（句柄泄漏、RSS）与业务指标（采集成功率、队列深度）来自同一时间轴。

pub mod farm;
pub mod port;
pub mod runner;
pub mod sampler;
pub mod spec;
pub mod timeline;
pub mod verdict;
