//! 上行测试历史存储
//!
//! 每次通过 `/api/uplink/test` 真实发送测试数据到 broker 时，
//! 自动记录一份历史：模板、渲染结果、变量、是否成功、延迟、错误信息。
//! 前端可在 UplinkTester 翻查历史，对比不同模板/参数的发送效果。
//! 存储在 SQLite 中（与 config_versions 同库，独立表），保留最近 200 条。

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::engine::error::GatewayResult;

use super::conn::SharedConn;

/// 上行测试历史记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UplinkTestHistory {
    pub id: i64,
    /// 创建时间戳（毫秒）
    pub timestamp: i64,
    /// 当时使用的模板
    pub template: String,
    /// 渲染后的 payload
    pub rendered: String,
    /// 输入变量 JSON
    pub vars_json: String,
    /// 是否真实发送到 broker
    pub send: bool,
    /// broker 是否确认接收
    pub delivered: bool,
    /// 发送耗时（毫秒）
    pub latency_ms: u64,
    /// 失败原因（成功时为 None）
    pub error: Option<String>,
}

/// `UplinkTestHistoryStore::save` 的入参集合：字段较多，
/// 用结构体承载以避免超长参数列表（clippy::too_many_arguments）。
pub struct NewUplinkTestHistory<'a> {
    /// 当时使用的模板
    pub template: &'a str,
    /// 渲染后的 payload
    pub rendered: &'a str,
    /// 输入变量 JSON
    pub vars_json: &'a str,
    /// 是否真实发送到 broker
    pub send: bool,
    /// broker 是否确认接收
    pub delivered: bool,
    /// 发送耗时（毫秒）
    pub latency_ms: u64,
    /// 失败原因（成功时为 None）
    pub error: Option<&'a str>,
}

/// 上行测试历史存储
pub struct UplinkTestHistoryStore {
    /// 长驻连接：避免每次记录/查询重开库
    conn: SharedConn,
}

impl UplinkTestHistoryStore {
    /// 创建历史存储并初始化表
    pub fn new(db_path: &str) -> GatewayResult<Self> {
        let conn = SharedConn::open(std::path::Path::new(db_path), "open test history db")?;
        // r39-VER1：本库文件由 uplink_test_history / config_versions / disk_history
        // 三处共享，各持独立连接并发写，WAL + busy_timeout 由 SharedConn::open 统一设置
        // （与 outbox C18 一致）。
        conn.get()
            .execute(
                "CREATE TABLE IF NOT EXISTS uplink_test_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                template TEXT NOT NULL,
                rendered TEXT NOT NULL,
                vars_json TEXT NOT NULL,
                send INTEGER NOT NULL,
                delivered INTEGER NOT NULL,
                latency_ms INTEGER NOT NULL,
                error TEXT
            )",
                [],
            )
            .map_err(|e| {
                crate::engine::error::GatewayError::Queue(format!(
                    "create test history table failed: {}",
                    e
                ))
            })?;
        Ok(Self { conn })
    }

    /// 保存一条测试历史。返回新记录 id。
    pub fn save(&self, input: NewUplinkTestHistory<'_>) -> GatewayResult<i64> {
        let NewUplinkTestHistory {
            template,
            rendered,
            vars_json,
            send,
            delivered,
            latency_ms,
            error,
        } = input;
        let timestamp = chrono::Utc::now().timestamp_millis();
        let conn = self.conn();
        conn.execute(
            "INSERT INTO uplink_test_history (timestamp, template, rendered, vars_json, send, delivered, latency_ms, error) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![timestamp, template, rendered, vars_json, send as i32, delivered as i32, latency_ms as i64, error],
        )
        .map_err(|e| crate::engine::error::GatewayError::Queue(format!("insert test history failed: {}", e)))?;
        let id = conn.last_insert_rowid();
        // 保留最近 200 条，防止无限增长
        // P2-d: 原实现用 `let _ =` 静默吞掉裁剪失败，测试历史表可能持续膨胀而不留痕。
        if let Err(e) = conn.execute(
            "DELETE FROM uplink_test_history WHERE id NOT IN (
                SELECT id FROM uplink_test_history ORDER BY id DESC LIMIT 200
            )",
            [],
        ) {
            tracing::warn!("uplink_test_history retention cleanup failed: {}", e);
        }
        Ok(id)
    }

    /// 列出最近的测试历史（按 id 降序，最新在前）
    pub fn list_recent(&self, limit: usize) -> GatewayResult<Vec<UplinkTestHistory>> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare("SELECT id, timestamp, template, rendered, vars_json, send, delivered, latency_ms, error FROM uplink_test_history ORDER BY id DESC LIMIT ?1")
            .map_err(|e| crate::engine::error::GatewayError::Queue(format!("prepare failed: {}", e)))?;
        let rows = stmt
            .query_map(params![limit as i64], |row| {
                Ok(UplinkTestHistory {
                    id: row.get(0)?,
                    timestamp: row.get(1)?,
                    template: row.get(2)?,
                    rendered: row.get(3)?,
                    vars_json: row.get(4)?,
                    send: row.get::<_, i32>(5)? != 0,
                    delivered: row.get::<_, i32>(6)? != 0,
                    latency_ms: row.get::<_, i64>(7)? as u64,
                    error: row.get::<_, Option<String>>(8)?,
                })
            })
            .map_err(|e| {
                crate::engine::error::GatewayError::Queue(format!("query failed: {}", e))
            })?;
        let mut items = Vec::new();
        for row in rows {
            items.push(row.map_err(|e| {
                crate::engine::error::GatewayError::Queue(format!("row failed: {}", e))
            })?);
        }
        Ok(items)
    }

    /// 清空所有历史
    pub fn clear(&self) -> GatewayResult<()> {
        let conn = self.conn();
        conn.execute("DELETE FROM uplink_test_history", [])
            .map_err(|e| {
                crate::engine::error::GatewayError::Queue(format!("clear failed: {}", e))
            })?;
        Ok(())
    }

    fn conn(&self) -> parking_lot::MutexGuard<'_, Connection> {
        self.conn.get()
    }
}

#[cfg(test)]
mod tests;
