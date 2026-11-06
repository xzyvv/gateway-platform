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
        "armv7-unknown-linux-musleabihf" => "arm-linux-musleabihf",
        "armv7-unknown-linux-musleabi" => "arm-linux-musleabi",
        // 部分 crate（cc-rs）的 build.rs 自己写死了不带版本号的 rust 三元组
        "arm-unknown-linux-gnueabihf" => "arm-linux-gnueabihf",
        "arm-unknown-linux-gnueabi" => "arm-linux-gnueabi",
        "arm-unknown-linux-musleabihf" => "arm-linux-musleabihf",
        "arm-unknown-linux-musleabi" => "arm-linux-musleabi",
        "aarch64-unknown-linux-gnueabi" => "aarch64-linux-gnu",
        "x86_64-unknown-linux-musl" => "x86_64-linux-musl",
        _ => return None,
    };
    Some(zig.to_string())
}

fn main() {
    // 由 build.rs / ps1 注入实际 zig 路径；未注入时回退到 PATH 中的 zig。
    let zig = std::env::var("ZIG_EXE").unwrap_or_else(|_| "zig".to_string());

    // 目标三元组的兜底来源。
    //
    // 关键：rustc **只在编译阶段**把 `--target=<三元组>` 传给 C 编译器，
    // **链接阶段不会传**。链接时它直接调用链接器并给出 `-lgcc_s -lutil -lrt
    // -lpthread -lm -ldl -lc` 这类系统库名。此时若不给 zig 目标三元组，
    // zig 会按**宿主**（Windows）去找库，报
    //   unable to find dynamic system library 'util' using strategy 'no_fallback'
    // 且搜索路径是 `util.dll` 之类的 Windows 命名 —— 完全驴唇不对马嘴。
    //
    // 所以链接器模式必须由调用方显式告知目标：ps1 通过 ZIG_TARGET 传入
    // rust 三元组（或直接给 zig 三元组），这里转成 zig 三元组补上 `-target`。
    let explicit_target = std::env::var("ZIG_TARGET").ok().and_then(|t| {
        let t = t.trim();
        if t.is_empty() {
            None
        } else {
            Some(match rust_to_zig(t) {
                Some(z) => z,
                // 已经是 zig 三元组（arch-os-abi）则原样使用
                None => t.to_string(),
            })
        }
    });

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
    // 链接器模式下 rustc 不传 "--target="，需要用 ZIG_TARGET 补一个。
    // ar 模式不需要（归档与目标 ABI 无关）。
    let mut target_seen = false;
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
            target_seen = true;
        } else if let Some(inner) = a.strip_prefix("-Wl,") {
            // zig cc 对 `-Wl,` 后面跟的链接器参数有**白名单**（zig 0.14.1
            // `src/main.zig` 的 `.wl` 分支 + "Parse linker args" 循环），
            // 白名单外的参数直接 `fatal("unsupported linker arg")`。
            //
            // 具体场景：rustc 的 aarch64-unknown-linux-gnu / -musl target spec
            // 用 `pre_link_args` 无条件注入
            //   `-Wl,--fix-cortex-a53-843419`（Cortex-A53 erratum 缓解）
            // 它属于 **rustc 注入**，与 zig 无关（zig 0.14.1 全仓无
            // "fix-cortex" 字样），且 zig 自带的 linker 实现也不支持该缓解。
            //
            // 绕过手段实测全部无效（zig 0.14.1）：
            //   `-Wl,--fix-cortex-a53-843419`            → unsupported linker arg
            //   `-Xlinker --fix-cortex-a53-843419`       → unsupported linker arg
            //   裸 `--fix-cortex-a53-843419`             → Unknown Clang option
            //   `-fuse-ld=lld -Wl,--fix-cortex-...`      → unsupported linker arg
            // 白名单校验发生在**参数解析阶段**，早于链接器选择，故无法绕过。
            //
            // 因此这里**丢弃**该参数，只在 stderr 留痕。
            //
            // 影响范围评估：
            // 1. 该缓解只在真实 Cortex-A53（AArch64 早期核心）上影响正确性；
            // 2. **现场产物不是本地链路出的** —— `deploy/build-release.sh` /
            //    CI 的 cross-arm 作业用 Ubuntu 原生 `aarch64-linux-gnu-gcc`
            //    （GNU ld 支持 `--fix-cortex-a53-843419`，binutils ≥ 2.33），
            //    缓解照常生效；
            // 3. 本脚本只是 Windows 开发机上的**编译验证门禁**，产物不部署。
            // 故丢弃不改变线上行为，仅使本地门禁能跑通。
            if inner.split(',').any(|p| p.starts_with("--fix-cortex-a53-")) {
                eprintln!(
                    "zigwrap: 丢弃 zig cc 不支持的链接器参数 `-Wl,{}` \
                     （Cortex-A53 erratum 缓解；本地门禁链路产物不用于部署，\
                     现场产物由 GNU ld 链路生成、缓解照常生效）",
                    inner
                );
            } else {
                out.push(a.clone());
            }
        } else {
            out.push(a.clone());
        }
    }

    // 链接器模式补目标三元组。
    //
    // 放在参数循环**之后**（而不是循环内）：`-target` 是 zig cc 的全局选项，
    // 放在最前面才能确保 zig 在解析任何 `-l` 之前就知道目标 ABI。
    // 已从 "--target=" 拿到目标时不重复注入。
    if !target_seen && mode != "ar" {
        if let Some(z) = explicit_target {
            out.insert(0, "-target".to_string());
            out.insert(1, z);
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

