//! 鉴权中间件：token 提取 / 恒定时间比对 / 主 token 与 API 密钥鉴权 / 写域 scope 判定。

use std::sync::Arc;

use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};

use crate::web::error::ApiError;
use crate::web::types::AppState;

/// 从请求头中提取 token：支持 `Authorization: Bearer <token>` 或 `X-API-Token: <token>`
pub(crate) fn extract_token_from_headers(headers: &axum::http::HeaderMap) -> Option<String> {
    if let Some(auth) = headers.get(header::AUTHORIZATION) {
        if let Ok(s) = auth.to_str() {
            if let Some(rest) = s.strip_prefix("Bearer ") {
                return Some(rest.trim().to_string());
            }
            if let Some(rest) = s.strip_prefix("bearer ") {
                return Some(rest.trim().to_string());
            }
        }
    }
    if let Some(t) = headers.get("x-api-token") {
        if let Ok(s) = t.to_str() {
            return Some(s.trim().to_string());
        }
    }
    None
}

/// 从 WebSocket Upgrade 协商的 `Sec-WebSocket-Protocol` 子协议中提取 token。
///
/// 浏览器原生 WebSocket 无法设置 `Authorization` 头，但可通过第二参数传入子协议。
/// 前端约定用形如 `bearer.<token>` 的子协议传 token；后端从握手头解析。
///
/// 仅识别 `bearer.` 前缀的子协议——其他子协议（`graphql-ws` / `soap` / `wamp` 等）
/// 一律忽略，避免误用公共子协议名造成 token 泄露。
///
/// 注意：子协议为客户端可声明的字符串，需做长度上限（防止异常大的子协议头）
/// 与字符集限制（避免 NUL / 控制字符污染日志）。
pub(crate) fn extract_token_from_ws_protocol(headers: &axum::http::HeaderMap) -> Option<String> {
    const MAX_PROTOCOL_LEN: usize = 512;
    const PREFIX: &str = "bearer.";
    let raw = headers.get("sec-websocket-protocol")?;
    let s = raw.to_str().ok()?;
    for proto in s.split(',') {
        let p = proto.trim();
        if p.len() > MAX_PROTOCOL_LEN {
            // 异常长的子协议直接拒，防 DoS / 日志膨胀
            continue;
        }
        // 字符集白名单：可见 ASCII 字母数字 + 常见 base64/url-safe 符号
        if !p.bytes().all(|b| b.is_ascii_graphic() || b == b' ') {
            continue;
        }
        if let Some(rest) = p.strip_prefix(PREFIX) {
            let token = rest.trim();
            if !token.is_empty() {
                return Some(token.to_string());
            }
        }
    }
    None
}

/// 从 WebSocket Upgrade 请求的 URL query string 中提取 `token=<value>`。
///
/// 仅在 `allow_anon_read=true` 时接受（query string 易落反代 access log，
/// 严格模式下应改用 `Sec-WebSocket-Protocol` 子协议传 token）。
/// token 长度上限 4 KB，字符集白名单（避免 NUL / 控制字符）。
///
/// 手写 percent-decode 以避免引入 `percent-encoding` 依赖（只识别
/// `%XX` hex 形式；`+` 视为字面量，因为 token 通常不含空格）。
pub(crate) fn extract_token_from_ws_query(uri: &axum::http::Uri) -> Option<String> {
    const MAX_TOKEN_LEN: usize = 4096;
    let q = uri.query()?;
    for pair in q.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if k != "token" {
            continue;
        }
        // 手写 percent-decode
        let bytes = v.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(h), Some(l)) = (hi, lo) {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                    continue;
                }
            }
            out.push(bytes[i]);
            i += 1;
        }
        let token = match String::from_utf8(out) {
            Ok(s) => s,
            Err(_) => continue,
        };
        if token.is_empty() || token.len() > MAX_TOKEN_LEN {
            continue;
        }
        if !token.bytes().all(|b| b.is_ascii_graphic() || b == b' ') {
            continue;
        }
        return Some(token);
    }
    None
}

