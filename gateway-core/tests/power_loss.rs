//! 断电一致性（abrupt kill）回归
//!
//! 用「父进程反复强杀子进程」模拟掉电：子进程持续向 SQLite 队列提交已 commit 的数据，
//! 父进程在写入过程中 `SIGKILL`（Windows 上等价于 `TerminateProcess`），随后重开数据库断言：
//!
//! 1. 重开必须成功——`init_queue_db` 启动即跑 `integrity_check`，失败会按数据保护策略
//!    拒绝启动，所以这条能直接抓到「掉电把库写坏 / WAL 恢复不了」；
//! 2. 已落盘的历史行必须还能读出来（已 commit 事务不能丢）；
//! 3. 队列深度单调不减（崩溃不会把队列清空或回滚成旧快照）。
//!
//! 子进程复用本测试二进制：父进程以 `--exact power_loss_child_writer` 拉起自己，
//! 靠环境变量 `GATEWAY_POWER_LOSS_DB` 进入「持续写入」分支；常规 `cargo test`
//! 时该用例立即返回，不会挂住测试进程。

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gateway_core::engine::metrics::Metrics;
use gateway_core::engine::model::DataPoint;
use gateway_core::queue::{QueueStorage, SqliteStorage};

/// 子进程数据库路径的环境变量（同时也是「进入写入分支」的开关）。
const CHILD_DB_ENV: &str = "GATEWAY_POWER_LOSS_DB";

/// 子进程侧：持续入队直到被杀。父进程不注入环境变量时立即返回。
#[test]
fn power_loss_child_writer() {
    let Ok(db) = std::env::var(CHILD_DB_ENV) else {
        return;
    };
    let storage = match SqliteStorage::new(&db, Arc::new(Metrics::new())) {
        Ok(storage) => storage,
        // 用退出码区分「打不开库」和「正常被杀」：父进程看到 2 会报错。
        Err(_) => std::process::exit(2),
    };

    let mut seq: i64 = 0;
    loop {
        let batch: Vec<DataPoint> = (0..16)
            .map(|i| {
                let n = seq + i as i64;
                // 轮转 point_id，避免单点历史过长影响读取断言。
                DataPoint::good(
                    &format!("pt_{}", n % 64),
                    "dev_1",
                    n as f64,
                    n as f64,
                    "u16",
                )
            })
            .collect();
        if storage.enqueue_batch(&batch).is_err() {
            std::process::exit(3);
        }
        seq += 16;
    }
}

/// 以「只跑子进程用例」的方式拉起自己。
fn spawn_writer(db_path: &Path) -> Child {
    let exe = std::env::current_exe().expect("current exe");
    Command::new(exe)
        .args(["--exact", "power_loss_child_writer", "--nocapture"])
        .env(CHILD_DB_ENV, db_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn child writer")
}

/// 等子进程把「至少一行」真正 commit 进队列。
///
/// 不能用「固定 sleep 一段时间就认为已经有 commit」：加密构建首次打开要做
/// SQLCipher 密钥派生 + integrity_check + 启动备份，冷启动可能远超 250ms，
/// 慢机上会在还没有任何提交时就强杀子进程，断言失败但其实是测试自身的时序假设不成立。
/// 这里用一条独立只读连接观察 `data_queue` 的可见行数，等到 >0 再强杀——
/// 这样「已 commit 的数据必须存活」才是有效的断言。
///
/// 等待期间若子进程自行退出（打不开库 / 入队报错），直接报出退出码，
/// 避免把子进程的启动失败伪装成「断电丢数据」。
pub fn wait_until_rows_visible(db_path: &Path, child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().expect("try_wait child") {
            panic!("子进程在提交任何数据前退出（status={status}），不是断电场景");
        }
        if count_queue_rows(db_path).is_ok_and(|n| n > 0) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("30s 内子进程没有提交任何可读的队列行");
}

/// 只读连接数行：不参与初始化（不建表、不备份），只做一次 COUNT。
///
/// 加密构建必须先 `PRAGMA key`，密钥文件路径与 `storage` 保持同一约定（`<db>.key`）。
fn count_queue_rows(db_path: &Path) -> rusqlite::Result<i64> {
    let conn =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    #[cfg(feature = "encryption")]
    {
        let mut key_path = db_path.as_os_str().to_owned();
        key_path.push(".key");
        let key_bytes = std::fs::read(std::path::PathBuf::from(key_path)).expect("read keyfile");
        let key_hex: String = key_bytes.iter().map(|b| format!("{b:02x}")).collect();
        conn.pragma_update(None, "key", key_hex.as_str())?;
    }
    // 建表还没完成时这里会报 no such table，交给调用方当作「再等等」。
    conn.query_row("SELECT COUNT(*) FROM data_queue", [], |row| row.get(0))
}

/// 等子进程把库文件写出来（首次还要建表 + 备份，给足 10s）。
fn wait_until_written(db_path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if std::fs::metadata(db_path)
            .map(|m| m.len() > 0)
            .unwrap_or(false)
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("子进程 10s 内没有写出数据库文件");
}

#[test]
fn abrupt_kill_keeps_committed_rows_and_queue_depth() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("power.db");
    let db_str = db.to_str().expect("utf8 path").to_string();

    let mut last_len = 0usize;
    for round in 1..=3 {
        let mut child = spawn_writer(&db);
        wait_until_written(&db);
        // 等到「确实有行 commit」再强杀：断电断言的语义是「已 commit 的行不能丢」，
        // 不能在还什么都没提交时就杀。
        wait_until_rows_visible(&db, &mut child);
        child
            .kill()
            .expect("kill child（Unix 为 SIGKILL，Windows 为 TerminateProcess）");
        let _ = child.wait();

        let storage = SqliteStorage::new(&db_str, Arc::new(Metrics::new())).unwrap_or_else(|e| {
            panic!("第 {round} 轮强杀后重开失败（库被判损坏，说明断电不可恢复）: {e}")
        });

        let len = storage.len();
        assert!(
            len >= last_len,
            "第 {round} 轮强杀后队列深度回退: {last_len} -> {len}"
        );
        assert!(len > 0, "第 {round} 轮强杀后队列为空，已 commit 数据丢失");

        let rows = storage
            .history_query("pt_0", 0, i64::MAX, 10)
            .expect("查询崩溃前的历史行");
        assert!(!rows.is_empty(), "第 {round} 轮强杀后历史行不可读");

        last_len = len;
    }
}
