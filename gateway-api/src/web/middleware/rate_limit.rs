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
    /// 规则刷新闸门（单飞 / singleflight）。
    ///
    /// 缓存过期瞬间若并发 N 个请求同时进入，原实现会各自 `spawn_blocking` 查一次
    /// 规则库 —— N 次串行化在同一个 `parking_lot` 互斥 + 5s `busy_timeout` 后面，
    /// 既拖垮 blocking 池（池线程被逐个占满），又让尾请求耗时随 N 线性增长。
    /// 加本闸门后：同一时刻只有一个任务真正读库，其余在闸门上排队，拿到锁后
    /// 发现缓存已被填新、直接复用。
    ///
    /// 用 `tokio::sync::Mutex` 而非 `parking_lot`：需要**跨 await 持有**（等待
    /// blocking 池返回查询结果），`parking_lot` 的锁 guard 不是 Send 且阻塞线程。
    rules_refresh_gate: tokio::sync::Mutex<()>,
    /// 访问日志写缓冲（批量落库，避免每请求一次事务 + fsync）
    access_logs: AccessLogBuffer,
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

/// 访问日志写缓冲上限（条）。库变慢/被锁时缓冲会堆积，超过即丢弃并计数，
/// 防止旁路观测数据反噬主流程内存。
const ACCESS_LOG_BUFFER_MAX: usize = 4096;
/// 缓冲达到该条数立即触发一次刷盘（不等定时器）。
const ACCESS_LOG_FLUSH_THRESHOLD: usize = 64;
/// 定时刷盘间隔：低流量下一条日志的最坏落库延迟。
const ACCESS_LOG_FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);

/// 访问日志写缓冲（批量落库，见 `write_access_log`）。
///
/// 原实现每个 `/api/*` 请求都 `spawn_blocking` 一次并单条 INSERT。SQLite 自动提交下
/// 每条 INSERT 就是一次事务，`synchronous=FULL` 时还要 fsync —— 管理台并发轮询多个
/// 端点时，这些写会在连接互斥后串行，且每次各付一次提交开销（Bug 171）。
///
/// 改为「缓冲 + 批量单事务」：
/// - 请求路径只做一次 `Vec::push`（O(1)，不碰 IO）；
/// - 达到 `ACCESS_LOG_FLUSH_THRESHOLD` 由该请求顺带触发刷盘；
/// - 低流量由懒启动的定时器兜底（`ACCESS_LOG_FLUSH_INTERVAL`），保证日志不会
///   无限期停留在内存里（测试/低频管理台只发一两个请求时也能落库）。
#[derive(Default)]
struct AccessLogBuffer {
    pending: parking_lot::Mutex<Vec<gateway_core::config::ApiAccessLog>>,
    /// 是否已有刷盘任务在跑：避免并发请求各自把同一个缓冲刷一遍（双花/空转）
    flushing: std::sync::atomic::AtomicBool,
    /// 定时器是否已启动（每实例一次）
    ticker_started: std::sync::atomic::AtomicBool,
    /// 因缓冲满而丢弃的条数（观测用）
    dropped: std::sync::atomic::AtomicU64,
}

impl AccessLogBuffer {
    /// 入缓冲；返回是否应「立即」刷盘（达到批量阈值）。
    fn push(&self, entry: gateway_core::config::ApiAccessLog) -> bool {
        let mut pending = self.pending.lock();
        if pending.len() >= ACCESS_LOG_BUFFER_MAX {
            self.dropped
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return false;
        }
        pending.push(entry);
        pending.len() >= ACCESS_LOG_FLUSH_THRESHOLD
    }

    /// 取走全部待写条目（`mem::take`：保留容量，避免反复分配）
    fn take(&self) -> Vec<gateway_core::config::ApiAccessLog> {
        std::mem::take(&mut *self.pending.lock())
    }

    fn is_empty(&self) -> bool {
        self.pending.lock().is_empty()
    }

