//! MQTT QoS1 持久化 Outbox（T2.2）
//!
//! 解决问题：当前 MQTT 投递在网络抖动 / broker 重启场景下，
//! 未确认消息可能在 in_flight 超时后丢失。
//!
//! 设计：
//! - 出站消息先写 outbox 表（同步持久化），再投递
//! - 收到 PUBACK → mark_acked → 定期清理
//! - 进程启动时 replay_pending → 重发未确认消息
//! - 与现有 DataQueue.in_flight 集成：outbox_id = DataPoint.id
//!
//! 表结构：
//! ```sql
//! CREATE TABLE mqtt_outbox (
//!     id INTEGER PRIMARY KEY,
//!     data_id INTEGER NOT NULL,      -- 关联 DataPoint.id
//!     topic TEXT NOT NULL,
//!     payload BLOB NOT NULL,
//!     qos INTEGER NOT NULL,           -- 1
//!     created_at INTEGER NOT NULL,    -- ms epoch
//!     last_attempt_at INTEGER,        -- 最近一次发送时间
//!     attempt_count INTEGER NOT NULL DEFAULT 0,
//!     status INTEGER NOT NULL         -- 0=pending, 1=acked
//! );
//! ```

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};

use crate::engine::error::{GatewayError, GatewayResult};

/// 待发送消息
#[derive(Debug, Clone)]
pub struct OutgoingMsg {
    pub data_id: i64,
    pub data_ids: Vec<i64>,
    /// MQTT: topic; HTTP: target URL
    pub topic: String,
    pub payload: Vec<u8>,
    pub qos: u8,
    /// 协议标识："mqtt" 或 "http"
    pub protocol: String,
}

/// Outbox 存储模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboxMode {
    /// 持久化到磁盘文件（正常模式）
    Sqlite,
    /// Bug-N1 (fix)：降级到 `:memory:` 仅内存模式。
    ///
    /// 由 `OutboxStore::open` 在 sqlite 文件 open / 创建表失败时自动启用，
    /// 进程崩溃时内存 outbox 消息全部丢失——但 MQTT / Webhook / Kafka 仍能
    /// 上报当前进程采集到的数据，不会"全栈坍塌"。
    Memory,
}

/// Outbox 存储
pub struct OutboxStore {
    conn: Connection,
    mode: OutboxMode,
}

impl OutboxStore {
    /// 打开（自动建表）。
    ///
    /// Bug-N1 (fix)：
    ///
    /// 旧实现 open 失败时直接返回 `Err`——上层 bootstrap 收到错误后**禁用整个北向通道**
    /// （warn + NorthConnector::register 不调用），所有数据上报停止。工业现场若
    /// outbox 文件被运维误删 / 目录权限异常 / 磁盘临时满，会导致**整个北向永久瘫痪**
    /// 直到运维介入修复文件系统问题。
    ///
    /// 修复策略：open 失败 → 降级到 SQLite **内存模式** (`:memory:`)，让北向通道继续工作。
    /// 代价：进程崩溃时内存 outbox 消息全部丢失——但网关自身已经在崩溃，丢失的消息
    /// 本来也无法投递；正常运行时（不崩溃）持久化能力由下层 sqlite 保留。
    ///
    /// 仍然递增 metrics（`metrics.outbox_degraded_total`），Grafana 告警
    /// `rate(gateway_outbox_degraded_total[5m]) > 0` 可报"outbox 异常降级到内存"。
    pub fn open(db_path: &Path) -> GatewayResult<Self> {
        match Self::open_sqlite(db_path) {
            Ok(store) => Ok(store),
            Err(e) => {
                let reason = e.to_string();
                tracing::error!(
                    "outbox: 持久化模式 open 失败，降级到内存模式 — db_path={:?} reason={}",
                    db_path,
                    reason
                );
                let conn = Connection::open_in_memory().map_err(|e2| {
                    GatewayError::Io(format!(
                        "outbox open_in_memory failed after sqlite open failed: {} (original: {})",
                        e2, reason
                    ))
                })?;
                let mut store = Self {
                    conn,
                    mode: OutboxMode::Memory,
                };
                // 初始化内存库的 schema（与文件模式相同）
                store
                    .init_schema()
                    .map_err(|e3| GatewayError::Io(format!("outbox memory init schema: {}", e3)))?;
                Ok(store)
            }
        }
    }

