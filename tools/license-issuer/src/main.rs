//! License 签发工具（离线，Ed25519）
//!
//! 用法：
//! ```text
//! license-issuer keygen [--keys <dir>]            # 生成钥对（默认 ./keys）
//! license-issuer issue [参数]                     # 签发 .lic 文件
//! ```
//!
//! issue 参数：
//! - `--license-id`   唯一编号，如 LIC-2026-0001（必填）
//! - `--customer`     客户标识（必填）
//! - `--sku`          行业 SKU（sku-minimal / sku-standard / sku-building / sku-power / sku-plc / full）。
//!   传入后 `--tier` 与 `--max-points` 可省略，按**采购推荐**自动填充
//!   （推荐口径见 `gateway_core::domain::sku`）。
//! - `--tier`         core / standard / pro / ultra（与 `--sku` 二选一提供；均缺则报错）
//! - `--max-points`   v2.0 配额：最大采集点数（数字或 "none"=无限，可选；缺省按档位推荐值）
//! - `--max-protocols`  v2.0 配额：最大协议数（同上；v2.1 起四档默认不限）
//! - `--storage-days`   v2.0 配额：数据保留天数（同上；v2.1 起四档统一 30 天）
//! - `--northbound-channels`  v2.0 配额：北向通道数（同上；v2.1 起四档默认不限）
//! - `--machine-key`  主键指纹哈希（sha256 hex，64 字符，必填）
//! - `--machine-parts` 回退项单项哈希，逗号分隔 1~3 个（可选，缺项为空串）
//! - `--expires-at`   RFC3339 过期时间（可选，缺省 = 永久授权）
//! - `--keys`         私钥目录（默认 ./keys，keygen 生成）
//! - `--out`          输出 .lic 路径（默认 <license-id>.lic）
//!
//! 私钥红线：keys/ 目录已进 .gitignore，永不提交仓库 / CI / 产物。
//! 本工具随源码公开，但**签发用的私钥不在仓库里**——拿到本仓库源码 ≠ 能签发
//! 网关认可的授权文件（签发格式实现虽在公开的 gateway-core 里，但签名需要私钥）。
//! 社区使用者可自行 keygen + 替换 `PUBLIC_KEYS` 自签发自用。
//! 现场机器指纹哈希由网关端输出，操作员把哈希值原样填入本工具，
//! 两侧共用同一套规范化与哈希实现，无漂移风险.

use std::path::PathBuf;

use ring::signature::{Ed25519KeyPair, KeyPair};

use gateway_core::domain::sku::Sku;
use gateway_core::license::fingerprint::{compute_overall, hash_id_field};
use gateway_core::license::verify::{parse_rfc3339, sign_license, LicensePayload, Quota};
use gateway_core::license::Tier;

/// 解析 v2.0 配额参数：数字 → Some(n)；"none"/"infinite" → None；空 → 退出
fn parse_quota_opt(args: &[String], key: &str) -> Option<u32> {
    let raw = take_opt(args, key)?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        die(&format!("{} 不可为空，填数字或 'none'（无限）", key));
    }
    if trimmed.eq_ignore_ascii_case("none") || trimmed.eq_ignore_ascii_case("infinite") {
        return None;
    }
    match trimmed.parse::<u32>() {
        Ok(n) => Some(n),
        Err(_) => die(&format!("{} 非法：'{}'（期望数字或 'none'）", key, raw)),
    }
}

/// 判断某个可选参数是否出现过（用于区分「未指定」与「显式 none」）
fn has_opt(args: &[String], key: &str) -> bool {
    args.iter().any(|a| a == key)
}

fn die(msg: &str) -> ! {
    eprintln!("错误：{}", msg);
    std::process::exit(1);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("keygen") => cmd_keygen(&args[1..]),
        Some("issue") => cmd_issue(&args[1..]),
        Some("--help" | "-h" | "help") | None => print_usage(),
        Some(other) => die(&format!("未知子命令 {}，见 --help", other)),
    }
}

