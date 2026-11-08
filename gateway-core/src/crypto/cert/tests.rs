use super::*;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

// 测试用唯一目录计数器，避免并行测试冲突
static COUNTER: AtomicU32 = AtomicU32::new(0);

/// 环境变量是进程级全局状态：`GATEWAY_REQUIRE_EXTERNAL_CERT` 会影响
/// `get_or_create()`。凡调用 `get_or_create` 的测试都必须持有此锁，
/// 否则并发时可能读到别的测试刚设置的值。
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn env_guard() -> std::sync::MutexGuard<'static, ()> {
    // 某个测试 panic 导致锁中毒时继续使用，避免后续测试全部连带失败
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// 临时改写环境变量，Drop 时恢复原值（测试 panic 也不会把状态泄漏给其他用例）。
struct EnvVarGuard {
    key: &'static str,
    prev: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let prev = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, prev }
    }

    fn clear(key: &'static str) -> Self {
        let prev = std::env::var(key).ok();
        std::env::remove_var(key);
        Self { key, prev }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => std::env::set_var(self.key, v),
            None => std::env::remove_var(self.key),
        }
    }
}

/// 每个用例独占一个临时目录（进程号 + 进程内自增序号）。
///
/// 旧实现只带进程内计数器（`gw-cert-test-{n}`），跨进程仍会撞路径：两个测试
/// 二进制并发时（例如同机并行跑多个 crate 的测试）会用同一个目录，且开头
/// `remove_dir_all` 会互删对方的证书/私钥，表现为与被测逻辑无关的偶发失败。
/// 加入进程号后跨进程也不冲突。
fn temp_dir() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("gw-cert-test-{}-{}", std::process::id(), n));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn test_generate_and_load() {
    let dir = temp_dir();
    let mgr = CertManager::new(dir.join("gw.crt"), dir.join("gw.key"))
        .with_cn("Test Gateway")
        .with_san_hosts(vec!["localhost".to_string(), "127.0.0.1".to_string()]);

    // 生成
    let pair1 = mgr.generate().unwrap();
    assert!(!pair1.cert_pem.is_empty());
    assert!(!pair1.key_pem.is_empty());
    assert!(!pair1.cert_der.is_empty());
    assert!(!pair1.key_der.is_empty());
    assert!(pair1.cert_pem.contains("BEGIN CERTIFICATE"));
    assert!(pair1.key_pem.contains("PRIVATE KEY"));

    // 加载应得到相同的 DER
    let pair2 = mgr.load().unwrap();
    assert_eq!(pair1.cert_der, pair2.cert_der);
    assert_eq!(pair1.key_der, pair2.key_der);
}

#[test]
fn test_get_or_create_creates_then_loads() {
    // 走 get_or_create：必须与改写环境变量的用例串行，且确保生成开关未被外部置位
    let _env = env_guard();
    let _var = EnvVarGuard::clear("GATEWAY_REQUIRE_EXTERNAL_CERT");
    let dir = temp_dir();
    let mgr = CertManager::new(dir.join("gw.crt"), dir.join("gw.key"));

    // 第一次：文件不存在 → 生成
    let pair1 = mgr.get_or_create().unwrap();
    assert!(mgr.cert_path().exists());
    assert!(mgr.key_path().exists());

    // 第二次：文件存在 → 加载（不重新生成）
    let pair2 = mgr.get_or_create().unwrap();
    assert_eq!(pair1.cert_der, pair2.cert_der);
}

#[test]
fn test_cert_is_valid_x509() {
    let dir = temp_dir();
    let mgr = CertManager::new(dir.join("gw.crt"), dir.join("gw.key"));
    let pair = mgr.generate().unwrap();

    // 用 pem crate 重新解析证书，确认是合法 PEM 且 DER 一致
    let parsed = pem::parse(&pair.cert_pem).unwrap();
    assert_eq!(parsed.contents(), pair.cert_der.as_slice());
}

#[test]
fn test_load_nonexistent_fails() {
    let dir = temp_dir();
    let mgr = CertManager::new(dir.join("nope.crt"), dir.join("nope.key"));
    assert!(mgr.load().is_err());
}