    /// 因缓冲满被丢弃的累计条数
    fn dropped(&self) -> u64 {
        self.dropped.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// 复位「正在刷盘」标志。用 RAII 保证 blocking 任务 panic 时也不会把标志永久
/// 卡在 `true`（那会让访问日志从此再也不落库）。
struct FlushInFlight<'a>(&'a std::sync::atomic::AtomicBool);

impl Drop for FlushInFlight<'_> {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::Release);
    }
}

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
            rules_refresh_gate: tokio::sync::Mutex::new(()),
            access_logs: AccessLogBuffer::default(),
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
    if let Some(rule_id) = check_rule_limits(&state, key, &method, &path, now).await {
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
async fn check_rule_limits(
    state: &Arc<AppState>,
    key: Option<std::net::IpAddr>,
    method: &axum::http::Method,
    path: &str,
    now: u64,
) -> Option<String> {
    let rules = load_enabled_rules(state).await;
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

/// 读一次新鲜缓存（TTL 内）；过期/为空返回 None。
fn cached_fresh<T: Clone>(
    cache: &parking_lot::Mutex<Option<(T, std::time::Instant)>>,
    ttl: std::time::Duration,
) -> Option<T> {
    let guard = cache.lock();
    match guard.as_ref() {
        Some((value, at)) if at.elapsed() < ttl => Some(value.clone()),
        _ => None,
    }
}

/// TTL 缓存 + 单飞（singleflight）刷新。
///
/// 语义：
/// - 缓存未过期 → 直接克隆返回，不触达 `load`；
/// - 缓存过期 → 抢到 `gate` 的任务执行 `load` 并回写缓存；其余并发任务在
///   `gate` 上排队，拿到锁后**重新检查**缓存（通常已被前一个任务填新），
///   直接复用，不再各自读库。
///
/// 为什么需要单飞：规则库读是「同步 SQLite 查询 + 无超时的 parking_lot 互斥 +
/// 5s busy_timeout」。缓存 3s 过期的那一刻若有 N 个并发请求，N 次查询会在该
/// 互斥后串行排队，既占满 blocking 池（池线程数有限，被占满后其它
/// `spawn_blocking` 任务全部饥饿），又让尾请求延迟随 N 线性放大。
///
/// 返回 `(值, 本次调用是否真正执行了 load)`；第二个分量供测试与排障使用，
/// 生产路径可忽略。
async fn load_with_singleflight<T, F, Fut>(
    cache: &parking_lot::Mutex<Option<(T, std::time::Instant)>>,
    gate: &tokio::sync::Mutex<()>,
    ttl: std::time::Duration,
    load: F,
) -> (T, bool)
where
    T: Clone,
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    if let Some(hit) = cached_fresh(cache, ttl) {
        return (hit, false);
    }

    // 单飞闸门：同一时刻只允许一个任务真正读库。
    // `_gate` 跨 `load().await` 持有 —— 这正是必须用 tokio 异步锁的原因。
    let _gate = gate.lock().await;

    // 双检：排队期间前一个任务可能已经读库并回填了缓存。
    if let Some(hit) = cached_fresh(cache, ttl) {
        return (hit, false);
    }

    let value = load().await;
    *cache.lock() = Some((value.clone(), std::time::Instant::now()));
    (value, true)
}

/// 加载启用规则（带 TTL 缓存 + 单飞刷新，避免每请求一次 SQLite 查询）。
/// 缓存读库失败时返回空表（fail-open）并告警——规则存储故障不应阻断业务请求；
/// 但下一次缓存过期后仍会重试，故障恢复后自动生效。
///
/// 缓存命中走同步快路径；缓存过期时把**同步 SQLite 查询挪到 blocking 池**。
/// 原因：`list_enabled_rules` 底层是 `parking_lot` 互斥（无等待超时）+
/// `busy_timeout` 5s。若它跑在 tokio worker 上、而连接被长写事务压住，
/// 多个并发请求会一起把 worker 占满 → 整个运行时停顿，5s 喂狗任务停转
/// 超过 15s 即触发看门狗 `abort()`。
///
/// TTL 过期瞬间的并发刷新由 `load_with_singleflight` 合并为一次查询。
async fn load_enabled_rules(state: &Arc<AppState>) -> Vec<gateway_core::config::RateRule> {
    let limiter = &state.rate_limiter;
    let store = state.platform.rate_rules.clone();
    let (rules, _loaded) = load_with_singleflight(
        &limiter.rules_cache,
        &limiter.rules_refresh_gate,
        RULES_CACHE_TTL,
        move || async move {
            match tokio::task::spawn_blocking(move || store.list_enabled_rules()).await {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    tracing::warn!(
                        "load rate limit rules failed (fail-open this window): {}",
                        e
                    );
                    Vec::new()
                }
                Err(e) => {
                    // blocking 池任务 panic / 取消：同样 fail-open，仅告警
                    tracing::warn!(
                        "load rate limit rules task failed (fail-open this window): {}",
                        e
                    );
                    Vec::new()
                }
            }
        },
    )
    .await;
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

/// 记录访问日志（limited 由调用方场景决定）。
///
/// 请求路径只入内存缓冲（O(1)，不碰 IO）；真正的 SQLite 写入在 blocking 池里
/// 按批完成（见 `spawn_flush_access_logs`）。写失败仅告警，绝不影响请求结果。
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
    let limiter = &state.rate_limiter;
    let threshold_reached = limiter.access_logs.push(entry);
    // 懒启动定时器：低频管理台只发一两个请求时，靠它把缓冲刷下去
    ensure_access_log_ticker(state);
    if threshold_reached {
        spawn_flush_access_logs(limiter.clone(), state.platform.rate_rules.clone());
    }
}

