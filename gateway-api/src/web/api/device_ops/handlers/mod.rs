//! 设备运维 handler 聚合
//!
//! 路由：
//! - GET    /api/device-groups        -> list_groups
//! - POST   /api/device-groups        -> create_group
//! - PUT    /api/device-groups/{id}   -> update_group
//! - DELETE /api/device-groups/{id}   -> delete_group
//! - POST   /api/device-groups/{id}/devices        -> add_device_to_group
//! - DELETE /api/device-groups/{id}/devices/{did}  -> remove_device_from_group
//! - GET    /api/devices/assets       -> list_assets
//! - GET    /api/devices/{id}/asset   -> get_asset
//! - PUT    /api/devices/{id}/asset   -> update_asset
//! - GET    /api/devices/health       -> list_health
//! - GET    /api/devices/health/summary -> get_health_summary
//! - GET    /api/devices/{id}/health  -> get_device_health
//! - GET    /api/devices/shadows      -> list_shadows
//! - GET    /api/devices/{id}/shadow  -> get_device_shadow

pub mod assets;
pub mod groups;
pub mod health;
pub mod shadows;

pub(crate) use assets::*;
pub(crate) use groups::*;
pub(crate) use health::*;
pub(crate) use shadows::*;