/// Bug r11-CERT3 (fix): 人为错配的 cert + key 加载时应被拒绝。
/// 场景：A 设备生成 cert + key 写到文件，运维误把 B 设备的 cert 也放到
/// 同一文件路径（key 来自 A）—— 旧实现静默加载，启动无错；新实现
/// 在 load() 时检测公钥不一致 → 立即 Err 阻断启动。
#[test]
fn test_load_mismatched_cert_key_fails() {
    let dir = temp_dir();

    // 生成 A 的 cert + key，写盘
    let a_crt = dir.join("a.crt");
    let a_key = dir.join("a.key");
    let mgr_a = CertManager::new(&a_crt, &a_key);
    mgr_a.generate().expect("generate A");

    // 生成 B 的 cert（独立 key），但**保留 A 的 key**——模拟错配
    let b_crt = dir.join("a.crt"); // 同名覆盖 A 的 cert
    let b_key = dir.join("a.key"); // 保留 A 的 key
                                   // 直接生成 B 的 cert 内容后写入（不通过 CertManager，否则会同时写 key）
    let mgr_b = CertManager::new(&b_crt, &b_key);
    // mgr_b.generate() 会**同时写 A 的 key 被 B 的 key 覆盖**——
    // 我们需要的是：A 的 key + B 的 cert。
    // 所以：先备份 A 的 key，mgr_b.generate() 后用 A 的 key 覆盖回去。
    let a_key_pem = std::fs::read_to_string(&a_key).expect("read A key");
    mgr_b.generate().expect("generate B (覆盖了 A 的 key)");
    std::fs::write(&a_key, a_key_pem).expect("restore A key");

    // 现在路径上：A.key + B.crt（错配）
    let mgr_load = CertManager::new(&a_crt, &a_key);
    match mgr_load.load() {
        Ok(_) => panic!("错配必须被拒绝，实际 Ok"),
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("CERT3") || msg.contains("do NOT match"),
                "error 应明确指向 cert/key 不匹配，实际: {}",
                msg
            );
        }
    }
}

/// r28-CRT1：generate() 必须先写 key、再写 cert、最后做配对验证。
/// 关键不变量：生成成功后 cert_path 与 key_path 都存在且互相配对。
/// 任意中间步骤失败都必须不留下半成品文件（要么两个都在，要么两个都不在）。
#[test]
fn test_generate_writes_key_first_then_cert() {
    let dir = temp_dir();
    let mgr = CertManager::new(dir.join("gw.crt"), dir.join("gw.key"));

    let pair = mgr.generate().expect("generate");
    assert!(mgr.cert_path().exists(), "cert 必须存在");
    assert!(mgr.key_path().exists(), "key 必须存在");

    // 加载再次确认配对（不匹配会被 verify_cert_key_match 拒绝）
    let loaded = mgr.load().expect("load after generate");
    assert_eq!(pair.cert_der, loaded.cert_der);
    assert_eq!(pair.key_der, loaded.key_der);
}

/// r28-CRT1 边界：generate() 必须能在全新目录首次调用成功（即不存在半成品风险）。
/// 旧实现的"先写 cert 再写 key"在磁盘满场景下会留下无 key 的 cert，
/// 本测试确保正常路径下两个文件同时落盘。
#[test]
fn test_generate_atomic_on_happy_path() {
    let dir = temp_dir();
    let cert_path = dir.join("gw.crt");
    let key_path = dir.join("gw.key");
    assert!(!cert_path.exists());
    assert!(!key_path.exists());

    let mgr = CertManager::new(&cert_path, &key_path);
    mgr.generate().expect("generate");

    // 两个文件必须都存在
    assert!(cert_path.exists() && key_path.exists());
    // 两个文件非空
    assert!(std::fs::metadata(&cert_path).unwrap().len() > 0);
    assert!(std::fs::metadata(&key_path).unwrap().len() > 0);
}

