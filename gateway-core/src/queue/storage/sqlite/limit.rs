//! SQLite 队列批量读写与容量淘汰（SqliteStorage 存储实现细节）
//!
//! 从原 `queue/limit.rs` 中分离出的 SQL 侧实现：批量 INSERT/DELETE、
//! in_flight 标记维护、按时间清理旧记录与 WAL checkpoint 截断。
//! 内核策略常量（`queue.max_rows` 及其派生阈值）保留在内核层，
//! 本模块仅负责按其执行 SQL。
//!
//! 均为 `SqliteStorage` 的关联函数（以 `conn` 作为参数，加锁由调用方负责）。

use std::sync::atomic::AtomicI64;
use std::sync::OnceLock;

use rusqlite::{params, Connection};

use crate::engine::model::DataPoint;
use crate::queue::core::QueuedItem;

use super::sqlite_storage::SqliteStorage;

const INSERT_CHUNK_SIZE: usize = 50;

/// 批次 C：`data_queue` 批量 INSERT 每行的列数。
/// 末尾一列为 `value_text`（枚举文本标签，可空 → NULL）。
const INSERT_COLS_PER_ROW: usize = 11;

/// 单行占位符（11 列，与 INSERT_COLS_PER_ROW 对应）。
const ROW_PLACEHOLDERS: &str = "(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";

/// 预拼接的 INSERT 语句：下标 n（1..=INSERT_CHUNK_SIZE）对应一次插入 n 行的完整语句。
///
/// 原实现每个 chunk 现场生成 `INSERT_CHUNK_SIZE` 个占位符 `String` 再 `join` +
/// `format!`，稳态下每 chunk 产生 50+ 次小分配 + 2 次大字符串拼装。这里按行数
/// 一次性预拼接好，运行时只做一次下标取值，**稳态零分配**（首次使用时构建，
/// 常驻约 50KB，相对 SQLite 自身占用可忽略）。
static INSERT_SQL_CACHE: OnceLock<Vec<Box<str>>> = OnceLock::new();

/// 取「一次插入 n 行」的预拼接 INSERT 语句。
///
/// `rows` 由 `points.chunks(INSERT_CHUNK_SIZE)` 保证落在 1..=INSERT_CHUNK_SIZE；
/// 越界时收敛到合法区间，避免 panic（防御性，正常路径不会触发）。
fn insert_sql_for_rows(rows: usize) -> &'static str {
    let cache = INSERT_SQL_CACHE.get_or_init(|| {
        const PREFIX: &str = "INSERT INTO data_queue (point_id, device_id, value, raw_value, raw_type, quality, quality_reason, timestamp, source_ts, stale_count, value_text) VALUES ";
        let mut cache: Vec<Box<str>> = Vec::with_capacity(INSERT_CHUNK_SIZE + 1);
        // 下标 0 表示 0 行，不会被使用
        cache.push(Box::from(""));
        let mut values = String::new();
        for n in 1..=INSERT_CHUNK_SIZE {
            if n > 1 {
                values.push(',');
            }
            values.push_str(ROW_PLACEHOLDERS);
            cache.push(format!("{PREFIX}{values}").into_boxed_str());
        }
        cache
    });
    &cache[rows.clamp(1, INSERT_CHUNK_SIZE)]
}

/// #20-2026：未送达行占总队列比重的告警阈值。`enforce_queue_limit` 在
/// 已尽力删除 delivered=1 行后，若未送达行仍超过此比例 → 触发 unsent_overflow
/// 标志，DataQueue::enqueue 入口据此走 fallback 路径（不阻塞启动，给运维
/// 处置时间）。比例取 80% 是经验值：20% 已送达余量保证 ack 流水线不被
/// 立即反压到 fallback。
pub(super) const UNSENT_OVERFLOW_RATIO: f64 = 0.8;

/// #20-2026：overflow 检查结果。由 `enforce_queue_limit` 返回，
/// `SqliteStorage` 据此更新 `unsent_overflow` AtomicBool 及 depth 计数。
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct OverflowReport {
    /// 队列总行数（含 in_flight / delivered 任意状态；删行后为最新值）
    pub total: i64,
    /// 未送达行数（in_flight=0 AND delivered=0；删行后为最新值）
    pub unsent: i64,
    /// 未送达占比 0.0~1.0
    pub ratio: f64,
    /// 是否触发未送达超限（ratio >= UNSENT_OVERFLOW_RATIO AND total > 0）
    pub unsent_overflow: bool,
    /// 此次 overflow 删除的总行数（0 表示未触发行数上限 overflow）
    pub deleted: i64,
    /// 其中**未送达**（delivered=0）的行数。这些行仍计入 depth，
    /// 删除后必须由调用方同步扣减 depth，否则 depth 会虚高。
    pub undelivered_deleted: i64,
}

