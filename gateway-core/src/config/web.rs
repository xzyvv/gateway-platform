//! Web 服务配置（API 鉴权）

use serde::{Deserialize, Serialize};

/// Web 服务配置（§11.4 API 鉴权）
///
/// 工业现场不能无鉴权暴露控制接口（IO 控制/配置热重载/OTA/模板覆盖），
/// 必须在配置中显式设置 api_token 才允许启动 Web 服务。
#[derive(Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, ts_rs::TS)]
#[ts(export)]
pub struct WebConfig {
    /// API Token（用于保护所有 POST 写接口）
    /// - 通过 `Authorization: Bearer <token>` 或 `X-API-Token: <token>` 传入
    /// - 未配置时拒绝启动 Web 服务
    #[serde(default)]
    pub api_token: String,

    /// 登录用户名（默认 admin）
    #[serde(default = "default_web_username")]
    pub username: String,

    /// PBKDF2-HMAC-SHA256 密码哈希（格式：pbkdf2$<iter>$<salt_b64>$<hash_b64>）
    /// 空字符串表示使用默认凭据 admin/admin（启动时告警）
    #[serde(default)]
    pub password_hash: String,

    /// Web 服务监听地址（如 "0.0.0.0:8080"）
    #[serde(default = "default_listen_addr")]
    pub listen_addr: String,

    /// 是否允许 GET 类只读接口匿名访问（Bug 172：默认收敛为 false，需显式开启）
    #[serde(default = "default_allow_anon_read")]
    pub allow_anon_read: bool,

    /// 实时数据广播通道容量（默认 1024）
    ///
    /// 容量过小 → 客户端消费慢时 Lagged 丢数据；
    /// 容量过大 → 占用更多内存。1024 条对绝大多数场景足够。
    #[serde(default = "default_realtime_buffer")]
    pub realtime_buffer: usize,

    /// OTA 固件签名密钥（HMAC-SHA256，base64 编码）
    /// None=不验签（向后兼容 CRC32-only），Some=强制验签
    #[serde(default)]
    pub ota_signing_key: Option<String>,

    /// API 速率限制：每秒最大请求数，None=不限
    #[serde(default)]
    pub rate_limit_per_sec: Option<u32>,

    /// CORS 允许的来源列表（如 ["https://gateway.example.com", "http://localhost:5173"]）
    /// - 空列表 = 拒绝所有跨域请求（生产推荐）
    /// - 默认 None = 开发模式允许所有（向后兼容）
    #[serde(default)]
    pub cors_origins: Option<Vec<String>>,

    /// 是否允许使用默认凭据 admin/admin（password_hash 为空时）
    /// - true（默认）：开发/调试方便，工厂直连场景使用 admin/admin，启动时告警提醒改密
    /// - false：生产环境强制要求 password_hash，启动时若为空则拒绝启动 Web 服务
    #[serde(default = "default_allow_default_token")]
    pub allow_default_token: bool,

    /// 用户列表（多用户**元数据**，与上方单用户 username/password_hash 并存）
    ///
    /// #10 说明：当前版本**仅实现用户元数据 CRUD**（创建/列表/更新/删除/启停），
    /// 不实现密码哈希与登录流程；Web 登录始终只认上方单账号的
    /// username + password_hash。因此此处新增的用户**无法登录**，
    /// 该字段保留供后续多用户版本使用。用户管理页已就此显著标注。
    #[serde(default)]
    pub users: Vec<WebUser>,

    /// Webhook 连通性测试（`POST /api/webhook/test`）允许访问的目标主机白名单。
    ///
    /// 支持 `host` 或 `host:port`（大小写不敏感，如 `["10.1.2.3", "erp.local:8080"]`）。
    /// 空列表 = 不限制目标（保持厂区内网自测的既有用法）；非空时只放行清单内的主机，
    /// 把该接口的 SSRF 面收敛到运维显式登记的目标。
    #[serde(default)]
    pub webhook_test_allow_hosts: Vec<String>,

    /// Webhook 连通性测试是否禁止私网 / 回环 / 链路本地目标（默认 false）。
    ///
    /// 该接口的正当用途是打厂区内网 webhook，故默认不封私网；生产环境若只允许测试
    /// 公网目标，可置 true（此时回环、RFC1918 私网、CGNAT、IPv6 ULA 一律拒绝）。
    #[serde(default)]
    pub webhook_test_block_private: bool,

    /// 是否在协议工具（报文回放）中注入预置演示会话（默认 false）
    ///
    /// 预置会话 `s1`/`s2` 是演示数据，不是真实回放记录；混进生产列表会让现场
    /// 误以为存在历史会话。仅演示 / 培训环境显式置 true。
    #[serde(default)]
    pub demo_replay_sessions: bool,
}

/// `api_token` / `password_hash` / `ota_signing_key` 不得随 `{:?}` 进入日志。
impl std::fmt::Debug for WebConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        crate::config::fmt_redacted_debug(self, f)
    }
}

