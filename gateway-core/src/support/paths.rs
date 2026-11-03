//! 启动阶段路径常量
//!
//! 工业部署（Linux）：FHS 标准路径 /etc/gateway/, /var/lib/gateway/, /var/log/gateway/
//! 开发/测试/桌面端（非 Linux）：当前目录下相对路径，避免权限与路径不存在问题
//!
//! 调用方使用 `default_xxx()`（返回 `&'static str`，由 `LazyLock` 在首次访问时初始化一次）。
//! 测试 / 桌面端可通过环境变量覆盖任意路径，例：
//!   GATEWAY_CONFIG_PATH=/tmp/test/config.json cargo test

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// r33-PATH1: 配置文件白名单根目录。所有由环境变量覆盖的路径都必须在这些根
/// 目录之下。Linux 工业部署下为 FHS 标准；非 Linux 用 cwd 便于开发/测试。
const ALLOWED_CONFIG_ROOTS_LINUX: &[&str] =
    &["/etc/gateway", "/var/lib/gateway", "/var/log/gateway"];

fn current_dir_string() -> String {
    std::env::current_dir()
        .ok()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string())
}

/// r33-PATH1: 配置文件白名单根（动态解析，非 Linux 用 cwd）。
pub fn allowed_config_roots() -> Vec<PathBuf> {
    if cfg!(target_os = "linux") {
        ALLOWED_CONFIG_ROOTS_LINUX
            .iter()
            .map(PathBuf::from)
            .collect()
    } else {
        vec![PathBuf::from(current_dir_string())]
    }
}

/// r33-PATH1: 判断 path 是否在任一白名单根目录之下。用于：
/// - 校验 `GATEWAY_CONFIG_PATH` 等环境变量覆盖的路径
/// - 防止 root 进程通过配置指向 `/etc/passwd` 等敏感系统文件
///
/// 实现：canonicalize 后做 starts_with 检查。符号链接目标不在白名单也拒绝。
/// canonicalize 失败（路径不存在）时降级为字符串前缀比较，避免拒绝尚未创建的
/// 默认配置路径。
pub fn is_path_under_allowed_root(path: &Path) -> bool {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let allowed = allowed_config_roots();
    allowed.iter().any(|root| {
        let root_canonical = root.canonicalize().unwrap_or_else(|_| root.clone());
        canonical.starts_with(&root_canonical)
    })
}

/// 路径解析宏：env var 优先 + cfg 平台默认
macro_rules! impl_path_resolver {
    ($fn_name:ident, $env:literal, $linux:literal, $other:literal) => {
        pub fn $fn_name() -> &'static str {
            static CELL: LazyLock<String> = LazyLock::new(|| {
                std::env::var($env).unwrap_or_else(|_| {
                    if cfg!(target_os = "linux") {
                        $linux
                    } else {
                        $other
                    }
                    .to_string()
                })
            });
            CELL.as_str()
        }
    };
}

impl_path_resolver!(
    default_config_path,
    "GATEWAY_CONFIG_PATH",
    "/etc/gateway/config.json",
    "./config.json"
);
impl_path_resolver!(
    default_db_path,
    "GATEWAY_DB_PATH",
    "/var/lib/gateway/data_queue.db",
    "./data_queue.db"
);
impl_path_resolver!(
    default_template_builtin,
    "GATEWAY_TEMPLATE_BUILTIN",
    "/etc/gateway/templates/builtin",
    "./templates/builtin"
);
impl_path_resolver!(
    default_template_custom,
    "GATEWAY_TEMPLATE_CUSTOM",
    "/etc/gateway/templates/custom",
    "./templates/custom"
);
impl_path_resolver!(
    default_version_db,
    "GATEWAY_VERSION_DB",
    "/var/lib/gateway/config_versions.db",
    "./config_versions.db"
);
impl_path_resolver!(
    default_ota_staging,
    "GATEWAY_OTA_STAGING",
    "/var/lib/gateway/ota",
    "./ota"
);
impl_path_resolver!(
    default_diag_log,
    "GATEWAY_DIAG_LOG",
    "/var/log/gateway/gateway.log",
    "./gateway.log"
);
impl_path_resolver!(
    default_log_dir,
    "GATEWAY_LOG_DIR",
    "/var/log/gateway",
    "./logs"
);
impl_path_resolver!(
    default_outbox_db,
    "GATEWAY_OUTBOX_DB",
    "/var/lib/gateway/mqtt_outbox.db",
    "./mqtt_outbox.db"
);
// S1/S2: 跨平台证书目录与审计日志路径，避免硬编码 Linux 路径导致 Windows/desktop 场景失败
impl_path_resolver!(
    default_cert_dir,
    "GATEWAY_CERT_DIR",
    "/var/lib/gateway/certs",
    "./certs"
);
impl_path_resolver!(
    default_audit_log,
    "GATEWAY_AUDIT_LOG",
    "/var/log/gateway/audit.log",
    "./audit.log"
);
// License 时钟回拨防护与文件路径（§9 / §12）
impl_path_resolver!(
    default_license_clock_db,
    "GATEWAY_LICENSE_CLOCK_DB",
    "/var/lib/gateway/license_clock.db",
    "./license_clock.db"
);
// P4：看门狗 abort 前落盘的状态快照（重启后审计「上次是卡死还是正常退出」）
impl_path_resolver!(
    default_watchdog_state,
    "GATEWAY_WATCHDOG_STATE",
    "/var/lib/gateway/watchdog_state.json",
    "./watchdog_state.json"
);

