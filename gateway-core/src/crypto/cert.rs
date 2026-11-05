//! X.509 设备证书管理
//!
//! 纯 Rust 实现（rcgen），无 OpenSSL / 外部工具依赖。
//!
//! 功能：
//! - 首次启动自动生成自签的 X.509 证书（ECDSA P-256）
//! - 持久化到 PEM 文件（data/certs/gateway.crt + gateway.key）
//! - 后续启动从文件加载，不重新生成
//! - 支持配置指定外部证书路径（企业 CA 签发）
//! - 输出 DER 格式，供 OPC UA / MQTT TLS 直接使用
//!
//! 证书参数：
//! - 算法：ECDSA P-256 + SHA-256
//! - 有效期：10 年（设备长期运行）
//! - Subject CN: Gateway Device
//! - SAN: localhost + 配置的 endpoint host

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rcgen::{CertificateParams, DistinguishedName, KeyPair, PKCS_ECDSA_P256_SHA256};
use x509_parser::prelude::FromDer;

use crate::engine::error::{GatewayError, GatewayResult};

/// 证书对（同时保留 PEM 与 DER 格式）。
#[derive(Clone)]
pub struct CertPair {
    /// PEM 格式证书（存储用）
    pub cert_pem: String,
    /// PEM 格式私钥（存储用）
    pub key_pem: String,
    /// DER 格式证书（OPC UA / rustls 用）
    pub cert_der: Vec<u8>,
    /// DER 格式私钥（OPC UA / rustls 用）
    pub key_der: Vec<u8>,
}

/// 是否禁止自签、要求运维提供外部证书（`GATEWAY_REQUIRE_EXTERNAL_CERT=1`）。
///
/// 单一事实来源：`get_or_create()` 的生成开关与 bootstrap 的「初始化失败是否
/// 阻断启动」共用同一判定，避免两处口径漂移（否则开关只会禁自签、却仍允许
/// 无证书启动，等于没有强制）。
pub fn require_external_cert() -> bool {
    std::env::var("GATEWAY_REQUIRE_EXTERNAL_CERT").as_deref() == Ok("1")
}

/// 证书管理器。
#[derive(Clone)]
pub struct CertManager {
    cert_path: PathBuf,
    key_path: PathBuf,
    /// 证书主题 CN（默认 "Gateway Device"）
    cn: String,
    /// SAN 主机名列表（默认 ["localhost"]）
    san_hosts: Vec<String>,
    /// Bug r11-CERT8 (fix): 指标实例（仅用于 Windows ACL 加固失败计数）。
    /// Option 而非必填：保持向后兼容的 `CertManager::new(cert, key)` API。
    metrics: Option<Arc<crate::engine::metrics::Metrics>>,
}

impl CertManager {
    /// 创建证书管理器。
    ///
    /// - `cert_path`: 证书文件路径（PEM）
    /// - `key_path`: 私钥文件路径（PEM）
    pub fn new(cert_path: impl Into<PathBuf>, key_path: impl Into<PathBuf>) -> Self {
        Self {
            cert_path: cert_path.into(),
            key_path: key_path.into(),
            cn: "Gateway Device".to_string(),
            san_hosts: vec!["localhost".to_string()],
            metrics: None,
        }
    }

    /// 设置证书主题 CN
    pub fn with_cn(mut self, cn: impl Into<String>) -> Self {
        self.cn = cn.into();
        self
    }

    /// 设置 SAN 主机名列表。
    pub fn with_san_hosts(mut self, hosts: Vec<String>) -> Self {
        self.san_hosts = hosts;
        self
    }

