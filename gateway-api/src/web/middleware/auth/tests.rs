use super::*;
use axum::http::{header, HeaderMap, HeaderValue};

#[test]
fn extract_bearer_token() {
    let mut h = HeaderMap::new();
    h.insert(
        header::AUTHORIZATION,
        HeaderValue::from_static("Bearer abc123"),
    );
    assert_eq!(extract_token_from_headers(&h), Some("abc123".to_string()));
}

#[test]
fn extract_bearer_token_lowercase() {
    let mut h = HeaderMap::new();
    h.insert(
        header::AUTHORIZATION,
        HeaderValue::from_static("bearer abc123"),
    );
    assert_eq!(extract_token_from_headers(&h), Some("abc123".to_string()));
}

#[test]
fn extract_x_api_token() {
    let mut h = HeaderMap::new();
    h.insert("x-api-token", HeaderValue::from_static("xyz789"));
    assert_eq!(extract_token_from_headers(&h), Some("xyz789".to_string()));
}

#[test]
fn extract_no_token() {
    let h = HeaderMap::new();
    assert_eq!(extract_token_from_headers(&h), None);
}

#[test]
fn extract_bearer_prefers_over_x_api_token() {
    let mut h = HeaderMap::new();
    h.insert(
        header::AUTHORIZATION,
        HeaderValue::from_static("Bearer primary"),
    );
    h.insert("x-api-token", HeaderValue::from_static("secondary"));
    assert_eq!(extract_token_from_headers(&h), Some("primary".to_string()));
}

#[test]
fn token_matches_exact() {
    assert!(token_matches("secret-token-123", "secret-token-123"));
}

#[test]
fn token_matches_mismatch() {
    assert!(!token_matches("secret-token-123", "secret-token-456"));
}

#[test]
fn token_matches_different_length() {
    assert!(!token_matches("short", "much-longer-token"));
}

#[test]
fn token_matches_empty() {
    assert!(token_matches("", ""));
    assert!(!token_matches("", "nonempty"));
}

// ===== B1: WebSocket 子协议 / URL query token 提取测试 =====

#[test]
fn extract_ws_subprotocol_token() {
    use axum::http::HeaderValue;
    let mut h = HeaderMap::new();
    h.insert(
        "sec-websocket-protocol",
        HeaderValue::from_static("bearer.secret-token-123"),
    );
    assert_eq!(
        extract_token_from_ws_protocol(&h),
        Some("secret-token-123".to_string())
    );
}

#[test]
fn extract_ws_subprotocol_with_other_protocols() {
    use axum::http::HeaderValue;
    let mut h = HeaderMap::new();
    h.insert(
        "sec-websocket-protocol",
        HeaderValue::from_static("graphql-transport-ws, bearer.tok-abc, soap"),
    );
    // 只识别 `bearer.` 前缀，其他子协议（即使更靠前）应被忽略
    assert_eq!(
        extract_token_from_ws_protocol(&h),
        Some("tok-abc".to_string())
    );
}

#[test]
fn extract_ws_subprotocol_case_sensitive() {
    // `Bearer.` 不识别（避免大小写绕过白名单）
    use axum::http::HeaderValue;
    let mut h = HeaderMap::new();
    h.insert(
        "sec-websocket-protocol",
        HeaderValue::from_static("Bearer.tok-abc"),
    );
    assert_eq!(extract_token_from_ws_protocol(&h), None);
}

#[test]
fn extract_ws_subprotocol_too_long() {
    use axum::http::HeaderValue;
    let mut h = HeaderMap::new();
    let big = format!("bearer.{}", "x".repeat(600));
    h.insert(
        "sec-websocket-protocol",
        HeaderValue::from_str(&big).unwrap(),
    );
    assert_eq!(extract_token_from_ws_protocol(&h), None);
}

#[test]
fn extract_ws_subprotocol_empty_token_rejected() {
    // 仅有 `bearer.` 前缀、token 为空：应被拒（trim 后空）
    use axum::http::HeaderValue;
    let mut h = HeaderMap::new();
    h.insert(
        "sec-websocket-protocol",
        HeaderValue::from_static("bearer."),
    );
    assert_eq!(extract_token_from_ws_protocol(&h), None);
}

#[test]
fn extract_ws_subprotocol_bearer_only_no_token() {
    // 只有 `bearer` 没有点号与 token：strip_prefix 失败，返回 None
    use axum::http::HeaderValue;
    let mut h = HeaderMap::new();
    h.insert("sec-websocket-protocol", HeaderValue::from_static("bearer"));
    assert_eq!(extract_token_from_ws_protocol(&h), None);
}