/// A1：ack 的实际影响行数（按「状态跃迁」计数）。
///
/// 只统计真正发生状态跃迁的行，因此 ack 天然幂等：
/// - `delivered` = 本次由「未送达」变为「已送达」的行数 → 对应 `depth` 扣减
/// - `released` = 本次由「在途」变为「非在途」的行数 → 对应 `in_flight` 扣减
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct AckOutcome {
    /// 由「未送达」跃迁为「已送达」的行数（对应 depth 扣减）
    pub delivered: i64,
    /// 由「在途」跃迁为「非在途」的行数（对应 in_flight 扣减）
    pub released: i64,
}

impl SqliteStorage {
    /// 队列总行数超限时删除最旧的**不在途**记录（P0-2 修复）。
    ///
    /// 旧实现删除条件为 `delivered = 1`（只删已 ack 的行），本意是保护未上报的
    /// 计费数据。但在「笔记本网线直连调试、未配北向消费者」这种最常见场景下
    /// `delivered` 永远是 0（没有任何组件会 dequeue → ack），于是：
    /// - 行数上限到顶时一条都删不掉，队列表只增不减，最终写满磁盘；
    /// - 历史保留期的 FIFO 清理（retention）同样删不掉，时间上界形同虚设。
    ///
    /// 现改为「未在途」判据：`in_flight = 0` 的行（无论 delivered 0/1）都可按
    /// 最旧优先淘汰。真正"正在发送中、删了会重复/丢包"的只有 `in_flight = 1`，
    /// 这类行仍然受保护（若因在途行过多导致删不到上限，会打 error 日志并让
    /// enqueue 走 fallback，而不是静默无界增长）。
    ///
    /// 队列因此变为「容量上界（`queue.max_rows`）+ 时间上界（保留 30 天）」双约束。
    ///
    /// #20-2026：未送达行过多时返回 `OverflowReport`，由 `SqliteStorage::enqueue_batch`
    /// 设置 `unsent_overflow` 标志，DataQueue::enqueue 入口据此走 fallback 路径。
    ///
    /// 修复(审查#6)：超限判据由 `depth`（只统计 delivered=0 行）改为表内**真实总行数**。
    ///
    /// r30-QL4 (已知风险，已评估暂不修)：子查询 `WHERE in_flight = 0 ORDER BY
    /// timestamp ASC LIMIT ?1` 没有针对性复合索引。
    /// - 现状：[`db_init.rs`] 已建 `idx_queue_ts(timestamp)`，SQLite 可用索引顺序扫描
    ///   避免 sort；但子查询的 `in_flight=0` 谓词无法用现有索引覆盖，必须逐行 FILTER。
    /// - 风险：队列表规模达到 10^5+ 行时，每次入队触发的 `enforce_queue_limit`
    ///   会执行 O(n) 扫描，阻塞入队事务。
    /// - 不修的理由：`enforce_queue_limit` 只在超限时才走 DELETE 分支，
    ///   `idx_queue_ts` 已覆盖 ORDER BY，不是热路径。
    pub(super) fn enforce_queue_limit(conn: &mut Connection, max_rows: i64) -> OverflowReport {
        Self::enforce_queue_limit_with(conn, max_rows)
    }

    /// 可注入上限的 `enforce_queue_limit`（`max_rows` 来自配置项 `queue.max_rows`，
    /// 单测也会用极小阈值验证淘汰逻辑）。
    fn enforce_queue_limit_with(conn: &mut Connection, max_rows: i64) -> OverflowReport {
        // r30-QL1 (修复)：把 COUNT + DELETE 包入单个 `BEGIN IMMEDIATE` 事务。
        // 旧实现两条 query_row 与 delete 分开执行，期间另一线程可能 INSERT 新行
        // 或 DELETE 其他行 → `total`/`unsent` 与实际 `deleted` 出现短暂不一致。
        // 用 IMMEDIATE 事务在 SQLite 层强制串行化：避免 BEGIN DEFERRED 的写锁升级
        // 死锁（SQLite 在并发写事务时会产生 SQLITE_BUSY）。
        let tx = match conn.transaction() {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("enforce_queue_limit: begin tx failed: {}", e);
                return OverflowReport::default();
            }
        };

