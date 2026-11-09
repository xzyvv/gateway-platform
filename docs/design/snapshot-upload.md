# 快照上报（snapshot）最小改造设计

**状态**：已实施（v1：仅 `report_mode` 单一开关，开放问题见 §6）
**目标读者**：负责 MQTT 上行链路的开发 / 评审者
**解决问题**：当前 1s 采样 + 5s 上报时，每个采集样本都作为独立行上行（一个点位上送周期内出现 5 次），与主流网关「周期快照」语义不一致（详见本文 §1）。本设计给出**出队侧折叠**的最小改造路径，不触碰采集侧与报文结构。

---

## 1. 现状与目标

### 1.1 现状（`report_mode = stream`）

| 环节 | 行为 | 代码位置 |
|---|---|---|
| 采样 | 每个 Good 样本都入 `data_queue` 一行（`deadband=0`、`on_change=false` 全通过） | `pipeline/edge.rs`、`pipeline/mod.rs` |
| 出队 | 纯 FIFO：`WHERE in_flight=0 AND delivered=0 ORDER BY id ASC LIMIT ?`，**不按点位折叠** | `queue/storage/sqlite/limit.rs::dequeue_batch` |
| 发布 | 每 `publish_interval` 调 `drain_pending` 连续发到队列空 | `north/mqtt/event_loop.rs::publish_loop` |
| 报文 | 顶层 `ts` = 发布时刻，点位逐条展开、不去重 | `north/mqtt/payload.rs::build_payload_default` |

结果：1 点位 1s 采样 / 5s 上报 → 每个周期一条消息、内含 5 个同 `point_id` 样本（`source_ts` 相差 1s，顶层 `ts` 相同）。

> 补充事实：`data_queue` 有唯一索引 `idx_queue_dedup(point_id, timestamp)`（`db_init.rs::ensure_dedup_unique_index`），但只在**同一毫秒**内去重，不解决跨采集周期的折叠需求。

### 1.2 目标（`report_mode = snapshot`）

每个上报周期内，每个 `(device_id, point_id)` **只上行最新一行**；更早的样本标记为已送达但不上行。

与 OPC UA 订阅模型对齐：`samplingInterval` = 点位 `interval`，`publishingInterval` = `publish_interval`，`queueSize = 1`，`discardOldest = true`。

### 1.3 非目标

- 不改采集侧、不改 `DataPoint`/`QueuedItem`、不改报文 schema（`points[]` 结构不变）。
- 不做 Store & Forward 的新机制（现有 `data_queue` + `outbox` 保留）。
- 不覆盖 Webhook / OPC UA Server / Modbus Server 连接器（它们不消费 `data_queue` 出队路径）。
- 不做服务端侧聚合（`aggregation` 已可独立实现窗口聚合，与本设计正交）。

---

## 2. 配置设计

`MqttConfig`（`config/mqtt.rs`）新增：

```rust
/// 上报模式：stream = 每个采样都上行（现状，默认）；snapshot = 每周期每点位只上行最新值
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportMode {
    #[default]
    Stream,
    Snapshot,
}

#[serde(default)]
pub report_mode: ReportMode,
```

- `#[serde(default)]` → 存量配置不写该字段即为 `stream`，**零行为变更**。
- `validate_cycle_conflict`（`config/validate/gate.rs`）**不随模式放宽**：两种模式都要求 `interval < publish_interval`。`interval == publish_interval` 时 `stream` 本来也是每周期约一条，`snapshot` 并不额外受益，且点位接口（`gateway-api` / Web 点位编辑器）仍按严格小于校验，放宽会造成「MQTT 页可保存、点位页被拒」的自相矛盾。
- 前端 `views/system/config/tabs/MqttConfigTab.vue` 增加「上报模式」下拉 + 说明文案；`api/generated` 类型同步。
- 不做点位级/设备级覆盖（保持单一连接级开关，避免组合爆炸）。

---

## 3. 核心改动：出队折叠

### 3.1 选中规则