impl WebConfig {
    /// 汇总当前配置里"对生产不安全"的默认项
    ///
    /// 这些默认值面向「笔记本直连调试」场景（现场插网线即可访问），
    /// 但网关常被接入车间网，需要显式提醒运维收敛。
    /// 启动告警（bootstrap）与前端 whoami 提示共用本判定，避免两处口径漂移。
    /// 返回空 Vec 表示当前配置无明显风险项。
    pub fn security_warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        if self.password_hash.is_empty() && self.allow_default_token {
            warnings.push(
                "正在使用默认凭据 admin/admin，请立即修改密码（POST /api/auth/password）"
                    .to_string(),
            );
        }
        if self.allow_anon_read {
            warnings.push(
                "已开启匿名只读（web.allow_anon_read=true）：同网段任意主机无需 token 即可读取监控/配置数据"
                    .to_string(),
            );
        }
        if self.cors_origins.is_none() {
            warnings.push(
                "未限制跨域来源（web.cors_origins 未设置）：允许任意网站跨域访问，生产环境建议设为 []"
                    .to_string(),
            );
        }
        if self.binds_all_interfaces() {
            warnings.push(format!(
                "Web 服务监听 {}（所有网卡），同网段主机均可访问，建议限定为管理网段",
                self.listen_addr
            ));
        }
        warnings
    }

    /// Bug 172: 是否绑定到所有网卡（0.0.0.0 / [::]）。
    ///
    /// 绑定所有网卡意味着同网段任意主机都能访问控制接口；与「默认凭据」组合
    /// 即为必须 fail-closed 的硬性违规（见 [`Self::security_violations`]）。
    pub fn binds_all_interfaces(&self) -> bool {
        self.listen_addr.starts_with("0.0.0.0") || self.listen_addr.starts_with("[::]")
    }

    /// Bug 172: 汇总**必须阻断启动**的不安全组合（区别于 [`Self::security_warnings`] 的软告警）。
    ///
    /// 原实现只对「监听所有网卡 + 使用默认凭据 admin/admin」发告警后照常启动，
    /// 出厂即全网卡监听且可用默认密码登录控制接口。现改为 fail-closed：要么设置
    /// `web.password_hash`，要么把 `web.listen_addr` 收敛到 127.0.0.1；
    /// 调用方（bootstrap）在返回非空时应拒绝启动。返回空 Vec 表示无硬性违规。
    pub fn security_violations(&self) -> Vec<String> {
        let mut violations = Vec::new();
        if self.binds_all_interfaces() && self.password_hash.is_empty() {
            violations.push(format!(
                "Web 服务监听所有网卡({})且未设置 web.password_hash（默认凭据 admin/admin）：同网段任意主机可登录控制接口。请设置 web.password_hash 或把 web.listen_addr 收敛到 127.0.0.1",
                self.listen_addr
            ));
        }
        violations
    }
}

/// 角色枚举：与前端 [types/roles.ts] 的 admin/operator/viewer 一一对应
///
/// 语义（V1 诚实说明）：角色只是**登记信息 + 界面分组**，后端不做任何按角色的授权判定。
/// 登录始终校验 [`WebConfig::username`] / [`WebConfig::password_hash`]，通过后签发同一个
/// 主 api_token；真正的权限边界是「主 token（管理员等价）vs API 密钥 scope」，
/// 由 gateway-api 的 `web/middleware.rs` 强制执行。
///
/// 前端不再假定「当前用户恒为 admin」，而是按 `GET /api/auth/whoami` 返回的
/// `credential` / `scopes` 推导界面权限。
#[derive(
    Debug,
    Default,
    Clone,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    ts_rs::TS,
)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum UserRole {
    Admin,
    Operator,
    #[default]
    Viewer,
}

/// Web 用户元数据（V1：不含密码；改密走 POST /api/auth/password）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, ts_rs::TS)]
#[ts(export)]
pub struct WebUser {
    /// 唯一 ID（前端用 ulid/uuid；后端不校验格式，由前端保证）
    pub id: String,
    /// 登录用户名（唯一约束由 handler 层校验）
    pub username: String,
    /// 显示名（用户管理 UI 展示）
    pub display_name: String,
    /// 角色 ID（admin/operator/viewer）
    #[serde(default)]
    pub role_id: UserRole,
    /// 是否启用（false=禁止登录/隐藏 UI）
    #[serde(default = "default_user_enabled")]
    pub enabled: bool,
    /// 创建时间戳（毫秒）
    pub created_at: i64,
    /// 最近登录时间戳（毫秒，可选）
    #[serde(default)]
    pub last_login_at: Option<i64>,
}

// ===== 默认值函数 =====

pub fn default_allow_anon_read() -> bool {
    // Bug 172: 默认收敛为 false——匿名只读会让同网段任意主机无需 token 即可读取
    // 监控/配置数据；需要调试便利时由运维在 config.json 显式开启。
    false
}

pub fn default_listen_addr() -> String {
    // Bug 172: 默认收敛为回环地址——默认绑定所有网卡会把控制接口暴露给整段车间网；
    // 需要「插网线直连调试」时由运维显式设为 0.0.0.0:8080（并配合 password_hash）。
    "127.0.0.1:8080".to_string()
}

pub fn default_realtime_buffer() -> usize {
    1024
}

pub fn default_web_username() -> String {
    "admin".to_string()
}

pub fn default_allow_default_token() -> bool {
    // 默认 true 保持向后兼容；生产部署建议显式设为 false 强制使用 password_hash
    true
}

pub fn default_user_enabled() -> bool {
    true
}
