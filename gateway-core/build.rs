//! build script：注入 ts-rs 导出目录环境变量
//!
//! 让 #[ts(export)] 直接导出到 gateway-web/src/api/generated/，
//! 不再产生 gateway-core/bindings/ 中间目录。
//!
//! 选择 build.rs 而非 .cargo/config.toml 的原因：
//! - cargo 在 workspace 根目录运行时只读根目录 .cargo/config.toml，
//!   子 crate 的 .cargo/config.toml 不被读取。
//! - cargo:rustc-env 在编译期对过程宏可见，与运行目录无关。
fn main() {
    // 路径相对于 CARGO_MANIFEST_DIR（gateway-core/）
    println!("cargo:rustc-env=TS_RS_EXPORT_DIR=../gateway-web/src/api/generated");
}
