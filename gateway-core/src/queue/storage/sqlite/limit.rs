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

/// 是否为「库级 / IO 级」致命错误：这类错误与具体数据行无关，不能当坏点跳过。
///
/// 典型场景是磁盘满（SQLITE_FULL）：SQLite 会自动回滚事务，若继续逐行重试，
/// 每行都会失败并只留下 warn 日志，最终 commit 报「no transaction is active」，
/// 真正的「磁盘满」被彻底掩盖。这里把这类错误直接上抛。
fn is_fatal_storage_error(e: &rusqlite::Error) -> bool {
    use rusqlite::ErrorCode;
    matches!(
        e,
        rusqlite::Error::SqliteFailure(inner, _)
            if matches!(
                inner.code,
                ErrorCode::DiskFull
                    | ErrorCode::SystemIoFailure
                    | ErrorCode::DatabaseCorrupt
                    | ErrorCode::ReadOnly
                    | ErrorCode::CannotOpen
                    | ErrorCode::NotADatabase
                    | ErrorCode::TooBig
            )
    )
}

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
        // P2-1：末尾追加冲突子句。`(point_id, timestamp)` 上有唯一索引
        // `idx_queue_dedup`，重复行（persist 重试、上游重复投递、同一批点被两条
        // 投递路径各写一次）在存储层被静默跳过，不当作错误——避免触发逐行回退
        // 的 warn 风暴。冲突目标必须与唯一索引完全一致，否则 SQLite 直接报错
        // （索引缺失会在启动初始化阶段暴露，不会运行期才发现）。
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
            cache.push(
                format!("{PREFIX}{values} ON CONFLICT(point_id, timestamp) DO NOTHING")
                    .into_boxed_str(),
            );
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

/// 方案 A：两次容量淘汰之间的最小间隔（毫秒）。
///
/// 超限后若每批入队都跑一次 `DELETE`，队列表 10^6 行时单轮可达数百毫秒，
/// 写锁几乎被 trim 占满，`dequeue → ack` 长期拿不到锁 → 队列只涨不落、CPU 飙升。
/// 冷却期把 trim 频率从「每批」压到「每秒最多 5 次」，留出写锁时间给 dequeue/ack。
///
/// 取 200ms 而不是 1s：单次删除量已被 `TRIM_MAX_ROWS_PER_RUN` 限死，若冷却期仍是
/// 1s，「每秒可淘汰行数」会低于 8000 点/s 的入队速率，队列表将无界超出 `max_rows`。
/// 5 次/s × 2000 行/s = 1 万行/s 的淘汰能力，才对 8000 点/s 留出余量。
pub(super) const TRIM_COOLDOWN_MS: i64 = 200;

/// 方案 A：单次 trim 最多删除的行数（下限，实际还会随本批入队量放大）。
///
/// 一次削几十万行会让 `DELETE` 长时间占住写锁，采集侧请求被阻塞、p99 直接爆掉
/// （CI 实测 s8000 的 p99 从 99ms 涨到 496ms）。这里把单轮成本压在「2000 行」这个
/// 量级——与历史实现单批删除的规模一致，实测 p99 稳定在 100ms 内。
const TRIM_MAX_ROWS_PER_RUN: i64 = 2000;

/// 方案 A：淘汰目标水位 = `max_rows` 的 90%。
///
/// 一次削到上限的 9 成而不是刚好削到上限，是「滞回」：削完之后还有 10% 的
/// 余量承接后续入队，不必每批都重新触顶后再 trim。单次删除量受
/// `TRIM_MAX_ROWS_PER_RUN` 限制，因此远距离超限时分多轮逐步逼近该水位。
const TRIM_TARGET_NUM: i64 = 9;
const TRIM_TARGET_DEN: i64 = 10;

