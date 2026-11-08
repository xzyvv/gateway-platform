//! SQLCipher 加密落盘回归测试（仅 `--features encryption` 编译）。
//!
//! 此前 `encryption` feature 在 CI 里只有 `cargo check`：加密分支（`PRAGMA key`
//! 喂入、密文库读写）从未被真正执行过，改错了不会有任何用例失败。
//!
//! 本模块验证的是**可观察的加密语义**，而不是「能编译」：
//! 1. 落盘文件里看不到明文 SQLite 头与表名；
//! 2. 同一把 keyfile 重新打开能读回数据（密钥持久化正确）；
//! 3. 换掉 keyfile 后旧数据读不出来（密钥确实是解密前提）。

#![cfg(feature = "encryption")]

use std::path::PathBuf;
use std::sync::Arc;

use crate::engine::metrics::Metrics;
use crate::engine::model::DataPoint;
use crate::engine::model::Quality;
use crate::queue::storage::QueueStorage;
use crate::queue::storage::SqliteStorage;

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

/// 换掉 keyfile 后旧数据读不出来：证明数据确实由密钥保护。
///
/// 生产语义（见 `db_init::init_queue_db`）：integrity_check 失败先尝试备份恢复，
/// 没有可用备份就删库重建 —— 所以错误密钥下 `new()` 可能返回 Ok，但那是一份空库。
/// 本条钉的是「旧行绝不能解密成功」，而不是「必须报错」：将来若改成直接返回 Err，
/// 本用例同样成立。
#[test]
fn wrong_keyfile_cannot_read_old_data() {
    let fx = Fixture::new();
    {
        let storage = fx.open();
        storage
            .enqueue_batch(&[make_point("p1", 1.0)])
            .expect("enqueue");
    }

    // 覆盖成另一把合法的 32 字节密钥（长度合法，内容错误）。
    std::fs::write(fx.keyfile(), [0x5A_u8; 32]).expect("write wrong keyfile");

    // 打开失败、或重建出空库，都算「旧库打不开」；只有读出旧行才是泄漏。
    let leaked = match SqliteStorage::new(fx.db_path_str(), Arc::new(Metrics::new())) {
        Ok(storage) => match storage.history_query("p1", 0, i64::MAX, 10) {
            Ok(items) => items.len(),
            Err(_) => 0,
        },
        Err(_) => 0,
    };
    assert_eq!(leaked, 0, "错误密钥下不得读出旧数据，实际读到 {leaked} 行");
}