pub(crate) fn token_matches(provided: &str, expected: &str) -> bool {
    // S8: 先做恒定时间按位比较，仅在末尾才比对长度，
    // 避免长度不等立即返回带来的长度侧信道（可枚举 token 长度）。
    let mut diff: u8 = 0;
    let mut pb = provided.bytes();
    let mut eb = expected.bytes();
    // 恒定时间比较：遍历到较长者结束，缺失字节视为 0（混入 diff），
    // 避免长度不等时提前返回导致的长度侧信道。
    loop {
        match (pb.next(), eb.next()) {
            (Some(a), Some(b)) => diff |= a ^ b,
            (Some(_), None) => diff |= 1,
            (None, Some(_)) => diff |= 1,
            (None, None) => break,
        }
    }
    diff == 0
}

pub(crate) async fn require_auth(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    // 写端点按「域」校验密钥 scope：取方法 + 路径参与判定（只读请求不受限）
    let method = req.method().as_str().to_string();
    let path = req.uri().path().to_string();
    let headers = req.headers();
    match extract_token_from_headers(headers) {
        Some(token) if token_matches(&token, &api_token(&state)) => next.run(req).await,
        // 主 token 未命中 → 尝试 API 密钥（写接口按路径域校验写权限 scope）
        Some(token) => match authenticate_api_key(&state, &token, Some((&method, &path))) {
            Ok(()) => next.run(req).await,
            Err(e) => e.into_response(),
        },
        None => ApiError::unauthorized(
            "missing api token (use Authorization: Bearer <token> or X-API-Token)",
        )
        .into_response(),
    }
}

/// 诊断命令执行的角色门控（`POST /api/diagnostics/exec`）。
///
/// 该接口能执行白名单内的主机命令，属于最高权限操作，只允许：
/// - 主 api_token（等同管理员），或
/// - 持有 `*` / `admin` / `diagnostics:exec` scope 的 API 密钥。
///
/// 其余密钥即便带 `*:write` 也拒绝，避免「能改配置」的密钥顺带获得主机命令执行权。
pub(crate) async fn require_admin_or_exec_scope(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let headers = req.headers();
    match extract_token_from_headers(headers) {
        Some(token) if token_matches(&token, &api_token(&state)) => next.run(req).await,
        Some(token) => match state.platform.api_keys.authenticate(&token) {
            Ok(Some(key)) => {
                let allowed = key
                    .scopes
                    .iter()
                    .any(|s| s == "*" || s == "admin" || s == "diagnostics:exec");
                if allowed {
                    next.run(req).await
                } else {
                    ApiError {
                        status: StatusCode::FORBIDDEN,
                        code: 8004,
                        message: "api key lacks diagnostics exec scope".to_string(),
                    }
                    .into_response()
                }
            }
            Ok(None) => ApiError::unauthorized("invalid api token").into_response(),
            Err(e) => {
                tracing::warn!("api key authenticate error: {}", e);
                ApiError::internal("api key store error").into_response()
            }
        },
        None => ApiError::unauthorized(
            "missing api token (use Authorization: Bearer <token> or X-API-Token)",
        )
        .into_response(),
    }
}

/// API 密钥鉴权：命中且启用且未过期则放行。
///
/// `write_ctx`：`Some((method, path))` 表示这是 require_auth 保护的接口；
/// 其中写方法（POST/PUT/PATCH/DELETE）要求密钥持有**对应域**的写权限 scope，
/// 只读方法（GET/HEAD/OPTIONS）只要求密钥有效。只读接口
/// （require_auth_if_anon_disabled）传 `None`，任何启用密钥均可。
/// 密钥不存在/停用/过期与「错误 token」一样返回 401，不对外区分（防枚举）。
///
/// 错误类型用 `ApiError` 而非 `Response`：`Response` 体积 ≥128 字节，
/// 作为 `Result` 的 Err 变体会在每次返回时做大块栈拷贝（clippy::result_large_err）。
fn authenticate_api_key(
    state: &Arc<AppState>,
    token: &str,
    write_ctx: Option<(&str, &str)>,
) -> Result<(), ApiError> {
    match state.platform.api_keys.authenticate(token) {
        Ok(Some(key)) => {
            if let Some((method, path)) = write_ctx {
                if is_write_method(method) && !key_can_write(&key.scopes, path) {
                    let required = required_write_scope(path).unwrap_or("任一 :write");
                    // 不把 domain 拼进 message：前端 errorCodes.ts 按原文精确匹配
                    tracing::warn!(
                        "api key write denied: path={} required={} scopes={:?}",
                        path,
                        required,
                        key.scopes
                    );
                    return Err(ApiError {
                        status: StatusCode::FORBIDDEN,
                        code: 8003,
                        message: "api key lacks write scope".to_string(),
                    });
                }
            }
            Ok(())
        }
        Ok(None) => Err(ApiError::unauthorized("invalid api token")),
        Err(e) => {
            tracing::warn!("api key authenticate error: {}", e);
            Err(ApiError::internal("api key store error"))
        }
    }
}

