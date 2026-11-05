//! 队列内核层：命令循环、容量策略、记录模型与加密密钥。
//!
//! 本层为存储无关的“内核”，不得依赖具体存储后端（SQL / SQLite 等）；
//! 仅通过 [`crate::queue::storage::QueueStorage`] trait 与后端交互。

mod item;
// r15-KEY1：key 模块的核心逻辑（长度校验 / 安全落盘 / 权限收紧 / hex 编码）
// 无条件编译以便跨平台单测，其中一部分仅在 `encryption` feature 打开时
// 才会被生产路径调用 —— 默认构建下这些 helper 属于「预留 API」。
#[allow(dead_code)]
mod key;
pub(crate) mod limit;
mod task;

pub use item::QueuedItem;

pub(crate) use key::ensure_key;
pub(crate) use task::{queue_task, QueueCommand};
