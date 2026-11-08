//! `remote_diag` 的单元测试（O-6 从原单文件拆出）。
//!
//! 与被测代码同属 `remote_diag` 模块，`use super::*` 的可见性语义不变。
use super::log_archive::{
    archive_path, is_date_suffix, list_log_archives_blocking, newest_log_file,
};
#[cfg(feature = "remote-shell")]
use super::shell::{
    parse_command, truncate_command_output, validate_whitelist, MAX_CMD_OUTPUT_BYTES,
};
use super::tail::{tail_consumable, tail_resume_offset};
use super::TAIL_MAX_LOOKBACK;
use tempfile::TempDir;

#[cfg(feature = "remote-shell")]
fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

// ===== 白名单绕过修复：systemctl 首参语义 =====

/// 漏洞复现：旧实现“任一参数 == status 即放行”，`systemctl restart nginx status`
/// 会通过校验，而 systemctl 实际执行的是首参 restart
#[cfg(feature = "remote-shell")]
#[test]
fn whitelist_rejects_systemctl_restart_with_status_arg() {
    assert!(
        validate_whitelist("systemctl", &args(&["restart", "nginx", "status"])).is_err(),
        "must reject systemctl restart even if a later arg is 'status'"
    );
}

#[cfg(feature = "remote-shell")]
#[test]
fn whitelist_allows_systemctl_readonly_verbs() {
    assert!(validate_whitelist("systemctl", &args(&["status", "nginx"])).is_ok());
    assert!(validate_whitelist("systemctl", &args(&["is-enabled", "nginx"])).is_ok());
    assert!(validate_whitelist("systemctl", &args(&["list-units"])).is_ok());
    // 选项在动词之前也要正确识别“第一个非选项参数”
    assert!(validate_whitelist("systemctl", &args(&["--no-pager", "status", "nginx"])).is_ok());
}

#[cfg(feature = "remote-shell")]
#[test]
fn whitelist_rejects_systemctl_without_readonly_verb_first() {
    assert!(validate_whitelist("systemctl", &args(&[])).is_err());
    assert!(validate_whitelist("systemctl", &args(&["restart", "nginx"])).is_err());
    assert!(validate_whitelist("systemctl", &args(&["enable", "nginx"])).is_err());
    assert!(validate_whitelist("systemctl", &args(&["nginx", "status"])).is_err());
}

// ===== 白名单绕过修复：ip 只读约束 =====

#[cfg(feature = "remote-shell")]
#[test]
fn whitelist_allows_ip_readonly() {
    assert!(validate_whitelist("ip", &args(&["addr", "show"])).is_ok());
    assert!(validate_whitelist("ip", &args(&["-br", "a"])).is_ok());
    assert!(validate_whitelist("ip", &args(&["route"])).is_ok());
    assert!(validate_whitelist("ip", &args(&["neigh", "show"])).is_ok());
}

#[cfg(feature = "remote-shell")]
#[test]
fn whitelist_rejects_ip_write_operations() {
    assert!(validate_whitelist("ip", &args(&["link", "set", "eth0", "up"])).is_err());
    assert!(
        validate_whitelist("ip", &args(&["addr", "add", "10.0.0.1/24", "dev", "eth0"])).is_err()
    );
    assert!(validate_whitelist("ip", &args(&["route", "del", "10.0.0.0/24"])).is_err());
    assert!(validate_whitelist("ip", &args(&["link", "down", "eth0"])).is_err());
}

// ===== 白名单绕过修复：通用读文件 / 挂载命令已移除 =====

#[cfg(feature = "remote-shell")]
#[test]
fn whitelist_rejects_arbitrary_file_read_and_mount() {
    assert!(validate_whitelist("cat", &args(&["/etc/shadow"])).is_err());
    assert!(validate_whitelist("ls", &args(&["/root"])).is_err());
    assert!(validate_whitelist("mount", &args(&["/dev/sda1", "/mnt"])).is_err());
    assert!(validate_whitelist("rm", &args(&["-rf", "/"])).is_err());
}

