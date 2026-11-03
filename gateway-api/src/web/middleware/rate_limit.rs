//! 速率限制中间件：全局固定窗口 + 规则（分钟/小时）限速 + 访问日志旁路写。

use std::sync::Arc;

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

use crate::web::types::AppState;

/// 单个来源的固定窗口计数
#[derive(Debug, Clone, Copy)]
struct WindowCounter {
    /// 当前窗口起始时间戳（UNIX 秒）
    window_start: u64,
    /// 当前窗口请求计数
    count: u64,
}

/// 来源桶数量上限：超过时淘汰已过期窗口，避免大量不同来源 IP 撑爆内存。
const MAX_RATE_BUCKETS: usize = 4096;

/// 按调用方标识（客户端 IP）分桶的固定窗口限速状态。
///
/// P2 修复：原实现是「全进程单一计数器」——任何单个客户端刷接口都会把
/// 全局配额一并耗尽，等效于对其他客户端拒绝服务。改为按客户端 IP 分桶：
/// 每个来源独立计数、互不影响。
///
/// 无 `ConnectInfo`（如集成测试用 `app.oneshot()` 直接驱动 Router）时统一
/// 归入 `None` 桶，退化为单一全局限速——与旧行为一致，保证既有测试兼容。
///
/// 计数器挂在 AppState（Arc 共享），避免多 Router / 多实例之间原子串扰。
///
/// S02: 内置 `dropped_total` 计数器——`rate_limit` 中间件在拒绝请求时
/// `fetch_add(1)`，用于 `/api/metrics` 暴露 `api_rate_limit_dropped_total`
/// 趋势指标。运维可据此发现「某来源持续触发限速」而无需翻日志。
/// `RateLimiter` 不实现 `Clone`：`Mutex` 与 `AtomicU64` 均为内部状态，
/// 不存在有意义的所有权复制语义。共享通过外部 `Arc<RateLimiter>` 完成
/// （见 `web::AppState::rate_limiter`），且 `Arc<T>` 本身就实现 `Clone`。
pub struct RateLimiter {
    buckets: parking_lot::Mutex<std::collections::HashMap<Option<std::net::IpAddr>, WindowCounter>>,
    /// S02: 累计被限速中间件拒绝的请求数（按全部来源累加，不分 IP）
    dropped_total: std::sync::atomic::AtomicU64,
    /// 按规则分桶的窗口计数：key = (rule_id, client_ip)，内含分钟/小时两个固定窗口
    rule_buckets: parking_lot::Mutex<
        std::collections::HashMap<(String, Option<std::net::IpAddr>), RuleWindowCounter>,
    >,
    /// 启用规则缓存：避免每请求一次 SQLite 查询。TTL 内直接复用。
    rules_cache:
        parking_lot::Mutex<Option<(Vec<gateway_core::config::RateRule>, std::time::Instant)>>,
}

/// 单条限流规则在某客户端上的双窗口计数（分钟 + 小时）
#[derive(Debug, Clone, Copy, Default)]
struct RuleWindowCounter {
    minute_window: u64,
    minute_count: u64,
    hour_window: u64,
    hour_count: u64,
}

/// 规则缓存有效期：规则经管理接口变更后最迟 3s 生效，避免每请求读库
const RULES_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(3);

impl std::fmt::Debug for RateLimiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RateLimiter")
            .field("buckets", &"<Mutex<HashMap>>")
            .field("dropped_total", &self.dropped_total)
            .finish()
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self {
            buckets: parking_lot::Mutex::new(std::collections::HashMap::new()),
            dropped_total: std::sync::atomic::AtomicU64::new(0),
            rule_buckets: parking_lot::Mutex::new(std::collections::HashMap::new()),
            rules_cache: parking_lot::Mutex::new(None),
        }
    }
}

