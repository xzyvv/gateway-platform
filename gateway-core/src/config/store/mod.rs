//! 配置域的运行时持久化服务（SQLite）
//!
//! 与「配置模型」刻意区分：本子模块只存放落盘 SQLite 的运行时存储
//! （配置版本快照、上行测试历史、限流规则、API 密钥、计划任务、数据质量规则），
//! 配置模型（`GatewayConfig` 及各 `*Config`
//! 结构体）仍留在 `config/` 顶层。对外仍经 `config` 根重新导出，
//! 路径 `gateway_core::config::ConfigVersionStore` 等保持不变。

mod api_keys;
mod quality_rules;
mod rate_rules;
mod scheduled_tasks;
mod uplink_test_history;
mod version;

pub use api_keys::{ApiKeyRecord, ApiKeyStore};
pub use quality_rules::{QualityRule, QualityRuleStore};
pub use rate_rules::{ApiAccessLog, RateLimitStats, RateRule, RateRuleStore};
pub use scheduled_tasks::{ScheduledTask, ScheduledTaskStore, TaskRunStatus, TaskType};
pub use uplink_test_history::{NewUplinkTestHistory, UplinkTestHistory, UplinkTestHistoryStore};
pub use version::{ConfigVersion, ConfigVersionStore};