/// 方案 A：判定本批入队后是否执行容量淘汰，以及淘汰的目标水位。
///
/// 返回 `None` = 本批不 trim；返回 `Some(target)` = 把表削到 `target` 行。
///
/// - `total`：当前表内总行数（O(1) 内存计数器）
/// - `max_rows`：配置的硬上限
/// - `last_trim_ms`：上次 trim 的时刻（0 表示从未 trim 过 → 立即触发）
/// - `now_ms`：当前时刻
/// - `inserted`：本批实际插入行数，用于放大单轮删除预算
///
/// 单轮删除预算 = `max(TRIM_MAX_ROWS_PER_RUN, 2 × inserted)`：固定下限保证低速
/// 场景也能把积压削下去；随入队量放大则保证「淘汰速率 ≥ 2 倍入队速率」，高速
/// 场景不会因为预算太小而让队列表无界增长。
pub(super) fn trim_target(
    total: i64,
    max_rows: i64,
    last_trim_ms: i64,
    now_ms: i64,
    inserted: i64,
) -> Option<i64> {
    // 未超限：热路径常态，零 DB 开销。
    if total <= max_rows {
        return None;
    }
    // 冷却期内：即使仍超限也先不动手，把写锁让给 dequeue/ack。
    if last_trim_ms > 0 && now_ms.saturating_sub(last_trim_ms) < TRIM_COOLDOWN_MS {
        return None;
    }
    // 滞回水位：削到上限的 90%（至少 1 行，且不超过上限本身）。
    let hysteresis = max_rows
        .saturating_mul(TRIM_TARGET_NUM)
        .saturating_div(TRIM_TARGET_DEN)
        .max(1)
        .min(max_rows);
    // 单轮预算：目标水位不得低于 `total - 预算`，否则一次会删掉几万甚至几十万行。
    let budget = TRIM_MAX_ROWS_PER_RUN.max(inserted.saturating_mul(2));
    let capped = total.saturating_sub(budget).max(0);
    Some(hysteresis.max(capped))
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
    /// P1-1/P1-2 (性能修复)：`total` / `unsent` 由调用方通过 O(1) 内存计数器传入，
    /// 本函数**不再执行 `SELECT COUNT(*)`**。旧实现对每次入队都跑两条全表 COUNT，
    /// 队列表 10^6 行时单次入队要扫两遍全表（r30-QL4 记录的已知风险）。
    /// 同时补了部分索引 `idx_queue_evict(timestamp) WHERE in_flight = 0`，
    /// 淘汰子查询的 `ORDER BY timestamp, id LIMIT n` 可直接顺序读索引，
    /// 不再需要在超限时对全部非在途行排序。
    ///
    /// 计数器与表内真实行数严格一致：所有 INSERT / DELETE 都在同一 `conn` 锁内
    /// 完成，且每次增删同步更新计数器（见 `SqliteStorage::enqueue_batch` /
    /// `maintenance` / `cleanup_old_records`）。
    ///
    /// 方案 A：`target_rows` 是**本次淘汰的目标水位**，而非配置上限本身。
    /// 入队路径传入 `trim_target` 算出的滞回水位（上限的 90%），maintenance
    /// 路径仍直接传配置上限。函数只按 `total - target_rows` 计算要删的行数。
    pub(super) fn enforce_queue_limit(
        conn: &mut Connection,
        target_rows: i64,
        total: i64,
        unsent: i64,
    ) -> OverflowReport {
        Self::enforce_queue_limit_with(conn, target_rows, total, unsent)
    }

    /// 可注入目标水位的 `enforce_queue_limit`（生产路径传 `queue.max_rows` 或其
    /// 90% 水位，单测也会用极小阈值验证淘汰逻辑）。
    fn enforce_queue_limit_with(
        conn: &mut Connection,
        target_rows: i64,
        total: i64,
        unsent: i64,
    ) -> OverflowReport {
        // P1-2 快速路径：未超限时**不开事务、不写库**，仅用计数器算比例。
        // 这是入队热路径的常态（队列未满），代价是几次原子读。
        if total <= target_rows {
            let ratio = if total > 0 {
                unsent as f64 / total as f64
            } else {
                0.0
            };
            return OverflowReport {
                total,
                unsent,
                ratio,
                unsent_overflow: ratio >= UNSENT_OVERFLOW_RATIO && total > 0,
                deleted: 0,
                undelivered_deleted: 0,
            };
        }

        // r30-QL1 (修复)：把 DELETE 与其统计包入单个事务。
        // 旧实现 COUNT 与 DELETE 分开执行，期间另一线程可能 INSERT 新行
        // → `total`/`unsent` 与实际 `deleted` 出现短暂不一致。
        let tx = match conn.transaction() {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("enforce_queue_limit: begin tx failed: {}", e);
                return OverflowReport::default();
            }
        };

        let undelivered_deleted: i64;
        let deleted: i64;
        let overflow = total - target_rows;
        {
            // A1：探针与 DELETE 的排序键必须**完全一致**，否则两者选中的行集
            // 可能不同（同一毫秒的多行在 `ORDER BY timestamp` 下顺序不确定），
            // 于是「统计到的未送达行数」与「真正删掉的未送达行数」对不上 →
            // depth 漂移。追加 `id ASC` 作为确定性 tie-break：
            // `id` 是 rowid 别名，`idx_queue_evict` 隐含携带 rowid，
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
                    "queue overflow: total={} > target={}, deleted {} rows ({} rows protected: in_flight; enqueue stays rejected until backlog drains)",
                    total,
                    target_rows,
                    deleted,
                    overflow - deleted
                );
            } else {
                tracing::warn!(
                    "queue overflow: total={} > target={}, deleted {} oldest non-in-flight records ({} undelivered)",
                    total,
                    target_rows,
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
        // P2-1：改成累计「实际插入行数」而非「入参数 - 失败数」。
        // 有 `ON CONFLICT DO NOTHING` 后，被唯一约束跳过的重复行既不是成功也
        // 不是失败，必须单独计数，否则 depth / rows_total 会按入参长度虚高。
        let mut inserted: usize = 0;
        let mut skipped_dup: usize = 0;
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

            match tx.execute(sql, param_values.as_slice()) {
                Ok(n) => inserted += n,
                Err(chunk_err) => {
                    // 库级/IO 级错误（磁盘满、只读、IO 故障、库损坏）不能按「坏数据行跳过」
                    // 处理：SQLITE_FULL 等错误会让 SQLite 自动回滚当前事务，此时继续逐行
                    // 重试只会得到「no transaction is active」，最后用已失效的事务 commit，
                    // 把真正的磁盘满原因掩盖成一句无法定位的 commit 失败。
                    // 直接向上返回原始错误，让调用方看到 SQLITE_FULL 并走降级/告警。
                    if is_fatal_storage_error(&chunk_err) {
                        return Err(chunk_err);
                    }
                    // r30b-QL3: 旧 fallback 每行重新 `params![...]` 宏 → 每次 10 个参数
                    // 全 ToSql 转换（marshal）。改用 prepare_cached + 复用 param_values 切片：
                    // 一次 prepare，循环里只取每行对应的 10 个 slot 喂给 rusqlite，
                    // 避免 50 行 × 10 次 ToSql 转换 = 500 次冗余 marshal。
                    // P2-1：冲突子句必须与批量语句保持一致，否则逐行回退路径会把
                    // 重复行当成错误计入 failed。
                    let mut stmt = tx.prepare_cached(
                        "INSERT INTO data_queue (point_id, device_id, value, raw_value, raw_type, quality, quality_reason, timestamp, source_ts, stale_count, value_text)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                         ON CONFLICT(point_id, timestamp) DO NOTHING",
                    )?;
                    for (i, p) in chunk.iter().enumerate() {
                        let row_start = i * INSERT_COLS_PER_ROW;
                        // 元素本身是 Copy 的 `&dyn ToSql`，直接切片拷贝，无需再 as_ref
                        let row_params = &param_values[row_start..row_start + INSERT_COLS_PER_ROW];
                        match stmt.execute(row_params) {
                            // 1 = 真正插入；0 = 命中 (point_id, timestamp) 唯一约束被跳过
                            Ok(1) => inserted += 1,
                            Ok(_) => skipped_dup += 1,
                            Err(e) => {
                                if is_fatal_storage_error(&e) {
                                    return Err(e);
                                }
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
            }
        }

        tx.commit()?;
        if failed > 0 {
            tracing::warn!(
                "enqueue batch done: total={} inserted={} failed={} dup_skipped={} (skipped bad/duplicate points, kept good ones)",
                points.len(),
                inserted,
                failed,
                skipped_dup
            );
        }
        if skipped_dup > 0 {
            tracing::debug!(
                "enqueue batch: {} duplicate (point_id, timestamp) rows ignored by unique index",
                skipped_dup
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

    /// 快照出队：每个 `(device_id, point_id)` 只返回**最新一行**（`MAX(id)`），
    /// 且只取 `id <= cutoff` 的行。
    ///
    /// 折叠（把更旧的行标记 delivered）由调用方在每个上报周期开头通过
    /// `supersede_snapshot` 执行一次，不随批次重复执行——选择语句本身按点位
    /// 取 `MAX(id)` 已保证「同一批次内每点位仅一行」，即使尚有旧行未折叠也不会重复上行。
    ///
    /// `cutoff` 固定本周期数据边界：排空期间新入队的样本（id 更大）留到下个周期，
    /// 避免同一点位在一个周期内被上行两次。
    ///
    /// 步骤：选出每个点位的最新行 → 给选中行置 `in_flight`（同一事务）。
    pub(super) fn dequeue_snapshot_batch(
        conn: &mut Connection,
        limit: usize,
        cutoff: i64,
    ) -> rusqlite::Result<Vec<QueuedItem>> {
        let tx = conn.transaction()?;

        let (items, ids): (Vec<QueuedItem>, Vec<i64>) = {
            let mut stmt = tx.prepare_cached(
                // 「最新」按入队序 `MAX(id)` 判定（对应「网关缓存里的当前值」），
                // 不按 `source_ts`——迟到的旧样本不应覆盖后写入的新值。
                "WITH latest AS (
                     SELECT MAX(id) AS id FROM data_queue
                     WHERE in_flight = 0 AND delivered = 0 AND id <= ?2
                     GROUP BY device_id, point_id
                 )
                 SELECT id, point_id, device_id, value, raw_value, raw_type, quality, quality_reason, timestamp, source_ts, stale_count, value_text
                 FROM data_queue
                 WHERE id IN (SELECT id FROM latest)
                 ORDER BY id ASC LIMIT ?1",
            )?;
            let rows = stmt.query_map(params![limit as i64, cutoff], |row| {
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
            // 与普通出队一致：记录 dequeued_at，供 stale in_flight 按「出队时间」判定。
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

    /// 快照折叠：把所有「非本点位最新」的未送达行标记 `delivered = 1`（不上行）。
    ///
    /// 由 `publish_loop` 在每个上报周期开头调用一次，而非每批一次：同一周期内的
    /// 重复样本只需折叠一遍，避免「批次数 × 全量 GROUP BY」的冗余扫描。
    /// 反压（in_flight 达上限）时也应调用——即使不发数据，也要把旧样本收敛掉，
    /// 否则积压行会持续占磁盘并抬高队列水位。
    ///
    /// 注意：调用方必须保证此时没有「即将被选中」的行被误折叠——本语句以
    /// `in_flight = 0` 为前提，已在途（待 ACK）的行不受影响。
    ///
    /// 返回 `(折叠行数, cutoff)`：`cutoff` 是折叠时刻表内 `MAX(id)`，作为本周期
    /// 出队上界。与折叠同事务读取，保证「折叠边界 = 出队边界」，不会漏掉折叠前
    /// 已入队的样本，也不会把排空期间新到的样本算进本周期。
    pub(super) fn supersede_snapshot(conn: &mut Connection) -> rusqlite::Result<(usize, i64)> {
        let tx = conn.transaction()?;
        let superseded = tx.execute(
            "UPDATE data_queue SET delivered = 1
             WHERE delivered = 0 AND in_flight = 0
               AND id NOT IN (
                   SELECT MAX(id) FROM data_queue
                   WHERE in_flight = 0 AND delivered = 0
                   GROUP BY device_id, point_id
               )",
            [],
        )?;
        // MAX(id) 走主键索引，O(1)；空表返回 0，出队自然不会选中任何行。
        let cutoff: i64 = tx.query_row("SELECT COALESCE(MAX(id), 0) FROM data_queue", [], |r| {
            r.get(0)
        })?;
        tx.commit()?;
        Ok((superseded, cutoff))
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
        rows_total: &AtomicI64,
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
            // P1-2：retention 删的是全表行，`rows_total` 必须同步扣减，
            // 否则计数器单调虚高 → 容量判定误认为一直超限、每批入队都触发淘汰。
            Self::saturating_dec(rows_total, deleted);
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
mod trim_target_tests {
    use super::{trim_target, TRIM_COOLDOWN_MS, TRIM_MAX_ROWS_PER_RUN};

    /// 未超限时不 trim（入队热路径的常态，必须零 DB 开销）。
    #[test]
    fn none_when_below_limit() {
        assert_eq!(trim_target(999, 1000, 0, 10_000, 10), None);
        assert_eq!(trim_target(1000, 1000, 0, 10_000, 10), None);
        assert_eq!(trim_target(0, 1000, 0, 10_000, 0), None);
    }

    /// 刚超限且缺口小于单轮预算时，一次削到上限的 90%（滞回水位）。
    #[test]
    fn trims_to_ninety_percent_within_budget() {
        assert_eq!(trim_target(1001, 1000, 0, 5, 0), Some(900));
        // 缺口刚好等于预算（900 + 2000 = 2900）：仍然一次削到 90%，不浪费预算
        assert_eq!(
            trim_target(900 + TRIM_MAX_ROWS_PER_RUN, 1000, 0, 5, 0),
            Some(900)
        );
    }

    /// 远超上限时单轮只删 `TRIM_MAX_ROWS_PER_RUN` 行，避免长时间占住写锁。
    #[test]
    fn caps_rows_deleted_per_run() {
        let total = 1_000_001;
        let target = trim_target(total, 1_000_000, 0, 5, 0).expect("超限必须 trim");
        assert_eq!(
            total - target,
            TRIM_MAX_ROWS_PER_RUN,
            "单轮删除量必须被预算限住"
        );
        // 否则会一次删掉 10 万行——CI 上正是这样把 s8000 的 p99 拖到 496ms
        assert_ne!(target, 900_000);
    }

    /// 预算随本批插入量放大（2×），保证高速入队下淘汰能力不被限死。
    #[test]
    fn budget_scales_with_inserted() {
        let total = 2_000_000;
        let inserted = 50_000;
        let target = trim_target(total, 1_000_000, 0, 5, inserted).expect("超限必须 trim");
        assert_eq!(total - target, inserted * 2, "预算 = 2×本批插入量");
        // 放大后的预算仍受 90% 水位约束，不会把表削到水位以下
        let target2 = trim_target(total, 1_000_000, 0, 5, 10_000_000).expect("超限必须 trim");
        assert_eq!(target2, 900_000, "预算再大也不能削到 90% 水位以下");
    }

    /// 冷却期内即使仍超限也不 trim；冷却到期才允许再次 trim。
    #[test]
    fn respects_cooldown_window() {
        let last = 10_000;
        assert_eq!(
            trim_target(2_000, 1_000, last, last + TRIM_COOLDOWN_MS - 1, 0),
            None,
            "距上次 trim 不足冷却期时必须跳过"
        );
        assert_eq!(
            trim_target(2_000, 1_000, last, last + TRIM_COOLDOWN_MS, 0),
            Some(900),
            "冷却到期后必须恢复 trim"
        );
    }

    /// `last_trim_ms == 0` 表示「从未 trim 过」，必须立即触发（启动后首次触顶）。
    #[test]
    fn zero_last_trim_means_immediate() {
        assert_eq!(trim_target(2_000, 1_000, 0, 0, 0), Some(900));
        // 时钟回拨（now < last）不应永久禁用 trim：饱和减法得 0 < 冷却期，
        // 行为上是「再等一个冷却周期」，不会 panic / 下溢。
        assert_eq!(trim_target(2_000, 1_000, 10_000, 9_000, 0), None);
    }

    /// 极小上限的边界：目标水位至少 1 行、且不超过上限本身。
    #[test]
    fn clamps_tiny_limits() {
        assert_eq!(trim_target(2, 1, 0, 0, 0), Some(1));
        assert_eq!(trim_target(1, 0, 0, 0, 0), Some(0));
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

    /// 测试辅助：按表内真实行数调用 `enforce_queue_limit_with`。
    ///
    /// 生产路径由 O(1) 内存计数器传入 total/unsent（P1-2），测试里没有计数器，
    /// 直接查一次表即可——被测的仍是淘汰逻辑本身。
    fn enforce(conn: &mut Connection, max_rows: i64) -> super::OverflowReport {
        let (total, unsent): (i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(in_flight = 0 AND delivered = 0), 0) FROM data_queue",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("count rows");
        SqliteStorage::enforce_queue_limit_with(conn, max_rows, total, unsent)
    }

    /// 修复(审查#6) 回归：已送达行堆积到超过上限时必须被回收。
    #[test]
    fn delivered_rows_are_pruned_by_total_row_count() {
        let mut conn = setup_conn();
        insert_rows(&conn, 10, 1);
        let report = enforce(&mut conn, 4);

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
        let report = enforce(&mut conn, 4);

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
        let report = enforce(&mut conn, 2);

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
        let report = enforce(&mut conn, 1);

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
        let report = enforce(&mut conn, 1000);

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

        let report = enforce(&mut conn, 5);

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
        let report = enforce(&mut conn, 10);
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
        let rows_total = AtomicI64::new(3);

        let deleted = SqliteStorage::cleanup_old_records(&mut conn, Some(1), &depth, &rows_total);

        assert_eq!(deleted, 3, "超期未送达行必须被回收");
        assert_eq!(row_count(&conn), 0);
        assert_eq!(
            rows_total.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "retention 删行必须同步扣减 rows_total，否则容量判定恒超限"
        );
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
        let rows_total = AtomicI64::new(2);

        let deleted = SqliteStorage::cleanup_old_records(&mut conn, Some(1), &depth, &rows_total);

        assert_eq!(deleted, 0, "在途行不能删");
        assert_eq!(row_count(&conn), 2);
        assert_eq!(
            rows_total.load(std::sync::atomic::Ordering::Relaxed),
            2,
            "没删行时 rows_total 不得变化"
        );
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