「最新」定义为**入队序最大（`MAX(id)`）**，不是 `MAX(source_ts)`。理由：快照语义对应「网关缓存里的当前值」，即最后写入值；迟到但更旧的样本不应覆盖新值（`source_ts` 乱序只影响数据内容，不改变缓存语义）。

```sql
-- ① 每个点位取最新未送达行（按 id 判定最新），并限制在本周期上界内
WITH latest AS (
    SELECT MAX(id) AS id
    FROM data_queue
    WHERE in_flight = 0 AND delivered = 0 AND id <= ?2   -- ?2 = 折叠时得出的 cutoff
    GROUP BY device_id, point_id
)
SELECT id, point_id, device_id, value, raw_value, raw_type, quality,
       quality_reason, timestamp, source_ts, stale_count, value_text
FROM data_queue
WHERE id IN (SELECT id FROM latest)
ORDER BY id ASC
LIMIT ?1;
```

沿用现有事务：选中后置 `in_flight = 1`（`dequeue_batch` 原有逻辑不变）。选中语句本身按点位取 `MAX(id)`，因此在**同一批次内**天然不会重复上行同一点位；折叠交给 §3.2 的独立语句。

`cutoff`（本周期上界）是折叠时在同一事务内取到的 `MAX(id)`，作用是把「本周期」的数据边界钉死：排空期间新入队的样本（id 更大）留到下个周期。没有它时，排空要跑十几批且每批都重新取 `MAX(id)`，慢 Broker 下（每批等 PubAck）同一点位可能在一个周期内被上行两次。

### 3.2 折叠（supersede）规则

折叠由**上报循环每周期执行一次**，不再与出队同事务：

```rust
// event_loop.rs::publish_loop —— 每个周期睡眠到点后、drain_pending 之前
let cutoff = if self.config.is_snapshot() {
    // 折叠 + 取本周期上界；失败则退化为 i64::MAX（不限制，安全但会失去跨批去重）
    self.queue.collapse_snapshot().await.map(|c| c.cutoff).unwrap_or(i64::MAX)
} else {
    i64::MAX
};
```

```sql
-- 每个点位除最新一行外，其余未送达行标记 delivered（不上行）
UPDATE data_queue
SET delivered = 1
WHERE delivered = 0
  AND in_flight = 0
  AND id NOT IN (
      SELECT MAX(id) FROM data_queue
      WHERE in_flight = 0 AND delivered = 0
      GROUP BY device_id, point_id
  );
```

要点：

- **每周期一次，而不是每批一次**：同一周期的重复样本只需收敛一遍。8000 点 / 5s 场景下每周期 16 批，旧实现（折叠塞在出队里）要跑 16 次全量 `GROUP BY` 扫描，现在只跑 1 次。
- **反压时也折叠**：`in_flight` 达 `MAX_IN_FLIGHT` 时出队直接返回空（`dequeue_with_backpressure`），此时仍调一次 `collapse_quietly`——不发数据也要把旧样本收敛掉，否则积压行持续占磁盘、抬高队列水位。
- **幂等**：折叠后库里已不存在「非最新且未送达」的行，同周期重复调用影响 0 行，不会重复扣减 `depth`。
- 只处理 `in_flight = 0` 的行：在途（待 ACK）样本不受影响，避免「已发送待确认」的行被标记为已送达而丢数据。
- 折叠语句**对所有点位生效**，包括本轮未被 `LIMIT` 选中的点位——它们的旧样本同样已被新样本取代，属于快照语义的正当丢弃。
- 返回的影响行数用于扣减 `depth` 并累加指标，否则队列深度虚高、四级水位判定失真（沿用 `AckOutcome` 按「状态跃迁」计数的口径）。
- 同事务返回 `cutoff = MAX(id)`（主键索引 O(1)），作为本周期出队上界，见 §3.1。
- **折叠失败时本周期不排空**：命令通道 / DB 出错时跳过本轮 `drain_pending`（下周期重试），而不是退化为 `cutoff = i64::MAX` 继续发——否则每批都重新取当下 `MAX(id)`，慢 Broker 下排空期间新到的样本会让同一点位在一个周期内上行两次。
- **断网期间照常折叠**：`publish_loop` 不区分连接状态，每周期都会折叠。重连后只补「最新值」，断网期间的中间样本不会补发（本地历史仍完整）——这是快照语义的一部分，与 `resume_rate_limit` 的逐条补传预期不同，需在配置文档中明示。