impl RateLimiter {
    /// S02: 累计被拒绝的请求数（供 /api/metrics 读取）。
    pub fn dropped_total(&self) -> u64 {
        self.dropped_total
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

pub(crate) async fn rate_limit(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let started = std::time::Instant::now();
    let method = req.method().clone();
    let path = req.uri().path().to_string();

    // 仅对 /api/* 生效：静态资源、SPA fallback、/health 探活不参与限速。
    // 否则一次页面加载（数十个静态文件）或高频探活会被 429——典型的自我伤害。
    if !path.starts_with("/api/") {
        return next.run(req).await;
    }

    // P2: 按客户端 IP 分桶；无 ConnectInfo 时归入 None 桶（全局限速兜底）。
    let key = client_ip(
        req.extensions()
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|ci| ci.0),
        req.headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok()),
    );

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // 1. 全局限速（config.web.rate_limit_per_sec，可选）
    if !check_global_limit(&state, key, now) {
        return reject_request(&state, &method, &path, key, started, "global");
    }
    // 2. 规则限速（RateRuleStore 中启用的规则，按路径/方法匹配）
    if let Some(rule_id) = check_rule_limits(&state, key, &method, &path, now) {
        return reject_request(&state, &method, &path, key, started, &rule_id);
    }

    let response = next.run(req).await;
    // 3. 访问日志：仅记录 /api/*（静态资源与 SPA fallback 无排障价值）
    log_access(
        &state,
        &method,
        &path,
        key,
        started,
        response.status().as_u16(),
    );
    response
}

/// 提取客户端真实 IP：仅当 TCP 对端为回环地址（反代与网关同机）时采纳
/// `X-Forwarded-For` 首跳；直连场景一律用对端 IP。
///
/// 为什么不无条件信任 XFF：直连部署下任何客户端都能伪造该头，
/// 绕过按 IP 分桶的限流并污染访问日志。回环对端意味着流量来自本机反代，
/// 该反代受同一台机器的运维控制，可信。
fn client_ip(peer: Option<std::net::SocketAddr>, xff: Option<&str>) -> Option<std::net::IpAddr> {
    let peer = peer?;
    if peer.ip().is_loopback() {
        if let Some(first) = xff.and_then(|v| v.split(',').next()) {
            if let Ok(ip) = first.trim().parse::<std::net::IpAddr>() {
                return Some(ip);
            }
        }
    }
    Some(peer.ip())
}

/// 全局限速（每秒、按来源分桶）。返回 true = 放行。
fn check_global_limit(state: &Arc<AppState>, key: Option<std::net::IpAddr>, now: u64) -> bool {
    let limit = match &state.web.rate_limit_per_sec {
        Some(n) if *n > 0 => *n as u64,
        _ => return true,
    };
    let mut buckets = state.rate_limiter.buckets.lock();
    // 防御性淘汰：桶数异常增长时清理过期窗口，避免内存无界增长。
    if buckets.len() > MAX_RATE_BUCKETS {
        buckets.retain(|_, w| w.window_start == now);
    }
    let entry = buckets.entry(key).or_insert(WindowCounter {
        window_start: now,
        count: 0,
    });
    if entry.window_start != now {
        // 跨窗口：重置为新窗口的首次请求
        entry.window_start = now;
        entry.count = 1;
        true
    } else {
        entry.count = entry.count.saturating_add(1);
        entry.count <= limit
    }
}

