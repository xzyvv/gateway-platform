//! 架构依赖守护测试
//!
//! 用文本扫描固化分层规则，防止反向依赖回潮（纯源码扫描，不编译被测代码）：
//!
//! | 层 | 禁止引用 |
//! |----|----------|
//! | north | `crate::south::`（north 只认 engine 模型，协议产出经 queue/pipeline 中转） |
//! | queue | `crate::south::`、`crate::north::`（中立存储层，只依赖 engine） |
//! | south | `crate::north::`（采集层不知道上送层） |
//! | south/protocol | `crate::pipeline::`（协议解码层不认识处理管道） |
//! | config | `crate::license::`（配置层不反向依赖授权层） |
//! | domain | `crate::config::`、`crate::license::`、`crate::south::`、`crate::north::`、`crate::pipeline::`（中立层不依赖任何业务层） |
//!
//! Web 面（web/api 不得引用 gateway_core 的 north/south）随 Web 拆至 gateway-api，
//! 其断言见 `gateway-api/tests/arch_deps.rs`。
//!
//! 扫描前剥离注释（`//` 行注释与 `/* */` 块注释）与字符串字面量——
//! 文档注释中提及对偶模块（如 registry 的说明）不构成代码依赖，不应误报。
//!
//! 背景：2026-10 梯队三重构将 DataPoint 上提至 `crate::engine::model`、
//! crc16 上提至 `crate::engine::util::crc`，消除了 north→south 的全部反向依赖。
//! 后续两次下沉同样遵循"上提到中立层"的手法：
//! - 点位级换算公式 `apply_formula` 由 `pipeline::formula` 上提至
//!   `crate::engine::util::formula`（解除 south/protocol → pipeline 越层）；
//! - 授权档位与配额模型 `Tier` / `Quota` 由 `license` 下沉至
//!   `crate::domain::license`（解除 config ↔ license 双向循环）。

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

/// 收集一组路径下的全部 .rs 文件（路径可以是文件，也可以是目录）
fn collect_rs_paths(paths: &[PathBuf], out: &mut Vec<PathBuf>) {
    for path in paths {
        if path.is_dir() {
            collect_rs_files(path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path.clone());
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

/// 断言给定路径下所有 .rs 文件（剥注释后）不含任何禁止引用模式
///
/// `paths` 可混合传入目录与单个 .rs 文件。
fn assert_no_forbidden_at(paths: &[PathBuf], forbidden: &[&str]) {
    let mut files = Vec::new();
    collect_rs_paths(paths, &mut files);
    assert!(
        !files.is_empty(),
        "arch test: no .rs files found under {:?} (layout changed? update this test)",
        paths
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
                 分层规则：被测路径 {:?} 禁止引用该路径。\n\
                 若确需共享，请将类型/函数上提至 engine 中立层\n\
                 （参考 DataPoint -> crate::engine::model 的先例）。\n\
                 文件：{}",
                file.display(),
                pattern,
                paths,
                file.display()
            );
        }
    }
}

/// 断言 dir 下所有 .rs 文件（剥注释后）不含任何禁止引用模式
fn assert_no_forbidden(dir: &Path, forbidden: &[&str]) {
    assert_no_forbidden_at(&[dir.to_path_buf()], forbidden);
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

/// south/protocol（协议解析层）不得引用 pipeline（处理管道）
///
/// 协议层只负责"把字节解成原始值"，换算公式等共用能力属于中立层
/// `crate::engine::util::formula`。若协议层直接依赖 pipeline，
/// 会形成"协议层 → 处理层"的越层依赖：处理层的边缘过滤、虚拟点、
/// 入队等概念泄漏进协议解析。
#[test]
fn south_protocol_must_not_depend_on_pipeline() {
    assert_no_forbidden(
        &src_dir().join("south").join("protocol"),
        &["crate::pipeline::"],
    );
}

/// config（配置层）不得引用 license（授权层）
///
/// 二者曾经互指：`config::license` 需要 `Tier` / `Quota`，而 `LicenseManager`
/// 需要 `LicenseConfig`，形成 `config ↔ license` 双向循环。授权领域模型已下沉至
/// 中立层 `crate::domain::license`，配置层现在只引用中立层。
#[test]
fn config_must_not_depend_on_license() {
    assert_no_forbidden(&src_dir().join("config"), &["crate::license::"]);
}

/// domain（中立层）不得依赖任何业务层
///
/// 中立层是"上提/下沉共享类型"的落点：一旦它反向依赖 config / license /
/// south / north / pipeline，下沉就失去意义——依赖环只是被搬了个位置。
#[test]
fn domain_must_be_layer_neutral() {
    assert_no_forbidden_at(
        &[src_dir().join("domain.rs"), src_dir().join("domain")],
        &[
            "crate::config::",
            "crate::license::",
            "crate::south::",
            "crate::north::",
            "crate::pipeline::",
        ],
    );
}