### 3.3 索引

现有 `idx_queue_delivery(in_flight, delivered)` 支撑 ①② 的 `WHERE`，但 `GROUP BY device_id, point_id` + `MAX(id)` 需要按点位有序扫描。新增部分索引：

```sql
CREATE INDEX IF NOT EXISTS idx_queue_snapshot
    ON data_queue(device_id, point_id, id)
    WHERE delivered = 0;
```

索引在 [`db_init.rs`](../../gateway-core/src/queue/storage/sqlite/db_init.rs) 中随建表语句**无条件创建**（`IF NOT EXISTS`，幂等）。`report_mode` 目前不影响 schema：部分索引只收录 `delivered = 0` 的行，即积压本身，写入时多维护一个索引条目的开销有界；从 snapshot 切回 stream 时索引保留无副作用，避免运行期建索引的锁表风险。

### 3.4 与既有机制的交互

| 机制 | 交互 | 结论 |
|---|---|---|
| `in_flight` / `MAX_IN_FLIGHT` | 折叠只处理 `in_flight = 0` 的行；反压时照常折叠 | 不影响在途保护，且积压不会无界增长 |
| 周期 cutoff | 折叠时记录 `MAX(id)`，本周期只出队 `id <= cutoff` 的行 | 排空期间新到的样本留到下个周期，保证同周期每点位至多一条 |
| 断网 / 断路器打开 | 折叠不感知连接状态，每周期照常执行 | 重连后只补最新值，不补中间样本（快照语义的必然结果） |
| 发布失败 `release_in_flight` | 被选中的行回到未送达；折叠只以未送达行判最新，已送达 / 在途的新值不参与判定 → 慢 ACK 时旧行可能在更新的值之后重发一次 | 符合快照语义的合理降级，但**不是严格**「每周期每点位至多一条」，见 §3.5 |
| `outbox` 持久化 / replay | outbox 只记录**实际发送**的 `ids`；被 supersede 的行不进 outbox | 重连补传不会复活已丢弃样本 |
| 历史查询 | `history.rs` 按时间范围查，**无 `delivered` 过滤** | 标记 delivered 不影响历史曲线；行保留至 retention / 容量淘汰 |
| 容量淘汰 | `trim` 优先删最旧的非在途行 | 已送达行更早被回收，符合预期 |
| 令牌桶 / `MAX_BATCHES_PER_CYCLE` | 每周期待发行数从 `点数 × 周期比` 降为 `点数` | 8000 点场景从 ~80 批/周期降到 16 批，吞吐压力显著下降 |
| 微批入队（500 点/批） | 不变 | 与出队折叠独立 |

### 3.5 语义边界（必须写入文档）

