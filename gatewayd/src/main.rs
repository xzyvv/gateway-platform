//! 网关二进制入口
//!
//! main.rs 只保留 `#[tokio::main]` 入口，启动编排在 [`bootstrap`] 模块。

// 与 gateway-core 一致：生产路径（非测试编译）禁止 unwrap/expect。
// 二进制目标是独立的 crate root，不继承 gateway-core 的 crate 级 lint，故在此重复声明。
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

mod bootstrap;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    bootstrap::run().await
}