    /// 当前模式（用于诊断 / metrics / 上层告警）
    pub fn mode(&self) -> OutboxMode {
        self.mode
    }

    /// 是否处于降级模式（启动时文件 open 失败）
    pub fn is_degraded(&self) -> bool {
        matches!(self.mode, OutboxMode::Memory)
    }

    /// 实际打开 sqlite 文件 + 初始化 schema（失败返回 Err 触发降级）
    fn open_sqlite(db_path: &Path) -> GatewayResult<Self> {
        let conn = Connection::open(db_path)
            .map_err(|e| GatewayError::Io(format!("open outbox db: {}", e)))?;
        let mut store = Self {
            conn,
            mode: OutboxMode::Sqlite,
        };
        store.init_schema()?;
        Ok(store)
    }

    /// 执行 schema DDL（PRAGMA + CREATE TABLE/INDEX）。文件模式与内存模式共用。
    fn init_schema(&mut self) -> GatewayResult<()> {
        self.conn
            .execute_batch(
                // Bug C18: MQTT 与 Webhook 共享同一 outbox 库文件，bootstrap 会各自
                // `open()` 一个独立连接并发写。WAL 模式下写者互斥，未设置 busy_timeout
                // 时遇到 SQLITE_BUSY 会立即返回错误（默认 0ms），导致 outbox persist
                // 偶发失败、消息被丢弃。设置 5s 退避重试，让并发写自动排队。
                "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA busy_timeout=5000;
             CREATE TABLE IF NOT EXISTS mqtt_outbox (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 data_id INTEGER NOT NULL,
                 data_ids BLOB NOT NULL,
                 topic TEXT NOT NULL,
                 payload BLOB NOT NULL,
                 qos INTEGER NOT NULL,
                 created_at INTEGER NOT NULL,
                 last_attempt_at INTEGER,
                 attempt_count INTEGER NOT NULL DEFAULT 0,
                 status INTEGER NOT NULL DEFAULT 0,
                 protocol TEXT NOT NULL DEFAULT 'mqtt'
             );
             CREATE INDEX IF NOT EXISTS idx_outbox_status ON mqtt_outbox(status, id);
             -- 幂等约束：同一 data_id 的待发消息不允许重复写入（防止重复上报）
             CREATE UNIQUE INDEX IF NOT EXISTS idx_outbox_pending_data_id
                 ON mqtt_outbox(data_id) WHERE status = 0;
             -- r67-OBX1：replay 游标持久化。
             --
             -- replay_pending_outbox 的 after_id 此前是函数内的局部 i64，
             -- 每次调用从 0 开始重扫整张表。play 走完一页被 break 后再次调用
             -- 时，已经成功 mark_acked 的行被 status=0 过滤掉、未处理的行被
             -- 全表重扫定位——浪费 SQL 且对 in-flight 行存在『已 publish 但未
             -- 收到 PubAck』再次 republish 到 broker 的隐患。改为把已经安全
             -- 推进到的 after_id 持久化到本表，下次调用从该值起扫。
             --
             -- 表设计：单行 (protocol, after_id)——同一 outbox 库被 MQTT /
             -- Webhook / Kafka 共享（用 protocol 列隔离），每条协议独立维护
             -- 各自的游标，互不干扰。
             CREATE TABLE IF NOT EXISTS outbox_replay_cursor (
                 protocol TEXT PRIMARY KEY,
                 after_id INTEGER NOT NULL DEFAULT 0
             );",
            )
            .map_err(|e| GatewayError::Io(format!("init outbox schema: {}", e)))?;
        // S13: 兼容旧库——添加 protocol 列。SQLite 在列已存在时返回 "duplicate column name" 错误，
        // 这是预期场景；其他错误需告警，不应静默吞掉掩盖真实故障。
        if let Err(e) = self.conn.execute(
            "ALTER TABLE mqtt_outbox ADD COLUMN protocol TEXT NOT NULL DEFAULT 'mqtt'",
            [],
        ) {
            let msg = e.to_string();
            if !msg.contains("duplicate column") {
                tracing::warn!("outbox ALTER TABLE protocol column failed: {}", msg);
            }
        }
        Ok(())
    }

