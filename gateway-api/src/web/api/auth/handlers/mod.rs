//! 登录 / 改密 API 处理函数

use super::*;

/// POST /api/auth/login — 校验用户名密码，返回 api_token
///
/// S7: 按 username 维度做失败计数 + 指数锁定（5 次失败后锁定 60 秒），
/// 防止暴力破解默认 admin/admin 凭据。
pub(crate) async fn login(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    // S7: 失败计数检查（先于密码校验，避免浪费 CPU 在被锁定的账号上）
    check_login_rate_limit(&req.username)?;

    let cfg = state.config.load_full();
    let web = cfg.web.as_ref();

    let stored_hash = web.map(|w| w.password_hash.as_str()).unwrap_or("");
    let cfg_username = web.map(|w| w.username.as_str()).unwrap_or("admin");
    // Bug fix: api_token 缺失时 prepare_web_config 会自动生成并存入 AppState.web，
    // 但运行时 config.web 里仍是空串，login 直接读会 500 "api_token not configured"。
    // 这里回退到启动时生成的 token；热重载若写入了新 token（非空）仍优先使用。
    let mut api_token = web.map(|w| w.api_token.clone()).unwrap_or_default();
    if api_token.is_empty() {
        api_token = state.web.api_token.clone();
    }

    let allow_default = web.map(|w| w.allow_default_token).unwrap_or(true);
    let pw_ok = if stored_hash.is_empty() {
        if !allow_default {
            tracing::warn!("web login rejected: password_hash empty and allow_default_token=false");
            return Err(ApiError::unauthorized("default credentials disabled"));
        }
        constant_time_eq(req.password.as_bytes(), b"admin")
    } else {
        gateway_core::crypto::verify_password(&req.password, stored_hash)
    };

    let user_ok = constant_time_eq(req.username.as_bytes(), cfg_username.as_bytes());

    if !user_ok || !pw_ok {
        record_login_failure(&req.username);
        tracing::warn!("web login failed: user={}", req.username);
        return Err(ApiError::unauthorized("invalid credentials"));
    }
    if api_token.is_empty() {
        return Err(ApiError::internal("api_token not configured on server"));
    }

    clear_login_failures(&req.username);
    tracing::info!("web login success: user={}", req.username);
    Ok(Json(LoginResponse { token: api_token }))
}

// S7: 登录失败速率限制状态：username -> (失败次数, 首次失败时间)
static LOGIN_FAILURES: once_cell::sync::Lazy<
    parking_lot::Mutex<std::collections::HashMap<String, (u32, std::time::Instant)>>,
> = once_cell::sync::Lazy::new(|| parking_lot::Mutex::new(std::collections::HashMap::new()));
const MAX_LOGIN_FAILURES: u32 = 5;
const LOGIN_LOCKOUT_SECS: u64 = 60;
// #11: 限速表按"客户端任意提供的 username"建键。原先不设上限，攻击者可用随机
// 用户名把 HashMap 无限撑大（内存耗尽）。限制单 key 长度与表总量，超出时先清理
// 锁定期已过的条目，仍满则放弃记录（限速是尽力而为，绝不能拖垮网关）。
const MAX_USERNAME_LEN: usize = 64;
const MAX_RATE_BUCKETS: usize = 1024;

fn check_login_rate_limit(username: &str) -> Result<(), ApiError> {
    if username.len() > MAX_USERNAME_LEN {
        return Err(ApiError::bad_request("username too long"));
    }
    let guard = LOGIN_FAILURES.lock();
    if let Some(&(count, first_at)) = guard.get(username) {
        if count >= MAX_LOGIN_FAILURES {
            let elapsed = first_at.elapsed().as_secs();
            if elapsed < LOGIN_LOCKOUT_SECS {
                return Err(ApiError::bad_request(format!(
                    "too many login failures, try again in {}s",
                    LOGIN_LOCKOUT_SECS - elapsed
                )));
            }
        }
    }
    Ok(())
}

fn record_login_failure(username: &str) {
    if username.len() > MAX_USERNAME_LEN {
        return;
    }
    let mut guard = LOGIN_FAILURES.lock();
    // #11: 表满时先清理锁定期已过的条目；仍满则放弃记录（有界内存优先）
    if guard.len() >= MAX_RATE_BUCKETS {
        guard.retain(|_, (_, first_at)| first_at.elapsed().as_secs() < LOGIN_LOCKOUT_SECS);
        if guard.len() >= MAX_RATE_BUCKETS {
            return;
        }
    }
    let entry = guard
        .entry(username.to_string())
        .or_insert((0, std::time::Instant::now()));
    if entry.0 == 0 {
        entry.1 = std::time::Instant::now();
    }
    entry.0 = entry.0.saturating_add(1);
    // 锁定期满后自动重置
    if entry.0 > MAX_LOGIN_FAILURES && entry.1.elapsed().as_secs() > LOGIN_LOCKOUT_SECS {
        guard.remove(username);
    }
}

fn clear_login_failures(username: &str) {
    LOGIN_FAILURES.lock().remove(username);
}

