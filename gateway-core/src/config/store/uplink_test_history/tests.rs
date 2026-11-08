use super::*;

fn temp_db() -> String {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{}/uplink_test_history_{}_{}.db",
        std::env::temp_dir().to_string_lossy(),
        pid,
        nanos
    )
}

/// r39-VER1 回归：uplink_test_history 与 config 版本 / 磁盘历史共用同一库文件并发写，
/// 连接必须设置 busy_timeout，否则遇 SQLITE_BUSY 立即失败导致测试历史丢失。
#[test]
fn test_conn_sets_busy_timeout() {
    let path = temp_db();
    let store = UplinkTestHistoryStore::new(&path).expect("should succeed");
    let conn = store.conn();
    let timeout: i64 = conn
        .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
        .expect("should succeed");
    assert_eq!(timeout, 5000, "busy_timeout 应为 5000ms");
}

#[test]
fn test_save_and_list() {
    let path = temp_db();
    let store = UplinkTestHistoryStore::new(&path).expect("should succeed");
    let id = store
        .save(NewUplinkTestHistory {
            template: "hello ${var}",
            rendered: "hello world",
            vars_json: r#"{"var":"world"}"#,
            send: true,
            delivered: true,
            latency_ms: 12,
            error: None,
        })
        .expect("should succeed");
    assert!(id > 0);
    let items = store.list_recent(10).expect("should succeed");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].template, "hello ${var}");
    assert_eq!(items[0].rendered, "hello world");
    assert!(items[0].send);
    assert!(items[0].delivered);
    assert_eq!(items[0].latency_ms, 12);
    assert!(items[0].error.is_none());
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{}-wal", path));
    let _ = std::fs::remove_file(format!("{}-shm", path));
}

#[test]
fn test_clear() {
    let path = temp_db();
    let store = UplinkTestHistoryStore::new(&path).expect("should succeed");
    store
        .save(NewUplinkTestHistory {
            template: "t1",
            rendered: "r1",
            vars_json: "{}",
            send: false,
            delivered: false,
            latency_ms: 0,
            error: None,
        })
        .expect("should succeed");
    store
        .save(NewUplinkTestHistory {
            template: "t2",
            rendered: "r2",
            vars_json: "{}",
            send: false,
            delivered: false,
            latency_ms: 0,
            error: None,
        })
        .expect("should succeed");
    assert_eq!(store.list_recent(10).expect("should succeed").len(), 2);
    store.clear().expect("should succeed");
    assert_eq!(store.list_recent(10).expect("should succeed").len(), 0);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{}-wal", path));
    let _ = std::fs::remove_file(format!("{}-shm", path));
}

#[test]
fn test_retention_200() {
    let path = temp_db();
    let store = UplinkTestHistoryStore::new(&path).expect("should succeed");
    for i in 0..210 {
        store
            .save(NewUplinkTestHistory {
                template: &format!("t{}", i),
                rendered: "r",
                vars_json: "{}",
                send: false,
                delivered: false,
                latency_ms: 0,
                error: None,
            })
            .expect("should succeed");
    }
    let items = store.list_recent(300).expect("should succeed");
    assert_eq!(items.len(), 200, "should cap at 200");
    // 应保留最新的 200 条（id 较大的）
    assert!(items[0].template.starts_with("t2"));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{}-wal", path));
    let _ = std::fs::remove_file(format!("{}-shm", path));
}
#[test]
fn test_list_recent_empty() {
    let path = temp_db();
    let store = UplinkTestHistoryStore::new(&path).expect("should succeed");
    let items = store.list_recent(10).expect("should succeed");
    assert!(items.is_empty());
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{}-wal", path));
    let _ = std::fs::remove_file(format!("{}-shm", path));
}

#[test]
fn test_list_recent_limit_below_actual_count() {
    let path = temp_db();
    let store = UplinkTestHistoryStore::new(&path).expect("should succeed");
    for i in 0..5 {
        store
            .save(NewUplinkTestHistory {
                template: &format!("t{}", i),
                rendered: "r",
                vars_json: "{}",
                send: false,
                delivered: false,
                latency_ms: 0,
                error: None,
            })
            .expect("should succeed");
    }
    // limit=3 应只返回最新 3 条（id 最大的 3 个）
    let items = store.list_recent(3).expect("should succeed");
    assert_eq!(items.len(), 3);
    assert_eq!(items[0].template, "t4");
    assert_eq!(items[2].template, "t2");
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{}-wal", path));
    let _ = std::fs::remove_file(format!("{}-shm", path));
}

