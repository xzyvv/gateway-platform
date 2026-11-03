//! 数据质量规则 API
//!
//! 规则落库到 `QualityRuleStore`（SQLite），写操作成功后经
//! `crate::web::services::install_quality_rules` 重新安装处理管道快照，
//! 使 range / rate / nan 规则立即对后续采集数据生效。
//!
//! 路由：见 [`routes::quality_routes`]（GET/POST/PUT/DELETE /api/quality/rules）

mod handlers;
mod routes;

pub(crate) use routes::quality_routes;
