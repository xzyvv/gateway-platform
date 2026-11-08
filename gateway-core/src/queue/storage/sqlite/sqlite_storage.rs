//! SQLite 存储后端
//!
//! 封装现有 SQLite WAL 队列实现，通过 QueueStorage trait 暴露统一接口。
//! 内部维护 depth / in_flight 原子计数器和只读连接池。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::Arc;

use rusqlite::Connection;

use crate::engine::error::{GatewayError, GatewayResult};
use crate::engine::metrics::Metrics;
use crate::{engine::model::DataPoint, support::sqlite_maintenance::run_incremental_vacuum};

use super::db_init::init_queue_db;
use super::history::{aggregate_minute_windows, query_history_ex, query_raw_rows};
use super::read_pool::ReadPool;
use super::retention::run_retention_cleanup;
use super::wal::{check_wal_size, run_wal_checkpoint, wal_path_for};

use crate::queue::core::limit::{update_in_flight_metric, update_queue_depth_metric};
use crate::queue::core::{ensure_key, QueuedItem};
use crate::queue::storage::{HistoryQueryResult, QueueStorage};

/// in_flight 超时阈值（毫秒）：超过此时间未 ack 的 in_flight 记录自动释放
const INFLIGHT_TIMEOUT_MS: i64 = 5 * 60 * 1000;

/// 每次维护回收的空闲页数（4KB/页，256 页≈1MB）。
///
/// 分小批回收而不是一次 `VACUUM`：retention 每轮都会删行，空闲页是持续产生的，
/// 单次回收过多只会延长写锁持有时间，对采集入队没有收益。
const INCREMENTAL_VACUUM_PAGES: i64 = 256;

/// SQLite WAL 模式存储后端
pub struct SqliteStorage {
    /// 写连接（独占，通过 Mutex 保护）
    conn: Arc<parking_lot::Mutex<Connection>>,
    /// 只读连接池（WAL 模式下并发查询不阻塞写）
    read_pool: Arc<ReadPool>,
    /// 队列深度计数器（内存缓存，避免每次 COUNT(*)）
    depth: Arc<AtomicI64>,
    /// in_flight 计数器
    in_flight: Arc<AtomicI64>,
    /// 数据库文件路径（备份用）
    db_path: PathBuf,
    /// WAL 文件路径（大小监控用）
    wal_path: PathBuf,
    /// 维护 tick 计数器（每次 maintenance() 自增）
    maintenance_tick: AtomicU32,
    /// v2.0 Step 5: 历史数据保留天数（license storage_days 配额；None = 不限）。
    /// 启动默认 30 天（对齐旧硬编码行为），bootstrap 配额对账任务启动后立即覆盖。
    storage_retention_days: Arc<parking_lot::Mutex<Option<u32>>>,
    /// 1.2: 注入的指标实例（替代全局 metrics::get()）
    metrics: Arc<Metrics>,
    /// #20-2026：未送达行超限标志。被 `enforce_queue_limit` 在每次入队/维护后
    /// 根据 `OverflowReport::unsent_overflow` 更新；被 `QueueStorage::unsent_overflow`
    /// trait 方法暴露给 `DataQueue::enqueue` 入口判断走 fallback 路径。
    /// 恢复由 `maintenance` 周期再次检查后自动清除（unsent 比例降下来时）。
    unsent_overflow: Arc<AtomicBool>,
    /// 累计因容量上限被物理删除的行数（进程内累计，重启归零）。
    /// 与 `unsent_overflow` 的"当前是否超限"布尔不同，这是"历史上一共丢了多少"的
    /// 单调计数——用于告警（本次淘汰增量）与前端展示（累计淘汰量）。
    evicted_total: Arc<AtomicI64>,
    /// 上述淘汰中未送达（delivered=0）的行数，即彻底的计费/审计数据丢失。
    evicted_undelivered_total: Arc<AtomicI64>,
}