#[test]
fn extract_ws_subprotocol_unrelated_protocols() {
    // 没有 `bearer.` 前缀的子协议：全部忽略
    use axum::http::HeaderValue;
    let mut h = HeaderMap::new();
    h.insert(
        "sec-websocket-protocol",
        HeaderValue::from_static("graphql-ws, soap, wamp"),
    );
    assert_eq!(extract_token_from_ws_protocol(&h), None);
}

#[test]
fn extract_ws_query_token() {
    let uri: axum::http::Uri = "/api/diagnostics/logs?token=abc-123".parse().unwrap();
    assert_eq!(
        extract_token_from_ws_query(&uri),
        Some("abc-123".to_string())
    );
}

#[test]
fn extract_ws_query_token_percent_encoded() {
    let uri: axum::http::Uri = "/api/diagnostics/logs?token=abc%2D123%2Bxyz"
        .parse()
        .unwrap();
    assert_eq!(
        extract_token_from_ws_query(&uri),
        Some("abc-123+xyz".to_string())
    );
}

#[test]
fn extract_ws_query_token_missing() {
    let uri: axum::http::Uri = "/api/diagnostics/logs".parse().unwrap();
    assert_eq!(extract_token_from_ws_query(&uri), None);
}

#[test]
fn extract_ws_query_token_other_param_ignored() {
    let uri: axum::http::Uri = "/api/diagnostics/logs?other=value".parse().unwrap();
    assert_eq!(extract_token_from_ws_query(&uri), None);
}

// ===== 匿名读模式下的敏感读接口强制鉴权 =====

#[test]
fn sensitive_read_config_paths() {
    assert!(is_sensitive_read_path("/api/config"));
    assert!(is_sensitive_read_path("/api/config/"));
    assert!(is_sensitive_read_path("/api/config/versions"));
    assert!(is_sensitive_read_path("/api/config/versions/7"));
    assert!(is_sensitive_read_path("/api/config/versions/7/compare"));
}

#[test]
fn sensitive_read_audit_and_access_paths() {
    assert!(is_sensitive_read_path("/api/audit/logs"));
    assert!(is_sensitive_read_path("/api/access/api-keys"));
    assert!(is_sensitive_read_path("/api/access/rate-logs"));
    assert!(is_sensitive_read_path("/api/license"));
    assert!(is_sensitive_read_path("/api/certificates"));
}

#[test]
fn sensitive_read_device_health_paths() {
    assert!(is_sensitive_read_path("/api/devices/health"));
    // 设备列表 / 点位列表等常规读数据不受影响（可匿名）
    assert!(!is_sensitive_read_path("/api/devices"));
    assert!(!is_sensitive_read_path("/api/devices/dev-01/points"));
    assert!(!is_sensitive_read_path("/api/devices/assets"));
    assert!(!is_sensitive_read_path("/api/devices/healthx"));
}

#[test]
fn non_sensitive_read_paths_stay_anonymous() {
    assert!(!is_sensitive_read_path("/api/status"));
    assert!(!is_sensitive_read_path("/api/points"));
    assert!(!is_sensitive_read_path("/api/history/query"));
    assert!(!is_sensitive_read_path("/api/realtime/ws"));
    assert!(!is_sensitive_read_path("/health/live"));
}

// ===== 只读 API 密钥的写端点拒绝矩阵 =====

#[test]
fn only_state_changing_methods_count_as_write() {
    for method in ["POST", "PUT", "PATCH", "DELETE"] {
        assert!(is_write_method(method), "{method} 应视为写操作");
    }
    for method in ["GET", "HEAD", "OPTIONS"] {
        assert!(!is_write_method(method), "{method} 不应视为写操作");
    }
}

#[test]
fn readonly_scopes_cannot_write_any_domain() {
    let readonly: Vec<String> = ["points:read", "devices:read", "config:read"]
        .into_iter()
        .map(String::from)
        .collect();
    for path in [
        "/api/points",
        "/api/points/p-1",
        "/api/devices",
        "/api/config",
        "/api/buses",
        "/api/templates",
        "/api/access/api-keys",
        "/api/system/restart",
        "/api/certificates",
        "/api/protocol-tools/packet-editor/send",
        "/api/protocol-tools/auto-detect/run",
        "/api/protocol-tools/preset/apply",
    ] {
        assert!(
            !key_can_write(&readonly, path),
            "只读 scope 不得写入 {path}"
        );
    }
}

