//! Web API 设备端点：设备发现、IO 控制、OTA、NTP、运维扩展
//!
//! 数据模型集中在 `model`，handler 按业务子域拆分在 `handlers/` 下。

mod handlers;
mod routes;

pub(crate) use handlers::*;
pub(crate) use routes::devices_routes;

mod model;
