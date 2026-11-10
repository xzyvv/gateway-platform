//! 架构依赖守护测试（gateway-api / Web 调试面）
//!
//! 用文本扫描固化分层规则：`web/api` 是 HTTP handler 层，只允许引用
//! gateway_core 的**公开契约层**，不得触碰引擎内部实现模块。
//!
//! ## 白名单：允许 `web/api` 引用的 gateway_core 顶层模块
//!
//! | 模块 | 用途 |
//! |------|------|
//! | `config`   | 配置 DTO / 枚举（handler 读写配置的载体） |
//! | `engine`   | 中立模型层（error / model / metrics / audit / supervision） |
//! | `crypto`   | 口令哈希与校验（auth handler） |
//! | `license`  | License 快照与错误类型 |
//! | `shared`   | 跨层共享的纯工具（cron 求值 / 时钟可信度 / 默认路径） |
//!
//! 曾经的 `platform` 让步已消除：`OtaState` 现已收敛到 `crate::web::traits`
//! 的 Web 契约视图（`OtaStateView`），handler 不再直接引用
//! `gateway_core::platform`。
//!
//! ## 禁止引用
//!
//! `south`（协议实现）、`north`（上送实现）、`queue`（存储内核）、
//! `pipeline`（边缘处理内核），以及**任何未列入白名单的模块**。
//! handler 应经 `crate::web::services` 门面或 AppState 的 trait 对象访问底层。
//!
//! 采用白名单而非黑名单：新增 gateway_core 顶层模块时测试会立即失败，
//! 强制显式评估「Web 是否可直接引用」，避免默认放行造成边界缓慢腐化。
//!
//! 引擎侧（north/queue/south）的内部分层断言见 `gateway-core/tests/arch_deps.rs`。
//!
//! 扫描前剥离注释与字符串字面量——文档注释中提及对偶模块不构成代码依赖。

use std::fs;
use std::path::{Path, PathBuf};

/// 允许 `web/api` 引用的 gateway_core 顶层模块白名单。
///
/// 维护指引：仅当某模块确属「Web 契约层」时才加入。`platform` 曾因
/// 读取 `OtaState` 而临时列入，现已收敛到 `crate::web::traits`
/// 的 Web 契约视图，故将其移出——新增模块时须显式评估后再加入。
///
/// `shared` 取代了已解散的 `support` 层：它只含跨层共享的纯工具
/// （cron 求值、时钟可信度、默认路径），不触碰引擎内部状态，
/// 因此属于可被 web/api 直接引用的契约层。
const ALLOWED_CORE_MODULES: &[&str] = &["config", "engine", "crypto", "license", "shared"];

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

/// 剥离源码中的块注释与行注释；字符串字面量原样保留。
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
                for rest in chars.by_ref() {
                    if rest == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '"' => {
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

/// 提取源码（已剥注释）中所有 `gateway_core::<module>` 引用的顶层模块名。
///
/// 只取 `gateway_core::` 之后的第一段标识符，例如：
/// - `gateway_core::engine::error::GatewayError` -> `engine`
/// - `use gateway_core::config::{DeviceConfig}`  -> `config`
fn referenced_core_modules(code: &str) -> Vec<String> {
    const NEEDLE: &str = "gateway_core::";
    let mut mods = Vec::new();
    let mut from = 0usize;
    while let Some(pos) = code[from..].find(NEEDLE) {
        let start = from + pos + NEEDLE.len();
        let rest = &code[start..];
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        if end > 0 {
            mods.push(rest[..end].to_string());
        }
        from = start + end;
    }
    mods
}

/// `web/api` 只允许引用 gateway_core 的公开契约层（白名单外一律违规）
#[test]
fn web_api_must_only_reference_allowlisted_core_modules() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/web/api");
    let mut files = Vec::new();
    collect_rs_files(&dir, &mut files);
    assert!(
        !files.is_empty(),
        "arch test: no .rs files found under {} (layout changed? update this test)",
        dir.display()
    );

    let mut checked = 0usize;
    for file in files {
        let src = fs::read_to_string(&file).unwrap_or_else(|e| {
            panic!("arch test: read {} failed: {}", file.display(), e);
        });
        let code = strip_comments(&src);

        // 防绕过：`use gateway_core as xxx;` 会让下面的路径扫描失效。
        assert!(
            !code.contains("use gateway_core as"),
            "架构违规：{} 使用了 `use gateway_core as ...` 别名导入。\n\
             请改为直接引用具体模块路径，以便架构边界可被静态扫描。",
            file.display()
        );

        for module in referenced_core_modules(&code) {
            checked += 1;
            assert!(
                ALLOWED_CORE_MODULES.contains(&module.as_str()),
                "架构违规：{} 引用了 `gateway_core::{}::`。\n\
                 分层规则：web/api（HTTP handler 层）只允许引用白名单模块 {:?}，\n\
                 不得直接触碰引擎内部实现（south / north / queue / pipeline 等）。\n\
                 若确需访问底层能力，请经 `crate::web::services` 门面或\n\
                 AppState 的 trait 对象（scheduler / queue / gateway / ota）；\n\
                 若确需共享数据类型，请上提至 `gateway_core::engine` 中立层\n\
                 （参考 DataPoint -> crate::engine::model 的先例）。\n\
                 文件：{}",
                file.display(),
                module,
                ALLOWED_CORE_MODULES,
                file.display()
            );
        }
    }

    assert!(
        checked > 0,
        "arch test: 未在 {} 下扫描到任何 gateway_core:: 引用，\n\
         扫描逻辑可能已失效（web/api 至少应引用 config / engine）。",
        dir.display()
    );
}
