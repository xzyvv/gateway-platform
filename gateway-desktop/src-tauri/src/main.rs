//! 工业网关调试控制台 - 桌面客户端二进制入口
//!
//! 实际逻辑见 [`gateway_desktop::run`]，本文件仅作为二进制入口，
//! 便于 `cargo run -p gateway-desktop` / Tauri CLI 启动。

fn main() {
    gateway_desktop::run();
}
