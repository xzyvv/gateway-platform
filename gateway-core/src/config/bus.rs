//! 总线配置（串口/Modbus TCP）

use serde::{Deserialize, Serialize};
use validator::Validate;

use super::{
    validate_baud_rate, validate_data_bits, validate_dlt645_version, validate_modbus_mode,
    validate_opcua_auth, validate_opcua_security_mode, validate_opcua_security_policy,
    validate_parity, validate_protocol, validate_stop_bits, validate_tcp_addr,
    ReconnectBackoffConfig,
};

/// 总线配置（§3.2）
#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, Validate, schemars::JsonSchema, ts_rs::TS,
)]
#[ts(export)]
pub struct BusConfig {
    /// 串口设备名，如 "/dev/ttyS1"
    pub port_name: String,

    /// 波特率（必须是白名单中的标准值）
    #[serde(default = "default_baud")]
    #[validate(custom(function = "validate_baud_rate"))]
    pub baud_rate: u32,

    /// 校验位：none/even/odd
    #[serde(default = "default_parity")]
    #[validate(custom(function = "validate_parity"))]
    pub parity: String,

    /// 数据位（5/6/7/8，默认 8）
    #[serde(default = "default_data_bits")]
    #[validate(custom(function = "validate_data_bits"))]
    pub data_bits: u8,

    /// 停止位（1 或 2）
    #[serde(default = "default_stop_bits")]
    #[validate(custom(function = "validate_stop_bits"))]
    pub stop_bits: u8,

    /// 单次请求超时（ms，1~600000；0 会让每次请求立即超时）
    #[serde(default = "default_timeout")]
    #[validate(range(min = 1, max = 600000))]
    #[ts(type = "number")]
    pub timeout_ms: u64,

    /// 协议：modbus/dlt645/cjt188
    #[validate(custom(function = "validate_protocol"))]
    pub protocol: String,

    /// 失败后重试次数（0~100）
    #[serde(default = "default_retry_count")]
    #[validate(range(min = 0, max = 100))]
    pub retry_count: Option<u32>,

    /// 重试间隔（ms）
    #[serde(default = "default_retry_delay")]
    #[ts(type = "number")]
    pub retry_delay_ms: Option<u64>,

    /// RS485 方向控制引脚（DE），None=不控制
    #[serde(default)]
    pub de_pin: Option<u32>,

    /// Modbus 模式：rtu（默认）/ ascii
    #[serde(default = "default_modbus_mode")]
    #[validate(custom(function = "validate_modbus_mode"))]
    pub modbus_mode: String,

    /// DLT645 协议版本：1997（多表厂老款）/ 2007（默认，国标 GB/T 19882-63）。
    /// 1997 与 2007 在地址字节序、DI 编码、广播行为上有差异。
    /// 仅在 `protocol == "dlt645"` 时生效。
    /// **遗留字段**：新配置请优先使用统一变体字段 [`BusConfig::variant`]。
    #[serde(default = "default_dlt645_version")]
    #[validate(custom(function = "validate_dlt645_version"))]
    pub dlt645_version: String,

    /// 统一协议变体选择点（决策表 #24）。
    ///
    /// 各协议取值为：
    /// - modbus：rtu | ascii
    /// - dlt645：2007（默认）| 1997 | 1997-addr-be-plain | 1997-addr-le-plain
    ///   | 1997-addr-be-cipher | 1997-addr-le-cipher
    ///   （1997 厂家表按「地址字节序 × 数据是否 +0x33」选具体子变体）
    /// - cjt188：2004（CJ/T 188-2004 标准方言）| 2018（默认内建方言）
    /// - fins：tcp（默认）| udp
    /// - mc：3e（默认）| 4e | 1e
    /// - 其它协议：暂不限定取值
    ///
    /// 未设置（`None` / 空）时回退到遗留字段（modbus→`modbus_mode`，dlt645→`dlt645_version`），
    /// 见唯一解析点 [`BusConfig::effective_variant`]。
    #[serde(default)]
    pub variant: Option<String>,

