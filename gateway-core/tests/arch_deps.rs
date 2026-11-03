//! 架构依赖守护测试
//!
//! 用文本扫描固化分层规则，防止反向依赖回潮（纯源码扫描，不编译被测代码）：
//!
//! | 层 | 禁止引用 |
//! |----|----------|
//! | north | `crate::south::`（north 只认 engine 模型，协议产出经 queue/pipeline 中转） |
//! | queue | `crate::south::`、`crate::north::`（中立存储层，只依赖 engine） |
//! | south | `crate::north::`（采集层不知道上送层） |
//!
//! Web 面（web/api 不得引用 gateway_core 的 north/south）随 Web 拆至 gateway-api，
//! 其断言见 `gateway-api/tests/arch_deps.rs`。
//!
//! 扫描前剥离注释（`//` 行注释与 `/* */` 块注释）与字符串字面量——
//! 文档注释中提及对偶模块（如 registry 的说明）不构成代码依赖，不应误报。
//!
//! 背景：2026-10 梯队三重构将 DataPoint 上提至 `crate::engine::model`、
//! crc16 上提至 `crate::engine::util::crc`，消除了 north→south 的全部反向依赖。

use std::fs;
use std::path::{Path, PathBuf};

/// 递归收集目录下所有 .rs 文件
fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// 剥离源码中的块注释与行注释；字符串字面量原样保留（防止其中的
/// `//` 或 `/*` 被误判为注释起点）。
///
/// 简化处理：不解析 char 字面量与 raw string——本项目代码中不含
/// `"'"` 这类会让字符串判定错位的写法；即便错位，误扫区域也只会是
/// 少量字符，不会凭空产生 `crate::xxx::` 路径导致假阳性。
fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    let mut block_depth = 0usize;
    while let Some(c) = chars.next() {
        if block_depth > 0 {
            if c == '*' && chars.peek() == Some(&'/') {
                chars.next();
                block_depth -= 1;
            }
            continue;
        }
        match c {
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                block_depth += 1;
            }
            '/' if chars.peek() == Some(&'/') => {
                // 行注释：跳过至行尾（保留换行以维持行结构）
                for rest in chars.by_ref() {
                    if rest == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '"' => {
                // 字符串字面量：原样复制（处理转义）
                out.push(c);
                while let Some(s) = chars.next() {
                    out.push(s);
                    if s == '\\' {
                        if let Some(esc) = chars.next() {
                            out.push(esc);
                        }
                    } else if s == '"' {
                        break;
                    }
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// 断言 dir 下所有 .rs 文件（剥注释后）不含任何禁止引用模式
fn assert_no_forbidden(dir: &Path, forbidden: &[&str]) {
    let mut files = Vec::new();
    collect_rs_files(dir, &mut files);
    assert!(
        !files.is_empty(),
        "arch test: no .rs files found under {} (layout changed? update this test)",
        dir.display()
    );
    for file in files {
        let src = fs::read_to_string(&file).unwrap_or_else(|e| {
            panic!("arch test: read {} failed: {}", file.display(), e);
        });
        let code = strip_comments(&src);
        for pattern in forbidden {
            assert!(
                !code.contains(pattern),
                "架构违规：{} 引用了 \"{}\"。\n\
                 分层规则：被测目录 {} 禁止引用该路径。\n\
                 若确需共享，请将类型/函数上提至 engine 中立层\n\
                 （参考 DataPoint -> crate::engine::model 的先例）。\n\
                 文件：{}",
                file.display(),
                pattern,
                dir.display(),
                file.display()
            );
        }
    }
}

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// north（北向上送）不得引用 south（南向采集）
#[test]
fn north_must_not_depend_on_south() {
    assert_no_forbidden(&src_dir().join("north"), &["crate::south::"]);
}

/// queue（中立存储层）不得引用 south / north
#[test]
fn queue_must_be_layer_neutral() {
    assert_no_forbidden(
        &src_dir().join("queue"),
        &["crate::south::", "crate::north::"],
    );
}

/// queue 内核（`queue/core`）不得依赖具体存储后端（如 rusqlite）
///
/// 内核层只依赖 engine 模型与 `QueueStorage` trait；SQL / 连接等存储细节
/// 必须落在 `queue/storage` 下，防止存储实现反向渗入内核。
#[test]
fn queue_core_must_not_depend_on_storage_backend() {
    assert_no_forbidden(&src_dir().join("queue").join("core"), &["rusqlite"]);
}

/// south（南向采集）不得引用 north（北向上送）
#[test]
fn south_must_not_depend_on_north() {
    assert_no_forbidden(&src_dir().join("south"), &["crate::north::"]);
}