/// r28-CRT1 边界：调用 generate() 后下次 get_or_create() 应走 load() 路径，
/// 验证修复后的写入顺序能让 load() 成功——证明 cert 与 key 配对正确落盘。
#[test]
fn test_generate_then_get_or_create_loads_successfully() {
    // 走 get_or_create：与其他改写环境变量的用例串行
    let _env = env_guard();
    let _var = EnvVarGuard::clear("GATEWAY_REQUIRE_EXTERNAL_CERT");
    let dir = temp_dir();
    let mgr = CertManager::new(dir.join("gw.crt"), dir.join("gw.key"));

    let _ = mgr.generate().expect("generate");
    // 第二次 get_or_create：应走 load 路径（不重新生成），且配对验证通过
    let _loaded = mgr.get_or_create().expect("get_or_create loads existing");
    assert!(mgr.cert_path().exists());
    assert!(mgr.key_path().exists());
}

/// P2-8 契约：`GATEWAY_REQUIRE_EXTERNAL_CERT=1` 时缺证书必须拒绝自签，
/// 且不落盘任何文件——现场必须显式提供外部证书，不能悄悄用自签顶上。
#[test]
fn p2_require_external_cert_refuses_self_signed_generation() {
    let _env = env_guard();
    let _var = EnvVarGuard::set("GATEWAY_REQUIRE_EXTERNAL_CERT", "1");
    let dir = temp_dir();
    let mgr = CertManager::new(dir.join("gw.crt"), dir.join("gw.key"));

    let err = match mgr.get_or_create() {
        Ok(_) => panic!("REQUIRE_EXTERNAL_CERT=1 时必须拒绝自签，实际成功生成"),
        Err(e) => e,
    };

    assert!(
        matches!(err, crate::engine::error::GatewayError::Config(_)),
        "应为可直接展示给运维的 Config 错误，实际: {:?}",
        err
    );
    assert!(
        err.to_string().contains("GATEWAY_REQUIRE_EXTERNAL_CERT"),
        "错误信息必须点明开关名，实际: {}",
        err
    );
    assert!(!mgr.cert_path().exists(), "拒绝生成时不得落盘证书");
    assert!(!mgr.key_path().exists(), "拒绝生成时不得落盘私钥");
}

/// P2-8 契约：cert / key 只剩一个时拒绝生成（避免用新的一半去配旧的一半），
/// 既有文件保持原样；错误信息要指明「现存的是哪一个」。
#[test]
fn p2_incomplete_pair_refuses_generation_and_keeps_existing_file() {
    let _env = env_guard();
    let _var = EnvVarGuard::clear("GATEWAY_REQUIRE_EXTERNAL_CERT");
    let dir = temp_dir();
    let mgr = CertManager::new(dir.join("gw.crt"), dir.join("gw.key"));

    // 场景 A：只有 cert（key 丢失）
    mgr.generate().expect("generate");
    let cert_before = std::fs::read(mgr.cert_path()).expect("read cert");
    std::fs::remove_file(mgr.key_path()).expect("remove key");

    let err = match mgr.get_or_create() {
        Ok(_) => panic!("只有 cert 时必须拒绝生成，实际成功"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(
        msg.contains("cert pair incomplete"),
        "错误信息应包含 cert pair incomplete，实际: {}",
        msg
    );
    assert!(
        msg.contains("only cert exists"),
        "错误必须指明现存的是 cert，实际: {}",
        msg
    );
    assert_eq!(
        std::fs::read(mgr.cert_path()).expect("read cert after"),
        cert_before,
        "拒绝时不得改写既有 cert"
    );
    assert!(
        !mgr.key_path().exists(),
        "拒绝时不得生成新 key（会与既有 cert 失配）"
    );

    // 场景 B：只有 key（cert 丢失）
    mgr.generate().expect("regenerate pair");
    let key_before = std::fs::read(mgr.key_path()).expect("read key");
    std::fs::remove_file(mgr.cert_path()).expect("remove cert");

    let err = match mgr.get_or_create() {
        Ok(_) => panic!("只有 key 时必须拒绝生成，实际成功"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(
        msg.contains("only key exists"),
        "错误必须指明现存的是 key，实际: {}",
        msg
    );
    assert_eq!(
        std::fs::read(mgr.key_path()).expect("read key after"),
        key_before,
        "拒绝时不得改写既有 key"
    );
    assert!(
        !mgr.cert_path().exists(),
        "拒绝时不得生成新 cert（会与既有 key 失配）"
    );
}
