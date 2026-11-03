//! Golden 报文回归库（决策表 #10）
//!
//! 目标：把「最近新增/改动」的协议帧编解码结果固化为**仓库内可审阅**的十六进制
//! 文本样本（`gateway-core/tests/fixtures/golden/<protocol>/<scenario>.hex`），
//! 后续任何重构若改动了线上字节，会在此处被立即拦下。
//!
//! 为什么放在 crate 内部（`src/` 而不是 `tests/`）：
//! 各协议帧函数均为 `pub(crate)` 且 `frames` 模块私有，集成测试无法访问；
//! 故本模块以 `#[cfg(test)]` 挂载到 `south::protocol`，由各协议自己的
//! `tests.rs` 调用（`tests.rs` 是协议模块的子模块，可直接访问其私有 `frames`）。
//!
//! 样本分两类，都只用于**回归比对**：
//! - 规范推导：按协议规范/既有单元测试手工钉死的字节（如 BACnet/Modbus）；
//! - 编码器特征化冻结：由当前编码器输出冻结为基线（如 IEC104/BACnet 时间戳相关）。
//!
//! 出现差异即表示线上字节被改动，必须人工判定是 bug 还是有意变更。
//!
//! 刷新方式（仅当协议格式**有意**变更时；必须逐条复核样本 diff 后再提交）：
//! ```text
//! PowerShell: $env:GOLDEN_UPDATE="1"; cargo test -p gateway-core --lib golden
//! ```
//! 该开关会**重写**全部样本。为避免盲改，运行后务必 `git diff` 逐条确认。

use std::path::{Path, PathBuf};

/// 样本根目录（`gateway-core/tests/fixtures/golden`）
pub(crate) fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("golden")
}

/// 十六进制文本 → 字节序列
///
/// 允许：空白分隔、`0x` 前缀、`_` 分隔符、`#` 起至行尾的注释。
/// 非法 token 直接 panic：样本属人工资产，错误必须立即暴露而非静默跳过。
pub(crate) fn parse_hex(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for (lineno, raw) in text.lines().enumerate() {
        let line = match raw.find('#') {
            Some(i) => &raw[..i],
            None => raw,
        };
        for tok in line.split(|c: char| c.is_whitespace() || c == '_') {
            let t = tok
                .strip_prefix("0x")
                .or_else(|| tok.strip_prefix("0X"))
                .unwrap_or(tok);
            if t.is_empty() {
                continue;
            }
            match u8::from_str_radix(t, 16) {
                Ok(b) => out.push(b),
                Err(e) => panic!(
                    "golden 样本第 {} 行含非法十六进制 token {:?}：{}",
                    lineno + 1,
                    tok,
                    e
                ),
            }
        }
    }
    out
}

/// 字节序列 → 十六进制文本（每行 16 字节，行内空格分隔）
///
/// 仅输出纯十六进制，不含偏移/ASCII 列，保证 `parse_hex(encode_hex(b)) == b`。
pub(crate) fn encode_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 3 + 1);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            if i % 16 == 0 {
                s.push('\n');
            } else {
                s.push(' ');
            }
        }
        s.push_str(&format!("{:02x}", b));
    }
    s.push('\n');
    s
}

/// 读取样本（相对 `tests/fixtures/golden` 的路径，含 `.hex`）
pub(crate) fn load(rel: &str) -> Vec<u8> {
    let p = fixtures_root().join(rel);
    let text = std::fs::read_to_string(&p)
        .unwrap_or_else(|e| panic!("无法读取 golden 样本 {}：{}", p.display(), e));
    parse_hex(&text)
}

fn update_enabled() -> bool {
    std::env::var("GOLDEN_UPDATE")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// 断言 `got` 与样本逐字节一致。
///
/// `GOLDEN_UPDATE=1` 时改为写回样本（首次生成 / 有意变更），并打印变更摘要。
pub(crate) fn assert_golden(rel: &str, got: &[u8]) {
    let p = fixtures_root().join(rel);
    if update_enabled() {
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)
                .unwrap_or_else(|e| panic!("创建样本目录 {} 失败：{}", dir.display(), e));
        }
        let old = std::fs::read_to_string(&p).ok().map(|t| parse_hex(&t));
        std::fs::write(&p, encode_hex(got))
            .unwrap_or_else(|e| panic!("写入样本 {} 失败：{}", p.display(), e));
        match old {
            Some(o) if o == got => eprintln!("[golden] {rel} 未变化"),
            Some(o) => eprintln!(
                "[golden] {rel} 已更新：{} 字节 → {} 字节（请复核 diff）",
                o.len(),
                got.len()
            ),
            None => eprintln!("[golden] {rel} 已新建：{} 字节", got.len()),
        }
        return;
    }
    let expected = load(rel);
    if expected != got {
        panic!("{}", diff_report(rel, &expected, got));
    }
}

/// 生成可读差异报告（首个不一致下标 + 上下文）
fn diff_report(rel: &str, expected: &[u8], got: &[u8]) -> String {
    let mut msg = format!(
        "golden 样本不一致：{rel}\n  期望 {} 字节，实际 {} 字节",
        expected.len(),
        got.len()
    );
    let n = expected.len().min(got.len());
    let first = (0..n).find(|&i| expected[i] != got[i]);
    match first {
        Some(i) => {
            let lo = i.saturating_sub(8);
            let hi = (i + 9).min(n);
            msg.push_str(&format!(
                "\n  首个不一致字节 @ 偏移 {i}（0x{i:04X}）\n    期望[{lo}..{hi}] = {}\n    实际[{lo}..{hi}] = {}",
                hex_slice(&expected[lo..hi]),
                hex_slice(&got[lo..hi]),
            ));
        }
        None => msg.push_str("\n  前段字节一致，差异仅在长度"),
    }
    msg.push_str("\n  如为有意变更，设 GOLDEN_UPDATE=1 重跑并复核样本 diff。");
    msg
}

fn hex_slice(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{:02x}", x))
        .collect::<Vec<_>>()
        .join(" ")
}