    /// 持久化待发送消息（写入即返回 outbox_id）
    pub fn persist(&mut self, msg: &OutgoingMsg) -> GatewayResult<i64> {
        let now = now_ms();
        let data_ids_blob = encode_i64_list(&msg.data_ids);
        let affected = self.conn
            .execute(
                "INSERT OR IGNORE INTO mqtt_outbox (data_id, data_ids, topic, payload, qos, created_at, status, protocol)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7)",
                params![msg.data_id, data_ids_blob, msg.topic, msg.payload, msg.qos, now, msg.protocol],
            )
            .map_err(|e| GatewayError::Queue(format!("outbox persist: {}", e)))?;
        // affected == 0 表示 data_id 已存在（幂等跳过），返回 0 表示未插入新行
        if affected == 0 {
            return Ok(0);
        }
        Ok(self.conn.last_insert_rowid())
    }

    /// 批量持久化待发送消息（单事务多值 INSERT，减少事务开销）
    pub fn persist_batch(&mut self, msgs: &[OutgoingMsg]) -> GatewayResult<Vec<Option<i64>>> {
        if msgs.is_empty() {
            return Ok(Vec::new());
        }
        let now = now_ms();
        let tx = self
            .conn
            .transaction()
            .map_err(|e| GatewayError::Queue(format!("outbox batch tx: {}", e)))?;

        let mut ids: Vec<Option<i64>> = Vec::with_capacity(msgs.len());

        // Bug 55 修复：INSERT OR IGNORE 遇到已存在 data_id 会跳过，rowid 不连续。
        // 原实现 last_id - len + 1 推断 ID 会与实际 INSERT 出的 ID 错位。
        // 改为单条 INSERT OR IGNORE 逐条执行，仅 last_insert_rowid() == 0 时表示 IGNORE 跳过。
        let mut stmt = tx.prepare_cached(
            "INSERT OR IGNORE INTO mqtt_outbox (data_id, data_ids, topic, payload, qos, created_at, status, protocol)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7)",
        ).map_err(|e| GatewayError::Queue(format!("outbox batch prepare: {}", e)))?;
        for msg in msgs {
            match stmt.execute(params![
                msg.data_id,
                encode_i64_list(&msg.data_ids),
                msg.topic,
                msg.payload,
                msg.qos,
                now,
                msg.protocol,
            ]) {
                Ok(affected) => {
                    if affected == 0 {
                        // IGNORE 跳过：data_id 已有 pending 记录，幂等返回 None
                        ids.push(None);
                    } else {
                        ids.push(Some(tx.last_insert_rowid()));
                    }
                }
                Err(e) => {
                    tracing::warn!("outbox batch persist single failed: {}", e);
                    ids.push(None);
                }
            }
        }
        // Bug 55 后续修复：stmt 必须先 drop 才能 commit tx（rusqlite 要求）
        drop(stmt);
        tx.commit()
            .map_err(|e| GatewayError::Queue(format!("outbox batch commit: {}", e)))?;
        Ok(ids)
    }

    /// 标记已确认
    pub fn mark_acked(&mut self, outbox_id: i64) -> GatewayResult<()> {
        self.conn
            .execute(
                "UPDATE mqtt_outbox SET status = 1 WHERE id = ?1",
                params![outbox_id],
            )
            .map_err(|e| GatewayError::Queue(format!("outbox mark_acked: {}", e)))?;
        Ok(())
    }

