//! 集成测试公共工具

use std::io::Write;

/// 写临时文件，返回路径
/// （仅部分测试目标使用；common 目录按单元独立编译，未使用方会误报 dead_code）
///
/// 返回 [`tempfile::TempPath`]：调用方拿到的是完整路径，可以继续用 SQLite 打开，
/// 但测试结束（变量 drop）时文件自动删除——不依赖「测试记得清理」的约定。
/// 文件名由 tempfile 随机生成，并行执行也不会互相覆盖。
#[allow(dead_code)]
pub fn write_temp_file(name: &str, content: &str) -> tempfile::TempPath {
    let suffix = match name.rsplit_once('.') {
        Some((_, ext)) => format!(".{ext}"),
        None => String::new(),
    };
    let mut f = tempfile::Builder::new()
        .prefix("gateway-integration-")
        .suffix(&suffix)
        .tempfile()
        .expect("create temp file");
    f.write_all(content.as_bytes()).expect("write temp file");
    // 关闭句柄、保留路径；TempPath 在 drop 时删除文件。
    f.into_temp_path()
}

pub mod mock_state;
