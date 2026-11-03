//! 队列内核层：命令循环、容量策略、记录模型与加密密钥。
//!
//! 本层为存储无关的“内核”，不得依赖具体存储后端（SQL / SQLite 等）；
//! 仅通过 [`crate::queue::storage::QueueStorage`] trait 与后端交互。

mod item;
mod key;
pub(crate) mod limit;
mod task;

pub use item::QueuedItem;

pub(crate) use key::ensure_key;
pub(crate) use task::{queue_task, QueueCommand};
