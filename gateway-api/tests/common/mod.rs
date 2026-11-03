//! 集成测试公共工具

use std::io::Write;

/// 写临时文件，返回路径
/// （仅部分测试目标使用；common 目录按单元独立编译，未使用方会误报 dead_code）
#[allow(dead_code)]
pub fn write_temp_file(name: &str, content: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("gateway_integration_tests");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(content.as_bytes()).unwrap();
    path
}

pub mod mock_state;