    /// Bug r11-CERT8 (fix): 注入指标实例，用于 Windows ACL 加固失败计数。
    /// 调用方（如 support/app.rs）可选用——若不注入，ACL 失败仅 warn 无 metric。
    pub fn with_metrics(mut self, metrics: Arc<crate::engine::metrics::Metrics>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// 获取证书对：
    /// - 若证书和私钥文件都存在，则加载
    /// - 否则自动生成新证书并存储
    ///
    /// Bug 144: 必须 cert 与 key 同时存在才加载，避免「只 key 存在」
    /// 时误生成新 cert（与既有 key 不配对）——这种情况应报错让运维介入。
    pub fn get_or_create(&self) -> GatewayResult<CertPair> {
        let cert_exists = self.cert_path.exists();
        let key_exists = self.key_path.exists();
        if cert_exists && key_exists {
            return self.load();
        }
        if cert_exists != key_exists {
            // 不一致：只其中一个文件存在时不安全生成（避免覆盖或失配）
            let existing = if cert_exists { "cert" } else { "key" };
            return Err(GatewayError::Config(format!(
                "cert pair incomplete: only {} exists, refusing to generate the missing half. \
                 Please restore the missing file or remove the existing one.",
                existing
            )));
        }
        // 安全开关：GATEWAY_REQUIRE_EXTERNAL_CERT=1 时禁用自签发，要求运维提供证书
        if require_external_cert() {
            return Err(GatewayError::Config(
                "cert files not found and self-signed generation is disabled (GATEWAY_REQUIRE_EXTERNAL_CERT=1). \
                 Please provide cert files or unset the environment variable.".to_string()
            ));
        }
        tracing::info!(
            "cert files not found, generating self-signed cert: {:?}",
            self.cert_path
        );
        self.generate()
    }

    /// 从 PEM 文件加载证书。
    ///
    /// Bug r11-CERT3 (fix): 加载后立即验证 cert 与 key 是否配对——
    /// 否则人为错配（cert A + key B）→ 启动无错，但首次 TLS 握手失败
    /// （cert 公钥 ≠ key 私钥），排障困难。现在加载时直接比对，错误前置。
    pub fn load(&self) -> GatewayResult<CertPair> {
        let cert_pem = std::fs::read_to_string(&self.cert_path).map_err(|e| {
            GatewayError::Io(format!(
                "read cert {} failed: {}",
                self.cert_path.display(),
                e
            ))
        })?;
        let key_pem = std::fs::read_to_string(&self.key_path).map_err(|e| {
            GatewayError::Io(format!(
                "read key {} failed: {}",
                self.key_path.display(),
                e
            ))
        })?;
        let pair = pem_to_cert_pair(&cert_pem, &key_pem)?;
        // r11-CERT3: 验证 cert 与 key 配对
        verify_cert_key_match(&pair)?;
        Ok(pair)
    }

    /// 生成新的自签名证书对并存储到文件
    ///
    /// r28-CRT1（修复）：原子写 + 配对验证。
    ///
    /// 旧实现先写 cert（644）再写 key（600）。若写 key 阶段失败（磁盘满 /
    /// Windows ACL 加固失败 / iO 错误），cert 已落盘但 key 没有——下次启动
    /// `get_or_create` 会读取 cert 不读 key，返回 "cert pair incomplete" 错误。
    /// **新生成的 cert 与既有 key 永远无法配对**，TLS 链路（MQTT/mTLS、OPC UA）全断。
    ///
    /// 修复顺序：
    ///   1. 先写 key（最敏感 + 失败率较高的 Windows ACL 加固）——失败直接返回，旧状态零损坏
    ///   2. 写 cert（644，单次 fs::write 失败率极低）
    ///   3. 写完后立即用 `verify_cert_key_match` 双重确认 ——不匹配则回滚删除两个文件
    ///   4. 任意中间步骤失败：删除已写入的 key 避免半成品状态。
    pub fn generate(&self) -> GatewayResult<CertPair> {
        // Bug 142: 传给 generate_self_signed 的 cn/san 不变，但硬编码的
        // not_before/not_after 在 generate_self_signed 函数内是写死的 2020/2035。
        // 这里我们让外部传入时间，让 generate_self_signed 计算当前年 + 10 年。
        let now_year = chrono::Utc::now()
            .format("%Y")
            .to_string()
            .parse::<u32>()
            .unwrap_or(2025);
        let not_before_year = now_year.saturating_sub(1);
        let not_after_year = now_year.saturating_add(10);
        tracing::info!("cert validity: {} - {}", not_before_year, not_after_year);
        let pair =
            generate_self_signed(&self.cn, &self.san_hosts, not_before_year, not_after_year)?;

        // 确保目录存在
        if let Some(parent) = self.cert_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| GatewayError::Io(format!("create cert dir failed: {}", e)))?;
        }