    /// #9: 物理删除一条 outbox 记录。
    ///
    /// 用于 publish 失败路径（ack 通道关闭 / PubAck 超时 / publish 调用本身失败）：
    /// 这些路径已把队列 ids 通过 `queue.release` 退回（数据仍在 SQLite 队列中可被再次 dequeue），
    /// 如果 outbox 里还留着这条 pending 记录，重连后 `replay_pending` 会再发一次 →
    /// 与队列重新 dequeue 出来的同一批数据形成重复上报。
    ///
    /// 因此失败时统一清理 outbox+queue：outbox 物理删除，queue 仅 release（保留数据）。
    /// 这样只有"队列重新 dequeue → 重新 publish"这一条重发路径，无重复。
    /// 重启期间的健壮性由队列自身的 SQLite 持久化保证（release 后行仍在 data_queue 表中）。
    pub fn remove(&mut self, outbox_id: i64) -> GatewayResult<()> {
        self.conn
            .execute("DELETE FROM mqtt_outbox WHERE id = ?1", params![outbox_id])
            .map_err(|e| GatewayError::Queue(format!("outbox remove: {}", e)))?;
        Ok(())
    }

    /// P3: 按 data_id 标记已确认（异步持久化路径使用）
    ///
    /// r78-OBX1：增加 `protocol` 必传参数，SQL 加 `AND protocol = ?2` 过滤。
    /// 旧实现 `WHERE data_id=?1 AND status=0` **无协议过滤**——双协议（mqtt/http）
    /// 共享 outbox 表的架构下，理论可能跨协议相同 data_id（queue id 不按协议隔离），
    /// 旧行为会**误删另一个协议的 outbox 行**（HTTP 协议的唯一持久副本）→
    /// 永久数据丢失。这是单点脆弱性：现在正常路径不会触发，但任何将来的重构
    /// （如共享 queue id 空间、批量调度跨协议）都会成为 silent data loss 入口。
    pub fn mark_acked_by_data_id(&mut self, data_id: i64, protocol: &str) -> GatewayResult<()> {
        self.conn
            .execute(
                "UPDATE mqtt_outbox SET status = 1 WHERE data_id = ?1 AND status = 0 AND protocol = ?2",
                params![data_id, protocol],
            )
            .map_err(|e| {
                GatewayError::Queue(format!("outbox mark_acked_by_data_id: {}", e))
            })?;
        Ok(())
    }

    /// P3: 按 data_id 物理删除（异步持久化失败路径使用）
    ///
    /// r78-OBX1：与 mark_acked_by_data_id 同型——加 `protocol` 必传参数，
    /// 避免误删另一协议 outbox 行（HTTP 消息的唯一持久副本）。
    pub fn remove_by_data_id(&mut self, data_id: i64, protocol: &str) -> GatewayResult<()> {
        self.conn
            .execute(
                "DELETE FROM mqtt_outbox WHERE data_id = ?1 AND status = 0 AND protocol = ?2",
                params![data_id, protocol],
            )
            .map_err(|e| GatewayError::Queue(format!("outbox remove_by_data_id: {}", e)))?;
        Ok(())
    }

    /// 记录一次尝试发送
    pub fn record_attempt(&mut self, outbox_id: i64) -> GatewayResult<()> {
        let now = now_ms();
        self.conn
            .execute(
                "UPDATE mqtt_outbox SET last_attempt_at = ?1, attempt_count = attempt_count + 1
                 WHERE id = ?2",
                params![now, outbox_id],
            )
            .map_err(|e| GatewayError::Queue(format!("outbox record_attempt: {}", e)))?;
        Ok(())
    }