    /// Modbus TCP 目标地址（host:port），设置后走 TCP 而非串口。
    /// OPC UA 协议下此处填写 Server 端点（`opc.tcp://host:port`）。
    #[serde(default)]
    #[validate(custom(function = "validate_tcp_addr"))]
    pub tcp_addr: Option<String>,

    /// OPC UA Client 认证配置（P0-3），仅 `protocol == "opcua"` 时生效。
    ///
    /// 缺省（`None`）等价于历史行为：匿名认证 + `SecurityPolicy::None` 明文端点。
    ///
    /// 注意：validator 0.18 未为 `Option<T>` 实现 `Validate`，此处不能加
    /// `#[validate(nested)]`；嵌套校验由 `GatewayConfig::validate_all` 手动展开
    /// （与 `devices` / `points[].aggregation` 同一模式）。
    #[serde(default)]
    pub opcua: Option<OpcUaClientConfig>,

    /// P5: TCP 并发事务数（默认 8，允许多 slave 同时读取；1~1024）
    #[serde(default)]
    #[validate(range(min = 1, max = 1024))]
    pub tcp_concurrent: Option<usize>,

    /// 总线 mpsc 通道容量（默认 128；1~65535）。
    /// 慢速总线（如 9600bps Modbus RTU）若调度器并发提交过多请求，
    /// 通道满后 caller 的 send().await 会阻塞。可按现场情况调大此值。
    #[serde(default)]
    #[validate(range(min = 1, max = 65535))]
    #[ts(type = "number")]
    pub channel_capacity: Option<usize>,

    /// IEC104 ASDU 公共地址（CA）字节数：1 或 2（默认 2，兼容公共地址 ≤ 255 的常规站）。
    /// 设为 1 用于公共地址仅 1 字节的站（决策表 #28）。
    /// 仅在 `protocol == "iec104"` 时生效。
    #[serde(default)]
    pub asdu_addr_len: Option<u8>,

    /// TCP 重连退避参数（默认序列 1/2/5/10/30 秒、无抖动，与历史硬编码一致）。
    #[serde(default = "default_bus_reconnect_backoff")]
    #[validate(nested)]
    pub reconnect_backoff: ReconnectBackoffConfig,
}

impl Default for BusConfig {
    fn default() -> Self {
        Self {
            port_name: "/dev/ttyS1".to_string(),
            baud_rate: 9600,
            parity: "none".to_string(),
            data_bits: 8,
            stop_bits: 1,
            timeout_ms: 500,
            protocol: "modbus".to_string(),
            retry_count: Some(2),
            retry_delay_ms: Some(200),
            de_pin: None,
            modbus_mode: "rtu".to_string(),
            dlt645_version: "2007".to_string(),
            variant: None,
            tcp_addr: None,
            opcua: None,
            tcp_concurrent: None,
            channel_capacity: None,
            asdu_addr_len: None,
            reconnect_backoff: default_bus_reconnect_backoff(),
        }
    }
}

