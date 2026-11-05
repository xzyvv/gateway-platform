// zig 包装器：让 rustc / cc-rs 用 zig 作为 Linux 交叉 C 编译器与链接器。
//
// 背景：开发机是 Windows，产物部署到 Linux。为了让 Linux 专属代码
// （`#[cfg(target_os = "linux")]`）在**每次提交**都被编译验证，
// 需要一个能交叉编译到 x86_64-unknown-linux-gnu 的 C 工具链。
// 本机没有 gcc，但有 zig —— zig 自带完整的 libc 与 Linux 后端。
//
// 关键点：
// 1. rustc 传给 C 编译器的是 `--target=x86_64-unknown-linux-gnu`（rust 三元组），
//    zig 不认识该写法，需要换成 `-target x86_64-linux-gnu`（zig 三元组）。
// 2. rustc 调用链接器时的第一个参数是 gcc 风格参数（如 `-Wl,...`），
//    调用 ar 时是 `cq` / `crs`。本包装器按参数形态自动判定 cc / ar 模式。
// 3. 编译成 exe 而非 .cmd：规避 cmd.exe 8191 字符的命令行长度上限
//    （rustc 传的参数可能很长）。

use std::process::Command;

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
        if a == "--target=x86_64-unknown-linux-gnu" {
            out.push("-target".to_string());
            out.push("x86_64-linux-gnu".to_string());
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