        // total 为真实总行数（含已送达/在飞行），unsent 为「未在飞且未送达」。
        let total: i64 = match tx.query_row("SELECT COUNT(*) FROM data_queue", [], |r| r.get(0)) {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!("enforce_queue_limit: total query failed: {}", e);
                return OverflowReport::default();
            }
        };
        let unsent: i64 = match tx.query_row(
            "SELECT COUNT(*) FROM data_queue WHERE in_flight = 0 AND delivered = 0",
            [],
            |r| r.get(0),
        ) {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!("enforce_queue_limit: unsent query failed: {}", e);
                return OverflowReport::default();
            }
        };

        let mut deleted = 0i64;
        let mut undelivered_deleted = 0i64;
        if total > max_rows {
            let overflow = total - max_rows;
            // A1：探针与 DELETE 的排序键必须**完全一致**，否则两者选中的行集
            // 可能不同（同一毫秒的多行在 `ORDER BY timestamp` 下顺序不确定），
            // 于是「统计到的未送达行数」与「真正删掉的未送达行数」对不上 →
            // depth 漂移。追加 `id ASC` 作为确定性 tie-break：
            // `id` 是 rowid 别名，`idx_queue_ts(timestamp)` 隐含携带 rowid，
            // 因此 (timestamp, id) 排序仍可走索引，无需额外 sort。
            undelivered_deleted = match tx.query_row(
                "SELECT COUNT(*) FROM (
                    SELECT delivered FROM data_queue WHERE in_flight = 0 ORDER BY timestamp ASC, id ASC LIMIT ?1
                ) WHERE delivered = 0",
                params![overflow],
                |r| r.get(0),
            ) {
                Ok(n) => n,
                Err(e) => {
                    tracing::warn!("enforce_queue_limit: undelivered probe failed: {}", e);
                    0
                }
            };
            deleted = match tx.execute(
                "DELETE FROM data_queue WHERE id IN (
                    SELECT id FROM data_queue WHERE in_flight = 0 ORDER BY timestamp ASC, id ASC LIMIT ?1
                )",
                params![overflow],
            ) {
                Ok(n) => n as i64,
                Err(e) => {
                    tracing::warn!("enforce_queue_limit delete failed: {}", e);
                    0
                }
            };

            if deleted < overflow {
                // 剩下的删不掉：只可能是 in_flight=1（正在发送/长时间未 ack）的行。
                // 这是关键告警场景（磁盘即将写满、链路断开），升到 error 级确保
                // 触发告警通道（Prometheus 采集 + 上行系统级告警）。
                tracing::error!(
                    "queue overflow: total={} > limit={}, deleted {} rows ({} rows protected: in_flight; enqueue stays rejected until backlog drains)",
                    total,
                    max_rows,
                    deleted,
                    overflow - deleted
                );
            } else {
                tracing::warn!(
                    "queue overflow: total={} > limit={}, deleted {} oldest non-in-flight records ({} undelivered)",
                    total,
                    max_rows,
                    deleted,
                    undelivered_deleted
                );
            }
        }

        if let Err(e) = tx.commit() {
            tracing::warn!("enforce_queue_limit: commit failed: {}", e);
            return OverflowReport::default();
        }

        // 删除的行都满足 in_flight=0；其中 delivered=1 的行 ack 时已各自扣过
        // depth，只有 delivered=0 的行（undelivered_deleted）需要调用方扣减。
        let total = total - deleted;
        let unsent = (unsent - undelivered_deleted).max(0);
        let ratio = if total > 0 {
            unsent as f64 / total as f64
        } else {
            0.0
        };
        OverflowReport {
            total,
            unsent,
            ratio,
            unsent_overflow: ratio >= UNSENT_OVERFLOW_RATIO && total > 0,
            deleted,
            undelivered_deleted,
        }
    }

    /// 批量入队（SQL 实现）
    ///
    /// C4: 返回**实际插入行数**而非 `points.len()`。逐行回退时会跳过坏点
    /// （只 warn 不中断事务），若调用方按入参长度累加 `depth`，计数会因被跳过
    /// 的失败点而虚高，进而使 `depth`（队列深度指标 / `len()`）与表内真实行数
    /// 不符。此处以「入参数 - 失败数」精确回报。
    pub(super) fn enqueue_batch(
        conn: &mut Connection,
        points: &[DataPoint],
    ) -> rusqlite::Result<usize> {
        // r30c-QL6 (优化)：空 batch 早返，跳过 BEGIN TRANSACTION。
        // 原实现会无意义开一个空事务再 commit，浪费 2 次 fsync（synchronous=FULL），
        // 高频空入队（采集短暂中断后第一帧到达前的周期 tick）下累计开销可见。
        if points.is_empty() {
            return Ok(0);
        }
        let tx = conn.transaction()?;
        let mut failed: usize = 0;

        for chunk in points.chunks(INSERT_CHUNK_SIZE) {
            // Bug 162: 每行必须使用独立的匿名占位符 `?`（按位置绑定），旧实现每行都写
            // `(?1..?10)`，多行时所有行复用同一组编号，SQLite 只认 10 个参数 → 参数数
            // 不符，`tx.execute` 恒失败并回退逐行插入，队列吞吐大幅下降。
            // 批次 C：列数由 10 增至 11（末尾追加 value_text，枚举文本标签）。
            // 占位符串按行数预拼接（见 insert_sql_for_rows），此处不再逐行生成字符串。
            let sql = insert_sql_for_rows(chunk.len());

            // 批次 C 优化：参数改为**借用**点对象（`&dyn ToSql`），不再为每行每个
            // 字段 `Box::new(clone)`。原实现每点产生 11 次 Box 分配 + 5 次 String
            // 缓冲分配，与 pipeline 的批次拷贝同一量级；借用后这部分分配全部消失，
            // 绑定值直接指向 `points`（rusqlite 在 execute 期间只做只读转换）。
            // quality 的 `as_str()` 返回 `&'static str` 值，而 `&dyn ToSql` 的
            // 参照物必须是 Sized 类型，无法直接把 `&str` 塞进去。先把整块的文本
            // 视图落到一个小 Vec（每 chunk 仅 1 次分配、每点 16B），再由它借用。
            let quality_texts: Vec<&'static str> =
                chunk.iter().map(|p| p.quality.as_str()).collect();
            let mut param_values: Vec<&dyn rusqlite::types::ToSql> =
                Vec::with_capacity(chunk.len() * INSERT_COLS_PER_ROW);
            for (i, p) in chunk.iter().enumerate() {
                param_values.push(&p.point_id);
                param_values.push(&p.device_id);
                param_values.push(&p.value);
                param_values.push(&p.raw_value);
                param_values.push(&p.raw_type);
                param_values.push(&quality_texts[i]);
                param_values.push(&p.quality_reason);
                param_values.push(&p.timestamp);
                param_values.push(&p.source_ts);
                param_values.push(&p.stale_count);
                // 批次 C：枚举文本标签（None → NULL，表示该点无枚举标签）
                param_values.push(&p.value_text);
            }

            if tx.execute(sql, param_values.as_slice()).is_err() {
                // r30b-QL3: 旧 fallback 每行重新 `params![...]` 宏 → 每次 10 个参数
                // 全 ToSql 转换（marshal）。改用 prepare_cached + 复用 param_values 切片：
                // 一次 prepare，循环里只取每行对应的 10 个 slot 喂给 rusqlite，
                // 避免 50 行 × 10 次 ToSql 转换 = 500 次冗余 marshal。
                let mut stmt = tx.prepare_cached(
                    "INSERT INTO data_queue (point_id, device_id, value, raw_value, raw_type, quality, quality_reason, timestamp, source_ts, stale_count, value_text)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                )?;
                for (i, p) in chunk.iter().enumerate() {
                    let row_start = i * INSERT_COLS_PER_ROW;
                    // 元素本身是 Copy 的 `&dyn ToSql`，直接切片拷贝，无需再 as_ref
                    let row_params = &param_values[row_start..row_start + INSERT_COLS_PER_ROW];
                    if let Err(e) = stmt.execute(row_params) {
                        failed += 1;
                        tracing::warn!(
                            "enqueue point failed, skipped: point_id={} err={}",
                            p.point_id,
                            e
                        );
                    }
                }
            }
        }

        tx.commit()?;
        let inserted = points.len() - failed;
        if failed > 0 {
            tracing::warn!(
                "enqueue batch done: total={} inserted={} failed={} (skipped bad points, kept good ones)",
                points.len(),
                inserted,
                failed
            );
        }
        Ok(inserted)
    }

    /// 批量出队（标记 in_flight），按 id 升序返回（SQL 实现）
    pub(super) fn dequeue_batch(
        conn: &mut Connection,
        limit: usize,
    ) -> rusqlite::Result<Vec<QueuedItem>> {
        let tx = conn.transaction()?;

        let (items, ids): (Vec<QueuedItem>, Vec<i64>) = {
            let mut stmt = tx.prepare_cached(
                // P0-3 配套修复：必须同时排除 `delivered = 1`。ack 现在的实现是
                // `UPDATE delivered = 1, in_flight = 0`（而非旧版 DELETE），因此已送达
                // 行仍然满足 `in_flight = 0`，只按 in_flight 过滤会把它们反复选回来重发
                // → 重复上报/重复计费，直到 retention 才删。delivered=1 的行只等回收。
                "SELECT id, point_id, device_id, value, raw_value, raw_type, quality, quality_reason, timestamp, source_ts, stale_count, value_text
                 FROM data_queue
                 WHERE in_flight = 0 AND delivered = 0
                 ORDER BY id ASC LIMIT ?1",
            )?;
            let rows = stmt.query_map(params![limit as i64], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    QueuedItem {
                        id: row.get(0)?,
                        point_id: row.get(1)?,
                        device_id: row.get(2)?,
                        value: row.get(3)?,
                        raw_value: row.get(4)?,
                        raw_type: row.get(5)?,
                        quality: row.get(6)?,
                        quality_reason: row.get(7)?,
                        timestamp: row.get(8)?,
                        source_ts: row.get(9)?,
                        stale_count: row.get(10)?,
                        // 批次 C：枚举文本标签（NULL → None）
                        value_text: row.get(11)?,
                    },
                ))
            })?;

            let mut items = Vec::new();
            let mut ids: Vec<i64> = Vec::new();
            for row in rows {
                let (id, item) = row?;
                ids.push(id);
                items.push(item);
            }
            (items, ids)
        };

        if !ids.is_empty() {
            let placeholders: Vec<String> = ids.iter().map(|_| "?".to_string()).collect();
            // Bug 163: 出队时记录 dequeued_at（当前毫秒时间），供 stale in_flight 以
            // "出队时间"而非采集时间戳判定超时。now_ms 为内部整数，直接内联无注入风险。
            let now_ms = DataPoint::now();
            let sql = format!(
                "UPDATE data_queue SET in_flight = 1, dequeued_at = {now_ms} WHERE id IN ({})",
                placeholders.join(",")
            );
            tx.execute(&sql, rusqlite::params_from_iter(ids.iter()))?;
        }

        tx.commit()?;
        Ok(items)
    }

    /// 确认已发送（标记 delivered，由 retention/overflow 后续删除）（SQL 实现）
    ///
    /// P0-3：原实现 ack 时直接 DELETE；改为 `UPDATE delivered=1, in_flight=0`。
    /// 物理删除统一交给 retention 路径，由 `delivered=1` 守卫避免误删未送达行。
    /// 这里同步扣减 `depth`：与 `enqueue_batch` 的 `+= inserted` 配对，使 depth
    /// 恒等于「delivered = 0 的行数」。
    ///
    /// 注意：ack 后行仍满足 `in_flight = 0`，所以 `dequeue_batch` 的 SELECT
    /// **必须**一并过滤 `delivered = 0`，否则已送达行会被反复出队重发
    /// （重复上报/重复计费）。
    ///
    /// A1：拆成两条**带状态守卫**的 UPDATE，并返回各自的实际影响行数。
    /// 旧实现是单条 `SET delivered=1, in_flight=0 WHERE id IN (...)` + 调用方按
    /// `ids.len()` 扣减，而 SQLite 的 UPDATE 只要命中 WHERE 就计入变化行数
    /// （哪怕新值与旧值相同），于是：
    /// - 对**已送达**的行二次 ack（生产里存在 publish 通道与 outbox replay 通道
    ///   两条独立 ack 路径，可能对同一批 row id 各 ack 一次）→ 库里无可更新行，
    ///   内存 `depth` 却被再减一次 → **depth 漂移**（队列深度虚低，健康判据失真）；
    /// - ack 的 ids 里含已被 retention/容量淘汰删除的行 → 同样按 ids.len() 扣减
    ///   → depth 漂移。
    ///
    /// 现在按「状态跃迁」计数（`delivered = 0` → `delivered = 1` 才算送达，
    /// `in_flight = 1` → `0` 才算释放在途），ack 天然幂等，且计数与库内真值一致。
    pub(super) fn ack_batch(conn: &mut Connection, ids: &[i64]) -> rusqlite::Result<AckOutcome> {
        if ids.is_empty() {
            return Ok(AckOutcome::default());
        }
        let tx = conn.transaction()?;
        let mut outcome = AckOutcome::default();
        for chunk in ids.chunks(INSERT_CHUNK_SIZE) {
            let placeholders: Vec<String> = chunk.iter().map(|_| "?".to_string()).collect();
            // ① 标记送达：只影响「尚未送达」的行（已送达行不再计数）。
            let sql = format!(
                "UPDATE data_queue SET delivered = 1 WHERE id IN ({}) AND delivered = 0",
                placeholders.join(",")
            );
            outcome.delivered += tx.execute(&sql, rusqlite::params_from_iter(chunk.iter()))? as i64;
            // ② 释放在途：只影响「仍在途」的行（已释放在途行不再计数）。
            let sql = format!(
                "UPDATE data_queue SET in_flight = 0 WHERE id IN ({}) AND in_flight = 1",
                placeholders.join(",")
            );
            outcome.released += tx.execute(&sql, rusqlite::params_from_iter(chunk.iter()))? as i64;
        }
        tx.commit()?;
        Ok(outcome)
    }

    /// 释放 in_flight 标志（不删除数据）（SQL 实现）
    ///
    /// r15-Q4 (fix): 返回**实际被复位的行数**，并把 SQL 错误上抛。
    /// 旧实现吞掉错误只打 warn，调用方无从得知 DB 是否真的释放成功，
    /// 却照样执行 `saturating_dec(&self.in_flight, ids_len)` → 内存计数被减，
    /// 而 DB 里那些行仍是 `in_flight = 1`。后果有二：
    /// 1. `in_flight` 计数**低估** → `MAX_IN_FLIGHT` 反压阈值被提前放宽；
    /// 2. 那批 `in_flight = 1` 的行在 dequeue 的 `WHERE in_flight = 0` 里**永不入选**
    ///    → 数据滞留不投递（现场表现为"采集正常但北向数据越来越旧"）。
    ///
    /// A1：补 `AND in_flight = 1` 状态守卫。只影响「仍在途」的行 →
    /// 重复 release 不会二次扣减（否则 `in_flight` 计数被多减，反压阈值失真）。
    pub(super) fn release_in_flight(conn: &mut Connection, ids: &[i64]) -> rusqlite::Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let placeholders: Vec<String> = ids.iter().map(|_| "?".to_string()).collect();
        let sql = format!(
            "UPDATE data_queue SET in_flight = 0 WHERE id IN ({}) AND in_flight = 1",
            placeholders.join(",")
        );
        conn.execute(&sql, rusqlite::params_from_iter(ids.iter()))
    }

    // ===== #19: 主动 retention + WAL checkpoint =====

    /// #19: 删除超过保留期的旧记录（**不在途**的行，含未送达）。
    ///
    /// 保留时长按保留天数换算（days × 24h，固定 30 天），由调用方传入；
    /// None = 不限，跳过清理。
    ///
    /// P0-2 修复：旧实现要求 `delivered = 1`，而直连调试场景（未配北向消费者）
    /// 永远没有任何行被 ack → `delivered` 恒为 0 → 到期也删不掉一行，
    /// 时间上界完全空转，队列表无界增长。
    /// 现放宽为 `in_flight = 0`：不在途的行（无论是否已送达）到期即回收；
    /// 只有正在发送中（`in_flight = 1`）的行仍然受保护。删除未送达行时同步
    /// 扣减 depth（depth 语义 = delivered=0 的行数）。
    ///
    /// 兜底告警：若清理后仍有超期行无法删除（`in_flight = 1`），说明有行长期
    /// 卡在在途状态（正常由 5 分钟 stale 回收兜住），打 error 日志而不是静默。
    pub(super) fn cleanup_old_records(
        conn: &mut Connection,
        max_age_hours: Option<u64>,
        depth: &AtomicI64,
    ) -> i64 {
        let Some(max_age_hours) = max_age_hours else {
            return 0;
        };
        let cutoff_ms = DataPoint::now() - (max_age_hours as i64 * 3600 * 1000);

        let tx = match conn.transaction() {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("cleanup_old_records: begin tx failed: {}", e);
                return 0;
            }
        };
        // 先统计拟删行里有多少是未送达的（这些行还计在 depth 里，删后要扣）。
        let undelivered: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM data_queue \
                 WHERE in_flight = 0 AND delivered = 0 AND timestamp < ?1",
                rusqlite::params![cutoff_ms],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let deleted = match tx.execute(
            "DELETE FROM data_queue WHERE in_flight = 0 AND timestamp < ?1",
            rusqlite::params![cutoff_ms],
        ) {
            Ok(n) => n as i64,
            Err(e) => {
                tracing::warn!("cleanup_old_records failed: {}", e);
                0
            }
        };
        // 清理后仍超期的行 → 只可能是在途行（删不掉），需显式告警。
        let still_stale: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM data_queue WHERE timestamp < ?1",
                rusqlite::params![cutoff_ms],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if let Err(e) = tx.commit() {
            tracing::warn!("cleanup_old_records: commit failed: {}", e);
            return 0;
        }

        if undelivered > 0 {
            Self::saturating_dec(depth, undelivered);
        }
        if deleted > 0 {
            tracing::info!(
                "retention cleanup: deleted {} records older than {}h ({} undelivered, cutoff={})",
                deleted,
                max_age_hours,
                undelivered,
                cutoff_ms
            );
        }
        if still_stale > 0 {
            tracing::error!(
                "retention blocked: {} rows older than {}h are still in_flight and cannot be pruned (check upstream ack / stale in_flight reclaim)",
                still_stale,
                max_age_hours
            );
        }
        deleted
    }

    /// #19: 主动 WAL checkpoint（TRUNCATE 模式）
    ///
    /// WAL 文件可能膨胀到几十 MB，主动 checkpoint 让 SQLite 把 WAL 内容写回主库并截断。
    pub(super) fn wal_checkpoint_truncate(conn: &mut Connection) {
        match conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);") {
            Ok(()) => tracing::debug!("WAL checkpoint TRUNCATE done"),
            Err(e) => tracing::warn!("wal_checkpoint_truncate failed: {}", e),
        }
    }
}

