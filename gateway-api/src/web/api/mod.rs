//! Web API 路由处理模块
//!
//! 本文件仅做各业务子域子模块的聚合与重导出。
//!
//! 各业务子域的 handler 实现下沉在 `<子域>/handlers/`，
//! 路由构建函数（`*_routes`）下沉在对应 `<子域>/routes.rs`，
//! 由各 `<子域>/mod.rs` 通过 `pub(crate) use routes::<fn>;` 重导出，
//! 再经本文件顶部的 `pub(crate) use <子域>::*;` 暴露给 `crate::web` 顶层装配。
//!
//! ## REST 命名空间地图（改端点先看这里，别全仓 grep）
//!
//! 同一 URL 前缀被多个子域各管一段，属历史演进结果。下表是**唯一权威索引**，
//! 新增端点时挂到语义所属的子域，并在该子域 `routes.rs` 顶部路由清单里登记。
//!
//! | 前缀 | 承担子域（`web/api/<子域>/routes.rs`） |
//! |---|---|
//! | `/api/devices*` | `config`：设备 CRUD、设备下点位；`device_ops`：健康/影子/资产/拓扑/质量/退避/控制；`devices`：IO、设备模板 |
//! | `/api/points*` | `config`：点位 CRUD、回收站；`points`：即时读写、批量读写；`points_ops`：虚拟点位、地址偏移、克隆、查找替换 |
//! | `/api/config*` | `system`：读取/写入/版本/回滚/diff；`config`：备份、导入、重载、校验 |
//! | `/api/templates` | `devices`：设备模板（原寄居 `history`，已迁回设备域） |
//! | `/api/quality*` | `quality`：数据质量规则 CRUD（落库并安装到处理管道） |
//!
//! 鉴权**不在本层统一注入**：各子域各自 `route_layer`，同一前缀的不同段可能落在
//! 不同策略（写类 `require_auth` / 读类 `require_auth_if_anon_disabled`）。
//! 新增路由必须显式挑选一个，不要假设前缀已有统一保护。

pub mod access;
mod audit;
mod auth;
mod config;
mod device_ops;
mod devices;
mod diagnostics;
mod history;
mod license;
/// 点位归属索引（按设备反查点位，替代 O(D×P) 线性扫描）
pub(crate) mod point_owner;
mod points;
mod points_ops;
mod protocol_tools;
mod quality;
mod realtime;
mod sim_consumer;
mod system;
mod uplink;

pub(crate) use access::*;
pub(crate) use audit::*;
pub(crate) use auth::*;
pub(crate) use config::*;
pub(crate) use device_ops::*;
pub(crate) use devices::*;
pub(crate) use license::*;
// 以下四个子域只在 `web` 顶层装配其路由构建函数，handler 仅供各自 routes.rs 内部使用，
// 故精确重导出 `*_routes`（而非 `*` 全量），避免 handler 在祖父级产生 unused_imports。
pub(crate) use diagnostics::diagnostics_routes;
pub(crate) use history::*;
pub(crate) use points::*;
pub(crate) use points_ops::points_ops_routes;
pub(crate) use protocol_tools::protocol_tools_routes;
pub(crate) use quality::quality_routes;
pub(crate) use realtime::*;
pub(crate) use sim_consumer::sim_consumer_routes;
pub(crate) use system::*;
pub(crate) use uplink::uplink_routes;