impl BusConfig {
    /// 解析当前总线生效的协议变体（决策表 #24 唯一解析点）。
    ///
    /// 优先级：显式 [`BusConfig::variant`]（非空）> 遗留字段（modbus→`modbus_mode`、
    /// dlt645→`dlt645_version`）> `None`。
    ///
    /// 运行时代码一律通过本方法读取变体，避免各处直接读遗留字段导致状态分歧。
    pub fn effective_variant(&self) -> Option<String> {
        if let Some(v) = self.variant.as_deref() {
            let t = v.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
        match self.protocol.to_ascii_lowercase().as_str() {
            "modbus" => Some(self.modbus_mode.clone()),
            "dlt645" => Some(self.dlt645_version.clone()),
            _ => None,
        }
    }
}

/// OPC UA Client 认证配置（P0-3）。
///
/// 仅在 `BusConfig.protocol == "opcua"` 时生效；`BusConfig.opcua` 为 `None` 时保持
/// 历史行为（匿名 + 明文 None 端点），存量配置无需迁移。
///
/// 认证方式与安全策略/模式相互独立：
/// - `auth = anonymous`：`IdentityToken::Anonymous`
/// - `auth = username`：`IdentityToken::UserName`，必须同时给出 `username` / `password`
/// - `auth = certificate`：`IdentityToken::X509`，必须给出 `client_cert_path` / `client_key_path`
///
/// 安全策略 `none` 只能配 `security_mode = none`；非 `none` 策略必须使用
/// `sign` / `sign_and_encrypt`，且客户端需要自带证书/私钥（见 `semantic_error`）。
#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, Validate, schemars::JsonSchema, ts_rs::TS,
)]
#[ts(export)]
pub struct OpcUaClientConfig {
    /// 认证方式：anonymous（默认）/ username / certificate
    #[serde(default = "default_opcua_auth")]
    #[validate(custom(function = "validate_opcua_auth"))]
    pub auth: String,

    /// 用户名（`auth == "username"` 时必填）
    #[serde(default)]
    pub username: Option<String>,

    /// 密码（`auth == "username"` 时必填）；支持 `enc:` 密文落盘
    #[serde(default)]
    pub password: Option<String>,

    /// 安全策略：none（默认）/ basic256sha256
    #[serde(default = "default_opcua_security_policy")]
    #[validate(custom(function = "validate_opcua_security_policy"))]
    pub security_policy: String,

    /// 消息安全模式：none（默认）/ sign / sign_and_encrypt
    #[serde(default = "default_opcua_security_mode")]
    #[validate(custom(function = "validate_opcua_security_mode"))]
    pub security_mode: String,

    /// 客户端证书路径（PEM/DER，`auth == "certificate"` 或非 none 安全策略时使用）
    #[serde(default)]
    pub client_cert_path: Option<String>,

    /// 客户端私钥路径（PEM/DER，与 `client_cert_path` 配对）
    #[serde(default)]
    pub client_key_path: Option<String>,

    /// 是否自动信任服务器证书（默认 true，保持历史匿名连接不断链）。
    /// 生产环境建议关闭并预置信任证书，避免中间人替换 Server 证书。
    #[serde(default = "default_true")]
    pub trust_server_certs: bool,
}

impl Default for OpcUaClientConfig {
    fn default() -> Self {
        Self {
            auth: default_opcua_auth(),
            username: None,
            password: None,
            security_policy: default_opcua_security_policy(),
            security_mode: default_opcua_security_mode(),
            client_cert_path: None,
            client_key_path: None,
            trust_server_certs: default_true(),
        }
    }
}

impl OpcUaClientConfig {
    /// 跨字段语义校验（validator 单字段自定义函数看不到其它字段，故单独提供）。
    ///
    /// 返回 `Some(错误描述)` 表示配置自相矛盾；`None` 表示通过。
    pub fn semantic_error(&self) -> Option<String> {
        let policy = self.security_policy.to_ascii_lowercase();
        let mode = self.security_mode.to_ascii_lowercase();
        if policy == "none" && mode != "none" {
            return Some(
                "security_policy = none 时 security_mode 必须为 none（无加密策略不支持签名/加密）"
                    .into(),
            );
        }
        if policy != "none" && mode == "none" {
            return Some(
                "非 none 的 security_policy 必须搭配 sign 或 sign_and_encrypt 的 security_mode"
                    .into(),
            );
        }
        // 空串 / 纯空白等同未填：用 match guard 表达「认证方式 + 缺字段」组合，
        // 避免嵌套 if 触发 clippy::collapsible_match。
        let blank = |v: &Option<String>| v.as_deref().unwrap_or("").trim().is_empty();
        match self.auth.to_ascii_lowercase().as_str() {
            "username" if blank(&self.username) || blank(&self.password) => {
                return Some("auth = username 时必须同时填写 username 与 password".into());
            }
            "certificate" if blank(&self.client_cert_path) || blank(&self.client_key_path) => {
                return Some(
                    "auth = certificate 时必须同时填写 client_cert_path 与 client_key_path".into(),
                );
            }
            _ => {}
        }
        if policy != "none"
            && (self
                .client_cert_path
                .as_deref()
                .unwrap_or("")
                .trim()
                .is_empty()
                || self
                    .client_key_path
                    .as_deref()
                    .unwrap_or("")
                    .trim()
                    .is_empty())
        {
            return Some(
                "非 none 的 security_policy 需要客户端证书：请填写 client_cert_path 与 client_key_path"
                    .into(),
            );
        }
        None
    }
}