/// 写方法判定：只有改变状态的 HTTP 方法才触发域写权限校验。
fn is_write_method(method: &str) -> bool {
    matches!(method, "POST" | "PUT" | "PATCH" | "DELETE")
}

/// 写端点所属的权限域（与 docs/api-access.md 的 scope 词表对齐）。
///
/// 系统域（`/api/access`、`/api/system` 等）映射为 `system:write`，
/// 但该 scope 不在可授予白名单（`ALL_SCOPES`）中——即系统域写操作只有主 token 能通过，
/// 避免 API 密钥通过建用户 / 建密钥提权；其余域按对应 `*:write` 授权。
///
/// 返回 `None` 表示未归类路径，退化为「持有任一 `*:write` 即放行」的旧语义，
/// 避免给尚未归类的边缘端点引入破坏性变更。
fn required_write_scope(path: &str) -> Option<&'static str> {
    if path.starts_with("/api/access")
        || path.starts_with("/api/auth")
        || path.starts_with("/api/license")
        || path.starts_with("/api/system")
        || path.starts_with("/api/watchdog")
        || path.starts_with("/api/webhook")
        || path.starts_with("/api/updates")
        || path.starts_with("/api/network")
        || path.starts_with("/api/ota")
        || path.starts_with("/api/ntp")
        || path.starts_with("/api/certificates")
    {
        Some("system:write")
    } else if is_bus_intrusive_tool(path) {
        // F-13：协议工具里"真实发帧 / 真实占总线"的写操作，侵入性与系统域相当，
        // 归入 system:write（不可授予 API 密钥，只有主 token / admin 可通过）。
        // 纯本地操作（presets / 抓包清理 / 回放会话 CRUD）仍留在 devices:write。
        Some("system:write")
    } else if path.starts_with("/api/protocol-tools/conversion") {
        // F-13：protocol-convert 的开关/规则写会持久化到 config 并触发热更，
        // 实际是配置写操作，不能仅凭 devices:write 放行。
        Some("config:write")
    } else if path.starts_with("/api/config")
        || path.starts_with("/api/buses")
        || path.starts_with("/api/uplink")
        || path.starts_with("/api/templates")
        || path.starts_with("/api/project")
        // 模拟消费者的启停属运行期控制面操作，改的是队列数据的去向
        // （被消费掉且不外发），与 uplink 同属「配置驱动的北向通道」，
        // 故归 config:write。
        || path.starts_with("/api/sim_consumer")
    {
        Some("config:write")
    } else if path.starts_with("/api/points") {
        Some("points:write")
    } else if path.starts_with("/api/devices")
        || path.starts_with("/api/device-groups")
        || path.starts_with("/api/protocol-tools")
        // 数据质量规则影响点位质量码（与设备/点位数据同一风险级），归 devices:write
        || path.starts_with("/api/quality")
        // F-13：诊断写操作（刷新总线负载 / 单点诊断）只影响单设备与总线读取，
        // 与 devices:write 同风险级；原先全域锁 system:write 反而比"会发帧"的
        // protocol-tools 更严，与真实风险倒挂。系统级诊断（exec / logs）有独立入口。
        || path.starts_with("/api/diagnostics")
        || path.starts_with("/api/io")
    {
        Some("devices:write")
    } else if path.starts_with("/api/history") {
        // 预留：当前 /api/history 只有 GET（查询 / CSV 导出）
        Some("history:write")
    } else {
        None
    }
}

/// 会真实占用总线 / 向现场设备发帧的协议工具（F-13）。
///
/// 这些写操作可能扰动运行中的设备（发帧抢占总线、改从站状态），
/// 风险与系统域写操作同级，不接受 `devices:write` 密钥。
fn is_bus_intrusive_tool(path: &str) -> bool {
    path.starts_with("/api/protocol-tools/packet-editor/send")
        || path.starts_with("/api/protocol-tools/register-scan")
        || path.starts_with("/api/protocol-tools/auto-detect/run")
        // 仅"回放执行"会逐帧重发；会话的增删查仍按 devices:write。
        || (path.starts_with("/api/protocol-tools/packet-replay") && path.ends_with("/execute"))
}

