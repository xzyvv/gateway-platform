//! SQLite 存储后端（WAL 模式）实现细节。
//!
//! 拆分为：连接初始化 / 只读连接池 / WAL 维护 / retention 清理 /
//! 历史查询与聚合 / 批量读写与容量淘汰。

mod db_init;
mod history;
mod limit;
mod read_pool;
mod retention;
mod sqlite_storage;
mod wal;

#[cfg(test)]
mod consistency_tests;

pub use sqlite_storage::SqliteStorage;
