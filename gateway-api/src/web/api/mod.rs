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
//! **硬约束：一个 URL 前缀只能有一个 `routes.rs` 注册点。**
//! 历史上同一前缀被多个子域各管一段（`/api/devices*` 由 config/device_ops/devices 三家分治），
//! 改一个端点要在三个文件里翻、grep 路径会命中多份，认知负担高且容易改漏。
//! 现已按「前缀」收口：路由入口看下表，实现文件由 handler 名引用。
//!
//! | 前缀 | 唯一注册点（`web/api/<子域>/routes.rs`） | handler 分布 |
//! |---|---|---|
//! | `/api/devices*` | `devices` | `devices`(IO/拓扑/质量)、`device_ops`(健康/影子/资产/分组)、`config`(设备 CRUD) |
//! | `/api/points*`、`/api/point-tools*`、`/api/point-trash*` | `points` | `points`(即时读写)、`points_ops`(虚拟/偏移/克隆/替换)、`config`(点位 CRUD/回收站) |
//! | `/api/config*` | `config` | `config`(导入/重载/校验/版本)、`system`(整份读写的 handler) |
//! | `/api/diagnostics*` | `diagnostics` | `diagnostics`、`realtime`(日志流/exec) |
//! | `/api/io/*`、`/api/templates` | `devices` | `devices` |
//! | `/api/project/*`、`/api/buses/*` | `config` | `config` |
//! | 其余单前缀 | 同名子域 | 同子域 |
//!
//! 新增端点：挂到**该前缀**的注册点（不是实现所在子域），并在该 `routes.rs` 顶部
//! 路由清单里登记。`device_ops` / `points_ops` 已无 `routes.rs`，只是 handler 库。
//!
//! 鉴权**不在本层统一注入**：各注册点内部按子 Router 各自 `route_layer`
//! （写类 `require_auth` / 读类 `require_auth_if_anon_disabled`）。同一前缀的不同段
//! 可能落在不同策略，新增路由必须显式挑选一个，不要假设前缀已有统一保护。

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
// `device_ops` / `points_ops` 只提供 handler（无 routes.rs），路由入口在
// `devices` / `points`，故只需全量重导出 handler。
pub(crate) use points_ops::*;
pub(crate) use protocol_tools::protocol_tools_routes;
pub(crate) use quality::quality_routes;
pub(crate) use realtime::*;
pub(crate) use sim_consumer::sim_consumer_routes;
pub(crate) use system::*;
pub(crate) use uplink::uplink_routes;