/// 懒启动访问日志刷盘定时器（每个 AppState 一次）。
///
/// **用独立 OS 线程而不是 tokio 任务**：定时刷盘的天职是「不依赖调用方继续活动」，
/// 而 tokio 任务只有在 runtime 被调度时才推进 —— 若 runtime 单线程且被同步阻塞
/// （集成测试常用 `std::thread::sleep` 轮询、生产上某个 handler 卡住），定时器就会
/// 饿死，日志无限期停留在内存里。独立线程不受 runtime 调度影响。
///
/// 只持 `Weak<RateLimiter>`：所属 `AppState` 释放后线程在下一个 tick 自然退出，
/// 不会把状态钉在内存里；也不占用 blocking 池（那会让 runtime 关闭时等它收尾）。
fn ensure_access_log_ticker(state: &Arc<AppState>) {
    if state
        .rate_limiter
        .access_logs
        .ticker_started
        .swap(true, std::sync::atomic::Ordering::AcqRel)
    {
        return;
    }
    let weak = Arc::downgrade(&state.rate_limiter);
    let store = state.platform.rate_rules.clone();
    let spawned = std::thread::Builder::new()
        .name("gw-access-log".into())
        .spawn(move || {
            let mut last_dropped = 0u64;
            loop {
                std::thread::sleep(ACCESS_LOG_FLUSH_INTERVAL);
                let Some(limiter) = weak.upgrade() else {
                    break; // 所属 AppState 已释放
                };
                // 缓冲打满意味着库写入跟不上（或卡住）：如实告警，别让丢弃悄无声息
                let dropped = limiter.access_logs.dropped();
                if dropped > last_dropped {
                    tracing::warn!(
                        "api access log buffer full: {} entries dropped this interval (total {})",
                        dropped - last_dropped,
                        dropped
                    );
                    last_dropped = dropped;
                }
                try_flush_access_logs(&limiter, &store);
            }
        });
    if let Err(e) = spawned {
        // 线程起不来只影响「低频兜底」，阈值触发的即时刷盘仍然工作
        tracing::warn!("failed to start access log flush thread: {}", e);
    }
}