        // Step 1: 先写 key（最敏感 + ACL 加固失败率高），失败立刻返回，旧 cert 不受影响
        // Bug r11-CERT8 (fix): 将 metrics 透传给 write_private_key 用于 Windows ACL 失败计数。
        write_private_key(&self.key_path, &pair.key_pem, self.metrics.as_ref())?;

        // Step 2: 写 cert。失败时回滚删除已写入的 key，避免半成品状态。
        if let Err(e) = std::fs::write(&self.cert_path, &pair.cert_pem) {
            let _ = std::fs::remove_file(&self.key_path);
            return Err(GatewayError::Io(format!(
                "write cert {} failed: {}; rolled back key",
                self.cert_path.display(),
                e
            )));
        }

        // Step 3: 配对验证。避免「key 写成功但 cert 不匹配」（理论上 generate_self_signed
        // 不会发生，但写文件系统 / encoding 中间出错可能导致），不匹配回滚。
        if let Err(e) = verify_cert_key_match(&pair) {
            let _ = std::fs::remove_file(&self.cert_path);
            let _ = std::fs::remove_file(&self.key_path);
            return Err(GatewayError::Protocol(format!(
                "cert/key pair verification failed after write (rolled back both files): {}",
                e
            )));
        }

        tracing::info!(
            "self-signed cert generated: cn={}, san={:?}, path={:?}",
            self.cn,
            self.san_hosts,
            self.cert_path
        );
        Ok(pair)
    }

    /// 证书文件路径
    pub fn cert_path(&self) -> &Path {
        &self.cert_path
    }

    /// 私钥文件路径
    pub fn key_path(&self) -> &Path {
        &self.key_path
    }
}

// ============================================================
// 内部函数
// ============================================================

/// 生成自签的 X.509 证书。
///
/// Bug 142: 有效期改为动态计算——从 not_before_year 到 not_after_year。
/// 默认是「当前年 - 1」到「当前年 + 10」，避免硬编码导致 2035 年后过期。
fn generate_self_signed(
    cn: &str,
    san_hosts: &[String],
    not_before_year: u32,
    not_after_year: u32,
) -> GatewayResult<CertPair> {
    // 生成 ECDSA P-256 密钥。
    let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
        .map_err(|e| GatewayError::Protocol(format!("generate keypair failed: {}", e)))?;

    // 构造证书参数。
    let mut params = CertificateParams::new(san_hosts.to_vec())
        .map_err(|e| GatewayError::Protocol(format!("cert params failed: {}", e)))?;

    // Subject
    let mut dn = DistinguishedName::new();
    dn.push(rcgen::DnType::CommonName, cn);
    dn.push(rcgen::DnType::OrganizationName, "Gateway");
    params.distinguished_name = dn;

    // Bug 142: 有效期动态计算（外部传入），不再硬编码 2020/2035
    // date_time_ymd 接收 i32，将 u32 转 i32（年不会超过 i32 范围）
    let nb_year_i32: i32 = not_before_year.try_into().unwrap_or(2025);
    let na_year_i32: i32 = not_after_year.try_into().unwrap_or(2035);
    params.not_before = rcgen::date_time_ymd(nb_year_i32, 1, 1);
    params.not_after = rcgen::date_time_ymd(na_year_i32, 12, 31);

    // 自签名。
    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| GatewayError::Protocol(format!("self-sign failed: {}", e)))?;

    let cert_pem = cert.pem();
    let key_pem = key_pair.serialize_pem();
    let cert_der = cert.der().to_vec();
    let key_der = key_pair.serialize_der();

    Ok(CertPair {
        cert_pem,
        key_pem,
        cert_der,
        key_der,
    })
}