fn print_usage() {
    println!(
        "license-issuer — 网关 license 签发工具\n\n子命令:\n  keygen  生成 Ed25519 钥对\n  issue   签发 .lic 文件（参数见源码头注释）"
    );
}

/// 解析 `--key value` 形式参数
fn take_opt(args: &[String], key: &str) -> Option<String> {
    args.iter()
        .position(|a| a == key)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn keys_dir(args: &[String]) -> PathBuf {
    PathBuf::from(take_opt(args, "--keys").unwrap_or_else(|| "keys".into()))
}

// ===== keygen =====

fn cmd_keygen(args: &[String]) {
    let dir = keys_dir(args);
    let pk8_path = dir.join("license_ed25519.pk8");
    if pk8_path.exists() {
        die(&format!(
            "私钥已存在：{}（拒绝覆盖，防误删钥对）",
            pk8_path.display()
        ));
    }
    let doc =
        Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new()).expect("generate pkcs8");
    std::fs::create_dir_all(&dir).expect("create keys dir");
    std::fs::write(&pk8_path, doc.as_ref()).expect("write pk8");

    let keypair = Ed25519KeyPair::from_pkcs8(doc.as_ref()).expect("parse pkcs8");
    let pub_hex: String = keypair
        .public_key()
        .as_ref()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();
    std::fs::write(dir.join("public_key.hex"), &pub_hex).expect("write pubkey");

    println!("钥对已生成：");
    println!("  私钥：{}", pk8_path.display());
    println!("  公钥：{}", dir.join("public_key.hex").display());
    println!();
    println!("公钥 hex（填入 gateway-core/src/license/mod.rs 的 PUBLIC_KEY 后随版本发布）：");
    println!("{}", pub_hex);
    println!();
    println!("警告：私钥文件已由 .gitignore 排除，请再自行备份到密码管理器/离线介质，");
    println!("      私钥丢失后已签发的 license 仍有效，但无法再签发新授权（需换钥发版）。");
}

// ===== issue =====