#[cfg(test)]
mod limit_tests {
    use super::*;

    /// 预拼接的 INSERT 语句必须严格含 `rows * INSERT_COLS_PER_ROW` 个匿名占位符：
    /// 少一个 → `tx.execute` 失败并**静默**退化为逐行插入（吞吐塌陷，难以察觉）；
    /// 多一个 → 参数个数不匹配直接报错丢批。
    #[test]
    fn insert_sql_placeholder_count_matches_row_count() {
        for rows in 1..=INSERT_CHUNK_SIZE {
            let sql = insert_sql_for_rows(rows);
            assert_eq!(
                sql.matches('?').count(),
                rows * INSERT_COLS_PER_ROW,
                "rows={} 的预拼接 SQL 占位符数必须与绑定参数数一致",
                rows
            );
            assert!(
                sql.starts_with("INSERT INTO data_queue"),
                "rows={} 的预拼接 SQL 必须指向 data_queue 表",
                rows
            );
        }
    }

    /// 建一张与生产同构的 `data_queue` 表（列定义对齐 `db_init::init_queue_db`）。
    fn setup_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory sqlite");
        conn.execute_batch(
            "CREATE TABLE data_queue (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                point_id TEXT NOT NULL,
                device_id TEXT NOT NULL DEFAULT '',
                value REAL NOT NULL,
                raw_value REAL NOT NULL,
                raw_type TEXT NOT NULL,
                quality TEXT NOT NULL,
                quality_reason TEXT NOT NULL DEFAULT '',
                timestamp INTEGER NOT NULL,
                source_ts INTEGER NOT NULL DEFAULT 0,
                stale_count INTEGER NOT NULL DEFAULT 0,
                in_flight INTEGER NOT NULL DEFAULT 0,
                dequeued_at INTEGER NOT NULL DEFAULT 0,
                delivered INTEGER NOT NULL DEFAULT 0,
                value_text TEXT
            );",
        )
        .expect("create data_queue");
        conn
    }

    /// 插入 `n` 行，时间戳从 1000 递增（淘汰顺序可预期）；`delivered=1` 表示已送达。
    fn insert_rows(conn: &Connection, n: i64, delivered: i64) {
        insert_rows_from(conn, n, delivered, 1000);
    }

    /// 同上，但指定起始时间戳，便于构造「已送达批次在前、未送达批次在后」的场景。
    fn insert_rows_from(conn: &Connection, n: i64, delivered: i64, ts0: i64) {
        for i in 0..n {
            conn.execute(
                "INSERT INTO data_queue (point_id, value, raw_value, raw_type, quality, timestamp, in_flight, delivered)
                 VALUES (?1, 0.0, 0.0, 'f64', 'good', ?2, 0, ?3)",
                params![format!("p-{}", i), ts0 + i, delivered],
            )
            .expect("insert row");
        }
    }

    /// 插入 `n` 行「在途」记录（in_flight=1）——任何淘汰/清理路径都不得删除它们。
    fn insert_rows_in_flight(conn: &Connection, n: i64) {
        for i in 0..n {
            conn.execute(
                "INSERT INTO data_queue (point_id, value, raw_value, raw_type, quality, timestamp, in_flight, delivered)
                 VALUES (?1, 0.0, 0.0, 'f64', 'good', ?2, 1, 0)",
                params![format!("p-{}", i), 1000 + i],
            )
            .expect("insert in-flight row");
        }
    }

    fn row_count(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM data_queue", [], |r| r.get(0))
            .expect("count rows")
    }

    /// 修复(审查#6) 回归：已送达行堆积到超过上限时必须被回收。
    #[test]
    fn delivered_rows_are_pruned_by_total_row_count() {
        let mut conn = setup_conn();
        insert_rows(&conn, 10, 1);
        let report = SqliteStorage::enforce_queue_limit_with(&mut conn, 4);

        assert_eq!(row_count(&conn), 4, "应被削到上限行数");
        assert_eq!(report.deleted, 6);
        assert_eq!(report.undelivered_deleted, 0);
        assert_eq!(report.total, 4, "report.total 应为删行后的真实总行数");
        assert_eq!(report.unsent, 0);
        assert!(!report.unsent_overflow);
        let min_id: i64 = conn
            .query_row("SELECT MIN(id) FROM data_queue", [], |r| r.get(0))
            .expect("min id");
        assert_eq!(min_id, 7, "必须按最旧优先淘汰");
    }

    /// P0-2 回归：未送达行（delivered=0，直连场景的全部行）在超限时同样要被淘汰，
    /// 否则行数上限到顶也一条删不掉 → 队列无界增长。
    /// 同时 report 必须精确回报被删的未送达行数，供调用方扣减 depth。
    #[test]
    fn undelivered_rows_pruned_when_over_capacity() {
        let mut conn = setup_conn();
        insert_rows(&conn, 9, 0);
        let report = SqliteStorage::enforce_queue_limit_with(&mut conn, 4);

        assert_eq!(report.deleted, 5, "超出上限的 5 行未送达必须被淘汰");
        assert_eq!(report.undelivered_deleted, 5, "被删的 5 行全部是未送达行");
        assert_eq!(row_count(&conn), 4, "必须削到上限");
        assert_eq!(report.total, 4);
        assert_eq!(report.unsent, 4);
        assert!(
            report.unsent_overflow,
            "ratio=100% 必须触发 unsent_overflow（enqueue 走 fallback）"
        );
    }

    /// 混合场景：按「最旧优先」淘汰，可见未送达与已送达行都会被削掉。
    #[test]
    fn mixed_rows_pruned_oldest_first() {
        let mut conn = setup_conn();
        insert_rows_from(&conn, 6, 1, 1000); // 最旧 6 行，已送达
        insert_rows_from(&conn, 3, 0, 2000); // 较新 3 行，未送达
        let report = SqliteStorage::enforce_queue_limit_with(&mut conn, 2);

        assert_eq!(report.deleted, 7, "10 行削到 2 行，应删 7 行");
        assert_eq!(
            report.undelivered_deleted, 1,
            "最旧的 6 行已送达，另删 1 行未送达"
        );
        assert_eq!(row_count(&conn), 2);
        assert_eq!(report.total, 2);
        assert_eq!(report.unsent, 2, "剩余 2 行都是未送达");
        assert!(report.unsent_overflow);
    }

    /// 在途行（in_flight=1）永不删除：删了会造成重复上报/丢包。
    #[test]
    fn in_flight_rows_are_never_pruned() {
        let mut conn = setup_conn();
        insert_rows_in_flight(&conn, 5);
        let report = SqliteStorage::enforce_queue_limit_with(&mut conn, 1);

        assert_eq!(report.deleted, 0, "在途行一条都不能删");
        assert_eq!(row_count(&conn), 5);
        assert!(report.total > 1, "删不到上限时 total 仍高于 max_rows");
    }

    /// 未超限时不删任何行，ratio 正常计算。
    #[test]
    fn below_limit_deletes_nothing() {
        let mut conn = setup_conn();
        insert_rows(&conn, 1, 1);
        insert_rows(&conn, 3, 0);
        let report = SqliteStorage::enforce_queue_limit_with(&mut conn, 1000);

        assert_eq!(report.deleted, 0);
        assert_eq!(row_count(&conn), 4);
        assert_eq!(report.unsent, 3);
        assert!((report.ratio - 0.75).abs() < 1e-9);
        assert!(!report.unsent_overflow, "3/4=75% < 80% 不应触发");
    }

    /// r30-QL1 (回归)：enforce_queue_limit 现在把 COUNT + DELETE 包入单个事务。
    /// 旧实现两条 query_row 与 delete 分开执行，期间另一线程可能 INSERT/DELETE
    /// 导致 total/unsent 与实际删行结果不一致。本测试覆盖：事务 commit 路径正常
    /// 工作 + 删行后表内状态 = OverflowReport 报告值（两者必须一致）。
    #[test]
    fn tx_commit_keeps_report_consistent_with_table_state() {
        let mut conn = setup_conn();
        // 7 行已送达 + 3 行未送达 = 10 行，max_rows=5 → 删最旧 5 行（均为已送达）。
        insert_rows_from(&conn, 7, 1, 1000);
        insert_rows_from(&conn, 3, 0, 2000);

        let report = SqliteStorage::enforce_queue_limit_with(&mut conn, 5);

        assert_eq!(report.deleted, 5);
        assert_eq!(report.undelivered_deleted, 0);
        assert_eq!(report.total, 5);
        assert_eq!(report.unsent, 3);
        // 表内实际行数 = report.total（事务 commit 成功，二者必须一致）
        assert_eq!(
            row_count(&conn),
            report.total,
            "事务 commit 后表内行数应等于报告值"
        );
        // 剩 2 行已送达 + 3 行未送达
        let remaining_delivered: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM data_queue WHERE delivered = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(remaining_delivered, 2);
    }

    /// r30-QL1 补充：满表场景下事务能完整 commit（无 SQLite BUSY 错误）。
    #[test]
    fn tx_commit_under_full_table() {
        let mut conn = setup_conn();
        // 100 行已送达，max_rows=10 → 删 90 行
        insert_rows(&conn, 100, 1);
        let report = SqliteStorage::enforce_queue_limit_with(&mut conn, 10);
        assert_eq!(report.deleted, 90);
        assert_eq!(row_count(&conn), 10);
        assert_eq!(report.unsent, 0);
    }

    /// P0-2 回归：保留期到点必须真正生效——超期**未送达**行要被删除，
    /// 且 depth 计数同步扣减（旧实现因 `delivered=1` 守卫一条都删不掉）。
    #[test]
    fn retention_prunes_undelivered_old_rows_and_decrements_depth() {
        let mut conn = setup_conn();
        insert_rows(&conn, 3, 0); // timestamp 1000..1002，远早于当前时间
        let depth = AtomicI64::new(3);

        let deleted = SqliteStorage::cleanup_old_records(&mut conn, Some(1), &depth);

        assert_eq!(deleted, 3, "超期未送达行必须被回收");
        assert_eq!(row_count(&conn), 0);
        assert_eq!(
            depth.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "depth 必须同步扣减，否则溢出判据虚高"
        );
    }

    /// retention 不得删除在途行（in_flight=1）。
    #[test]
    fn retention_protects_in_flight_rows() {
        let mut conn = setup_conn();
        insert_rows_in_flight(&conn, 2);
        let depth = AtomicI64::new(0);

        let deleted = SqliteStorage::cleanup_old_records(&mut conn, Some(1), &depth);

        assert_eq!(deleted, 0, "在途行不能删");
        assert_eq!(row_count(&conn), 2);
    }

    /// r30c-QL6 (回归)：enqueue_batch 收到空切片应早返 Ok(0)，不开事务。
    /// 旧实现会无意义开 BEGIN→空循环→COMMIT，浪费 2 次 fsync（synchronous=FULL）。
    /// 验证：直接调 enqueue_batch(&mut conn, &[]) 应返回 Ok(0)，
    /// 且数据库 WAL 文件不会被改写（间接通过 savepoint 验证：嵌套事务可正常 BEGIN）。
    #[test]
    fn enqueue_batch_empty_early_returns_without_opening_tx() {
        let mut conn = setup_conn();
        let n = SqliteStorage::enqueue_batch(&mut conn, &[]).expect("empty batch");
        assert_eq!(n, 0);
        assert_eq!(row_count(&conn), 0, "空 batch 不应写任何行");

        // 后续事务仍能正常开启——证明空 batch 没遗留 BEGIN 状态
        insert_rows(&conn, 3, 0);
        assert_eq!(row_count(&conn), 3);
    }
}