/// 密钥是否可写该路径：`*` / `admin` 全放行；已归类路径要求对应域写 scope；
/// 未归类路径接受任一 `*:write`。
fn key_can_write(scopes: &[String], path: &str) -> bool {
    if scopes.iter().any(|s| s == "*" || s == "admin") {
        return true;
    }
    match required_write_scope(path) {
        Some(domain) => scopes.iter().any(|s| s == domain),
        None => scopes.iter().any(|s| s.ends_with(":write")),
    }
}

pub(crate) fn api_token(state: &Arc<AppState>) -> String {
    let cfg_token = state
        .config
        .load_full()
        .web
        .as_ref()
        .map(|w| w.api_token.clone())
        .unwrap_or_default();
    if cfg_token.is_empty() {
        // Bug fix: prepare_web_config 在 api_token 缺失时自动生成并只存入 AppState.web，
        // 运行时 config.web 里仍是空串，直接读会导致登录后所有鉴权接口 401。
        // 回退到启动时生成的 token；热重载若写入了新 token（非空）仍优先使用。
        state.web.api_token.clone()
    } else {
        cfg_token
    }
}

pub(crate) fn allow_anon_read(state: &Arc<AppState>) -> bool {
    state
        .config
        .load_full()
        .web
        .as_ref()
        .map(|w| w.allow_anon_read)
        // Bug T4: 原为 `unwrap_or(true)`（fail-open）——当配置缺少 `web` 节时只读接口
        // 会默认匿名可读，与 Bug 172 将 `WebConfig::allow_anon_read` 默认收敛为 false
        // 的方向相反。统一 fail-closed：无 `web` 配置一律要求 token。
        .unwrap_or(false)
}

/// 敏感读接口：即便 `allow_anon_read=true` 也必须携带 token。
///
/// `allow_anon_read` 的语义是「放行现场看板类的低敏只读数据」，不是「全量配置对外公开」。
/// 以下路径即使开启匿名读也必须鉴权，避免未登记客户端直接拉到凭据结构 / 审计记录 /
/// 设备访问密钥：
/// - `/api/config*`：整份配置树（含 token、webhook 头、协议凭据，虽已脱敏但仍暴露拓扑）
/// - `/api/audit*`：审计日志（谁在什么时候改了什么，可被用于反查运维行为）
/// - `/api/access*`：API 密钥列表 / 访问限流日志（密钥即凭据）
/// - `/api/license*`：授权信息
/// - `/api/certificates*`：证书清单
/// - `/api/devices/health*`：设备健康度（可推断产线停机情况）
fn is_sensitive_read_path(path: &str) -> bool {
    let p = path.trim_end_matches('/');
    p == "/api/config"
        || p.starts_with("/api/config/")
        || p.starts_with("/api/audit")
        || p.starts_with("/api/access")
        || p.starts_with("/api/license")
        || p.starts_with("/api/certificates")
        || p == "/api/devices/health"
        || p.starts_with("/api/devices/health/")
}

pub(crate) async fn require_auth_if_anon_disabled(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let path = req.uri().path().to_string();
    // 匿名读模式下，敏感读接口仍强制鉴权（见 is_sensitive_read_path）
    if allow_anon_read(&state) && !is_sensitive_read_path(&path) {
        return next.run(req).await;
    }
    let headers = req.headers();
    // 浏览器 WebSocket 握手无法携带 Authorization 头，token 约定走
    // `bearer.<token>` Sec-WebSocket-Protocol 子协议；此处必须同样识别，
    // 否则 /api/realtime 等 WS 接口会在中间件层被 401，handler 内的
    // 子协议校验永远执行不到。
    let provided =
        extract_token_from_headers(headers).or_else(|| extract_token_from_ws_protocol(headers));
    match provided {
        Some(token) if token_matches(&token, &api_token(&state)) => next.run(req).await,
        // 主 token 未命中 → 尝试 API 密钥（只读接口不校验 scope）
        Some(token) => match authenticate_api_key(&state, &token, None) {
            Ok(()) => next.run(req).await,
            Err(e) => e.into_response(),
        },
        None => ApiError::unauthorized(
            "authentication required; provide Authorization: Bearer <token> or X-API-Token",
        )
        .into_response(),
    }
}

#[cfg(test)]
mod tests {
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
}