// ===== 默认值函数（与 BusConfig 同模块，供 serde default 引用）=====
fn default_baud() -> u32 {
    9600
}
fn default_parity() -> String {
    "none".to_string()
}
fn default_data_bits() -> u8 {
    8
}
fn default_stop_bits() -> u8 {
    1
}
fn default_timeout() -> u64 {
    500
}
fn default_retry_count() -> Option<u32> {
    Some(2)
}
fn default_retry_delay() -> Option<u64> {
    Some(200)
}
fn default_modbus_mode() -> String {
    "rtu".to_string()
}
fn default_dlt645_version() -> String {
    "2007".to_string()
}
fn default_opcua_auth() -> String {
    "anonymous".to_string()
}
fn default_opcua_security_policy() -> String {
    "none".to_string()
}
fn default_opcua_security_mode() -> String {
    "none".to_string()
}
fn default_true() -> bool {
    true
}

/// Modbus TCP 重连退避默认值：精确复刻历史硬编码序列 1/2/5/10/30 秒、无抖动。
fn default_bus_reconnect_backoff() -> ReconnectBackoffConfig {
    ReconnectBackoffConfig {
        seconds: vec![1, 2, 5, 10, 30],
        jitter_ratio: 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 决策表 #24：显式 variant 优先于遗留字段。
    #[test]
    fn effective_variant_explicit_wins_over_legacy() {
        let b = BusConfig {
            protocol: "modbus".into(),
            modbus_mode: "rtu".into(),
            variant: Some("ascii".into()),
            ..Default::default()
        };
        assert_eq!(b.effective_variant().as_deref(), Some("ascii"));
    }

    /// 遗留 modbus_mode 在 variant 缺省时仍生效（向后兼容）。
    #[test]
    fn effective_variant_falls_back_to_modbus_mode() {
        let b = BusConfig {
            protocol: "modbus".into(),
            modbus_mode: "ascii".into(),
            variant: None,
            ..Default::default()
        };
        assert_eq!(b.effective_variant().as_deref(), Some("ascii"));
    }

    /// 遗留 dlt645_version 在 variant 缺省时仍生效。
    #[test]
    fn effective_variant_falls_back_to_dlt645_version() {
        let b = BusConfig {
            protocol: "dlt645".into(),
            dlt645_version: "1997".into(),
            variant: None,
            ..Default::default()
        };
        assert_eq!(b.effective_variant().as_deref(), Some("1997"));
    }

    /// 空白 variant 视为未设置，回退遗留字段（避免空串绕过）。
    #[test]
    fn effective_variant_blank_falls_back() {
        let b = BusConfig {
            protocol: "modbus".into(),
            modbus_mode: "rtu".into(),
            variant: Some("   ".into()),
            ..Default::default()
        };
        assert_eq!(b.effective_variant().as_deref(), Some("rtu"));
    }

    /// 无遗留字段的协议（cjt188）：variant 缺省为 None，显式设置时透传。
    #[test]
    fn effective_variant_cjt188_only_explicit() {
        let none = BusConfig {
            protocol: "cjt188".into(),
            variant: None,
            ..Default::default()
        };
        assert_eq!(none.effective_variant(), None);

        let explicit = BusConfig {
            protocol: "cjt188".into(),
            variant: Some("2004".into()),
            ..Default::default()
        };
        assert_eq!(explicit.effective_variant().as_deref(), Some("2004"));
    }

    /// serde 兼容：缺省 variant 字段可反序列化（旧配置）。
    #[test]
    fn variant_is_serde_default() {
        let b: BusConfig =
            serde_json::from_str(r#"{"port_name":"/dev/ttyS1","protocol":"modbus"}"#).unwrap();
        assert_eq!(b.variant, None);
        assert_eq!(b.asdu_addr_len, None);
    }

    // ===== P0-3：OPC UA Client 认证配置 =====

    /// 缺省 opcua 块不出现（存量配置零迁移）；默认值等价历史行为：匿名 + 明文 + 信任证书。
    #[test]
    fn opcua_block_defaults_to_disabled_and_anonymous() {
        let b: BusConfig =
            serde_json::from_str(r#"{"port_name":"opcua_1","protocol":"opcua"}"#).unwrap();
        assert!(b.opcua.is_none(), "缺省不得自动物化 opcua 块");

        let cfg = OpcUaClientConfig::default();
        assert_eq!(cfg.auth, "anonymous");
        assert_eq!(cfg.security_policy, "none");
        assert_eq!(cfg.security_mode, "none");
        assert!(cfg.trust_server_certs);
        assert!(cfg.semantic_error().is_none());
    }

    /// 安全策略与模式必须配套：none 策略不能签名/加密，反之亦然。
    #[test]
    fn opcua_semantic_error_policy_mode_mismatch() {
        let plain = OpcUaClientConfig {
            security_mode: "sign".into(),
            ..Default::default()
        };
        assert!(plain.semantic_error().is_some());

        let secured = OpcUaClientConfig {
            security_policy: "basic256sha256".into(),
            security_mode: "none".into(),
            client_cert_path: Some("/etc/opcua/client.crt".into()),
            client_key_path: Some("/etc/opcua/client.key".into()),
            ..Default::default()
        };
        assert!(secured.semantic_error().is_some());
    }

    /// 非 none 策略必须带客户端证书/私钥（否则连不上加密端点）。
    #[test]
    fn opcua_semantic_error_secure_policy_without_cert() {
        let cfg = OpcUaClientConfig {
            security_policy: "basic256sha256".into(),
            security_mode: "sign_and_encrypt".into(),
            ..Default::default()
        };
        assert!(cfg
            .semantic_error()
            .expect("缺少证书必须报错")
            .contains("client_cert_path"));
    }

    /// 用户名认证必须成对提供 username / password，空串等同未填。
    #[test]
    fn opcua_semantic_error_username_requires_pair() {
        let cfg = OpcUaClientConfig {
            auth: "username".into(),
            username: Some("operator".into()),
            password: Some("   ".into()),
            ..Default::default()
        };
        assert!(cfg.semantic_error().is_some());

        let ok = OpcUaClientConfig {
            auth: "username".into(),
            username: Some("operator".into()),
            password: Some("secret".into()),
            ..Default::default()
        };
        assert!(ok.semantic_error().is_none());
    }

    /// 证书认证必须成对提供证书与私钥；配齐后通过语义校验。
    #[test]
    fn opcua_semantic_error_certificate_requires_pair() {
        let missing = OpcUaClientConfig {
            auth: "certificate".into(),
            client_cert_path: Some("/etc/opcua/client.crt".into()),
            ..Default::default()
        };
        assert!(missing.semantic_error().is_some());

        let ok = OpcUaClientConfig {
            auth: "certificate".into(),
            client_cert_path: Some("/etc/opcua/client.crt".into()),
            client_key_path: Some("/etc/opcua/client.key".into()),
            ..Default::default()
        };
        assert!(ok.semantic_error().is_none());
    }
}