fn cmd_issue(args: &[String]) {
    let license_id = take_opt(args, "--license-id").unwrap_or_default();
    let customer = take_opt(args, "--customer").unwrap_or_default();
    let tier_str = take_opt(args, "--tier").unwrap_or_default();
    let sku_str = take_opt(args, "--sku").unwrap_or_default();
    let mut max_points = parse_quota_opt(args, "--max-points");
    let max_protocols = parse_quota_opt(args, "--max-protocols");
    let storage_days = parse_quota_opt(args, "--storage-days");
    let northbound_channels = parse_quota_opt(args, "--northbound-channels");
    let machine_key_str = take_opt(args, "--machine-key").unwrap_or_default();
    let machine_raw = take_opt(args, "--machine-raw");
    let parts_str = take_opt(args, "--machine-parts").unwrap_or_default();
    let expires_at = take_opt(args, "--expires-at");
    let out = take_opt(args, "--out");

    if license_id.trim().is_empty() {
        die("缺少 --license-id");
    }
    if customer.trim().is_empty() {
        die("缺少 --customer");
    }

    // SKU → 采购推荐档位（仅作建议，不参与运行时判定；见 gateway_core::domain::sku）
    let sku = if sku_str.trim().is_empty() {
        None
    } else {
        let parsed = Sku::from_id(sku_str.trim()).unwrap_or_else(|| {
            die(&format!(
                "--sku 非法：{}（sku-minimal/sku-standard/sku-building/sku-power/sku-plc/full）",
                sku_str
            ))
        });
        println!(
            "SKU 推荐：{}（{}）→ 推荐档位 {:?}，推荐最大点数 {}",
            parsed.id(),
            parsed.label_zh(),
            parsed.recommended_tier(),
            parsed.recommended_max_points()
        );
        Some(parsed)
    };

    let tier = match tier_str.as_str() {
        "core" => Tier::Core,
        "standard" => Tier::Standard,
        "pro" => Tier::Pro,
        "ultra" => Tier::Ultra,
        "" => match sku {
            Some(s) => s.recommended_tier(),
            None => die("缺少 --tier（或用 --sku 由推荐档位自动填充）"),
        },
        other => die(&format!(
            "--tier 非法：{}（core/standard/pro/ultra）",
            other
        )),
    };

    // --max-points 未显式指定时：仅当给出了 --sku（明确的采购意图）才按推荐值填充。
    // 未给 --sku 时保持 quota=None（不写入 payload），由网关侧 tier_quotas 配置决定——
    // 这是既有行为，也让运维仍可通过 config.json 调整各档配额。
    if max_points.is_none() && !has_opt(args, "--max-points") {
        if let Some(s) = sku {
            max_points = Some(s.recommended_max_points());
        }
    }
    if expires_at.is_none() {
        println!(
            "提示：未指定 --expires-at，将签发**永久授权**（如需限期授权请显式传 --expires-at）。"
        );
    }

    // 主键：接受 sha256 hex（网关端 fingerprint 查询输出），或 --machine-raw 原始值现场哈希
    let machine_key = match machine_raw.as_deref() {
        Some(raw) => {
            println!("提示：--machine-raw 原始值仅在签发机本地哈希，不会写入日志或文件。");
            hash_id_field(raw).expect("--machine-raw 为空")
        }
        None => {
            if machine_key_str.len() != 64
                || !machine_key_str.bytes().all(|b| b.is_ascii_hexdigit())
            {
                die("--machine-key 必须是 64 位 sha256 hex（网关端 fingerprint 查询输出）");
            }
            machine_key_str.to_lowercase()
        }
    };

    // 回退项：1~3 个哈希，缺项补空串
    let mut parts: Vec<String> = parts_str
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
                die(&format!("--machine-parts 含非法哈希：{}", s));
            }
            s.to_lowercase()
        })
        .collect();
    if parts.len() > 3 {
        die("--machine-parts 最多 3 个");
    }
    parts.resize(3, String::new());

    // 整体指纹（审计字段）
    let part_refs: [&str; 3] = [&parts[0], &parts[1], &parts[2]];
    let machine = compute_overall(&machine_key, &part_refs);

    // v2.0 quota：4 个参数都为 None 时整体记 None（不写入 payload，向后兼容）；
    // 任意一个指定时整体记 Some，并按需填充（未指定字段默认 None = 无限）
    let quota = if max_points.is_none()
        && max_protocols.is_none()
        && storage_days.is_none()
        && northbound_channels.is_none()
    {
        None
    } else {
        Some(Quota {
            max_points,
            max_protocols,
            storage_days,
            northbound_channels,
        })
    };

    let payload = LicensePayload {
        v: 1,
        license_id: license_id.clone(),
        customer: customer.clone(),
        tier,
        quota,
        machine_key,
        machine,
        machine_parts: parts,
        issued_at: chrono_rfc3339_now(),
        expires_at: match &expires_at {
            Some(e) => {
                if parse_rfc3339(e).is_none() {
                    die("--expires-at 必须是 RFC3339 格式，如 2027-10-11T00:00:00Z");
                }
                Some(e.clone())
            }
            None => None,
        },
    };

    // 加载私钥并签名
    let pk8_path = keys_dir(args).join("license_ed25519.pk8");
    let pk8 = std::fs::read(&pk8_path).unwrap_or_else(|e| {
        die(&format!(
            "读取私钥失败 {}：{}（先执行 keygen）",
            pk8_path.display(),
            e
        ))
    });
    let keypair = Ed25519KeyPair::from_pkcs8(&pk8).expect("私钥格式非法");

    let json = serde_json::to_string(&payload).expect("serialize payload");
    let lic = sign_license(&json, &keypair);

    let out_path = out
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("{}.lic", license_id.replace(['/', '\\'], "_"))));
    std::fs::write(&out_path, &lic).expect("write lic");

    println!("已签发：{}", out_path.display());
    println!("  编号     ：{}", payload.license_id);
    println!("  客户     ：{}", payload.customer);
    println!("  档位     ：{:?}", payload.tier);
    match &payload.quota {
        Some(q) => {
            println!("  配额     ：");
            println!("    最大点数 ：{}", quota_disp(q.max_points));
            println!("    最大协议 ：{}", quota_disp(q.max_protocols));
            println!("    存储天数 ：{}", quota_disp(q.storage_days));
            println!("    北向通道 ：{}", quota_disp(q.northbound_channels));
        }
        None => println!("  配额     ：（未指定，v1.2 license 行为）"),
    }
    println!("  主键哈希 ：{}", payload.machine_key);
    println!(
        "  回退哈希 ：{} / {} / {}",
        empty_dash(&payload.machine_parts[0]),
        empty_dash(&payload.machine_parts[1]),
        empty_dash(&payload.machine_parts[2])
    );
    println!(
        "  过期时间 ：{}",
        payload.expires_at.as_deref().unwrap_or("永久")
    );
}