    /// r51-OBX1：删除指定协议中、已超过尝试上限的 pending 行。
    ///
    /// 用作 webhook / 其它持久重试队列的「持久化重试上限」：进程重启后
    /// in-memory `RetryState` 丢失，单纯靠内存里的 attempts 计数会被清零、
    /// 故障端点下同一行可被无限重试。本函数保证「attempt_count 已达上限的
    /// pending 行」在重启首轮就被清出 outbox，物理删除释放表空间。
    ///
    /// 返回被删除的行数。调用方在记 error 后无需再做 release / 物理删除。
    /// 删除条件 `attempt_count >= max_attempts` 涵盖重启前已用尽和当前正用尽
    /// 两种情况：后者由 `flush_pending` 在 Retriable 路径持久化 `record_attempt`
    /// 后，下一轮启动即可命中。
    pub fn drop_exhausted_pending(
        &mut self,
        protocol: &str,
        max_attempts: u32,
    ) -> GatewayResult<usize> {
        let n = self
            .conn
            .execute(
                "DELETE FROM mqtt_outbox
                 WHERE status = 0 AND protocol = ?1 AND attempt_count >= ?2",
                params![protocol, max_attempts as i64],
            )
            .map_err(|e| GatewayError::Queue(format!("outbox drop_exhausted_pending: {}", e)))?;
        Ok(n)
    }

    /// 取出所有待发送消息（启动时 + 周期调用）
    /// 注意：全量加载，仅用于测试。生产环境请用 replay_pending_batch。
    pub fn replay_pending(&mut self) -> GatewayResult<Vec<(i64, OutgoingMsg)>> {
        self.replay_pending_batch("mqtt", 0, i64::MAX)
    }

