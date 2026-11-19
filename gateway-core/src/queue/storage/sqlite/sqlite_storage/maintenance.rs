//! SQLite 队列的周期性维护流程（从 `sqlite_storage.rs` 抽出）。
//!
//! `maintenance()` 单方法承载 stale in_flight 清理、分钟聚合、WAL checkpoint、
//! retention 淘汰与定时备份五个阶段，几乎占满整个文件；抽出后读写主路径与
//! 维护路径可以各自演进。

use super::*;

/// 每次维护回收的空闲页数（4KB/页，256 页≈1MB）。
///
/// 分小批回收而不是一次 `VACUUM`：retention 每轮都会删行，空闲页是持续产生的，
/// 单次回收过多只会延长写锁持有时间，对采集入队没有收益。
const INCREMENTAL_VACUUM_PAGES: i64 = 256;

impl SqliteStorage {
    /// `QueueStorage::maintenance` 的实现体。
    pub(super) fn maintenance_impl(&self) -> GatewayResult<()> {
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
        //    - 保留天数固定 30 天（v2.1 起与 license 无关）；
        //    - 磁盘水位联动：Warn 收紧天数/上限，Critical 起清空聚合历史；
        //    - 行数上界按库所在分区容量自适应（小 eMMC 不再套用固定 500 万行）。
        if new_tick.is_multiple_of(120) {
            let days = *self.storage_retention_days.lock();
            let disk_level = self.metrics.disk_level_snapshot();
            // 容量探测放在取 conn 锁之前：statvfs 是系统调用，不应阻塞写路径。
            let disk_total = crate::engine::monitor::health::disk_total_bytes(
                self.db_path.to_str().unwrap_or_default(),
            )
            .unwrap_or(0);
            let mut guard = self.conn.lock();
            run_retention_cleanup(&mut guard, days, disk_level, disk_total);
        }

        // 5. 每 240 ticks (2 小时): 定时在线一致性备份
        // 缺陷 #8：旧实现 `checkpoint(TRUNCATE)` 在锁内、`fs::copy` 在锁外——释放锁后
        //   主循环继续写 WAL，autocheckpoint（每 1000 页）可改写主库页导致备份撕裂；
        //   fs::copy 还直接覆盖 .bak（非原子），中途崩溃留下半截文件，恢复路径用它
        //   覆盖主库会数据全损。
        // 改：全程持 conn 锁走 SQLite 在线备份 API，快照天然包含 WAL 已提交行且
        //   不受 autocheckpoint 干扰；先写同目录临时文件再原子 rename，.bak 永远是
        //   完整的一份。
        if new_tick.is_multiple_of(240) {
            let backup_path = self.db_path.with_extension("db.bak");
            let guard = self.conn.lock();
            match backup_db_online(&guard, &self.db_path, &backup_path, &self.key_hex) {
                Ok(()) => tracing::info!(
                    "periodic SQLite backup completed: {}",
                    backup_path.display()
                ),
                Err(e) => tracing::warn!("periodic SQLite backup failed: {}", e),
            }
        }

        // 6. 每次：cleanup_old_records（保留天数换算小时）+ WAL 大小监控
        let mut guard = self.conn.lock();
        let max_age_hours = self.storage_retention_days.lock().map(|d| d as u64 * 24);
        Self::cleanup_old_records(&mut guard, max_age_hours, &self.depth, &self.rows_total);
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
            // P1-2：维护路径同样改用内存计数器（O(1)），不再 `SELECT COUNT(*)`。
            let rows_total = self.rows_total.load(Ordering::Relaxed);
            let unsent = self.unsent_estimate();
            let report = Self::enforce_queue_limit(&mut guard, self.max_rows, rows_total, unsent);
            // P0-2：overflow 淘汰的未送达行需同步扣 depth（已送达行 ack 时已扣）。
            if report.undelivered_deleted > 0 {
                Self::saturating_dec(&self.depth, report.undelivered_deleted);
                update_queue_depth_metric(&self.depth, &self.metrics);
            }
            if report.deleted > 0 {
                Self::saturating_dec(&self.rows_total, report.deleted);
                Self::saturating_add(&self.evicted_total, report.deleted);
                Self::saturating_add(&self.evicted_undelivered_total, report.undelivered_deleted);
                self.metrics
                    .add_queue_evicted(report.deleted, report.undelivered_deleted);
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
}
