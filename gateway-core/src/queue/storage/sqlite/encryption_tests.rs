//! SQLCipher 加密落盘回归测试（仅 `--features encryption` 编译）。
//!
//! 此前 `encryption` feature 在 CI 里只有 `cargo check`：加密分支（`PRAGMA key`
//! 喂入、密文库读写）从未被真正执行过，改错了不会有任何用例失败。
//!
//! 本模块验证的是**可观察的加密语义**，而不是「能编译」：
//! 1. 落盘文件里看不到明文 SQLite 头与表名；
//! 2. 同一把 keyfile 重新打开能读回数据（密钥持久化正确）；
//! 3. 换掉 keyfile 后拒绝启动且不改写文件；只有显式打开自愈开关才会重建（旧数据仍不可读）。

#![cfg(feature = "encryption")]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::engine::metrics::Metrics;
use crate::engine::model::DataPoint;
use crate::engine::model::Quality;
use crate::queue::storage::QueueStorage;
use crate::queue::storage::SqliteStorage;

/// 环境变量是进程级全局状态：`GATEWAY_QUEUE_REBUILD_ON_CORRUPTION` 会改变
/// `db_init` 对损坏库的处置分支（拒绝启动 / 删库重建）。凡「修改它」或
/// 「依赖它未被设置」的用例都必须持有此锁，否则并行执行时会互相污染
/// （同 `crypto::cert::tests::ENV_LOCK` 的做法）。
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn env_guard() -> std::sync::MutexGuard<'static, ()> {
    // 某个测试 panic 导致锁中毒时继续使用，避免后续测试全部连带失败
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// 测试夹具：临时目录 + 队列库路径（keyfile 按约定为 `{db_path}.key`）。
struct Fixture {
    _dir: tempfile::TempDir,
    db_path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("create temp dir");
        let db_path = dir.path().join("queue.db");
        Self { _dir: dir, db_path }
    }

    fn db_path_str(&self) -> &str {
        self.db_path.to_str().expect("db path utf8")
    }

    fn open(&self) -> SqliteStorage {
        SqliteStorage::new(self.db_path_str(), Arc::new(Metrics::new()))
            .expect("open encrypted storage")
    }

    /// key.rs 约定：keyfile = `{db_path}.key`。
    fn keyfile(&self) -> PathBuf {
        let mut p = self.db_path.as_os_str().to_owned();
        p.push(".key");
        PathBuf::from(p)
    }
}

fn make_point(id: &str, value: f64) -> DataPoint {
    let now = DataPoint::now();
    DataPoint {
        point_id: id.into(),
        device_id: "dev1".into(),
        value,
        raw_value: value,
        raw_type: "u16".into(),
        quality: Quality::Good,
        quality_reason: String::new(),
        timestamp: now,
        source_ts: now,
        stale_count: 0,
        value_text: None,
    }
}

/// 落盘必须是密文：文件头不是 "SQLite format 3"，也搜不到表名明文。
#[test]
fn database_file_is_encrypted_on_disk() {
    let fx = Fixture::new();
    {
        let storage = fx.open();
        let inserted = storage
            .enqueue_batch(&[make_point("p1", 42.0)])
            .expect("enqueue");
        assert_eq!(inserted, 1);
    }

    let bytes = std::fs::read(&fx.db_path).expect("read db file");
    assert!(!bytes.is_empty(), "队列库文件不应为空");
    assert!(
        !bytes.starts_with(b"SQLite format 3\0"),
        "加密库文件不应保留明文 SQLite 头"
    );
    assert!(
        !bytes.windows(10).any(|w| w == b"data_queue"),
        "加密库文件里不应出现表名明文"
    );
}

/// keyfile 必须由首次打开生成且长度为 32 字节（SQLCipher 密钥长度）。
#[test]
fn keyfile_is_created_with_expected_length() {
    let fx = Fixture::new();
    drop(fx.open());

    let len = std::fs::metadata(fx.keyfile())
        .expect("keyfile must exist")
        .len();
    assert_eq!(len, 32, "keyfile 必须是 32 字节，实际 {len}");
}

