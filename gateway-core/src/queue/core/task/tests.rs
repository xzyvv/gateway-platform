use super::{is_disk_full_error, sqlite_code_from_message};
use crate::engine::error::GatewayError;

/// r15-Q1 (fix): 错误码前缀解析必须可靠。
///
/// 关键断言：解析出的主码要与写入时一致；没有前缀时返回 None
/// （不能瞎猜，否则会把普通 DB 错误误判成磁盘满 → 误触发 30s 退避）。
#[test]
fn parses_sqlite_code_prefix() {
    assert_eq!(
        sqlite_code_from_message("sqlite_error(code=13): full"),
        Some(13)
    );
    assert_eq!(
        sqlite_code_from_message("sqlite_error(code=10): io"),
        Some(10)
    );
    // 嵌在 GatewayError::Queue 的 Display 前缀里也要能解析
    assert_eq!(
        sqlite_code_from_message("queue error: sqlite_error(code=13): database or disk is full"),
        Some(13)
    );
    // 无前缀 → None
    assert_eq!(sqlite_code_from_message("queue error: locked"), None);
    assert_eq!(sqlite_code_from_message(""), None);
    // 前缀不完整 / 数字非法 → None
    assert_eq!(sqlite_code_from_message("sqlite_error(code=): x"), None);
    assert_eq!(sqlite_code_from_message("sqlite_error(code=abc): x"), None);
    assert_eq!(sqlite_code_from_message("sqlite_error(code=13"), None);
}

/// r15-Q1 (fix): 磁盘满判定必须**优先按主码**，文案仅作兜底。
///
/// 关键断言：
/// - code=13 (SQLITE_FULL) / code=10 (SQLITE_IOERR) → 判为磁盘满
/// - 其它主码 → **不**判为磁盘满（即使文案里出现 disk full 字样，
///   说明主码是权威来源）
/// - 无前缀的历史消息 → 走文案兜底
#[test]
fn disk_full_detection_prefers_stable_code() {
    assert!(is_disk_full_error(&GatewayError::Queue(
        "sqlite_error(code=13): database or disk is full".into()
    )));
    assert!(is_disk_full_error(&GatewayError::Queue(
        "sqlite_error(code=10): disk I/O error".into()
    )));
    // 主码是权威的：主码不是 13/10 时不判为磁盘满（文案不参与）
    assert!(!is_disk_full_error(&GatewayError::Queue(
        "sqlite_error(code=5): database or disk is full".into()
    )));
    assert!(!is_disk_full_error(&GatewayError::Queue(
        "sqlite_error(code=2): no such table".into()
    )));
    // 无前缀 → 文案兜底（旧消息格式）
    assert!(is_disk_full_error(&GatewayError::Queue(
        "database or disk is full".into()
    )));
    assert!(is_disk_full_error(&GatewayError::Queue(
        "disk I/O error".into()
    )));
    assert!(!is_disk_full_error(&GatewayError::Queue(
        "no such table: points".into()
    )));
}
