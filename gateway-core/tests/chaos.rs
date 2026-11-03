//! 故障注入测试入口（T1.3）
//!
//! 所有场景在 `scenarios.rs` 中定义。
//! 入口文件让 `cargo test --test chaos` 能正确发现并执行。

#[path = "chaos/harness.rs"]
mod harness;
#[path = "chaos/scenarios.rs"]
mod scenarios;