/// POST /api/auth/password — 修改密码（需 token 鉴权）
pub(crate) async fn change_password(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ChangePasswordRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Bug 140: 密码长度双侧校验——下限 6，上限 64。
    if req.new_password.len() < 6 {
        return Err(ApiError::bad_request("password too short (min 6 chars)"));
    }
    if req.new_password.len() > 64 {
        return Err(ApiError::bad_request("password too long (max 64 chars)"));
    }

    let _guard = state.config.config_reload_lock.lock().await;
    let old = state.config.load_full();
    let mut cfg = old.as_ref().clone();

    // #11: 服务端校验当前密码。此前仅由前端"再登录一次"在客户端代偿，服务端只要
    // 拿到有效 token 就能直接改密——一旦 token 泄漏（无需知道原密码）攻击者即可永久
    // 接管账号。这里必须服务端验证 old_password。
    {
        let web = cfg
            .web
            .as_ref()
            .ok_or_else(|| ApiError::internal("web config not set"))?;
        let old_ok = if web.password_hash.is_empty() {
            // 仍是默认密码：仅在允许默认凭据时才认 admin
            web.allow_default_token && constant_time_eq(req.old_password.as_bytes(), b"admin")
        } else {
            gateway_core::crypto::verify_password(&req.old_password, &web.password_hash)
        };
        if !old_ok {
            tracing::warn!("web password change rejected: current password mismatch");
            return Err(ApiError::unauthorized("current password incorrect"));
        }
    }

    let hash = gateway_core::crypto::hash_password(&req.new_password);
    // #11: 改密成功后轮换 api_token——旧 token 立即失效，避免长期静态凭证泄漏后
    // 仍可继续使用，使"改密"这个止损动作失去意义。
    let new_token = gateway_core::crypto::generate_api_token();
    if let Some(web) = cfg.web.as_mut() {
        web.password_hash = hash;
        web.api_token = new_token;
    } else {
        return Err(ApiError::internal("web config not set"));
    }
    let _ = crate::web::config_apply::reload_then_save(&state, &old, &cfg).await?;
    // 改密会轮换 api_token（旧凭证即刻失效），属账号安全事件，必须留痕
    state.platform.audit.log(
        "auth.password_change",
        "gateway",
        "web password changed; api_token rotated",
        "api",
    );
    tracing::info!("web password changed; api_token rotated");
    Ok(Json(serde_json::json!({"status":"ok"})))
}

/// 恒定时间比较，防止时序攻击
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

///
/// GET /api/auth/whoami — 校验当前 token 并返回用户身份
///
/// 用于前端路由守卫：localStorage 残留 token 时，先 ping 这个端点验证有效性，
/// 失败则清 token 并跳 /login，避免每次 API 401 才发现 token 过期。
///
/// 走 require_auth 中间件（即使 allow_anon_read=true，whoami 也强制鉴权，
/// 因为它的语义就是"我是谁"，必须真实拿到 token 才能回答）。
///
/// 额外返回 `credential` / `scopes`：后端没有「用户 → 角色」映射
/// （`WebConfig.users` 只是元数据，登录始终校验 `web.username/password_hash`），
/// 因此前端不再虚构角色，而是按**凭证真实能力**推导界面权限：
/// 主 token 为管理员，API 密钥按 scope 区分读写。
pub(crate) async fn whoami(
    State(state): State<Arc<AppState>>,
    authed: Option<axum::extract::Extension<AuthedApiKey>>,
    headers: axum::http::HeaderMap,
) -> Result<Json<WhoAmIResponse>, ApiError> {
    use crate::web::middleware::{api_token, extract_token_from_headers, token_matches};
    // require_auth 已把已认证的 API 密钥放进 request extensions，这里直接读取。
    // 旧实现在此处再调一次 authenticate：同一请求会查库两次、调用计数也加两次。
    let (credential, scopes) = match authed {
        Some(axum::extract::Extension(AuthedApiKey(key))) => ("api_key".to_string(), key.scopes),
        // 无扩展 = 主 token（require_auth 只对 API 密钥写扩展）。
        // 兜底按 header 判定，保持 handler 被直接调用时的旧语义。
        None => {
            let is_primary = extract_token_from_headers(&headers)
                .map(|t| token_matches(&t, &api_token(&state)))
                .unwrap_or(false);
            if is_primary {
                ("primary".to_string(), vec!["*".to_string()])
            } else {
                ("unknown".to_string(), Vec::new())
            }
        }
    };

    let cfg = state.config.load_full();
    let web = cfg.web.as_ref();
    let username = web
        .map(|w| w.username.clone())
        .unwrap_or_else(|| "admin".to_string());
    Ok(Json(WhoAmIResponse {
        username,
        // 透出当前是否还在使用默认密码（hash 为空 = 仍是 admin/admin），
        // 前端可据此在登录后弹一次"请尽快修改密码"的提醒。
        using_default_password: web.map(|w| w.password_hash.is_empty()).unwrap_or(true),
        security_warnings: web.map(|w| w.security_warnings()).unwrap_or_default(),
        credential,
        scopes,
    }))
}
