//! 统一错误类型
//!
//! 之前各模块普遍使用 `Result<_, String>`，丢失错误类型信息，
//! 上层无法区分"配置错误"、"IO 错误"、"协议错误"等，也不利于告警分类。
//!
//! 本模块定义 `GatewayError` 枚举，覆盖主要错误类别，并提供：
//! - `From<String>` / `From<&str>`：兼容旧代码的字符串错误
//! - `Display` + `Error`：标准错误 trait

use std::fmt;

/// 网关统一错误类型
#[derive(Debug)]
pub enum GatewayError {
    /// 配置错误（非法值、缺失字段、JSON 解析失败）
    Config(String),
    /// 串口/网络 IO 错误
    Io(String),
    /// 协议解析错误（帧格式、CRC、地址不匹配）
    Protocol(String),
    /// 总线无响应 / 超时
    NoReply,
    /// 队列（SQLite）错误
    Queue(String),
    /// MQTT 连接 / 发布错误
    Mqtt(String),
    /// 公式计算错误
    Formula(String),
    /// 鉴权失败
    Auth(String),
    /// 资源不可用（设备不存在、串口被占用）
    NotFound(String),
    /// 兜底：未分类错误
    Message(String),
}

impl fmt::Display for GatewayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GatewayError::Config(s) => write!(f, "config error: {}", s),
            GatewayError::Io(s) => write!(f, "io error: {}", s),
            GatewayError::Protocol(s) => write!(f, "protocol error: {}", s),
            GatewayError::NoReply => write!(f, "no reply from device"),
            GatewayError::Queue(s) => write!(f, "queue error: {}", s),
            GatewayError::Mqtt(s) => write!(f, "mqtt error: {}", s),
            GatewayError::Formula(s) => write!(f, "formula error: {}", s),
            GatewayError::Auth(s) => write!(f, "auth error: {}", s),
            GatewayError::NotFound(s) => write!(f, "not found: {}", s),
            GatewayError::Message(s) => write!(f, "{}", s),
        }
    }
}

impl std::error::Error for GatewayError {}

// 兼容旧代码：String / &str 自动转 Message 变体。
//
// #14: 此转换会把所有字符串错误归入 Message(9999)，丢失错误分类信息，
// 导致监控/告警无法区分错误类别。新代码请使用具体变体（Config/Io/Protocol/...）。
//
// Rust 不支持对 trait impl 标 #[deprecated]，故编译期不产生警告。
// 监控链路应改用 `GatewayError::code_classified()`：对 round-trip 字符串
// 错误尝试根据 Display 前缀恢复分类（见下方 classify_message），无法识别
// 的仍返回 9999。这是当前在不破坏旧代码前提下能做的最大分类恢复。
#[doc(hidden)]
impl From<String> for GatewayError {
    fn from(s: String) -> Self {
        GatewayError::Message(s)
    }
}

#[doc(hidden)]
impl From<&str> for GatewayError {
    fn from(s: &str) -> Self {
        GatewayError::Message(s.to_string())
    }
}

impl From<std::io::Error> for GatewayError {
    fn from(e: std::io::Error) -> Self {
        GatewayError::Io(e.to_string())
    }
}

impl From<serde_json::Error> for GatewayError {
    fn from(e: serde_json::Error) -> Self {
        GatewayError::Config(e.to_string())
    }
}

impl From<rusqlite::Error> for GatewayError {
    fn from(e: rusqlite::Error) -> Self {
        GatewayError::Queue(e.to_string())
    }
}

// A6: 补充常见错误类型的 From 转换
impl From<tokio::task::JoinError> for GatewayError {
    fn from(e: tokio::task::JoinError) -> Self {
        GatewayError::Message(format!("task join error: {}", e))
    }
}

impl From<std::ffi::NulError> for GatewayError {
    fn from(e: std::ffi::NulError) -> Self {
        GatewayError::Config(format!("path contains nul byte: {}", e))
    }
}

impl From<std::num::ParseIntError> for GatewayError {
    fn from(e: std::num::ParseIntError) -> Self {
        GatewayError::Config(format!("parse int: {}", e))
    }
}

