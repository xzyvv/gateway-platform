//! 导出 GatewayConfig 的 JSON Schema 到文件，供前端 Vue 用 ajv 做表单校验/生成。
//!
//! 用法：cargo run --example export_config_schema -- [output_path]
//! 默认输出到当前目录的 config.schema.json

use std::path::PathBuf;

fn main() {
    let path: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("config.schema.json"));
    match gateway_core::config::export_to_file(&path) {
        Ok(()) => println!("schema exported to {}", path.display()),
        Err(e) => {
            eprintln!("export failed: {}", e);
            std::process::exit(1);
        }
    }
}
