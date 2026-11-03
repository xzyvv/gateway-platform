//! 4G 蜂窝网络配置
//!
//! 配置类型归属 `config` 层，避免 `config` 反向依赖 `cellular`。
//! `cellular` 模块从此处 re-export `CellularConfig`。
//!
//! Bug 130: 字段顺序保持序列化兼容性——新增字段必须追加在末尾。
//! 启用 `failover_enabled` 时，以太网断开 → 自动启用 4G；4G 故障 → 自动回退。
//! 注意：以太网与 4G 不应同时作为主链路（route table 冲突由系统层处理）。

#[derive(
    Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema, ts_rs::TS,
)]
#[ts(export)]
pub struct CellularConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_at_port")]
    pub at_port: String,
    #[serde(default = "default_data_port")]
    pub data_port: String,
    #[serde(default = "default_baud")]
    pub baud_rate: u32,
    #[serde(default = "default_apn")]
    pub apn: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub pin: Option<String>,
    /// 拨号细则超时的总上限（兜底）。细分阶段超时（见下方各 `*_timeout_secs` 字段）
    /// 均不得超过该值；置 0 表示不设总上限。
    #[serde(default = "default_dial_timeout")]
    #[ts(type = "number")]
    pub dial_timeout_secs: u64,
    #[serde(default = "default_heartbeat")]
    #[ts(type = "number")]
    pub heartbeat_secs: u64,
    #[serde(default = "default_failover")]
    pub failover_enabled: bool,
    #[serde(default = "default_eth_iface")]
    pub eth_iface: String,
    #[serde(default = "default_ppp_iface")]
    pub ppp_iface: String,
    /// 单条 AT 指令的响应超时（秒）。
    #[serde(default = "default_at_cmd_timeout")]
    #[ts(type = "number")]
    pub at_cmd_timeout_secs: u64,
    /// 网络注册（`AT+COPS?` 轮询）的总超时（秒）。
    #[serde(default = "default_register_timeout")]
    #[ts(type = "number")]
    pub register_timeout_secs: u64,
    /// `ATD*99#` 拨号命令等待 CONNECT 的超时（秒）。
    #[serde(default = "default_connect_cmd_timeout")]
    #[ts(type = "number")]
    pub connect_cmd_timeout_secs: u64,
    /// pppd 启动后等待 PPP 接口拿到 IP 的超时（秒）；期间若 pppd 提前退出则立即失败。
    #[serde(default = "default_ppp_bringup_timeout")]
    #[ts(type = "number")]
    pub ppp_bringup_timeout_secs: u64,
    /// 停止 pppd 时的优雅退出等待（SIGTERM → 超时后 SIGKILL）时间（秒）。
    #[serde(default = "default_ppp_stop_timeout")]
    #[ts(type = "number")]
    pub ppp_stop_timeout_secs: u64,
    /// 拨号失败后的重试退避间隔（秒）。
    #[serde(default = "default_retry_backoff")]
    #[ts(type = "number")]
    pub retry_backoff_secs: u64,
}

impl Default for CellularConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            at_port: default_at_port(),
            data_port: default_data_port(),
            baud_rate: default_baud(),
            apn: default_apn(),
            username: String::new(),
            password: String::new(),
            pin: None,
            dial_timeout_secs: default_dial_timeout(),
            heartbeat_secs: default_heartbeat(),
            failover_enabled: default_failover(),
            eth_iface: default_eth_iface(),
            ppp_iface: default_ppp_iface(),
            at_cmd_timeout_secs: default_at_cmd_timeout(),
            register_timeout_secs: default_register_timeout(),
            connect_cmd_timeout_secs: default_connect_cmd_timeout(),
            ppp_bringup_timeout_secs: default_ppp_bringup_timeout(),
            ppp_stop_timeout_secs: default_ppp_stop_timeout(),
            retry_backoff_secs: default_retry_backoff(),
        }
    }
}

fn default_at_port() -> String {
    "/dev/ttyUSB0".to_string()
}
fn default_data_port() -> String {
    "/dev/ttyUSB2".to_string()
}
fn default_baud() -> u32 {
    115200
}
fn default_apn() -> String {
    "cmnet".to_string()
}
fn default_dial_timeout() -> u64 {
    60
}
fn default_heartbeat() -> u64 {
    30
}
fn default_failover() -> bool {
    true
}
fn default_eth_iface() -> String {
    "eth0".to_string()
}
fn default_ppp_iface() -> String {
    "ppp0".to_string()
}
fn default_at_cmd_timeout() -> u64 {
    2
}
fn default_register_timeout() -> u64 {
    60
}
fn default_connect_cmd_timeout() -> u64 {
    30
}
fn default_ppp_bringup_timeout() -> u64 {
    45
}
fn default_ppp_stop_timeout() -> u64 {
    2
}
fn default_retry_backoff() -> u64 {
    10
}

#[cfg(test)]
mod tests;