- supersede 掉的行**永远不会上行**。这是配置选择的结果，不是故障。
- 若本轮发布失败，被 supersede 的旧样本不会因为重发而回来；重发的是选中行（若仍是最新）。
- **慢 ACK 边界**：折叠只以「未送达行」判定最新，已送达 / 在途的新值不参与。因此当旧行的 PubAck 超时被 `release_in_flight` 回未送达、而更新的行已 ACK 时，旧值会在新值之后**重发一次**——快照语义在失败重试路径上退化为 at-least-once，不是严格的「每周期至多一条」。要彻底消除需按「全表 `MAX(id)`」折叠，代价是把部分索引换成覆盖全部行的 `(device_id, point_id)` 索引（5000 万行规模不划算），v1 明确接受该边界，并由 `snapshot_slow_ack_can_resend_stale_row_after_newer_delivered` 固化。
- 下游若依赖「每个样本都收到」，必须保持 `stream`。
- **与点位流水线配合**：`aggregation.window` 必须 ≥ `publish_interval`（`validate_snapshot_aggregation` 强制，`stream` 不限），否则同周期的中间聚合窗口会被折叠丢弃；Web 保存严格拒绝，磁盘加载/热重载路径则把窗口抬到上报周期并 `warn`（`sanitize_legacy_snapshot_aggregation`），避免手工改配置后网关起不来；Bad/Uncertain → Good 的首个样本由 `EdgeProcessor` 强制放行一次（`PipelineNode::process_recovered`），否则数值未变时恢复事件会被死区/变化过滤，云端停留在 Bad——聚合点位的该样本只能进入当前窗口，实际上行最多滞后一个窗口。

---

## 4. 改动清单

| 文件 | 改动 |
|---|---|
| `gateway-core/src/config/mqtt.rs` | 新增 `ReportMode` 枚举与 `report_mode` 字段（默认 `stream`） |
| `gateway-core/src/config/validate/gate.rs` | 周期校验**不随模式放宽**（两种模式均要求 `interval < publish_interval`） |
| `gateway-core/src/queue/storage/sqlite/limit.rs` | `dequeue_snapshot_batch`（最新行 + 周期 cutoff + 置 `in_flight`）与 `supersede_snapshot`（折叠 + 返回 cutoff） |
| `gateway-core/src/queue/storage/sqlite/sqlite_storage.rs` | 实现 `collapse_snapshot`：折叠 + `depth` 扣减 + 指标；`SnapshotCollapse { superseded, cutoff }` |
| `gateway-core/src/queue/storage/mod.rs` | `SnapshotCollapse` 类型与 trait 默认实现（内存后端退化为逐样本） |
| `gateway-core/src/queue/core/task.rs` | `QueueCommand::{CollapseSnapshot, DequeueSnapshot{cutoff}}`；反压分支 `collapse_quietly` |
| `gateway-core/src/queue/mod.rs` | `Queue::collapse_snapshot` / `dequeue_snapshot(limit, cutoff)` 命令封装 |
| `gateway-core/src/queue/storage/sqlite/db_init.rs` | 建 `idx_queue_snapshot` 部分索引（无条件、幂等） |
| `gateway-core/src/north/mqtt/event_loop.rs` | 每周期折叠一次并把 cutoff 传给 `drain_pending` |
| `gateway-core/src/north/mqtt/publish.rs` | 按 `report_mode` 分派出队；发布失败时打印 snapshot 语义提示 |
| `gateway-core/src/engine/metrics/{counter,queue_persist}.rs` | 新增 `gateway_queue_snapshot_superseded_total` |
| `gateway-core/src/config/*/tests`、`queue/storage/sqlite/consistency_tests.rs`、`queue/tests.rs` | 枚举反序列化 / 默认值 / 周期校验一致 / 折叠、cutoff 与 actor 命令 |
| `gateway-web/src/views/system/config/tabs/MqttConfigTab.vue` | 上报模式下拉 + 说明（含 `MqttConfigTab.spec.ts` 用例） |
| `gateway-web/src/api/generated/MqttConfig.ts` | 类型同步 |
| `docs/点位配置与换算.md` | 新增「上报节拍与快照模式」小节，含 `ts` / `source_ts` 口径 |
| `docs/INDEX.md` | 登记本文档 |

预估规模：后端 ~250 行（含测试），前端 ~30 行，文档 ~60 行。

---

## 5. 测试计划

