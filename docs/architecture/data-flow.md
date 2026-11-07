# 数据流

> Gateway Platform 文档子页：原 [architecture.md](../architecture.md) 第 7 节独立成页，便于按主题检索与维护。

---

## 7. 数据流

```
设备(Modbus/DLT645/...)
    │  串口/TCP 原始字节
    ▼
BusManager → FrameBuffer 帧提取
    │
    ▼
Collector 协议解析 → DataPoint
    │
    ▼  (mpsc 通道，背压)
ProcessingPipeline
    │
    ├─→ EdgeProcessor (死区/变化/聚合)
    ├─→ 公式计算 (scale/offset/CT/PT)
    │
    ├─→ DataQueue (SQLite WAL) ──MQTT QoS1 ACK──> broker
    ├─→ RealtimeBus (broadcast) ──WS──> 前端实时页
    └─→ ConversionManager ──┬──> MQTT
                            ├──> Webhook (HTTP)
                            └─→ OPC UA Server / Modbus Server
```

### 7.1 点位归属（`DataPoint.device_id`）

`DataPoint` 携带 `device_id`，表示该点位属于哪台设备，用于下游按**设备**聚合（设备级上送报文、
Web「按设备统计通信质量 / 最后成功采集」），而不是退化成按**总线名**归类。

- **直配点位**：取 `PointConfig.device_id`；设备未配置 `id` 时为空。
- **模板展开点位**：`config/template.rs::expand_devices()` 展开时写入所属设备 ID
  （早期实现固定为 `None`，导致模板化配置下全部点位丢失归属）。
- **为空时的回退**：南向上报按 `point.device_id.as_deref().unwrap_or(&point.bus)` 归类。
- **真值源**：`Scheduler::device_status()` 的快照包含展开点位及其 `device_id`；`cfg.points`
  只有直配点位，**不能**用来反查模板化配置的点位归属（Web API 一律优先读快照，配置仅作兜底）。

### 7.2 内置模拟消费者（`sim_consumer`）

现场/开发环境常常没有可用的 MQTT 通道。此时队列只会「进不出」：数据不断堆积，
而「队列积压」告警因 `has_consumer=false` 只能降级为存量提醒，运维无法区分
「还没上报」与「没人消费」。

打开 `sim_consumer` 开关（`gateway-core/src/north/sim_consumer.rs`）即可启用一个
**内置兜底消费者**：它与 MQTT 发布循环同型，按批 `dequeue → 处理 → ack`，
让数据真正走完队列链路，从而消除 `has_consumer=false` 的歧义。

- 配置项：`enabled`（默认 `false`）、`batch_size`（默认 100）、`interval_ms`（默认 1000）、
  `log_every`（默认 100，每消费满该条数打一条 INFO 摘要）。
- **不外发任何数据**：模拟处理恒视为成功，只消费、不上报、不写 outbox。
- **与 MQTT 互斥**：两者都是队列的消费者，同时开会分流同一批数据，被分流的那部分
  会被消费掉却不上报（静默丢数）。故 MQTT 已启用时只告警、不自动启动模拟消费者。
- 停机：走北向统一停机序（先发停止信号，再 await run 循环退出），不留孤儿 task。

#### 7.2.1 运行时可控

`sim_consumer` 是**运行时可启停**的组件，配置与运行期开关是两件事：

- 配置里的 `enabled` 只表示「**启动时是否自动拉起**」，不表示「当前是否在跑」；
- 运行期的开关由 API 控制，状态接口同时下发二者（`enabled` = 配置意图，
  `running` = 实际运行态），缺一不可：只看 `enabled` 无法区分「配置开了但被
  MQTT 互斥挡住」与「根本没配」，只看 `running` 又无法解释「为什么没跑」。

| 端点 | 方法 | 说明 |
|------|------|------|
| `/api/sim_consumer/status` | GET | 状态快照（`running` / `enabled` / `consumed` / `batches` / `queue_len` / `last_consumed_at_ms` / `started_at_ms` / `uptime_ms` / `batch_size` / `interval_ms` / `log_every` / `blocked_reason`） |
| `/api/sim_consumer/start` | POST | 启动；body 可选 `{ batch_size?, interval_ms?, log_every? }` |
| `/api/sim_consumer/stop` | POST | 停止 |

语义约定：

- `start` 幂等：已在运行时返回 **409**（`code=4009`）；MQTT 已启用时同样 409，
  状态接口的 `blocked_reason` 给出可读原因 `mqtt_enabled`。