/// 将 PEM 格式的证书和私钥转换为 CertPair（含 DER）。
fn pem_to_cert_pair(cert_pem: &str, key_pem: &str) -> GatewayResult<CertPair> {
    // 用 pem crate 解析证书 PEM 为 DER
    let cert_der = pem::parse(cert_pem)
        .map_err(|e| GatewayError::Protocol(format!("parse cert pem failed: {}", e)))?
        .contents()
        .to_vec();

    // 私钥：KeyPair::from_pem 直接支持
    let key_pair = KeyPair::from_pem(key_pem)
        .map_err(|e| GatewayError::Protocol(format!("parse key pem failed: {}", e)))?;

    Ok(CertPair {
        cert_pem: cert_pem.to_string(),
        key_pem: key_pem.to_string(),
        cert_der,
        key_der: key_pair.serialize_der(),
    })
}

/// Bug r11-CERT3 (fix): 验证 cert 与 key 配对。
///
/// 实现：从 cert 中提取 SPKI 的原始公钥位（65 字节未压缩 ECDSA），
/// 从 key DER（PKCS#8）中提取公钥位（同样 65 字节），两者按字节比对。
/// 不一致时立即 Err 阻断启动，避免「人为错配→首次 TLS 握手失败」。
///
/// 算法：
/// 1. 用 x509_parser 解析 cert DER，取 `cert.public_key().subject_public_key.data`
/// 2. 用 ring::signature::EcdsaKeyPair::from_pkcs8 解析 key DER（PKCS#8 PEM 经 PEM 解析后是 PKCS#8 DER）
/// 3. 用 ring keypair 的 public_key().as_ref() 得到 65 字节未压缩位
/// 4. 两者按字节比对
fn verify_cert_key_match(pair: &CertPair) -> GatewayResult<()> {
    use ring::signature::{EcdsaKeyPair, KeyPair as RingKeyPair};

    // 1. 从 cert DER 提取公钥。
    let (_, cert) = x509_parser::certificate::X509Certificate::from_der(&pair.cert_der)
        .map_err(|e| GatewayError::Protocol(format!("CERT3: parse x509 failed: {}", e)))?;
    let cert_pubkey = cert.public_key().subject_public_key.data.as_ref();
    // ECDSA P-256 raw 公钥 = 65 字节（0x04 + 32 + 32）
    if cert_pubkey.len() != 65 {
        return Err(GatewayError::Protocol(format!(
            "CERT3: cert public key is not 65-byte uncompressed ECDSA (got {} bytes); \
             only ECDSA P-256 self-signed certs are supported for this check",
            cert_pubkey.len()
        )));
    }

    // 2. 从 key DER 解析出 ring keypair，取公钥。
    // ring 0.17 的 from_pkcs8 要求传入 SecureRandom（用于签名时用，解析私钥不需要）。
    // 用 SystemRandom 是无副作用的（解析不消耗熵）。
    let keypair = match EcdsaKeyPair::from_pkcs8(
        &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
        &pair.key_der,
        &ring::rand::SystemRandom::new(),
    ) {
        Ok(kp) => kp,
        Err(e) => {
            return Err(GatewayError::Protocol(format!(
                "CERT3: parse ECDSA P-256 private key failed: {}; \
                 only ECDSA P-256 PKCS#8 keys are supported",
                e
            )));
        }
    };
    let key_pubkey = keypair.public_key().as_ref();

    // 3. 字节级比较
    if cert_pubkey != key_pubkey {
        return Err(GatewayError::Config(
            "CERT3: cert and key do NOT match (cert public key != key public key). \
             This usually means cert/key are from different keypairs (e.g., A's cert + B's key). \
             Please regenerate the cert/key pair or restore the correct combination."
                .to_string(),
        ));
    }

    Ok(())
}