#[test]
fn write_scope_is_domain_isolated() {
    let points = vec!["points:write".to_string()];
    assert!(key_can_write(&points, "/api/points"));
    assert!(key_can_write(&points, "/api/point-tools/batch"));
    assert!(key_can_write(&points, "/api/point-tools/write"));
    assert!(key_can_write(&points, "/api/point-trash/p-1"));
    assert!(!key_can_write(&points, "/api/config"));
    assert!(!key_can_write(&points, "/api/buses"));
    assert!(!key_can_write(&points, "/api/devices"));
    assert!(!key_can_write(&points, "/api/access/api-keys"));
    // 反向：devices 域密钥不得借「未归类兜底」写点位工具
    let devices = vec!["devices:write".to_string()];
    assert!(!key_can_write(&devices, "/api/point-tools/batch"));
    assert!(!key_can_write(&devices, "/api/point-trash/p-1"));
}

#[test]
fn devices_write_cannot_reach_bus_intrusive_tools() {
    // F-13：发帧 / 占总线 / 回放执行与系统域同风险，不接受 devices:write
    let devices = vec!["devices:write".to_string()];
    assert!(key_can_write(&devices, "/api/devices"));
    assert!(key_can_write(&devices, "/api/protocol-tools/presets"));
    assert!(!key_can_write(
        &devices,
        "/api/protocol-tools/packet-editor/send"
    ));
    assert!(!key_can_write(
        &devices,
        "/api/protocol-tools/register-scan"
    ));
    assert!(!key_can_write(
        &devices,
        "/api/protocol-tools/auto-detect/run"
    ));
    assert!(!key_can_write(
        &devices,
        "/api/protocol-tools/packet-replay/s-1/execute"
    ));
    // 真实发帧的从站扫描 / iec104 直读同样不得由 devices:write 执行
    assert!(!key_can_write(&devices, "/api/protocol-tools/slave-scan"));
    assert!(!key_can_write(
        &devices,
        "/api/protocol-tools/iec104/ioa-read"
    ));
    // protocol-convert 规则写会落 config，属配置域而非设备域
    assert!(!key_can_write(
        &devices,
        "/api/protocol-tools/convert/rules"
    ));
    // 死信清空/重投不得由设备域密钥执行
    assert!(!key_can_write(
        &devices,
        "/api/north/dead-letters/dl-1/replay"
    ));
    // remote-shell 下 /api/diagnostics/exec 还有路由级 exec 门控，
    // 全局 scope 分类只到 devices 域；exec 的最终拒绝由 8004 负责。
    assert!(key_can_write(&devices, "/api/diagnostics/bus-load/refresh"));
    assert!(key_can_write(&devices, "/api/diagnostics/task-diag/p-1"));
}

#[test]
fn config_write_covers_convert_rules_and_north_dead_letters() {
    let config = vec!["config:write".to_string()];
    // protocol-convert 规则写会持久化到 config.json
    assert!(key_can_write(&config, "/api/protocol-tools/convert/rules"));
    assert!(key_can_write(
        &config,
        "/api/protocol-tools/convert/rules/cv-1/toggle"
    ));
    // 死信重投 / 清理属北向通道运维
    assert!(key_can_write(
        &config,
        "/api/north/dead-letters/dl-1/replay"
    ));
    // 反向：point 域密钥不得清空死信（破坏性、不可逆）
    let points = vec!["points:write".to_string()];
    assert!(!key_can_write(&points, "/api/north/dead-letters"));
    assert!(!key_can_write(&config, "/api/points"));
}

#[test]
fn wildcard_and_admin_scopes_can_write_everything() {
    for scopes in [
        vec!["*".to_string()],
        vec!["admin".to_string()],
        vec!["points:write".to_string(), "admin".to_string()],
    ] {
        assert!(key_can_write(&scopes, "/api/points"));
        assert!(key_can_write(&scopes, "/api/access/api-keys"));
        assert!(key_can_write(&scopes, "/api/certificates"));
    }
}

#[test]
fn unclassified_path_falls_back_to_any_write_scope() {
    // 尚未归类的边缘端点：持有任一 `*:write` 即放行（保持旧语义，避免破坏性变更）
    assert!(key_can_write(
        &["points:write".to_string()],
        "/api/unknown-endpoint"
    ));
    assert!(!key_can_write(
        &["points:read".to_string()],
        "/api/unknown-endpoint"
    ));
}