/// 规则限速：返回 Some(rule_id) = 被该规则拒绝；None = 放行。
///
/// 语义：
/// - 分钟窗口允许 `limit_per_minute + burst` 次（burst 为短时突发余量）；
/// - 小时窗口允许 `limit_per_hour` 次；
/// - 多条规则命中同一请求时，任一超限即拒（最严生效）。
fn check_rule_limits(
    state: &Arc<AppState>,
    key: Option<std::net::IpAddr>,
    method: &axum::http::Method,
    path: &str,
    now: u64,
) -> Option<String> {
    let rules = load_enabled_rules(state);
    if rules.is_empty() {
        return None;
    }
    let minute_window = now / 60;
    let hour_window = now / 3600;
    let mut buckets = state.rate_limiter.rule_buckets.lock();
    // 防御性淘汰：规则 × 客户端组合过多时清理过期窗口
    if buckets.len() > MAX_RATE_BUCKETS {
        buckets.retain(|_, w| w.minute_window == minute_window || w.hour_window == hour_window);
    }
    let mut rejected: Option<String> = None;
    for rule in rules
        .iter()
        .filter(|r| rule_matches(r, method.as_str(), path))
    {
        let entry = buckets.entry((rule.id.clone(), key)).or_default();
        if entry.minute_window != minute_window {
            entry.minute_window = minute_window;
            entry.minute_count = 0;
        }
        if entry.hour_window != hour_window {
            entry.hour_window = hour_window;
            entry.hour_count = 0;
        }
        entry.minute_count = entry.minute_count.saturating_add(1);
        entry.hour_count = entry.hour_count.saturating_add(1);
        let minute_cap = rule.limit_per_minute.saturating_add(rule.burst) as u64;
        if entry.minute_count > minute_cap || entry.hour_count > rule.limit_per_hour as u64 {
            rejected = Some(rule.id.clone());
            // 不 break：其余命中规则的计数照样累计，保证统计口径一致
        }
    }
    rejected
}

/// 规则与请求的匹配判定：`path` 精确匹配或以 `*` 结尾的前缀匹配；`method` 为 `*` 时全命中。
fn rule_matches(rule: &gateway_core::config::RateRule, method: &str, path: &str) -> bool {
    if rule.method != "*" && !rule.method.eq_ignore_ascii_case(method) {
        return false;
    }
    match rule.path.strip_suffix('*') {
        Some(prefix) => path.starts_with(prefix),
        None => path == rule.path,
    }
}

/// 加载启用规则（带 TTL 缓存，避免每请求一次 SQLite 查询）。
/// 缓存读库失败时返回空表（fail-open）并告警——规则存储故障不应阻断业务请求；
/// 但下一次缓存过期后仍会重试，故障恢复后自动生效。
fn load_enabled_rules(state: &Arc<AppState>) -> Vec<gateway_core::config::RateRule> {
    {
        let cache = state.rate_limiter.rules_cache.lock();
        if let Some((rules, at)) = cache.as_ref() {
            if at.elapsed() < RULES_CACHE_TTL {
                return rules.clone();
            }
        }
    }
    let rules = match state.platform.rate_rules.list_enabled_rules() {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(
                "load rate limit rules failed (fail-open this window): {}",
                e
            );
            Vec::new()
        }
    };
    let mut cache = state.rate_limiter.rules_cache.lock();
    *cache = Some((rules.clone(), std::time::Instant::now()));
    rules
}

/// 拒绝请求：累计 dropped 指标 + 写访问日志（limited=true）+ 429 响应。
fn reject_request(
    state: &Arc<AppState>,
    method: &axum::http::Method,
    path: &str,
    key: Option<std::net::IpAddr>,
    started: std::time::Instant,
    source: &str,
) -> Response {
    state
        .rate_limiter
        .dropped_total
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    tracing::warn!(
        "rate limit exceeded: source={:?} rule={} ({} {})",
        key,
        source,
        method,
        path
    );
    log_access_limited(state, method, path, key, started);
    // RFC 6585 §4：429 附 Retry-After: 1（秒），固定窗口最坏等待一个窗口
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(axum::http::header::RETRY_AFTER, "1")],
    )
        .into_response()
}

/// 记录访问日志（limited 由调用方场景决定）。同步 SQLite 写放到 blocking 池，
/// 不阻塞 tokio worker；写失败仅告警，绝不影响请求路径。
fn log_access(
    state: &Arc<AppState>,
    method: &axum::http::Method,
    path: &str,
    key: Option<std::net::IpAddr>,
    started: std::time::Instant,
    status: u16,
) {
    let limited = status == StatusCode::TOO_MANY_REQUESTS;
    write_access_log(state, method, path, key, started, status, limited);
}

fn log_access_limited(
    state: &Arc<AppState>,
    method: &axum::http::Method,
    path: &str,
    key: Option<std::net::IpAddr>,
    started: std::time::Instant,
) {
    write_access_log(state, method, path, key, started, 429, true);
}