// ===== Bug 150: 白名单绕过——basename 通过但执行完整路径 =====

/// "/tmp/evil/df" 的 basename 是白名单内的 "df"，旧实现会放行并执行该路径；
/// 现要求可执行文件必须位于可信目录内（此处 /tmp 与不存在路径均被拒绝）。
#[cfg(feature = "remote-shell")]
#[test]
fn whitelist_rejects_full_path_outside_allowed_dirs() {
    assert!(validate_whitelist("/tmp/evil/df", &args(&[])).is_err());
    assert!(validate_whitelist("/usr/bin/../tmp/evil/df", &args(&[])).is_err());
    // 相对路径同样不能逃逸可信目录
    assert!(validate_whitelist("./evil/df", &args(&[])).is_err());
}

// ===== 同类写选项加固：date / dmesg =====

#[cfg(feature = "remote-shell")]
#[test]
fn whitelist_rejects_date_set_and_dmesg_clear() {
    assert!(validate_whitelist("date", &args(&["-s", "2020-01-01"])).is_err());
    assert!(validate_whitelist("date", &args(&["--set=2020-01-01"])).is_err());
    assert!(validate_whitelist("date", &args(&["+%Y-%m-%d"])).is_ok());

    assert!(validate_whitelist("dmesg", &args(&["-c"])).is_err());
    assert!(validate_whitelist("dmesg", &args(&["--clear"])).is_err());
    assert!(validate_whitelist("dmesg", &args(&["--follow"])).is_err());
    assert!(validate_whitelist("dmesg", &args(&["-T"])).is_ok());
}

// ===== 危险字符仍被拦截 =====

#[cfg(feature = "remote-shell")]
#[test]
fn whitelist_rejects_shell_metacharacters() {
    assert!(validate_whitelist("df", &args(&["|", "sh"])).is_err());
    assert!(validate_whitelist("df", &args(&[";", "rm"])).is_err());
    assert!(validate_whitelist("df", &args(&["$(id)"])).is_err());
}

// ===== parse_command 基础行为 =====

#[cfg(feature = "remote-shell")]
#[test]
fn parse_command_splits_program_and_args() {
    let (p, a) = parse_command("df -h").unwrap();
    assert_eq!(p, "df");
    assert_eq!(a, vec!["-h".to_string()]);
    assert!(parse_command("   ").is_err());
}

// ===== r37-DIAG1：日志 tail 游标推进 =====

/// 契约：只消费到最后一个换行为止，半行留给下一轮（不得拆行广播）
#[test]
fn tail_consumable_stops_at_last_newline() {
    assert_eq!(
        tail_consumable(b"a\nb\nc"),
        4,
        "只消费到 'b\\n'，'c' 留给下一轮"
    );
    assert_eq!(tail_consumable(b"a\nb\n"), 4);
    // 无换行（单行超长）时整块消费，保证游标前进不死循环
    assert_eq!(tail_consumable(b"no-newline-here"), 15);
    assert_eq!(tail_consumable(b""), 0);
}

/// 契约：首次/轮转后（last_pos=0）大文件只回看末尾 1 MiB，小文件从头读
#[test]
fn tail_resume_offset_caps_initial_lookback() {
    // 已有游标：原样返回（增量 tail 不受回看上限制约）
    assert_eq!(tail_resume_offset(4096, 10 * TAIL_MAX_LOOKBACK), 4096);
    // 首次读取大文件：只回看末尾 1 MiB
    assert_eq!(
        tail_resume_offset(0, 10 * TAIL_MAX_LOOKBACK),
        10 * TAIL_MAX_LOOKBACK - TAIL_MAX_LOOKBACK
    );
    // 首次读取小文件：从头读
    assert_eq!(tail_resume_offset(0, 1024), 0);
    assert_eq!(tail_resume_offset(0, TAIL_MAX_LOOKBACK), 0);
}

// ===== r37-DIAG2：命令输出上限 =====