impl SqliteStorage {
    /// 创建并初始化 SQLite 存储
    pub fn new(db_path: &str, metrics: Arc<Metrics>) -> GatewayResult<Self> {
        let db_path = PathBuf::from(db_path);
        let backup_path = db_path.with_extension("db.bak");

        let key_hex = ensure_key(&db_path)?;
        let depth = Arc::new(AtomicI64::new(0));
        let conn = init_queue_db(&db_path, &backup_path, &depth, &key_hex, &metrics)?;
        let read_pool = Arc::new(ReadPool::new(4, &db_path, &key_hex));
        // r42-Q2: 用字符串后缀派生（"<db>-wal"）以严格匹配 SQLite 的 WAL 命名，
        // 而非 with_extension（对非 ".db" 结尾的 GATEWAY_DB_PATH 会算错）。
        let wal_path = wal_path_for(&db_path);

        let in_flight = Arc::new(AtomicI64::new(0));

        Ok(Self {
            conn: Arc::new(parking_lot::Mutex::new(conn)),
            read_pool,
            depth,
            in_flight,
            db_path,
            wal_path,
            maintenance_tick: AtomicU32::new(0),
            storage_retention_days: Arc::new(parking_lot::Mutex::new(Some(30))),
            metrics,
            unsent_overflow: Arc::new(AtomicBool::new(false)),
            evicted_total: Arc::new(AtomicI64::new(0)),
            evicted_undelivered_total: Arc::new(AtomicI64::new(0)),
        })
    }

    /// 内部方法：在已持有锁的情况下清理 stale in_flight
    fn cleanup_stale_in_flight_locked(&self, conn: &mut Connection, cutoff_ts: i64) -> usize {
        match conn.execute(
            // Bug 163: 按“出队时间”判超时——旧实现用采集时间戳 `timestamp`，当采集
            // 周期（如 30s）短于阈值（5min）时，积压队列中的旧行一旦出队就会被立刻
            // 判为 stale 重置，造成重复 dequeue / 重复上报、in_flight 计数失真。
            // 现以 dequeued_at 为基准；对历史无 dequeued_at 的行回退到 timestamp。
            "UPDATE data_queue SET in_flight = 0 \
             WHERE in_flight = 1 AND COALESCE(NULLIF(dequeued_at, 0), timestamp) < ?1",
            rusqlite::params![cutoff_ts],
        ) {
            Ok(released) => {
                if released > 0 {
                    Self::saturating_dec(&self.in_flight, released as i64);
                }
                released
            }
            Err(e) => {
                tracing::warn!("cleanup stale in_flight failed: {}", e);
                0
            }
        }
    }

    /// P2-c: 原子计数器「饱和递减」——杜绝计数器越减为负。
    ///
    /// `fetch_sub` 在缺少相应 `fetch_add` 时会漂移到负值：典型场景是进程
    /// 重启后内存计数从 0 起步，而 DB 里上一生命周期残留的 `in_flight = 1`
    /// 行被 `cleanup_stale_in_flight` 一次性扣减（`released > 计数器现值`）→
    /// 计数器变负。读取端的 `.max(0)` 只掩盖症状，负漂移会持续低估真实
    /// 深度/在飞数，使队列上限判定与健康指标失真。
    ///
    /// 本结构内所有计数增减都在 `conn` 锁内串行执行，故 read-modify-write 精确。
    pub(super) fn saturating_dec(counter: &AtomicI64, n: i64) {
        if n <= 0 {
            return;
        }
        let cur = counter.load(Ordering::Relaxed);
        counter.store(cur.saturating_sub(n).max(0), Ordering::Relaxed);
    }

    /// 单调累加计数器（淘汰行数等只增不减的审计量）。
    /// 用 `fetch_add` 而非 load/store：淘汰可能发生在入队与维护两条路径上，
    /// 非原子的读改写会丢增量，导致"到底丢了多少"统计偏小。
    pub(super) fn saturating_add(counter: &AtomicI64, n: i64) {
        if n <= 0 {
            return;
        }
        counter.fetch_add(n, Ordering::Relaxed);
    }
}