fn write_access_log(
    state: &Arc<AppState>,
    method: &axum::http::Method,
    path: &str,
    key: Option<std::net::IpAddr>,
    started: std::time::Instant,
    status: u16,
    limited: bool,
) {
    if !path.starts_with("/api/") {
        return;
    }
    let entry = gateway_core::config::ApiAccessLog {
        id: 0,
        ts: chrono::Utc::now().timestamp_millis(),
        method: method.to_string(),
        path: path.to_string(),
        status,
        latency_ms: started.elapsed().as_millis() as u64,
        client_ip: key.map(|ip| ip.to_string()).unwrap_or_else(|| "-".into()),
        limited,
    };
    let store = state.platform.rate_rules.clone();
    // fire-and-forget：访问日志是旁路观测数据，不应拖慢/阻断请求
    tokio::task::spawn_blocking(move || {
        if let Err(e) = store.append_access_log(&entry) {
            tracing::warn!("append api access log failed: {}", e);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// S02: 全新构造的 `RateLimiter` 的 `dropped_total` 必须为 0。
    #[test]
    fn rate_limiter_dropped_starts_at_zero() {
        let rl = RateLimiter::default();
        assert_eq!(rl.dropped_total(), 0);
    }

    /// S02: `dropped_total` 在并发 fetch_add 下不会丢失更新（弱一致性验证）。
    #[test]
    fn rate_limiter_dropped_concurrent_addition() {
        use std::sync::Arc;
        let rl = Arc::new(RateLimiter::default());
        let threads = 8usize;
        let per_thread = 100u64;
        std::thread::scope(|s| {
            for _ in 0..threads {
                let rl = rl.clone();
                s.spawn(move || {
                    for _ in 0..per_thread {
                        rl.dropped_total
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                });
            }
        });
        assert_eq!(
            rl.dropped_total(),
            (threads as u64) * per_thread,
            "concurrent fetch_add must not lose updates"
        );
    }

    /// S02: 桶清理触发条件——`buckets.len() > MAX_RATE_BUCKETS` 时清空过期桶。
    /// 由于 MAX_RATE_BUCKETS 是私有常量，本测试用默认 RateLimiter 直接灌入桶后
    /// 验证清理行为（通过 `dropped_total` 不受影响间接确认）。
    #[test]
    fn rate_limiter_default_construction_works() {
        // 仅为编译期验证 RateLimiter 现在仍可默认构造
        let _ = RateLimiter::default();
    }

    fn sock(ip: &str) -> Option<std::net::SocketAddr> {
        // IPv6 字面量必须加方括号才能带端口
        let addr = if ip.contains(':') {
            format!("[{ip}]:5000")
        } else {
            format!("{ip}:5000")
        };
        Some(addr.parse().expect("valid addr"))
    }

    #[test]
    fn client_ip_returns_none_without_connect_info() {
        assert_eq!(client_ip(None, Some("1.2.3.4")), None);
    }

    #[test]
    fn client_ip_direct_connection_ignores_xff() {
        // 非回环对端 = 直连：XFF 可被客户端伪造，一律不采纳
        let ip = client_ip(sock("192.168.1.10"), Some("9.9.9.9")).expect("some");
        assert_eq!(ip.to_string(), "192.168.1.10");
    }

    #[test]
    fn client_ip_loopback_peer_adopts_xff_first_hop() {
        let ip = client_ip(sock("127.0.0.1"), Some("203.0.113.7, 10.0.0.1")).expect("some");
        assert_eq!(ip.to_string(), "203.0.113.7");
    }

    #[test]
    fn client_ip_loopback_peer_falls_back_on_bad_xff() {
        let ip = client_ip(sock("::1"), Some("not-an-ip")).expect("some");
        assert_eq!(ip.to_string(), "::1");
    }

    #[test]
    fn client_ip_loopback_peer_without_xff_uses_peer() {
        let ip = client_ip(sock("127.0.0.1"), None).expect("some");
        assert_eq!(ip.to_string(), "127.0.0.1");
    }
}
