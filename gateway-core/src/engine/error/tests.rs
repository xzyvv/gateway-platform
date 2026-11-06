use super::*;

#[test]
fn code_for_each_variant() {
    assert_eq!(GatewayError::Config("x".into()).code(), 1000);
    assert_eq!(GatewayError::Io("x".into()).code(), 2000);
    assert_eq!(GatewayError::Protocol("x".into()).code(), 3000);
    assert_eq!(GatewayError::NoReply.code(), 0);
    assert_eq!(GatewayError::Queue("x".into()).code(), 4000);
    assert_eq!(GatewayError::Mqtt("x".into()).code(), 5000);
    assert_eq!(GatewayError::Formula("x".into()).code(), 7000);
    assert_eq!(GatewayError::Auth("x".into()).code(), 8000);
    assert_eq!(GatewayError::NotFound("x".into()).code(), 9000);
    assert_eq!(GatewayError::Message("x".into()).code(), 9999);
}

#[test]
fn code_classified_recovers_round_trip() {
    // round-trip：具体变体 → Display → String → From → Message → code_classified
    let cases: [(GatewayError, u32); 8] = [
        (GatewayError::Config("bad json".into()), 1000),
        (GatewayError::Io("eof".into()), 2000),
        (GatewayError::Protocol("bad crc".into()), 3000),
        (GatewayError::Queue("locked".into()), 4000),
        (GatewayError::Mqtt("timeout".into()), 5000),
        (GatewayError::Formula("div0".into()), 7000),
        (GatewayError::Auth("bad token".into()), 8000),
        (GatewayError::NotFound("device".into()), 9000),
    ];
    for (err, expected) in cases {
        let s = err.to_string();
        let roundtrip: GatewayError = s.into();
        assert_eq!(
            roundtrip.code_classified(),
            expected,
            "round-trip 分类失败: {roundtrip}"
        );
    }
}

#[test]
fn code_classified_unknown_message_stays_9999() {
    let err: GatewayError = "some random string".into();
    assert_eq!(err.code_classified(), 9999);
}

#[test]
fn code_classified_non_message_uses_code() {
    assert_eq!(GatewayError::NoReply.code_classified(), 0);
    assert_eq!(GatewayError::Config("x".into()).code_classified(), 1000);
}

#[test]
fn from_string_preserves_content() {
    let err: GatewayError = String::from("hello").into();
    match err {
        GatewayError::Message(s) => assert_eq!(s, "hello"),
        _ => panic!("From<String> 应产生 Message 变体"),
    }
}

#[test]
fn from_str_preserves_content() {
    let err: GatewayError = "world".into();
    match err {
        GatewayError::Message(s) => assert_eq!(s, "world"),
        _ => panic!("From<&str> 应产生 Message 变体"),
    }
}

// ===== r15-Q1: SQLite 主错误码必须稳定地编进消息前缀 =====

/// r15-Q1: `sqlite_error_code` 必须取 SQLite **主码**（低 8 位）。
///
/// 关键断言：扩展错误码（`SQLITE_FULL | (N<<8)`）取主码后仍是 13，
/// 否则下游按码判定磁盘满会失效。
#[test]
fn sqlite_error_code_returns_primary_code() {
    use rusqlite::ffi::Error as FfiError;
    // SQLITE_FULL = 13
    let full = rusqlite::Error::SqliteFailure(FfiError::new(13), None);
    assert_eq!(sqlite_error_code(&full), Some(13));
    // SQLITE_IOERR = 10
    let io = rusqlite::Error::SqliteFailure(FfiError::new(10), None);
    assert_eq!(sqlite_error_code(&io), Some(10));
    // 扩展码（13 | (1<<8)）→ 主码仍是 13
    let ext = rusqlite::Error::SqliteFailure(FfiError::new(13 | (1 << 8)), None);
    assert_eq!(sqlite_error_code(&ext), Some(13));
    // 非 SqliteFailure 变体 → None
    assert_eq!(sqlite_error_code(&rusqlite::Error::InvalidQuery), None);
    assert_eq!(
        sqlite_error_code(&rusqlite::Error::QueryReturnedNoRows),
        None
    );
}

/// r15-Q1: `From<rusqlite::Error>` 必须把主码编进消息前缀，
/// 让下游（磁盘满退避保护）能按码判定而不是匹配文案。
#[test]
fn from_rusqlite_error_carries_code_prefix() {
    use rusqlite::ffi::Error as FfiError;
    let err: GatewayError =
        rusqlite::Error::SqliteFailure(FfiError::new(13), Some("database or disk is full".into()))
            .into();
    let msg = err.to_string();
    assert!(
        msg.contains("sqlite_error(code=13)"),
        "消息必须带主码前缀，实际: {msg}"
    );
    assert!(
        msg.contains("database or disk is full"),
        "原始文案必须保留，实际: {msg}"
    );
}
