//! bootstrap 辅助函数（从原 helpers.rs 按启动阶段拆分）
//!
//! 子模块划分：
//! - collection：采集链（Step 2-7）—— 串口总线、采集器、点位展开、看门狗
//! - north：北向服务（Step 8-11 + 北向 Server 启动）—— OPC UA/Modbus/透传/转换层
//! - assemble：装配阶段（Step 12-13）+ 优雅停机 —— AppState 构造、Web/报警/透传启动、停机序列
//!
//! 所有 pub(crate) 项在此统一 re-export，bootstrap/mod.rs 中 `use helpers::*` 无需改动。

mod assemble;
mod collection;
mod north;
mod shutdown;

pub(crate) use assemble::*;
pub(crate) use collection::*;
pub(crate) use north::*;
pub(crate) use shutdown::*;
