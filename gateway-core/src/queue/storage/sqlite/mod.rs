//! SQLite 存储后端（WAL 模式）实现细节。
//!
//! 拆分为：连接初始化 / 只读连接池 / WAL 维护 / retention 清理 /
//! 行数上界淘汰 / 历史查询与聚合 / 批量读写与容量淘汰。

mod aggregated_trim;
mod db_init;
mod history;
mod limit;
mod read_pool;
mod retention;
mod sqlite_storage;
mod wal;

#[cfg(test)]
mod consistency_tests;
/// SQLCipher 加密落盘回归（文件内 `#![cfg(feature = "encryption")]`，
/// 默认构建下为空模块）。
#[cfg(test)]
mod encryption_tests;
/// 磁盘满 / WAL 写入失败的恢复语义（用 `PRAGMA max_page_count` 造 SQLITE_FULL）。
#[cfg(test)]
mod wal_full_tests;

pub use sqlite_storage::SqliteStorage;