**单元（`queue/storage/sqlite/consistency_tests.rs`）**
1. 同点位 5 行未送达 → `collapse_snapshot` 折叠 4 行（`delivered = 1`），`db_unsent` 剩 1；再出队返回 1 行（id 最大）。
2. 多点位同时积压 → 每个点位各返回 1 行，`ORDER BY id ASC` 稳定。
3. `LIMIT` 小于点位数 → 被 `LIMIT` 截断的点位，其最新样本保持未送达（下轮再发）。
4. `depth` 扣减 = 折叠行数（出队不改变 depth）；折叠幂等（重复调用影响 0 行）且不动在途行。
5. `cutoff`：折叠后新到的行（`id > cutoff`）本周起不出队，下个周期重新折叠后才发送。
6. `stream` 模式回归：出队行为与本改动前逐字节一致（`stream_dequeue_still_returns_every_sample`）。
7. actor 级（`queue/tests.rs`）：`CollapseSnapshot` 命令回传 `{ superseded, cutoff }`；同一 cutoff 连续出队第二批为空；新样本等下周期。
8. 慢 ACK 已知边界（`snapshot_slow_ack_can_resend_stale_row_after_newer_delivered`）：旧行 release 后会在更新值之后重发一次，固化 §3.5 口径。

**集成（`north/mqtt`）**
9. 桩设备 1s 采样、`publish_interval=5`、snapshot → 每周期每点位仅 1 条，`source_ts` 等于该周期最后一拍。
10. 发布失败 → `release_in_flight` 后同一批次不会「删了又发」；仅当该行已被更新值取代、且更新值已 ACK 时才可能重发一次（§3.5 已知边界）。
11. 断网重连 replay：被 supersede 的行不出现在补传报文中。

**端到端**
12. 8000 点 × 5 样本/周期：出队 p99 < 50ms（本地 SSD 基线），单周期批次数 ≤ 16。
13. 从 `stream` 切 `snapshot`（配置热重载）后下一周期即生效，无需重启。

**配套（与点位流水线/配置校验）**
14. `validate_snapshot_aggregation`：snapshot 下 `aggregation.window < publish_interval` 必须拒绝，`stream` 不限（`snapshot_aggregation_window_must_cover_publish_interval`）。
15. 质量恢复：Bad/Uncertain → Good 的首个样本强制上行一次，之后恢复常规死区过滤（`test_quality_recovery_forces_upload`）。
16. `on_change` 单独接入管道时按 `deadband` 判变化（`test_on_change_uses_deadband_threshold`）。

---

## 6. 风险与回滚

| 风险 | 影响 | 处置 |
|---|---|---|
| 中间样本被丢弃 | 下游若按样本计费/做高频分析会缺数 | 默认 `stream` 不变；文档明示；上线时告知下游 |
| `GROUP BY` 扫描开销 | 大表（10⁶ 行）出队变慢 | 部分索引（无条件创建，仅 `delivered = 0` 行入索引）；实测 p99 不达标则改为「按点位游标分页折叠」 |
| supersede 计数遗漏 | `depth` 虚高 → 水位告警误报 | 复用 `AckOutcome` 状态跃迁口径 + 单测 4 |
| 历史查询受影响 | 曲线缺中间点 | 已确认 `history.rs` 无 `delivered` 过滤；单测补一条 |
| 回滚 | — | 配置改回 `stream` 即恢复，无需迁移、无 schema 变更（索引保留无害） |

**已决议（v1 不实施，理由如下）**
1. snapshot + RBE：**不做连接级实现**。省流量由点位级 `on_change`/`deadband` 承担（样本在 pipeline 即被过滤，snapshot 只折叠残余多值）；纯 RBE 会导致值长期不变时下游无法区分「没变化」与「链路挂了」，需额外心跳机制，收益不抵复杂度。
2. `batch_target` 口径：**沿用行数**（`pending.clamp(100, 500)`）。snapshot 下每点位单周期仅 1 行，行数与点位数恒等，无需分叉。
3. `max_age_ms`：**v1 不做**。陈旧性已由 `quality`（Good/Stale/Bad）与 `stale_count` 表达，下游可自行过滤；若现场对「重连后不发老值」有硬要求，再作为独立小改动（选中 SQL 增加 `source_ts >= now - max_age`）补上。