    /// 分页取出指定协议的待发送消息（生产环境使用，避免积压时一次性加载全部到内存）
    ///
    /// Bug C4: MQTT 与 Webhook 共用同一 outbox 库（bootstrap 两处都
    /// `OutboxStore::open(default_outbox_db())`），必须在 SQL 内按 protocol 隔离取数，
    /// 否则：
    /// - MQTT replay 会把 `protocol='http'` 的消息当 MQTT 消息 publish 到 webhook 的
    ///   URL（topic 字段），并对伪造的 data_ids 误执行 `queue.ack` → 丢数据；
    /// - Webhook 若"先 LIMIT 后 filter protocol"，MQTT 消息会占满 LIMIT 窗口导致
    ///   HTTP 消息长期饥饿（见 C19）。
    ///
    /// SQL 内先按 protocol 过滤再 LIMIT，一次解决这两点。
    pub fn replay_pending_batch(
        &mut self,
        protocol: &str,
        after_id: i64,
        limit: i64,
    ) -> GatewayResult<Vec<(i64, OutgoingMsg)>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, data_id, data_ids, topic, payload, qos, protocol
                 FROM mqtt_outbox
                 WHERE status = 0 AND protocol = ?1 AND id > ?2
                 ORDER BY id
                 LIMIT ?3",
            )
            .map_err(|e| GatewayError::Queue(format!("replay prepare: {}", e)))?;
        let rows = stmt
            .query_map(params![protocol, after_id, limit], |row| {
                let data_ids_blob: Vec<u8> = row.get(2)?;
                let data_ids = decode_i64_list(&data_ids_blob);
                Ok((
                    row.get::<_, i64>(0)?,
                    OutgoingMsg {
                        data_id: row.get(1)?,
                        data_ids,
                        topic: row.get(3)?,
                        payload: row.get(4)?,
                        qos: row.get(5)?,
                        protocol: row.get(6)?,
                    },
                ))
            })
            .map_err(|e| GatewayError::Queue(format!("replay query: {}", e)))?;
        let mut msgs = Vec::new();
        for r in rows {
            msgs.push(r.map_err(|e| GatewayError::Queue(format!("replay row: {}", e)))?);
        }
        Ok(msgs)
    }

    /// 清理已 ack 且超过 1 小时的旧记录（避免表无限增长）
    pub fn cleanup_acked(&mut self, older_than_ms: i64) -> GatewayResult<usize> {
        let cutoff = now_ms() - older_than_ms;
        let n = self
            .conn
            .execute(
                "DELETE FROM mqtt_outbox WHERE status = 1 AND created_at < ?1",
                params![cutoff],
            )
            .map_err(|e| GatewayError::Queue(format!("cleanup_acked: {}", e)))?;
        Ok(n)
    }

    /// F2: 限制未 ACK（pending）消息数量，防止断网期间 outbox 无限增长
    ///
    /// 当指定 `protocol` 的 pending 消息超过 `max_pending` 条时，删除该协议最旧的
    /// pending 消息。默认上限 100,000 条（约 50MB payload），可通过参数调整。
    ///
    /// F2 修复：MQTT 与 Webhook 共用同一张 `mqtt_outbox` 表（见 `protocol` 列），
    /// 因此**统计与删除都必须按协议过滤**。否则 MQTT 发布循环触发的一次维护会删掉
    /// Webhook（protocol='http'）尚未发送的消息，而 HTTP 消息的唯一持久副本就在
    /// 这张表里，被删即永久丢失且无补传通道。
    ///
    /// # r71-OBX1：`grace_ms` 宽限窗口
    ///
    /// r67 把 publish/ack fail 行交由本函数容量上限回收——但「立即删」存在 churn：
    /// broker 已收到但 PubAck 丢失的行（`status=0` 且 `last_attempt_at` 较新）若在
    /// 容量回收时被立即删掉，broker 端会保留这份数据但 outbox 已无副本——
    /// reconcile_puback 迟到时无对应行可对账，orphan_acks 永久 +1。
    ///
    /// `grace_ms` 指定「自上次尝试以来多少毫秒内**不删**」——broker 收到但 PubAck
    /// 仍可能在途的消息得以保留。`None` = 旧行为（无宽限，立即删最旧）。生产路径
    /// 推荐传 `Some(60_000)`（1 分钟）以匹配 ack_timeout_secs=10s 的 6 倍余量。
    ///
    /// 宽限期内若仍超限（graced 行占用了大部分容量），本函数在 commit 后 warn
    /// 一行 `still_over_limit` 提示运维——不会无限重试或死循环。
    pub fn enforce_pending_limit(
        &mut self,
        protocol: &str,
        max_pending: i64,
        grace_ms: Option<u64>,
    ) -> GatewayResult<usize> {
        // r27-OBX1（修复）：将 COUNT + DELETE 包在单个事务中。
        //
        // 旧实现两次独立 query：先 SELECT COUNT(*) 决定删除数量，再 DELETE 旧行。
        // 当前 &mut self 已保证单线程独占，无并发风险；但若未来重构放宽 self 约束
        // （如改为 &self、或允许多 writer 共享连接），中间窗口存在 TOCTOU：
        // 并发两次 enforce_pending_limit 可能计数两次、双倍删除。
        // 用事务包成原子单元后，read-modify-write 一致，COUNT 与 DELETE
        // 看到的是同一快照，避免双删/少删。
        //
        // r76-OBX1：replay_cursor 必须在 `tx` 创建之前读取（`load_replay_cursor`
        // 是 `&self`，与 `&mut self.conn.transaction()` 互斥）。读到的 cursor
        // 在事务内作为 SQL 参数使用——这里有意不重读，事务短、cursor 在该
        // 期间只会前进（更保守的视角），不会影响安全性。
        let cursor = self.load_replay_cursor(protocol);

        let tx = self
            .conn
            .transaction()
            .map_err(|e| GatewayError::Queue(format!("enforce_pending_limit tx: {}", e)))?;

        let count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM mqtt_outbox WHERE status = 0 AND protocol = ?1",
                params![protocol],
                |row| row.get(0),
            )
            .map_err(|e| GatewayError::Queue(format!("enforce_pending_limit count: {}", e)))?;

        if count <= max_pending {
            // 不需删除——但仍 commit 事务（SELECT 在事务内必须 commit 才能释放锁）
            tx.commit()
                .map_err(|e| GatewayError::Queue(format!("enforce_pending_limit commit: {}", e)))?;
            return Ok(0);
        }

        let to_delete = count - max_pending;
        // r71-OBX1：宽限窗口过滤——`last_attempt_at` 距今不足 `grace_ms` 的行
        // （或从未尝试过、`created_at` 距今不足的行）从候选中排除，避免 broker 已
        // 收到但 PubAck 在途的行被立即删掉，导致 reconcile 永久 orphan。
        // `None` → 不加宽限过滤（保持旧行为）。
        //
        // r76-OBX1：把「`id > replay_cursor`」并入删除候选过滤。
        // 旧实现按 `ORDER BY id ASC LIMIT N` 删最旧——**完全无视 replay_cursor**。
        // 这意味着可能删掉「id > cursor 但 status=0」的行，而这些行**仍在
        // replay 保护范围**（replay 用 `id > cursor` 拉取），删除后 replay
        // 永远不会再拉到它们 → **数据永久丢失**。
        //
        // 关键不变式：删除的行必须满足 `id > cursor`。
        // `id <= cursor` 的行已被 replay 视为「已推进过的下界」删除无害
        // （r67 已说明），但**当前 enforcement 不再需要**——cursor 之前的
        // 行并不在「积压」中（cursor 是 replay 的下界而非「已 ack」），删
        // 它们并不会释放更多 replay 视野。出于简化与一致性，只删 cursor 之后。
        //
        // 兼容性：r67 cursor 加载是 `load_replay_cursor(protocol)`——失败回退
        // 0（最保守的「重扫全表」语义）。即使 cursor=0 也满足 `id > 0` 删除
        // 全部旧行，**等同旧行为**——所以这是向后兼容的修复。
        //（cursor 已在事务前读取，见上方注释）
        let cutoff_ms: Option<i64> = grace_ms.map(|g| now_ms().saturating_sub(g as i64));
        let n = if let Some(cutoff) = cutoff_ms {
            tx.execute(
                "DELETE FROM mqtt_outbox WHERE id IN (
                    SELECT id FROM mqtt_outbox
                    WHERE status = 0 AND protocol = ?1
                      AND id > ?4
                      AND (last_attempt_at IS NOT NULL AND last_attempt_at < ?2
                           OR created_at < ?2)
                    ORDER BY id ASC LIMIT ?3
                )",
                params![protocol, cutoff, to_delete, cursor],
            )
            .map_err(|e| GatewayError::Queue(format!("enforce_pending_limit delete: {}", e)))?
        } else {
            tx.execute(
                "DELETE FROM mqtt_outbox WHERE id IN (
                SELECT id FROM mqtt_outbox WHERE status = 0 AND protocol = ?1
                  AND id > ?3
                ORDER BY id ASC LIMIT ?2
            )",
                params![protocol, to_delete, cursor],
            )
            .map_err(|e| GatewayError::Queue(format!("enforce_pending_limit delete: {}", e)))?
        };
        tx.commit()
            .map_err(|e| GatewayError::Queue(format!("enforce_pending_limit commit: {}", e)))?;

        // r71-OBX1：宽限模式下实际删除数可能 < to_delete。函数返回实际删除数，
        // 并在剩余仍超限时 warn——运维可据此判断是否需要调高 max_pending 或调高
        // grace_ms。`pending_count` 已在事务提交后，反映最新行数。
        if n > 0 {
            tracing::warn!(
                "outbox pending overflow: protocol={} had {} (limit {}), deleted {} oldest pending",
                protocol,
                count,
                max_pending,
                n
            );
        }
        if n < (to_delete as usize) {
            // 实际删除少于期望——说明有 (to_delete - n) 行处于宽限窗口内。
            // 重新计数当前 pending（事务已 commit），看是否仍超限。
            let after_count: i64 = self
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM mqtt_outbox WHERE status = 0 AND protocol = ?1",
                    params![protocol],
                    |row| row.get(0),
                )
                .map_err(|e| {
                    GatewayError::Queue(format!("enforce_pending_limit recount: {}", e))
                })?;
            if after_count > max_pending {
                tracing::warn!(
                    "outbox still over limit after grace-window reap: protocol={} pending={} limit={} grace_ms={:?} (宽限窗口内 {} 行被保留；考虑调高 max_pending 或降低 grace_ms)",
                    protocol, after_count, max_pending, grace_ms, to_delete as usize - n
                );
            }
        }
        Ok(n)
    }

    /// 统计待发送数（监控用）
    ///
    /// r27-OBX2（修复）：加 `protocol: Option<&str>` 参数过滤。
    ///
    /// 旧签名 `pending_count() -> i64` 永远返回所有 protocol 的总数。
    /// 与 `enforce_pending_limit(protocol, max_pending)` 的口径（按协议隔离）不一致——
    /// 监控告警若期望"protocol='mqtt' pending=N"，调用旧 API 会拿到 MQTT+HTTP 合计，
    /// 触发误报 / 漏报。`Some("mqtt")` / `Some("http")` 按协议过滤；`None` 保留旧行为（合计）。
    pub fn pending_count(&self, protocol: Option<&str>) -> GatewayResult<i64> {
        let n: i64 = match protocol {
            Some(proto) => self
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM mqtt_outbox WHERE status = 0 AND protocol = ?1",
                    params![proto],
                    |row| row.get(0),
                )
                .map_err(|e| GatewayError::Queue(format!("pending_count: {}", e)))?,
            None => self
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM mqtt_outbox WHERE status = 0",
                    [],
                    |row| row.get(0),
                )
                .map_err(|e| GatewayError::Queue(format!("pending_count: {}", e)))?,
        };
        Ok(n)
    }

    // ===== r67-OBX1：replay 游标持久化 =====

    /// 读取指定协议的 replay 游标（=「已经安全推进到的 outbox id 下界」）。
    ///
    /// 返回 0 表示该协议从未被 replay 推进过（或表刚建好）。
    /// 读取失败不 panic——视为 0，让上层走 fallback（重扫全表）。这是最保守
    /// 的选择：游标归零不会丢数据，只会让游标 reset 前的那段 pending 被多扫
    /// 一次；多扫得到的行要么是 status=0 真的需要重发，要么是 status=1 但
    /// SQL filter 已排除。
    pub fn load_replay_cursor(&self, protocol: &str) -> i64 {
        self.conn
            .query_row(
                "SELECT after_id FROM outbox_replay_cursor WHERE protocol = ?1",
                params![protocol],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => 0,
                other => {
                    tracing::warn!(
                        "outbox load_replay_cursor({}) failed: {}, fall back to 0",
                        protocol,
                        other
                    );
                    0
                }
            })
    }

    /// 保存指定协议的 replay 游标。
    ///
    /// `INSERT ... ON CONFLICT DO UPDATE`——首次写入直接插入，后续冲突时
    /// 覆盖；保证幂等。`save_replay_cursor` 失败时仅 warn（不抛错）——游标
    /// 没推进不会导致数据丢失，只会让下次重扫多走一段。
    pub fn save_replay_cursor(&self, protocol: &str, after_id: i64) {
        let res = self.conn.execute(
            "INSERT INTO outbox_replay_cursor (protocol, after_id) VALUES (?1, ?2)
             ON CONFLICT(protocol) DO UPDATE SET after_id = excluded.after_id",
            params![protocol, after_id],
        );
        if let Err(e) = res {
            tracing::warn!(
                "outbox save_replay_cursor({}, {}) failed: {}",
                protocol,
                after_id,
                e
            );
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 将 i64 列表编码为 BLOB（8 字节 per i64, little-endian）
fn encode_i64_list(ids: &[i64]) -> Vec<u8> {
    let mut out = Vec::with_capacity(ids.len() * 8);
    for &id in ids {
        out.extend_from_slice(&id.to_le_bytes());
    }
    out
}

/// 从 BLOB 解码 i64 列表
fn decode_i64_list(blob: &[u8]) -> Vec<i64> {
    if !blob.len().is_multiple_of(8) {
        tracing::warn!(
            "outbox data_ids BLOB length {} not multiple of 8",
            blob.len()
        );
        return Vec::new();
    }
    blob.as_chunks::<8>()
        .0
        .iter()
        .map(|chunk| {
            let mut arr = [0u8; 8];
            arr.copy_from_slice(chunk);
            i64::from_le_bytes(arr)
        })
        .collect()
}

#[cfg(test)]
mod tests;
