//! 访问控制域：API 限流规则、API 密钥、计划任务、访问日志
//!
//! - `handlers`：HTTP handler（校验 + 审计 + 调 store）
//! - `routes`：路由装配（读/写鉴权分层）
//! - `executor`：计划任务调度执行器（后台循环 + run-now 共用执行路径）

pub mod executor;
mod handlers;
mod routes;

pub(crate) use routes::access_routes;