impl QueueStorage for SqliteStorage {
    fn enqueue_batch(&self, points: &[DataPoint]) -> GatewayResult<usize> {
        if points.is_empty() {
            return Ok(0);
        }
        // Bug 81 入队前清理 stale in_flight（1 小时超时，与原实现一致）
        let cutoff = DataPoint::now() - 3600 * 1000;
        let mut guard = self.conn.lock();
        let conn = &mut *guard;
        self.cleanup_stale_in_flight_locked(conn, cutoff);
        // C4: 按实际插入行数累加 depth，避免坏点被跳过后计数虚高
        //（depth 现仅用于队列深度指标与 len()；超限淘汰已改用表内真实总行数判定）
        let inserted = Self::enqueue_batch(conn, points)?;
        if inserted > 0 {
            self.depth.fetch_add(inserted as i64, Ordering::Relaxed);
        }
        let report = Self::enforce_queue_limit(conn);
        // P0-2：overflow 现在也会淘汰**未送达**（delivered=0）行，这些行仍计入
        // depth，必须同步扣减；已送达行在 ack 路径已各自扣过，不再重复扣。
        if report.undelivered_deleted > 0 {
            Self::saturating_dec(&self.depth, report.undelivered_deleted);
        }
        if report.deleted > 0 {
            // 淘汰计数用于审计"丢了多少最旧数据"；warn 级细节已在 enforce_queue_limit 内输出
            tracing::debug!(
                "queue limit: trimmed {} rows ({} undelivered, total now {})",
                report.deleted,
                report.undelivered_deleted,
                report.total
            );
            // 单机网关无异地副本：这些行是**物理删除且不可恢复**的。
            // 只打 debug 日志（默认不输出）等于静默丢数据，因此必须同时
            // ① 累加进程内单调计数 → 供告警取增量、供前端展示累计值；
            // ② 累加 Prometheus 计数器 → 供运维配告警规则。
            Self::saturating_add(&self.evicted_total, report.deleted);
            Self::saturating_add(&self.evicted_undelivered_total, report.undelivered_deleted);
            self.metrics
                .add_queue_evicted(report.deleted, report.undelivered_deleted);
        }
        // #20-2026：把 OverflowReport 翻译为 unsent_overflow AtomicBool。
        // 这里只更新"硬超限"信号，DataQueue::enqueue 入口的 fallback 决策
        // 由 unsent_overflow() trait 方法在每次 enqueue 时实时读取。
        let prev = self
            .unsent_overflow
            .swap(report.unsent_overflow, Ordering::Relaxed);
        if report.unsent_overflow && !prev {
            tracing::error!(
                "unsent overflow: total={} unsent={} ratio={:.2} >= {:.2} threshold; DataQueue enqueue will route to fallback",
                report.total, report.unsent, report.ratio, super::limit::UNSENT_OVERFLOW_RATIO
            );
        } else if !report.unsent_overflow && prev {
            tracing::info!(
                "unsent overflow recovered: total={} unsent={} ratio={:.2}",
                report.total,
                report.unsent,
                report.ratio
            );
        }
        update_queue_depth_metric(&self.depth, &self.metrics);
        Ok(inserted)
    }

    fn dequeue_batch(&self, limit: usize) -> GatewayResult<Vec<QueuedItem>> {
        let mut guard = self.conn.lock();
        let items = Self::dequeue_batch(&mut guard, limit)?;
        if !items.is_empty() {
            self.in_flight
                .fetch_add(items.len() as i64, Ordering::Relaxed);
            update_in_flight_metric(&self.in_flight, &self.metrics);
        }
        Ok(items)
    }