fn quota_disp(v: Option<u32>) -> String {
    match v {
        None => "无限".to_string(),
        Some(n) => n.to_string(),
    }
}

fn empty_dash(s: &str) -> &str {
    if s.is_empty() {
        "-"
    } else {
        s
    }
}

/// 当前时间 RFC3339（签发时间戳）
fn chrono_rfc3339_now() -> String {
    // 纯标准库：unix 秒 → RFC3339（签发时间无需高精度，直接复用网关侧解析回读校验）
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock before epoch")
        .as_secs() as i64;
    rfc3339_from_unix(now)
}

/// unix 秒 → RFC3339 UTC（小型实现，issuer 独立运行时不依赖 chrono）
fn rfc3339_from_unix(ts: i64) -> String {
    // days since epoch → civil date（Howard Hinnant 算法）
    let days = ts.div_euclid(86400);
    let secs = ts.rem_euclid(86400);
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, d, h, m, s
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rfc3339_from_unix() {
        assert_eq!(rfc3339_from_unix(0), "1970-01-01T00:00:00Z");
        // 2026-01-01T00:00:00Z = 1767225600
        assert_eq!(rfc3339_from_unix(1_767_225_600), "2026-01-01T00:00:00Z");
        // 闰日：2024-02-29T12:34:56Z = 1709210096
        assert_eq!(rfc3339_from_unix(1_709_210_096), "2024-02-29T12:34:56Z");
        // 产出必须能被网关侧 parse_rfc3339 解析
        assert!(parse_rfc3339(&rfc3339_from_unix(1_767_225_600)).is_some());
    }

    /// v2.0 quota 参数解析：数字 / "none" / 缺省 / 非法
    #[test]
    fn test_parse_quota_opt() {
        // 缺省 → None（无 key）
        let args = vec!["--other".to_string(), "x".to_string()];
        assert_eq!(parse_quota_opt(&args, "--max-points"), None);

        // 数字 → Some(n)
        let args = vec!["--max-points".to_string(), "5000".to_string()];
        assert_eq!(parse_quota_opt(&args, "--max-points"), Some(5000));

        // "none"（大小写不敏感） → None（无限）
        for kw in ["none", "NONE", "None", "infinite", "INFINITE"] {
            let args = vec!["--max-points".to_string(), kw.to_string()];
            assert_eq!(parse_quota_opt(&args, "--max-points"), None, "kw={}", kw);
        }

        // 空字符串 → die（不可在单测里测 die，写为断言行为：调用方应避免空值）
    }

    /// 非法数字应 die 退出（单测里通过 std::process::Command 间接验证更稳，
    /// 这里的实现覆盖了 happy path 与 "none" 语义）。
    #[test]
    fn test_quota_round_trip_through_serialize() {
        // 确认 Quota 序列化产物是 JSON null/数字（前端 ts-rs 类型能正确解析）
        let q = Quota {
            max_points: Some(5000),
            max_protocols: None,
            storage_days: Some(365),
            northbound_channels: None,
        };
        let s = serde_json::to_string(&q).unwrap();
        assert!(s.contains("\"max_points\":5000"));
        assert!(s.contains("\"storage_days\":365"));
        assert!(s.contains("\"max_protocols\":null"));
        assert!(s.contains("\"northbound_channels\":null"));
    }
}
