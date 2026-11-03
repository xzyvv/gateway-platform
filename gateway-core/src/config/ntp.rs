//! NTP 配置（§3.4）

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, ts_rs::TS)]
#[ts(export)]
pub struct NtpConfig {
    #[serde(default = "default_ntp_server")]
    pub server: String,

    /// 同步间隔（秒）
    #[serde(default = "default_sync_interval")]
    #[ts(type = "number")]
    pub sync_interval: u64,
}

impl Default for NtpConfig {
    fn default() -> Self {
        Self {
            server: "ntp.aliyun.com".to_string(),
            sync_interval: 3600,
        }
    }
}

fn default_ntp_server() -> String {
    "ntp.aliyun.com".to_string()
}

fn default_sync_interval() -> u64 {
    3600
}

pub(crate) fn default_ntp() -> Option<NtpConfig> {
    Some(NtpConfig::default())
}