    fn ack_batch(&self, ids: &[i64]) -> GatewayResult<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut guard = self.conn.lock();
        match Self::ack_batch(&mut guard, ids) {
            // A1：按**实际状态跃迁行数**扣减，不再按 ids.len() 扣减。
            // 旧实现按 ids.len() 扣 → 重复 ack（publish 通道 + outbox replay 通道
            // 两条独立 ack 路径可能对同一批 row id 各 ack 一次）或 ids 含已被
            // retention/容量淘汰删除的行时，内存计数被多扣 → depth 漂移
            // （队列深度虚低、健康判据与反压阈值失真，且无任何告警）。
            Ok(outcome) => {
                Self::saturating_dec(&self.depth, outcome.delivered);
                Self::saturating_dec(&self.in_flight, outcome.released);
                update_queue_depth_metric(&self.depth, &self.metrics);
                update_in_flight_metric(&self.in_flight, &self.metrics);
                Ok(())
            }
            Err(e) => {
                tracing::warn!("ack batch failed, releasing in_flight: {}", e);
                // r15-Q4 (fix): 按**实际复位行数**扣减计数；失败则保留计数
                // （DB 里那些行仍是 in_flight=1，扣了就是低估）。
                match Self::release_in_flight(&mut guard, ids) {
                    Ok(released) => {
                        Self::saturating_dec(&self.in_flight, released as i64);
                    }
                    Err(release_err) => {
                        tracing::error!(
                            "release_in_flight after ack failure FAILED: {} — in_flight counter kept at {}; \
                             the affected rows stay in_flight=1 in the DB and will be recovered by the stale sweep",
                            release_err,
                            self.in_flight.load(Ordering::Relaxed)
                        );
                    }
                }
                update_in_flight_metric(&self.in_flight, &self.metrics);
                Err(GatewayError::Queue(format!("ack batch failed: {}", e)))
            }
        }
    }

    fn release_in_flight(&self, ids: &[i64]) -> GatewayResult<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut guard = self.conn.lock();
        // r15-Q4 (fix): 按实际复位行数扣减；失败上抛，调用方能感知。
        let released = Self::release_in_flight(&mut guard, ids)?;
        Self::saturating_dec(&self.in_flight, released as i64);
        update_in_flight_metric(&self.in_flight, &self.metrics);
        Ok(())
    }

    fn cleanup_stale_in_flight(&self, cutoff_ts: i64) -> GatewayResult<usize> {
        let mut guard = self.conn.lock();
        let released = self.cleanup_stale_in_flight_locked(&mut guard, cutoff_ts);
        if released > 0 {
            update_in_flight_metric(&self.in_flight, &self.metrics);
        }
        Ok(released)
    }

    fn len(&self) -> usize {
        self.depth.load(Ordering::Relaxed).max(0) as usize
    }

    fn in_flight_count(&self) -> i64 {
        self.in_flight.load(Ordering::Relaxed).max(0)
    }

    fn evicted_stats(&self) -> (i64, i64) {
        let total = self.evicted_total.load(Ordering::Relaxed).max(0);
        let undelivered = self
            .evicted_undelivered_total
            .load(Ordering::Relaxed)
            .max(0);
        (total, undelivered)
    }

    fn history_query(
        &self,
        point_id: &str,
        start: i64,
        end: i64,
        limit: usize,
    ) -> GatewayResult<Vec<QueuedItem>> {
        // 优先从只读池取连接，池空则降级到写连接。
        // 查询失败必须上抛：历史上用 unwrap_or_default 会把 DB 错误伪装成"无数据"，
        // 导致 /api/history 返回空数组而非报错，运维无法区分"没有数据"与"查询失败"。
        if let Some(rc) = self.read_pool.get() {
            let result = query_raw_rows(&rc, point_id, start, end, limit);
            self.read_pool.release(rc);
            Ok(result?)
        } else {
            let guard = self.conn.lock();
            Ok(query_raw_rows(&guard, point_id, start, end, limit)?)
        }
    }

    fn history_query_ex(
        &self,
        point_id: &str,
        start: i64,
        end: i64,
        limit: usize,
        bucket_secs: Option<u32>,
    ) -> GatewayResult<HistoryQueryResult> {
        // P2-1：带降采样与分页元信息的历史查询。与 history_query 同样优先走
        // 只读池；错误必须上抛（不能把 DB 故障伪装成"没有数据"）。
        let now = DataPoint::now();
        if let Some(rc) = self.read_pool.get() {
            let result = query_history_ex(&rc, point_id, start, end, limit, bucket_secs, now);
            self.read_pool.release(rc);
            Ok(result?)
        } else {
            let guard = self.conn.lock();
            Ok(query_history_ex(
                &guard,
                point_id,
                start,
                end,
                limit,
                bucket_secs,
                now,
            )?)
        }
    }

    fn maintenance(&self) -> GatewayResult<()> {
        let tick = self.maintenance_tick.fetch_add(1, Ordering::Relaxed);
        let new_tick = tick.wrapping_add(1);

        // 1. 每次：清理 stale in_flight（5 分钟超时）
        let cutoff = DataPoint::now() - INFLIGHT_TIMEOUT_MS;
        let mut guard = self.conn.lock();
        let conn = &mut *guard;
        self.cleanup_stale_in_flight_locked(conn, cutoff);
        drop(guard);

        // 2. 每 2 ticks (60 秒)：分钟窗口聚合（P2-3）
        //
        // 把 24 小时之前的原始行压成分钟桶写入 data_queue_aggregated，
        // 使「近 7/30 天」这类长窗口查询不必扫原始大表；24 小时以内保持原始精度。
        if new_tick.is_multiple_of(2) {
            let guard = self.conn.lock();
            match aggregate_minute_windows(&guard, DataPoint::now()) {
                Ok(0) => {}
                Ok(n) => tracing::debug!("aggregate_minute_windows: {} 个分钟桶已写入", n),
                Err(e) => tracing::warn!("aggregate_minute_windows failed: {}", e),
            }
        }

        // 3. 每 20 ticks (10 分钟): WAL checkpoint + 空闲页回收
        if new_tick.is_multiple_of(20) {
            let mut guard = self.conn.lock();
            // S01: 透传 metrics，使 wal_checkpoint_total 指标按 reason 累计（schedule vs wal_overflow）
            run_wal_checkpoint(&mut guard, "schedule", Some(&self.metrics));
            Self::wal_checkpoint_truncate(&mut guard);
            // auto_vacuum=INCREMENTAL 才生效：把前面各步删除释放的空闲页
            // 分小批还给文件系统，避免主库文件只增不减。
            run_incremental_vacuum(&guard, INCREMENTAL_VACUUM_PAGES, "队列主库");
        }

        // 4. 每 120 ticks (1 小时): retention cleanup
        //    - 保留天数由 license storage_days 配额驱动；
        //    - 磁盘水位联动：Warn 收紧天数/上限，Critical 起清空聚合历史；
        //    - 行数上界按库所在分区容量自适应（小 eMMC 不再套用固定 500 万行）。
        if new_tick.is_multiple_of(120) {
            let days = *self.storage_retention_days.lock();
            let disk_level = self.metrics.disk_level.load(Ordering::Relaxed);
            // 容量探测放在取 conn 锁之前：statvfs 是系统调用，不应阻塞写路径。
            let disk_total = crate::engine::monitor::health::disk_total_bytes(
                self.db_path.to_str().unwrap_or_default(),
            )
            .unwrap_or(0);
            let mut guard = self.conn.lock();
            run_retention_cleanup(&mut guard, days, disk_level, disk_total);
        }

        // 5. 每 240 ticks (2 小时): 定时备份
        // #2 修复：原 `std::fs::copy(self.db_path, backup_path)` 三大问题
        //   1. 不含 WAL 中已 commit 但未 checkpoint 的行——备份"看起来完整"但
        //      恢复时丢近期数据
        //   2. 没先 `wal_checkpoint(TRUNCATE)`：备份过程中 auto-checkpoint 触发
        //      会改写主库页状态，备份文件可能撕裂
        //   3. 没持 conn 锁：复制期间 SQLite 可能正在 commit/rollback，导致
        //      备份中混有不同事务的页
        // 修：先在 conn 锁内 `wal_checkpoint(TRUNCATE)` 把 WAL 全部刷回主库，
        // 释放锁后再 fs::copy（复制期间 SQLite 仍能正常读写主库，因主库页已
        // 完整且 fs::copy 是 page-aligned 顺序读）。VACUUM INTO 虽更优雅但
        // 会整体重写主库（嵌入式 ARM 数百 ms 锁表），对在跑采集的网关过重；
        // checkpoint + fs::copy 是 SQLite 官方推荐的"在线热备份"组合。
        if new_tick.is_multiple_of(240) {
            let backup_path = self.db_path.with_extension("db.bak");
            // 5a: checkpoint TRUNCATE（WAL → 主库同步写回）
            {
                let mut guard = self.conn.lock();
                run_wal_checkpoint(&mut guard, "periodic_backup", Some(&self.metrics));
            }
            // 5b: 复制主库。checkpoint 已完成，复制窗口内新写入只会进 WAL
            // （下次启动自动 replay），不会撕裂主库页
            match std::fs::copy(&self.db_path, &backup_path) {
                Ok(n) => tracing::info!(
                    "periodic SQLite backup completed: {} bytes -> {}",
                    n,
                    backup_path.display()
                ),
                Err(e) => tracing::warn!("periodic SQLite backup failed: {}", e),
            }
        }

        // 6. 每次：cleanup_old_records（storage_days 配额换算小时）+ WAL 大小监控
        let mut guard = self.conn.lock();
        let max_age_hours = self.storage_retention_days.lock().map(|d| d as u64 * 24);
        Self::cleanup_old_records(&mut guard, max_age_hours, &self.depth);
        // Bug 184 (N05): 注入 metrics，让 WAL 监控能累计 `wal_size_warn_total` 指标
        // （50MB 早期预警 + 100MB 硬告警都使用同一计数器）。
        check_wal_size(&mut guard, &self.wal_path, Some(&self.metrics));
        // 主库文件大小 gauge：与 WAL 大小一起反映磁盘占用增长趋势。
        let db_size = std::fs::metadata(&self.db_path)
            .map(|m| m.len())
            .unwrap_or(0);
        self.metrics
            .queue_persist
            .db_size_bytes
            .store(db_size, Ordering::Relaxed);
        update_queue_depth_metric(&self.depth, &self.metrics);

        // #20-2026：maintenance 周期重新检查 unsent_overflow 标志。
        // 场景：broker 长期不可用时 retention 持续清 delivered=1 行，但
        // unsent=0 行（仍 in_flight=1）逐渐退出 → unsent 比例上升。
        // 每 30s 维护 tick 跑一次让未送达比例触发的阈值能及时被上层观察到。
        //
        // 死锁修复：此处直接复用上面已持有的 `guard`，不再 `self.conn.lock()`。
        // `parking_lot::Mutex` 不可重入，而 `let mut guard = self.conn.lock();`
        // 对同名变量只是「遮蔽」——被遮蔽的绑定不会提前 drop（仍存活至函数作用域
        // 结束），于是二次加锁必然自锁死。表现为调用 `maintenance()` 的单测永久挂起。
        {
            let report = Self::enforce_queue_limit(&mut guard);
            // P0-2：overflow 淘汰的未送达行需同步扣 depth（已送达行 ack 时已扣）。
            if report.undelivered_deleted > 0 {
                Self::saturating_dec(&self.depth, report.undelivered_deleted);
                update_queue_depth_metric(&self.depth, &self.metrics);
            }
            let prev = self
                .unsent_overflow
                .swap(report.unsent_overflow, Ordering::Relaxed);
            if report.unsent_overflow && !prev {
                tracing::error!(
                    "maintenance: unsent overflow detected, total={} unsent={} ratio={:.2}; DataQueue enqueue will route to fallback",
                    report.total, report.unsent, report.ratio
                );
            } else if !report.unsent_overflow && prev {
                tracing::info!(
                    "maintenance: unsent overflow recovered, total={} unsent={} ratio={:.2}",
                    report.total,
                    report.unsent,
                    report.ratio
                );
            }
        }

        Ok(())
    }

    fn needs_maintenance(&self) -> bool {
        true
    }

    fn set_storage_retention_days(&self, days: Option<u32>) {
        let prev = *self.storage_retention_days.lock();
        if prev != days {
            *self.storage_retention_days.lock() = days;
            tracing::info!(
                "storage retention days updated: {:?} -> {:?} (license storage_days quota)",
                prev,
                days
            );
        }
    }

    fn oldest_data_timestamp(&self) -> Option<i64> {
        // 原始表与聚合表都要看：超过 24h 的原始行会被压成分钟桶搬进聚合表，
        // 只看 data_queue 会把已保留的长历史误判为"没有历史"。
        // 失败时降级为 None（用量按 0 计），不能因统计失败而阻断接口。
        let sql = "SELECT MIN(t) FROM (SELECT MIN(timestamp) AS t FROM data_queue \
                   UNION ALL SELECT MIN(window_start) AS t FROM data_queue_aggregated)";
        let run = |conn: &Connection| -> rusqlite::Result<Option<i64>> {
            conn.query_row(sql, [], |r| r.get::<_, Option<i64>>(0))
        };
        let res = if let Some(rc) = self.read_pool.get() {
            let out = run(&rc);
            self.read_pool.release(rc);
            out
        } else {
            let guard = self.conn.lock();
            run(&guard)
        };
        match res {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("oldest_data_timestamp query failed: {}", e);
                None
            }
        }
    }

    fn name(&self) -> &str {
        "sqlite"
    }

    fn unsent_overflow(&self) -> bool {
        self.unsent_overflow.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod saturating_dec_tests {
    use super::SqliteStorage;
    use std::sync::atomic::{AtomicI64, Ordering};

    /// P2-c 回归：计数器饱和递减，绝不越减为负。
    #[test]
    fn dec_never_goes_negative() {
        let c = AtomicI64::new(0);
        // 计数器为 0 时扣减不应变负
        SqliteStorage::saturating_dec(&c, 10);
        assert_eq!(c.load(Ordering::Relaxed), 0);

        c.store(5, Ordering::Relaxed);
        SqliteStorage::saturating_dec(&c, 3);
        assert_eq!(c.load(Ordering::Relaxed), 2);
        SqliteStorage::saturating_dec(&c, 100);
        assert_eq!(c.load(Ordering::Relaxed), 0);

        // n <= 0 时为 no-op
        c.store(7, Ordering::Relaxed);
        SqliteStorage::saturating_dec(&c, 0);
        assert_eq!(c.load(Ordering::Relaxed), 7);
    }
}

/// v2.0 配额用量口径：`oldest_data_timestamp` 必须同时看原始表与聚合表。
///
/// 超过 24h 的原始行会被压成分钟桶写入 `data_queue_aggregated`；只看原始表会
/// 把"已保留的长历史"误判为"没有历史"，导致存储天数用量被低估。
#[cfg(test)]
mod oldest_ts_tests {
    use super::SqliteStorage;
    use crate::engine::metrics::Metrics;
    use crate::queue::storage::QueueStorage;
    use std::sync::Arc;

    #[test]
    fn includes_aggregated_table() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("queue.db");
        let storage =
            SqliteStorage::new(db_path.to_str().unwrap(), Arc::new(Metrics::new())).unwrap();

        let old_ts = 1_700_000_000_000i64;
        // 只往聚合表写一行（模拟原始行已被 retention 清理、只剩分钟桶的历史）
        {
            let guard = storage.conn.lock();
            guard
                .execute(
                    "INSERT INTO data_queue_aggregated \
                         (point_id, window_start, avg_value, min_value, max_value, sample_count) \
                     VALUES (?1, ?2, 1.0, 1.0, 1.0, 1)",
                    rusqlite::params!["p-agg", old_ts],
                )
                .unwrap();
        }

        assert_eq!(
            storage.oldest_data_timestamp(),
            Some(old_ts),
            "聚合表里的历史也必须计入保留跨度"
        );
    }
}