/// 契约：超长输出被截断到上限并追加提示；短输出原样返回
#[cfg(feature = "remote-shell")]
#[test]
fn command_output_is_bounded() {
    let short = "df output".to_string();
    assert_eq!(truncate_command_output(short.clone()), short);

    let big = "x".repeat(MAX_CMD_OUTPUT_BYTES + 5_000);
    let out = truncate_command_output(big);
    assert!(out.len() < MAX_CMD_OUTPUT_BYTES + 200, "截断后应有界");
    assert!(out.starts_with(&"x".repeat(16)));
    assert!(out.contains("output truncated"), "应追加截断提示");
}

/// 契约：多字节 UTF-8 下按字符边界截断，不得产生非法 UTF-8（此处仅要求不 panic 且可解码）
#[cfg(feature = "remote-shell")]
#[test]
fn command_output_truncation_is_utf8_safe() {
    // 构造恰好跨上限的中文字符串（每字 3 字节），确保截断点落在字符中间
    let unit = "中".repeat((MAX_CMD_OUTPUT_BYTES / 3) + 10);
    let out = truncate_command_output(unit);
    assert!(out.contains("output truncated"));
    // 上界内的部分必须仍是合法 UTF-8（String 保证，这里验证前缀未截断出乱码）
    assert!(out.starts_with("中中中"));
}

// ===== 日志归档：日期后缀校验与目录扫描 =====

#[test]
fn date_suffix_accepts_only_strict_yyyy_mm_dd() {
    assert!(is_date_suffix("2026-10-30"));
    // 非法日期写法、路径穿越、长度不符一律拒绝
    assert!(!is_date_suffix("2026-10-3"));
    assert!(!is_date_suffix("../etc"));
    assert!(!is_date_suffix("2026/10/30"));
    assert!(!is_date_suffix("2026-10-30.log"));
    assert!(!is_date_suffix(""));
}

#[test]
fn archive_path_rejects_traversal() {
    // 合法日期拼出同目录归档文件
    let p = archive_path("/var/log/gateway/gateway.log", "2026-10-30")
        .expect("valid date must produce path");
    assert_eq!(
        p.to_string_lossy().replace('\\', "/"),
        "/var/log/gateway/gateway.log.2026-10-30"
    );
    // 路径穿越写法直接拒绝，不允许拼出目录外的路径
    assert!(archive_path("/var/log/gateway/gateway.log", "../../etc/passwd").is_none());
}

#[test]
fn list_log_archives_reads_real_directory() {
    let (_dir, dir) = make_temp_dir();
    let base = dir.join("gateway.log");
    std::fs::write(dir.join("gateway.log.2020-01-01"), b"old\n").unwrap();
    std::fs::write(dir.join("gateway.log.2020-01-02"), b"newer\n").unwrap();
    std::fs::write(&base, b"current\n").unwrap();
    // 干扰项：不符合 `<base>.<date>` 命名的文件必须被忽略
    std::fs::write(dir.join("other.log"), b"x").unwrap();
    std::fs::write(dir.join("gateway.log.2020-01-02.bak"), b"x").unwrap();

    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let list = list_log_archives_blocking(&base.to_string_lossy());
    let dates: Vec<&str> = list.iter().map(|a| a.date.as_str()).collect();
    // 当期文件按修改时间推断为今天（必然晚于历史归档），并按日期倒序排在最前
    assert_eq!(dates, vec![today.as_str(), "2020-01-02", "2020-01-01"]);
    assert!(list[0].current, "当日文件应标记为当期");
    assert!(!list[1].current);
    assert_eq!(list[1].size_bytes, 6);

    // 当期文件缺失时，尾日志回退到最新的滚动文件
    std::fs::remove_file(&base).unwrap();
    let newest = newest_log_file(&base.to_string_lossy()).expect("dated file must be found");
    assert!(newest.to_string_lossy().ends_with("2020-01-02"));
}

/// 创建独立临时目录（测试并行时互不干扰）
fn make_temp_dir() -> (TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().to_path_buf();
    (dir, path)
}