- `stop` 幂等：未在运行时返回 **409**；返回前会**等到 run 循环真正退出**
  （上限 5s），故接口返回后队列不会再被消费。
- 运行参数（`batch_size` / `interval_ms` / `log_every`）**运行中不可改**：
  `update_params` 返回错误（`config error: sim consumer 正在运行：…`），
  需先 `stop` 再改再 `start`。
- 读接口匿名可读（跟随 `web.allow_anon_read`），写接口需 token；API 密钥需持
  `config:write`（与 `uplink` 同域）。

#### 7.2.2 指标

Prometheus 暴露（`gateway_core::engine::metrics::sim_consumer`）：

| 指标名 | 类型 | 含义 |
|--------|------|------|
| `gateway_sim_consumer_consumed_total` | counter | 累计成功消费（ack）的数据条数 |
| `gateway_sim_consumer_batches_total` | counter | 累计成功消费的批次数 |
| `gateway_sim_consumer_running` | gauge | 运行状态（1 = 在跑，0 = 已停） |

未接入 `Metrics` 的部署（单测 / 轻量场景）不写这些指标，但功能完全正常——
指标写入是可选旁路，不影响消费链路。

---

## 7.3 数据留存与恢复边界

> 本节是**产品边界声明**，不是实现说明。交付前必须让客户知道本节内容，
> 否则"我以为数据有备份"会成为最贵的一类现场纠纷。

### 7.3.1 唯一副本

本产品形态为**单机边缘网关**（无云平台、无双机热备），因此：

- 历史数据的**唯一副本**在本机 `data_queue.db`（路径见 `gateway-core/src/support/paths.rs`
  的 `default_db_path`，默认 `/var/lib/gateway/data_queue.db`，可由 `GATEWAY_DB_PATH` 覆盖）。
- **不存在异地副本**，也不存在集群副本。设备磁盘或整机损坏时，
  该时段的历史数据**不可恢复**。
- 交付检查单会打印本节结论（见 `deploy/verify-deploy.sh` 的 `[1g] 数据留存`），
  并写入导出的 Markdown 检查单，供客户签字留档。

### 7.3.2 已有的数据保护（及其边界）

网关已实现下列保护，覆盖**逻辑损坏**与**进程级故障**，但**不覆盖物理损坏**：

| 保护机制 | 覆盖场景 | 实现位置 |
|---|---|---|
| `synchronous=FULL` | 断电/掉电后不丢已提交数据 | `queue/storage/sqlite/db_init.rs` |
| 启动时 `PRAGMA integrity_check` + 自动从备份恢复 | SQLite 文件逻辑损坏 | `queue/storage/sqlite/db_init.rs` |
| 备份一致性（先 checkpoint 再拷贝） | 备份不漏掉 `-wal` 中已提交的行 | `queue/storage/sqlite/wal.rs` |
| `disk_full` 标志 + 退避 | 磁盘满时退避，避免 CPU 空转 | 队列写入路径 |
| 看门狗超时前冲刷 | 重启时未持久化数据 | `platform/watchdog_flush.rs` |
| FIFO 按 `storage_days` 截断 | 超出保留期的数据按最旧优先丢弃 | 队列保留策略（配额见 `domain/license.rs` 的 `Quota.storage_days`） |

> **关键区分**：备份文件 `data_queue.db.bak` 与主库**位于同一块盘**。
> 它能救回「SQLite 文件逻辑损坏」，救不回「磁盘物理损坏」或「整机丢失」。
> 交付时必须向客户讲清这个区别。

### 7.3.3 需要审计留存时的唯一出路

客户合同若含数据留存 SLA / 审计要求，**正确的做法是定期导出**，而不是指望本地冗余：

- 导出入口：`GET /api/history/export`（见 `gateway-api/src/web/api/history/routes.rs`，
  handler 为 `history_export_csv`），前端对应 `gateway-web/src/utils/exportCsv.ts` 与 `utils/zip.ts`。
- 导出结果应落到**网关之外**的介质（客户侧 NAS / 人工归档），
  落回本机不构成任何冗余。
- 长周期查询走降采样表 `data_queue_aggregated`（分钟桶 avg/min/max），
  导出长区间时不必逐行扫原始数据。

### 7.3.4 明确不做

- 不做本地多副本（RAID / 双盘镜像）——目标设备为低成本工控板，无冗余存储硬件。
- 不做自动异地备份推送——无云平台即无备份通道；如需自动备份，
  需客户侧提供接收端（当前形态下尚未支持自动推送，见 `docs/architecture/constraints.md` 的非目标声明）。

---
