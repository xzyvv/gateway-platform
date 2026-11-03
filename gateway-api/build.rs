//! build script：注入 ts-rs 导出目录 + 构建信息
//!
//! Web 面（原 gateway-core/src/web）拆至本 crate 后，其中的 #[ts(export)] 类型
//! 仍须导出到 gateway-web/src/api/generated/。缺少本脚本时 ts-rs 会退回到
//! 默认目录（gateway-api/bindings/），导致前端契约绑定错位。
//!
//! 与 gateway-core/build.rs 同口径：cargo:rustc-env 在编译期对过程宏可见，
//! 路径相对于 CARGO_MANIFEST_DIR（gateway-api/）。
//!
//! S7：同时注入 BUILD_TIME / GIT_HASH。
//! 前端「系统信息」页此前固定显示「构建 unknown · Git Hash unknown」，根因就在
//! handlers/config.rs 用 `option_env!` 读取这两个变量，而本脚本从未设置它们
//! —— 编译期取不到就只能是 "unknown"，与运行时无关。
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    println!("cargo:rustc-env=TS_RS_EXPORT_DIR=../gateway-web/src/api/generated");

    // 构建时间：UTC，形如 2026-10-29T08:25:00Z（前端直接展示，无需再解析）
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=BUILD_TIME={}", format_utc(secs));

    // Git 短哈希：优先 git 命令；失败则尝试直接读取 .git/HEAD（离线/无 git 环境兜底）
    println!("cargo:rustc-env=GIT_HASH={}", git_hash());

    // HEAD 或引用变化时重新执行本脚本，保证版本号跟提交走
    println!("cargo:rerun-if-changed=build.rs");
    let git_head = std::path::Path::new("../.git/HEAD");
    if git_head.exists() {
        println!("cargo:rerun-if-changed=../.git/HEAD");
    }
    println!("cargo:rerun-if-env-changed=GATEWAY_BUILD_TIME");
    println!("cargo:rerun-if-env-changed=GATEWAY_GIT_HASH");
}

/// 允许用环境变量覆盖（CI 里可从 tag 取版本，比本地 git 更准）
fn git_hash() -> String {
    if let Ok(v) = std::env::var("GATEWAY_GIT_HASH") {
        if !v.trim().is_empty() {
            return v.trim().to_string();
        }
    }
    if let Ok(out) = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    "unknown".to_string()
}

fn format_utc(mut secs: u64) -> String {
    if let Ok(v) = std::env::var("GATEWAY_BUILD_TIME") {
        if !v.trim().is_empty() {
            return v.trim().to_string();
        }
    }
    if secs == 0 {
        return "unknown".to_string();
    }
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    secs = rem;
    let (h, mi, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    // 1970-01-01 起算的民用日 → (y,m,d)，用 Howard Hinnant 的 days_from_civil 逆运算
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", year, m, d, h, mi, s)
}