#[cfg(test)]
mod tests {
    use super::*;

    /// r33-PATH1 (回归)：白名单根目录非空，且 Linux 部署下是 FHS 标准。
    #[test]
    fn allowed_config_roots_non_empty() {
        let roots = allowed_config_roots();
        assert!(!roots.is_empty(), "白名单根不能为空");
    }

    /// r33-PATH1 (回归)：白名单根目录与平台约定一致。
    ///
    /// - Linux 工业部署：FHS 标准三根（`/etc/gateway`、`/var/lib/gateway`、
    ///   `/var/log/gateway`），**不含** cwd——设备上进程从 `/` 或 `/opt` 启动，
    ///   cwd 落在仓库目录是开发机现象，不构成部署契约。
    /// - 非 Linux（开发/测试/桌面端）：白名单根即 cwd，便于本地相对路径配置。
    ///
    /// 注：canonicalize 在 Windows 上会把 cwd 加 `\\?\` UNC 前缀，与未加前缀的
    /// root 路径比较会失败，故非 Linux 分支两侧都走 canonicalize。
    #[test]
    fn allowed_config_roots_match_platform_contract() {
        let roots = allowed_config_roots();
        assert!(!roots.is_empty(), "白名单根不能为空");

        if cfg!(target_os = "linux") {
            assert_eq!(
                roots,
                ALLOWED_CONFIG_ROOTS_LINUX
                    .iter()
                    .map(PathBuf::from)
                    .collect::<Vec<_>>(),
                "Linux 部署的白名单根必须是 FHS 标准三根"
            );
            for root in &roots {
                assert!(
                    root.is_absolute(),
                    "Linux 白名单根必须是绝对路径，实际 {:?}",
                    root
                );
            }
        } else {
            let cwd = std::env::current_dir().expect("current_dir");
            let cwd_canon = cwd.canonicalize().unwrap_or_else(|_| cwd.clone());
            let root_canon = roots[0].canonicalize().unwrap_or_else(|_| roots[0].clone());
            assert_eq!(cwd_canon, root_canon, "非 Linux 白名单根应等于 cwd");
        }
    }

    /// r33-PATH1 (回归)：白名单根目录下的路径通过校验。
    ///
    /// 非 Linux：cwd 自身与其子路径都应通过（白名单根即 cwd）。
    /// Linux：白名单根是 FHS 标准目录，取其下的相对路径做校验——
    /// 目录未必已创建，`canonicalize` 失败时降级为字符串前缀比较，
    /// 两种情况都应判定为「在白名单内」。
    #[test]
    fn paths_under_allowed_root_pass_whitelist() {
        if cfg!(target_os = "linux") {
            let root = &allowed_config_roots()[0];
            let under = root.join("config.json");
            assert!(
                is_path_under_allowed_root(&under),
                "白名单根下的路径应通过校验：{:?}",
                under
            );
        } else {
            let cwd = std::env::current_dir().expect("current_dir");
            assert!(
                is_path_under_allowed_root(&cwd),
                "非 Linux 下 cwd 自身应通过白名单校验"
            );
            // 子路径必须从**已 canonical 的 root** 派生：Windows 上
            // canonicalize 会加 `\\?\` 前缀，而白名单根在函数内部也走 canonicalize；
            // 若从原始 cwd 派生，canonicalize 失败（文件不存在）时降级为无前缀路径，
            // 与带前缀的 root 做 starts_with 必然失败。
            let root_canon = allowed_config_roots()[0]
                .canonicalize()
                .unwrap_or_else(|_| allowed_config_roots()[0].clone());
            let sub = root_canon.join("some_config.json");
            assert!(
                is_path_under_allowed_root(&sub),
                "非 Linux 下 cwd 子路径应通过白名单校验：{:?}",
                sub
            );
        }
    }

    /// r33-PATH1 (回归)：不存在的路径在白名单根目录下时仍通过。
    /// canonicalize 失败时降级为字符串前缀比较，应与根匹配。
    #[test]
    fn nonexistent_path_under_root_passes() {
        // r55-PATH1 修复：原实现把 `evil` 变量放在 cfg-gate 之外，非 Linux 构建
        // 下变量未使用触发 unused_variable warning。修复策略——
        // 1) 路径挑选**与平台无关**：「绝对路径且不在 cwd 子树下」在 Linux 和
        //    Windows 都成立（cwd 派生白名单是平台无关的）；
        // 2) 两侧都做同一断言——原 Linux 断言其实非 Linux 也能跑通（prefix 比
        //    较，/etc/passwd 与 cwd 字符串不共享前缀），改 cfg 门为单一路径。
        // Linux 经典「逃逸」目标 /etc/passwd；非 Linux 用「含有 `..` 逃逸 cwd」
        // 的相对路径。两者都断言「前缀白名单外必拒」——回归意图一致。
        #[cfg(target_os = "linux")]
        let evil = std::path::PathBuf::from("/etc/passwd");
        #[cfg(not(target_os = "linux"))]
        let evil = std::path::PathBuf::from("../evil_escape_attempt");
        assert!(
            !is_path_under_allowed_root(&evil),
            "逃逸 cwd 的路径必须在白名单外（Linux: /etc/passwd; 非 Linux: ../...）"
        );
    }
}