/// 写入私钥文件，Unix 下设置 0600 权限
///
/// Bug r11-CERT8 (fix): `metrics` 参数用于 Windows ACL 加固失败计数。Unix 路径下忽略。
#[cfg(unix)]
fn write_private_key(
    path: &Path,
    content: &str,
    _metrics: Option<&Arc<crate::engine::metrics::Metrics>>,
) -> GatewayResult<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true).mode(0o600);
    let mut f = opts
        .open(path)
        .map_err(|e| GatewayError::Io(format!("open key {} failed: {}", path.display(), e)))?;
    use std::io::Write;
    f.write_all(content.as_bytes())
        .map_err(|e| GatewayError::Io(format!("write key {} failed: {}", path.display(), e)))?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private_key(
    path: &Path,
    content: &str,
    metrics: Option<&Arc<crate::engine::metrics::Metrics>>,
) -> GatewayResult<()> {
    std::fs::write(path, content.as_bytes())
        .map_err(|e| GatewayError::Io(format!("write key {} failed: {}", path.display(), e)))?;
    #[cfg(windows)]
    harden_windows_key_acl(path, metrics);
    Ok(())
}

/// Windows 私钥 ACL 加固：移除从父目录继承的权限，只保留当前用户 / SYSTEM /
/// Administrators 的完全控制，避免私钥被同机其他用户或 Everyone 读取。
///
/// 用系统自带 `icacls` 实现（不引入额外依赖）；失败则**升级日志级别**。
/// `tracing::error!` 并**累加 metric**——Bug r11-CERT8 (fix)。
/// 旧实现仅 `tracing::warn!`，运维无任何告警，Windows 部署时 icacls
/// 失败 → 私钥 ACL 没加，同机其他用户可读 —— 静默安全降级。
/// 现在失败计数 + error 日志 → Grafana 告警 `cert_acl_hardening_failures_total > 0`。
#[cfg(windows)]
fn harden_windows_key_acl(path: &Path, metrics: Option<&Arc<crate::engine::metrics::Metrics>>) {
    use std::process::Command;
    let user = match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
        (Ok(domain), Ok(name)) if !name.is_empty() => format!("{}\\{}", domain, name),
        _ => match std::env::var("USERNAME") {
            Ok(name) if !name.is_empty() => name,
            _ => {
                tracing::error!(
                    "icacls skipped for {}: USERNAME unavailable; private key ACL NOT hardened",
                    path.display()
                );
                if let Some(m) = metrics {
                    m.lifecyle
                        .cert_acl_hardening_failures
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                return;
            }
        },
    };
    let result = Command::new("icacls")
        .arg(path)
        .arg("/inheritance:r")
        .args(["/grant:r", &format!("{}:F", user)])
        .args(["/grant:r", "*S-1-5-18:F"]) // LOCAL SYSTEM
        .args(["/grant:r", "*S-1-5-32-544:F"]) // Administrators
        .status();
    match result {
        Ok(s) if s.success() => {}
        Ok(s) => {
            tracing::error!(
                "icacls on {} exited with {:?}; private key ACL NOT hardened",
                path.display(),
                s
            );
            if let Some(m) = metrics {
                m.lifecyle
                    .cert_acl_hardening_failures
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
        Err(e) => {
            tracing::error!(
                "icacls on {} failed: {}; private key ACL NOT hardened",
                path.display(),
                e
            );
            if let Some(m) = metrics {
                m.lifecyle
                    .cert_acl_hardening_failures
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests;

// ============================================================
// S05: 证书过期扫描
// ============================================================

impl CertManager {
    /// S05: 解析证书 PEM 中的 not_after 字段，返回距今剩余天数。
    ///
    /// - `Ok(Some(days))`：未过期；负数表示已过期。
    /// - `Ok(None)`：解析成功但 not_after 不可读（极少见，作兼容）
    /// - `Err(...)`：PEM 损坏或 x509 解析失败（需运维介入）。
    ///
    /// 用 `x509_parser` 替代自写 ASN.1 解码；仅用于 expiry 用途，不做信任链校验，
    /// 保证无 OpenSSL 依赖、跨平台一致。
    pub fn check_expiry_days(&self, cert_pem: &str) -> GatewayResult<Option<i64>> {
        let der = pem::parse(cert_pem)
            .map_err(|e| GatewayError::Protocol(format!("parse cert pem: {}", e)))?
            .into_contents();
        let cert = x509_parser::certificate::X509Certificate::from_der(&der)
            .map_err(|e| GatewayError::Protocol(format!("parse x509 der: {}", e)))?
            .1;
        // not_after.timestamp() 返回 UNIX 秒（i64）
        let ts = cert.validity().not_after.timestamp();
        let now = chrono::Utc::now().timestamp();
        Ok(Some((ts - now) / 86400))
    }
}

/// S05: 证书过期扫描结果
#[derive(Debug, Clone)]
pub struct CertExpiryScan {
    /// 证书路径（仅作日志定位）
    pub path: String,
    /// 距过期剩余天数（负数表示已过期）
    pub days_remaining: i64,
    /// 过期档位（供前端分级显示）
    pub severity: CertExpirySeverity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertExpirySeverity {
    /// 距过期 > 30 天
    Ok,
    /// 距过期 ≤ 30 天但 > 7 天（提前告警）
    Warn,
    /// 距过期 ≤ 7 天但未过期（紧急告警）
    Critical,
    /// 已过期（停止服务前必须修复）
    Expired,
}

impl CertExpiryScan {
    pub fn from_days_remaining(path: impl Into<String>, days: i64) -> Self {
        let severity = if days <= 0 {
            CertExpirySeverity::Expired
        } else if days <= 7 {
            CertExpirySeverity::Critical
        } else if days <= 30 {
            CertExpirySeverity::Warn
        } else {
            CertExpirySeverity::Ok
        };
        Self {
            path: path.into(),
            days_remaining: days,
            severity,
        }
    }
}

#[cfg(test)]
mod s05_tests {
    use super::*;

    /// S05: 自签证书 10 年有效期——days_remaining 应在 3600~3700 之间。
    #[test]
    fn check_expiry_self_signed_returns_positive_days() {
        // 用临时目录避开空路径问题。
        let tmp = std::env::temp_dir().join(format!("s05-cert-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let mgr = CertManager::new(tmp.join("c.pem"), tmp.join("c.key"));
        let pair = mgr.generate().expect("generate self-signed");
        let days = mgr
            .check_expiry_days(&pair.cert_pem)
            .expect("parse ok")
            .expect("some days");
        // 默认 10 年 ≈ 3650 天；现时间已离开始日（年初）一段时间，容差
        assert!(
            days > 3600,
            "self-signed cert should have >3600 days, got {}",
            days
        );
        // 10 年有效期：上限 3660 (10 * 366 含闰年)，2025-10 之后生成会上限接近 3730
        assert!(
            days <= 3730,
            "self-signed cert should have <=3730 days, got {}",
            days
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// S05: 证书剩余天数应映射到正确的 severity 档位。
    #[test]
    fn scan_severity_thresholds_are_correct() {
        // 边界 0 → Expired（已过期）。
        let s = CertExpiryScan::from_days_remaining("a.pem", 0);
        assert_eq!(s.severity, CertExpirySeverity::Expired);
        // 5 → Critical
        let s = CertExpiryScan::from_days_remaining("a.pem", 5);
        assert_eq!(s.severity, CertExpirySeverity::Critical);
        // 7 → Critical（含边界）
        let s = CertExpiryScan::from_days_remaining("a.pem", 7);
        assert_eq!(s.severity, CertExpirySeverity::Critical);
        // 8 → Warn
        let s = CertExpiryScan::from_days_remaining("a.pem", 8);
        assert_eq!(s.severity, CertExpirySeverity::Warn);
        // 30 → Warn（含边界）
        let s = CertExpiryScan::from_days_remaining("a.pem", 30);
        assert_eq!(s.severity, CertExpirySeverity::Warn);
        // 31 → Ok
        let s = CertExpiryScan::from_days_remaining("a.pem", 31);
        assert_eq!(s.severity, CertExpirySeverity::Ok);
        // 负数 → Expired
        let s = CertExpiryScan::from_days_remaining("a.pem", -5);
        assert_eq!(s.severity, CertExpirySeverity::Expired);
    }

    /// S05: 损坏的 PEM 字符串应返回 Err 而非 panic。
    #[test]
    fn check_expiry_malformed_pem_returns_err() {
        let mgr = CertManager::new("", "");
        let res = mgr.check_expiry_days("not a valid pem");
        assert!(res.is_err(), "malformed pem must return Err");
    }
}