/// 同一把 keyfile 重新打开必须能读回数据（密钥持久化，而不是每次启动换新密钥）。
#[test]
fn reopen_with_same_keyfile_reads_back_data() {
    let fx = Fixture::new();
    {
        let storage = fx.open();
        storage
            .enqueue_batch(&[make_point("p1", 1.25)])
            .expect("enqueue");
    }

    let storage = fx.open();
    let items = storage
        .history_query("p1", 0, i64::MAX, 10)
        .expect("history query");
    assert_eq!(items.len(), 1, "重启后历史行必须仍在");
    assert!(
        (items[0].value - 1.25).abs() < 1e-9,
        "读回的值必须与写入一致，实际 {}",
        items[0].value
    );
}

/// 换掉 keyfile 后必须**拒绝启动**：不删库、不覆盖、不返回空库。
///
/// 旧行为是 integrity_check 失败后删库重建，现场表现为「数据静默清零」。
/// 现语义见 `db_init::refuse_unusable_db`：默认报错退出，由运维确认后再开自愈。
#[test]
fn wrong_keyfile_refuses_to_start_and_keeps_files() {
    let _guard = env_guard();
    let fx = Fixture::new();
    {
        let storage = fx.open();
        storage
            .enqueue_batch(&[make_point("p1", 1.0)])
            .expect("enqueue");
    }

    // 覆盖成另一把合法的 32 字节密钥（长度合法，内容错误）。
    let good_key = std::fs::read(fx.keyfile()).expect("read keyfile");
    std::fs::write(fx.keyfile(), [0x5A_u8; 32]).expect("write wrong keyfile");

    // `SqliteStorage` 没有实现 Debug，不能用 `expect_err`，这里手工匹配。
    let err = match SqliteStorage::new(fx.db_path_str(), Arc::new(Metrics::new())) {
        Ok(_) => panic!("keyfile 与库不匹配时必须拒绝启动"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(
        msg.contains("GATEWAY_QUEUE_REBUILD_ON_CORRUPTION"),
        "错误信息必须给出处置开关，实际: {msg}"
    );

    // 关键断言：拒绝启动的过程不得删库、清库或把库重新加密成错误密钥。
    // 不用字节级相等：SQLite 打开时会做 WAL 恢复/页整理，即使数据没变字节也会变，
    // 那样断言会因为存储引擎内部行为而误报。真正要守的语义是「数据不丢」：
    // 换回正确 keyfile 后必须仍能读出原值（若库被错误密钥重写，这里会读失败）。
    let db_after = std::fs::read(&fx.db_path).expect("拒绝启动后数据库文件必须仍存在");
    assert!(!db_after.is_empty(), "拒绝启动后不得把库文件清空");
    std::fs::write(fx.keyfile(), &good_key).expect("restore keyfile");
    let storage = fx.open();
    let items = storage
        .history_query("p1", 0, i64::MAX, 10)
        .expect("history query");
    assert_eq!(items.len(), 1, "拒绝启动后原始数据必须仍可读");
    assert_eq!(
        std::fs::metadata(fx.keyfile()).expect("keyfile").len(),
        32,
        "拒绝启动时不得改写 keyfile"
    );
}

/// 显式打开逃生门后恢复旧自愈行为：能启动（空库），旧数据仍不可读。
///
/// 现场确认「库确实不可用」时才会这么用，因此这里同时钉住「仍然不会解密出旧行」。
#[test]
fn rebuild_env_restores_self_heal_without_leaking_old_rows() {
    let _guard = env_guard();
    let fx = Fixture::new();
    {
        let storage = fx.open();
        storage
            .enqueue_batch(&[make_point("p1", 1.0)])
            .expect("enqueue");
    }
    std::fs::write(fx.keyfile(), [0x5A_u8; 32]).expect("write wrong keyfile");
    // 首次打开会自动生成 `<db>.db.bak`，而备份同样是用旧密钥加密的：
    // 保留它会让「恢复备份」路径必然再次失败，测不到「无备份 → 重建空库」这条分支。
    let _ = std::fs::remove_file(fx.db_path.with_extension("db.bak"));

    std::env::set_var("GATEWAY_QUEUE_REBUILD_ON_CORRUPTION", "1");
    let opened = SqliteStorage::new(fx.db_path_str(), Arc::new(Metrics::new()));
    std::env::remove_var("GATEWAY_QUEUE_REBUILD_ON_CORRUPTION");

    let storage = opened.expect("开启逃生门后应允许重建并启动");
    let leaked = storage
        .history_query("p1", 0, i64::MAX, 10)
        .map(|items| items.len())
        .unwrap_or(0);
    assert_eq!(leaked, 0, "重建出的空库里不得出现旧数据");
}