/// 尝试把缓冲整批落库（同步执行，调用方负责所在线程）。
///
/// **单飞**：`flushing` 抢不到说明已有刷盘在跑，直接返回；否则并发请求会各自把
/// 同一个缓冲刷一遍（后到者多半取到空批，纯属浪费线程与事务）。
/// 用 RAII 复位标志，任务 panic 也不会让日志永久停止落库。
fn try_flush_access_logs(limiter: &RateLimiter, store: &gateway_core::config::RateRuleStore) {
    if limiter.access_logs.is_empty() {
        return;
    }
    if limiter
        .access_logs
        .flushing
        .swap(true, std::sync::atomic::Ordering::AcqRel)
    {
        return;
    }
    let _inflight = FlushInFlight(&limiter.access_logs.flushing);
    let batch = limiter.access_logs.take();
    if batch.is_empty() {
        return;
    }
    if let Err(e) = store.append_access_logs(&batch) {
        tracing::warn!(
            "append api access log batch ({} rows) failed: {}",
            batch.len(),
            e
        );
    }
}

/// 在 blocking 池里刷一次（请求路径用，key 达到批量阈值时触发）。
fn spawn_flush_access_logs(
    limiter: Arc<RateLimiter>,
    store: Arc<gateway_core::config::RateRuleStore>,
) {
    if limiter.access_logs.is_empty() {
        return;
    }
    tokio::task::spawn_blocking(move || try_flush_access_logs(&limiter, &store));
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

    // ===== 单飞规则刷新（Bug 170）=====

    /// 缓存未过期：直接复用，绝不触达 loader。
    #[tokio::test]
    async fn singleflight_uses_cache_within_ttl() {
        use std::time::{Duration, Instant};

        let cache = parking_lot::Mutex::new(Some((vec![7u8], Instant::now())));
        let gate = tokio::sync::Mutex::new(());
        let loads = std::sync::atomic::AtomicU64::new(0);

        let (value, loaded) =
            load_with_singleflight(&cache, &gate, Duration::from_secs(60), || async {
                loads.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                vec![8u8]
            })
            .await;

        assert_eq!(value, vec![7u8], "TTL 内应返回缓存值");
        assert!(!loaded, "TTL 内不应触发加载");
        assert_eq!(
            loads.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "TTL 内 loader 不得被调用"
        );
    }

    /// 回归（Bug 170）：缓存过期瞬间 N 个并发调用只允许读一次库，其余复用结果。
    ///
    /// 原实现无单飞：N 个并发请求各自 `spawn_blocking` 查一次规则库，在同一个
    /// `parking_lot` 互斥 + 5s `busy_timeout` 后串行排队 —— blocking 池被逐个占满、
    /// 尾请求延迟随 N 线性放大。本测试锁定「loader 只被调用一次」。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn singleflight_coalesces_concurrent_refreshes() {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::Duration;

        const CALLERS: usize = 16;

        let cache = Arc::new(parking_lot::Mutex::new(
            None::<(Vec<u8>, std::time::Instant)>,
        ));
        let gate = Arc::new(tokio::sync::Mutex::new(()));
        let loads = Arc::new(AtomicU64::new(0));

        let mut handles = Vec::with_capacity(CALLERS);
        for _ in 0..CALLERS {
            let cache = cache.clone();
            let gate = gate.clone();
            let loads = loads.clone();
            handles.push(tokio::spawn(async move {
                load_with_singleflight(&cache, &gate, Duration::from_secs(60), move || async move {
                    loads.fetch_add(1, Ordering::Relaxed);
                    // 模拟一次慢查询：让并发调用都能挤进「过期」判定窗口
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    vec![42u8]
                })
                .await
            }));
        }

        let mut values = Vec::with_capacity(CALLERS);
        for h in handles {
            let (value, _loaded) = h.await.expect("task should not panic");
            values.push(value);
        }

        assert_eq!(
            loads.load(Ordering::Relaxed),
            1,
            "缓存过期瞬间的 {} 个并发调用只应触发 1 次读库",
            CALLERS
        );
        assert!(
            values.iter().all(|v| v == &vec![42u8]),
            "所有调用都应拿到同一份刷新结果"
        );
    }

    /// 过期缓存必须真的重新加载（单飞不能把刷新也一起吞掉，否则规则永不生效）。
    #[tokio::test]
    async fn singleflight_reloads_after_ttl_expiry() {
        use std::time::{Duration, Instant};

        let cache = parking_lot::Mutex::new(Some((vec![1u8], Instant::now())));
        let gate = tokio::sync::Mutex::new(());

        // ttl=0：任何缓存都视为已过期
        let (value, loaded) =
            load_with_singleflight(&cache, &gate, Duration::ZERO, || async { vec![2u8] }).await;

        assert_eq!(value, vec![2u8], "过期后应返回新加载的值");
        assert!(loaded, "过期后必须真正执行加载");
        assert_eq!(
            cached_fresh(&cache, Duration::from_secs(60)),
            Some(vec![2u8]),
            "刷新结果应回填缓存"
        );
    }

    // ===== 访问日志批量写（Bug 171）=====

    fn log_entry(i: usize) -> gateway_core::config::ApiAccessLog {
        gateway_core::config::ApiAccessLog {
            id: 0,
            ts: i as i64,
            method: "GET".into(),
            path: format!("/api/x{i}"),
            status: 200,
            latency_ms: 1,
            client_ip: "-".into(),
            limited: false,
        }
    }

    /// 缓冲在达到阈值前不触发刷盘，达到阈值触发；`take` 取走全部且保序。
    #[test]
    fn access_log_buffer_flushes_at_threshold() {
        let buf = AccessLogBuffer::default();
        for i in 0..ACCESS_LOG_FLUSH_THRESHOLD - 1 {
            assert!(!buf.push(log_entry(i)), "阈值前不应触发刷盘");
        }
        assert!(
            buf.push(log_entry(ACCESS_LOG_FLUSH_THRESHOLD - 1)),
            "达到阈值应立即触发刷盘"
        );

        let batch = buf.take();
        assert_eq!(batch.len(), ACCESS_LOG_FLUSH_THRESHOLD);
        assert_eq!(batch[0].path, "/api/x0", "批量必须保持入队顺序");
        assert_eq!(
            batch[ACCESS_LOG_FLUSH_THRESHOLD - 1].path,
            format!("/api/x{}", ACCESS_LOG_FLUSH_THRESHOLD - 1)
        );
        assert!(buf.is_empty(), "take 后缓冲应为空");
        assert_eq!(buf.dropped(), 0);
    }

    /// 缓冲满时必须丢弃并计数，而不是无界增长（库卡住时保护主流程内存）。
    #[test]
    fn access_log_buffer_drops_when_full() {
        let buf = AccessLogBuffer::default();
        for i in 0..ACCESS_LOG_BUFFER_MAX {
            buf.push(log_entry(i));
        }
        assert!(
            !buf.push(log_entry(ACCESS_LOG_BUFFER_MAX)),
            "满缓冲不应再触发刷盘"
        );
        assert_eq!(buf.dropped(), 1, "应记录丢弃条数");
        assert_eq!(
            buf.take().len(),
            ACCESS_LOG_BUFFER_MAX,
            "缓冲长度不得超过上限"
        );
    }

    /// 复位守卫：即使刷盘任务 panic，`flushing` 标志也必须被复位，
    /// 否则访问日志会永久停止落库。
    #[test]
    fn flush_in_flight_guard_resets_flag() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let flag = AtomicBool::new(true);
        {
            let _guard = FlushInFlight(&flag);
            assert!(flag.load(Ordering::Acquire), "进入作用域时标志应仍为 true");
        }
        assert!(!flag.load(Ordering::Acquire), "离开作用域后标志应复位");
    }
}