#[test]
fn test_list_recent_limit_exceeds_actual_count() {
    let path = temp_db();
    let store = UplinkTestHistoryStore::new(&path).expect("should succeed");
    store
        .save(NewUplinkTestHistory {
            template: "only",
            rendered: "r",
            vars_json: "{}",
            send: true,
            delivered: true,
            latency_ms: 5,
            error: None,
        })
        .expect("should succeed");
    // limit=100 但只有 1 条，应只返回 1 条
    let items = store.list_recent(100).expect("should succeed");
    assert_eq!(items.len(), 1);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{}-wal", path));
    let _ = std::fs::remove_file(format!("{}-shm", path));
}

#[test]
fn test_save_with_error_field() {
    let path = temp_db();
    let store = UplinkTestHistoryStore::new(&path).expect("should succeed");
    store
        .save(NewUplinkTestHistory {
            template: "t",
            rendered: "r",
            vars_json: "{}",
            send: true,
            delivered: false,
            latency_ms: 0,
            error: Some("broker unreachable"),
        })
        .expect("should succeed");
    let items = store.list_recent(10).expect("should succeed");
    assert_eq!(items.len(), 1);
    assert!(!items[0].delivered);
    assert_eq!(items[0].error.as_deref(), Some("broker unreachable"));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{}-wal", path));
    let _ = std::fs::remove_file(format!("{}-shm", path));
}

#[test]
fn test_save_disabled_send_recorded() {
    let path = temp_db();
    let store = UplinkTestHistoryStore::new(&path).expect("should succeed");
    // store 层不限制 send=false 也可保存（仅渲染由调用方决定是否入库）
    store
        .save(NewUplinkTestHistory {
            template: "t",
            rendered: "r",
            vars_json: "{}",
            send: false,
            delivered: false,
            latency_ms: 0,
            error: None,
        })
        .expect("should succeed");
    let items = store.list_recent(10).expect("should succeed");
    assert_eq!(items.len(), 1);
    assert!(!items[0].send);
    assert!(!items[0].delivered);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{}-wal", path));
    let _ = std::fs::remove_file(format!("{}-shm", path));
}

#[test]
fn test_save_large_payload() {
    let path = temp_db();
    let store = UplinkTestHistoryStore::new(&path).expect("should succeed");
    let big_template = "x".repeat(10_000);
    let big_rendered = "y".repeat(20_000);
    store
        .save(NewUplinkTestHistory {
            template: &big_template,
            rendered: &big_rendered,
            vars_json: "{}",
            send: true,
            delivered: true,
            latency_ms: 100,
            error: None,
        })
        .expect("should succeed");
    let items = store.list_recent(10).expect("should succeed");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].template.len(), 10_000);
    assert_eq!(items[0].rendered.len(), 20_000);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{}-wal", path));
    let _ = std::fs::remove_file(format!("{}-shm", path));
}

#[test]
fn test_list_recent_descending_order() {
    let path = temp_db();
    let store = UplinkTestHistoryStore::new(&path).expect("should succeed");
    for i in 0..5 {
        store
            .save(NewUplinkTestHistory {
                template: &format!("t{}", i),
                rendered: "r",
                vars_json: "{}",
                send: false,
                delivered: false,
                latency_ms: 0,
                error: None,
            })
            .expect("should succeed");
    }
    let items = store.list_recent(10).expect("should succeed");
    // 应按 id 降序：最新在前
    assert_eq!(items.len(), 5);
    assert_eq!(items[0].template, "t4");
    assert_eq!(items[1].template, "t3");
    assert_eq!(items[2].template, "t2");
    assert_eq!(items[3].template, "t1");
    assert_eq!(items[4].template, "t0");
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{}-wal", path));
    let _ = std::fs::remove_file(format!("{}-shm", path));
}
