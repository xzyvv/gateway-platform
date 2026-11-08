//! 集成测试公共工具
//!
//! `tests/common` 会被每个测试目标各自编译成一个 crate，而各目标只用到其中
//! 一部分构造器 / 字段（例如仅自愈测试用 `MockSchedulerWithDevices`）。
//! 逐项标注 `#[allow(dead_code)]` 既嘈杂又容易漏，故在此统一豁免夹具模块；
//! 公共夹具之外的真实死代码仍由默认 lint 提示。

#![allow(dead_code)]

use std::io::Write;

/// 写临时文件，返回路径。
///
/// 返回 [`tempfile::TempPath`]：调用方拿到的是完整路径，可以继续用 SQLite 打开，
/// 但测试结束（变量 drop）时文件自动删除——不依赖「测试记得清理」的约定。
/// 文件名由 tempfile 随机生成，并行执行也不会互相覆盖。
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
