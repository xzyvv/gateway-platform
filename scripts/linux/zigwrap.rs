// zig 包装器：让 rustc / cc-rs 用 zig 作为 Linux 交叉 C 编译器与链接器。
//
// 背景：开发机是 Windows，产物部署到 Linux。为了让 Linux 专属代码
// （`#[cfg(target_os = "linux")]`）在**每次提交**都被编译验证，
// 需要一个能交叉编译到 linux-gnu 的 C 工具链。
// 本机没有 gcc，但有 zig —— zig 自带完整的 libc 与 Linux 后端。
//
// 关键点：
// 1. rustc 传给 C 编译器的是 `--target=<rust 三元组>`（如
//    `--target=aarch64-unknown-linux-gnu`），zig 不认识该写法，需要换成
//    `-target <zig 三元组>`（如 `aarch64-linux-gnu`）。映射表见 `rust_to_zig`。
// 2. rustc 调用链接器时的第一个参数是 gcc 风格参数（如 `-Wl,...`），
//    调用 ar 时是 `cq` / `crs`。本包装器按参数形态自动判定 cc / ar 模式。
// 3. 编译成 exe 而非 .cmd：规避 cmd.exe 8191 字符的命令行长度上限
//    （rustc 传的参数可能很长）。

use std::process::Command;

/// rust 三元组 → zig 三元组。
///
/// zig 用 `arch-os-abi` 的三段式，且架构名与 rust 有差异
/// （如 rust 的 `armv7` 在 zig 里是 `arm`，需要补 `-eabi` / `-eabihf`）。
/// 返回 None 表示该 target 不在支持范围内，原样透传给 zig 让它自己报错。
fn rust_to_zig(triple: &str) -> Option<String> {
    let zig = match triple {
        "x86_64-unknown-linux-gnu" => "x86_64-linux-gnu",
        "aarch64-unknown-linux-gnu" => "aarch64-linux-gnu",
        "aarch64-unknown-linux-musl" => "aarch64-linux-musl",
        "armv7-unknown-linux-gnueabihf" => "arm-linux-gnueabihf",
        "armv7-unknown-linux-gnueabi" => "arm-linux-gnueabi",
        // 部分 crate（cc-rs）的 build.rs 自己写死了不带版本号的 rust 三元组
        "arm-unknown-linux-gnueabihf" => "arm-linux-gnueabihf",
        "arm-unknown-linux-gnueabi" => "arm-linux-gnueabi",
        "aarch64-unknown-linux-gnueabi" => "aarch64-linux-gnu",
        "x86_64-unknown-linux-musl" => "x86_64-linux-musl",
        _ => return None,
    };
    Some(zig.to_string())
}

fn main() {
    // 由 build.rs / ps1 注入实际 zig 路径；未注入时回退到 PATH 中的 zig。
    let zig = std::env::var("ZIG_EXE").unwrap_or_else(|_| "zig".to_string());

    let args: Vec<String> = std::env::args().collect();
    let mut mode = "cc".to_string();
    let mut start = 1usize;
    if let Some(first) = args.get(1) {
        let f = first.as_str();
        if f == "cc" || f == "ar" {
            mode = f.to_string();
            start = 2;
        } else if !f.starts_with('-')
            && f.len() <= 4
            && f.chars().all(|c| matches!(c, 'c' | 'q' | 'r' | 's' | 'u' | 'v' | 'D'))
        {
            // ar 模式：cc-rs 用 "cq"，rustc 用 "crs"
            mode = "ar".to_string();
        }
    }

    let mut out: Vec<String> = Vec::new();
    for a in args.iter().skip(start) {
        if let Some(triple) = a.strip_prefix("--target=") {
            match rust_to_zig(triple) {
                Some(z) => {
                    out.push("-target".to_string());
                    out.push(z);
                }
                // 未知 target：原样透传，zig 会给出明确错误
                None => out.push(a.clone()),
            }
        } else if let Some(inner) = a.strip_prefix("-Wl,") {
            // zig cc 对 `-Wl,` 后面跟的链接器参数有白名单，遇到不认识的会直接报
            // "unsupported linker arg" —— 但它内嵌的 lld 本身**认识**这个参数。
            //
            // 具体场景：rustc 在 aarch64 musl target 上会传
            // `-Wl,--fix-cortex-a53-843419`（Cortex-A53 erratum 缓解）。
            // zig cc 拒绝它，但 `zig ld.lld` 完全支持。
            // 解法：改用 `-Xlinker <arg>` 透传给 lld，绕开 zig cc 的白名单。
            if inner.starts_with("--fix-cortex-a53-") {
                out.push("-Xlinker".to_string());
                out.push(inner.to_string());
            } else {
                out.push(a.clone());
            }
        } else {
            out.push(a.clone());
        }
    }

    let status = Command::new(&zig)
        .arg(&mode)
        .args(&out)
        .status()
        .unwrap_or_else(|e| {
            eprintln!("zigwrap: failed to spawn zig ({}): {}", zig, e);
            std::process::exit(1);
        });
    std::process::exit(status.code().unwrap_or(1));
}

#[cfg(test)]
mod tests {
    use super::rust_to_zig;

    #[test]
    fn maps_known_rust_triples_to_zig() {
        assert_eq!(
            rust_to_zig("x86_64-unknown-linux-gnu").as_deref(),
            Some("x86_64-linux-gnu")
        );
        assert_eq!(
            rust_to_zig("aarch64-unknown-linux-gnu").as_deref(),
            Some("aarch64-linux-gnu")
        );
        assert_eq!(
            rust_to_zig("armv7-unknown-linux-gnueabihf").as_deref(),
            Some("arm-linux-gnueabihf")
        );
    }

    #[test]
    fn unknown_triple_is_not_silently_mapped() {
        // 未知 target 必须返回 None（调用方原样透传），
        // 不能猜一个错误架构——那会编译出「能过但架构不对」的产物。
        assert!(rust_to_zig("x86_64-pc-windows-msvc").is_none());
        assert!(rust_to_zig("aarch64-apple-darwin").is_none());
    }
}