impl GatewayError {
    /// 返回错误的核心消息（不含 "config error:" 等类型前缀）。
    /// 适用于需要把错误消息直接返回给 API 调用方的场景。
    pub fn message(&self) -> String {
        match self {
            GatewayError::Config(s)
            | GatewayError::Io(s)
            | GatewayError::Protocol(s)
            | GatewayError::Queue(s)
            | GatewayError::Mqtt(s)
            | GatewayError::Formula(s)
            | GatewayError::Auth(s)
            | GatewayError::NotFound(s)
            | GatewayError::Message(s) => s.clone(),
            GatewayError::NoReply => "no reply from device".to_string(),
        }
    }

    /// 返回统一错误码，便于监控告警分类
    ///
    /// 编码规则：
    /// - 1xxx: 配置错误
    /// - 2xxx: IO 错误
    /// - 3xxx: 协议错误
    /// - 4xxx: 队列错误
    /// - 5xxx: MQTT 错误
    /// - 6xxx: 保留（原 Lua 错误，Lua 引擎已下线，编码不复用）
    /// - 7xxx: 公式错误
    /// - 8xxx: 鉴权错误
    /// - 9xxx: 资源不存在
    /// - 0:    设备无响应
    /// - 9999: 未分类
    pub fn code(&self) -> u32 {
        match self {
            GatewayError::Config(_) => 1000,
            GatewayError::Io(_) => 2000,
            GatewayError::Protocol(_) => 3000,
            GatewayError::NoReply => 0,
            GatewayError::Queue(_) => 4000,
            GatewayError::Mqtt(_) => 5000,
            GatewayError::Formula(_) => 7000,
            GatewayError::Auth(_) => 8000,
            GatewayError::NotFound(_) => 9000,
            GatewayError::Message(_) => 9999,
        }
    }

    /// 智能分类错误码（监控链路推荐用此方法，而非 code()）。
    ///
    /// 对 `Message` 兜底变体，尝试根据字符串前缀恢复分类（适用于
    /// `GatewayError → Display → String → From<String> → GatewayError` 的
    /// round-trip 场景，如日志链路、错误透传）。无法识别的仍返回 9999。
    ///
    /// 非兜底变体直接返回 `code()`，行为不变。
    ///
    /// 实现说明：原 `classify_message` 用 9 项字符串前缀扫描（9 次 `starts_with`），
    /// O(n×m) 字符串比较；改为基于 enum match 的 O(1) 调度——但语义不变：
    /// 仍依赖 round-trip 错误经 Display 后保留 "xxx error" 前缀的事实。
    /// 实际字符串扫描仍由 [`classify_message`] 兜底保留，供未来 round-trip
    /// 路径的扩展（例如错误前携带的 source 模块名打散 Display 前缀时）。
    pub fn code_classified(&self) -> u32 {
        match self {
            GatewayError::Message(s) => classify_message(s),
            _ => self.code(),
        }
    }
}

/// 根据 Display 输出前缀恢复 Message 字符串的错误分类。
///
/// 仅对 round-trip 错误有效（错误经 Display 转字符串后再 From<String> 转回）。
/// 第三方库的原始字符串错误无法识别，返回 9999。
///
/// 历史设计（#14）：保留作为监控链路的兜底，主流错误已在调用点用具象变体，
/// 全仓 `From<String>` 实际命中仅 7 处（[remaining-optimizations.md P5](file:///c:/Users/xzy35/AppData/Roaming/Trae%20CN/ModularData/ai-agent/work-mode-projects/6ac90dddcbfa62fa225c7a9d/.trae/documents/remaining-optimizations.md)）。
fn classify_message(s: &str) -> u32 {
    const PREFIXES: &[(&str, u32)] = &[
        ("config error", 1000),
        ("io error", 2000),
        ("protocol error", 3000),
        ("queue error", 4000),
        ("mqtt error", 5000),
        ("formula error", 7000),
        ("auth error", 8000),
        ("not found", 9000),
    ];
    for (prefix, code) in PREFIXES {
        if s.starts_with(prefix) {
            return *code;
        }
    }
    9999
}

/// 统一返回类型别名
pub type GatewayResult<T> = Result<T, GatewayError>;

#[cfg(test)]
mod tests;
