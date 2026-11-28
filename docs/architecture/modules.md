# 核心模块说明

> Gateway Platform 文档子页：原 [architecture.md](../architecture.md) 第 6 节独立成页，便于按主题检索与维护。

---

## 6. 核心模块说明

### 6.1 启动流程（gatewayd/src/bootstrap/）

13 步启动编排位于 `gatewayd` crate（`gatewayd/src/bootstrap/`）；`gateway-core` 侧仅保留启动期共享支撑：`runtime/`（`GatewayRuntime` 组件装配与配置热更）与 `shared/`（clock / cron / paths / sqlite_maintenance）。

顺序启动，失败即中止（fail-fast），主要阶段如下（步骤编号以 `bootstrap/mod.rs` 与 `helpers/*.rs` 注释为准）：

1. **加载配置** — `ConfigLoader` 读取 `config.json` + `validate_all` 校验
2. **初始化日志** — tracing JSON + 按天滚动
3. **看门狗** — 软件 + 硬件 `/dev/watchdog`（15s 超时）
4. **NTP 同步** — `NtpManager`，默认 `pool.ntp.org`，3600s 间隔
5. **GPIO** — `GpioController`（Linux）
6. **串口总线** — `SerialManager` + `BusManager`，按总线名建 HashMap
7. **Collector** — `ProtocolRegistry` 按协议创建采集器
8. **SQLite 队列** — `DataQueue`（WAL，1 万条有界通道）；随后初始化 X.509 设备证书（Step 8b，供 OPC UA / MQTT 双向 TLS；`GATEWAY_REQUIRE_EXTERNAL_CERT=1` 时初始化失败即阻断启动）
9. **MQTT 客户端** — `MqttClient`（可选）；`sim_consumer` 恒创建实例（Step 9b，无 MQTT 时的兜底消费者，运行期由 API 启停）
10. **边缘处理 + 管道 + 实时总线** — `EdgeProcessor` / `ProcessingPipeline` / `RealtimeBus`，并装配 `NorthDispatcher`（注册 MQTT / Webhook connector，见 §6.6）
11. **北向 Server** — OPC UA / Modbus Server 由 `NorthServerRegistry` 按配置装配（被动暴露点位，见 §6.6）
12. **调度器** — `Scheduler` 按协议 + 总线分组启动定时任务
13. **Web Server** — axum 监听 `web.listen_addr`（默认 `127.0.0.1:8080`；设为 `0.0.0.0:8080` 时须先设置 `password_hash`，否则 fail-closed 拒绝启动）；同一阶段还启动报警引擎、MQTT 透传、蜂窝模块与计划任务执行器

停机：捕获 SIGTERM/SIGINT → 进入 `graceful_shutdown`（`gatewayd/src/bootstrap/helpers/shutdown.rs`）。
顺序与启动相反且更细，**控制面先静默、看门狗最后解除**，全程由外层 `tokio::time::timeout(80s)`
兜底（命中则 `shutdown_forced_total+1` 并强制退出，日志明确提示可能丢未发数据）。`graceful_shutdown_inner` 依次为：

1. **Web 优雅停机**（停机序第一步）— `sig.shutdown()` 停止接受新连接并等在飞请求结束，再 await run 句柄；避免 80s 窗口内 HTTP 仍接受写请求、与后续拆解并发。
2. **API Key 调用统计落盘** — Web 已停、鉴权热路径不再增量，强制刷新最后一个周期。
3. **自愈执行器** — 先 `cancel` 再 await（35s，配置重载等动作可能重 IO）。
4. **调度器** — `scheduler.stop()`。
5. **管道** — `pipeline.shutdown()` + 500ms 冲刷。
6. **关闭北向专用队列发送端** — `pipeline.close_north_feed()`，让 `NorthDispatcher` 消费完积压后自然退出；必须在 drain 之前，确保最后一批已 fan-out。
7. **北向 fan-out** — `shutdown_token()` 触发后 await run 循环真正退出（5s，超时只告警不 abort）。
8. **蜂窝 / 健康监控** — `cell.stop()`，health 句柄 `abort()`。
9. **队列排空** — 先同步冲刷 fallback 缓冲（`flush_fallback_sync`，纯内存最多 5000 点，异步路径此刻可能已失效），再 `drain_and_wait()`（30s；超时仅告警，未发数据落 SQLite 下次启动 replay）。
10. **北向 run 循环** — 按通道表顺序「发信号 → await 退出」（webhook → HA → 报警 → 透传），须在 mqtt.stop 之前停干净。
11. **北向 Server** — OPC UA / Modbus / 透传并行 `stop()`（整组 3s；旧实现串行 N×3s 与 80s 宽限失配）。
12. **MQTT** — `mqtt.stop()`（5s）。
13. **解除硬件看门狗** — `stop_platform_watchdog()` 置于**最后**：停机期间看门狗继续保护进程（写卡死即整机复位），全部资源释放后才写 magic char 'V' 解除 armed。

> 启动期由 `StartupCleanupGuard`（`bootstrap/mod.rs`）包裹：任一步 `?` 提前返回都在 Drop 中统一 `stop_platform_watchdog()` + `shutdown_logging()`；两个动作均幂等，与正常停机路径不冲突。

### 6.2 配置层（config/）

| 文件 | 职责 |
|---|---|
| `loader.rs` | `ConfigLoader`：读取/解析 JSON，失败返回错误（不静默） |
| `migrate.rs` | 配置 Schema 版本迁移（v0→v1 逐步升级） |
| `validate/`（`validate.rs` 已拆为目录） | 字段校验函数 + `validate_all` / `validate_slave_ids` / `validate_cycle_conflict`；`field.rs` 字段规则、`gate.rs` + `gate/` 校验门禁、`report.rs` + `report/` 校验报告 |
| `schema.rs` | `schemars` 派生的 JSON Schema（结构层校验，先于 migrate） |
| `compat.rs` | S6 向后兼容回归：旧配置可加载 + Schema 字段契约不收缩（crate 内单测） |
| `shard.rs` | 配置分片：`include` 机制多文件加载合并 |
| `reload_scope.rs` | 分节热更能力判定（`applied` / `restart_required` / 纯元数据） |
| `convert.rs` | 协议转换规则配置（raw / linear / enum 三档模板） |
| `bus.rs` / `point.rs` / `virtual_point.rs` / `mqtt.rs` / `web.rs` / `opcua.rs` / `webhook.rs` / `modbus_server.rs` / `modbus_passthrough.rs` / `cellular.rs` / `ntp.rs` / `telemetry.rs` / `backoff.rs` / `license.rs` / `self_heal.rs` / `groups_assets.rs` | 各配置结构体（Serde 反序列化；cellular 归属 config 层避免反向依赖） |
| `template.rs` | `TemplateManager`：内置+自定义设备模板 |
| `store/version.rs` | `ConfigVersionStore`：配置版本快照（SQLite） |
| `store/uplink_test_history.rs` | 上送测试历史记录（SQLite 落盘） |
| `store/api_keys.rs` | API 密钥（scope / 过期 / 吊销）存储（SQLite） |
| `store/rate_rules.rs` | API 限流规则 + 访问日志（SQLite） |
| `store/scheduled_tasks.rs` | 计划任务（cron）存储与调度（SQLite） |

> `template.rs`（模板定义 + 展开）属配置域逻辑，留在 `config/` 顶层；落盘持久化（配置版本快照、上行测试历史）统一收在 `config/store/`，与纯配置模型区分。

配置热加载：`arc-swap` 原子替换，读取无锁；`Scheduler::reload` 动态增删采集任务。

### 6.3 南向采集层（south/）

```
Scheduler (按协议+总线分组)
   │  定时触发（interval）
   ▼
BusManager (串口/TCP)
   │  收发原始字节 + 帧提取（FrameBuffer）
   ▼
Collector (协议解析 → DataPoint)
   │
   ▼
ProcessingPipeline (mpsc 通道，背压)
```

- **`scheduler/`** — 按总线分组调度，支持配置热重载（`reload.rs`）
- **`bus/`** — `SerialManager` 打开串口，`BusManager` 封装读写，`FrameBuffer` 帧缓冲，`serial_task`/`tcp_task` 收发任务
- **`protocol/`** — 各协议 `Collector` trait 实现，`registry.rs` 统一注册

### 6.4 数据处理管道（pipeline/）

采集与处理解耦，采集侧通过 mpsc 发送 `CollectedBatch`，处理侧独立消费。真实处理链（`pipeline/mod.rs`）：

1. **EdgeProcessor**（`edge.rs` + `edge/`）— 死区过滤、变化上传、时间窗口聚合（last/avg/max/min/sum）
2. **协议转换模板**（`convert.rs` 的 `ConvertEngine`）— `raw` 直通 / `linear`（`value*scale+offset`）/ `enum`（映射文本到 `value_text`）三档免代码模板，按 `convert_rules` 配置命中，ArcSwap 原子热更；**不是**工程量换算（那是南向 `decode.rs`，见下）
3. **数据质量规则**（`quality_rules.rs`）— range / rate / nan 规则（`stale` 由调度器连续失败/超时负责，此处不重复）
4. **虚拟点位**（`virtual_point.rs` + `vformula.rs`）— 刷新实时值缓存后求派生量，与普通点一同广播/入队/上送
5. **入队** → `DataQueue`（SQLite WAL）
6. **实时广播** → `RealtimeBus`（broadcast channel）→ WebSocket 实时页；北向另行投递 `NorthFeed`（独立无损 mpsc 队列，不与广播总线共用，满时背压不丢帧）

> **工程量换算的位置**（易错）：`raw →(公式或 scale/offset)→ 工程量 →(×CT×PT，按单位门控)→ 一次侧` 在**南向协议解码阶段**
> `south/protocol/common/decode.rs::convert_value()` 完成（公式求值复用 `engine/formula.rs` 的 evalexpr），
> 不在 pipeline。pipeline 的 `ConvertEngine` 是另一套 raw/linear/enum 通用模板，两者刻意分离。
> 顺序、单位门控与写路径反算见 [`../点位配置与换算.md`](../点位配置与换算.md)。

**透传模式**：跳过边缘聚合+队列，直接 MQTT 直发（由 MQTT connector 实现），适用于低延迟、可接受断网丢失的点位。

**虚拟点位公式**（`vformula.rs`）是另一套自研递归下降引擎（多变量、可调函数、可引用其它点位），
与 `engine/formula.rs` 的 evalexpr（点位级工程量换算）刻意不合并，避免把点位换算的白名单边界撑破。
它在浏览器端有一份**等价实现**
（`gateway-web/src/utils/formulaEngine.ts`）用于表单预演与语法校验，
两侧语义由**同一份用例契约**钉住：[shared/formula_cases.json](../../shared/formula_cases.json)。
Rust 侧（`vformula/tests.rs`）与前端侧（`formulaEngine.spec.ts`）读同一个文件——
改语义只需改契约，只改单侧会在另一侧立刻转红。

### 6.5 队列层（queue/）

- SQLite WAL 模式，单线程独占 Connection（mpsc 串行访问，无死锁）
- 有界通道 1 万条，满了丢旧数据
- MQTT QoS1 ACK 机制：发布成功删除记录，失败保留重传
- in-flight 超时自动释放（5 分钟）
- `disk_full` 标志 + 退避时间：磁盘满时阻止入队，防 CPU 浪费
- 优雅退出排空等待

### 6.6 北向上报层（north/）

| 子模块 | 职责 |
|---|---|
| `mqtt/` | `MqttClient` 连接 broker，`outbox` 离线队列，`publish` 批量/透传 |
| `webhook/` | HTTP POST 回调上报（已拆为目录：`mod.rs` 入口、`client.rs` 发送/replay/flush 主循环、`retry.rs` 重试分类与退避、同目录测试文件）；数据先持久化到 outbox 再异步 POST，进程重启 replay 未确认消息 |
| `dispatch.rs` | `NorthDispatcher`：消费北向专用无损队列 `NorthFeed`，按注册顺序分发到各 `NorthConnector`（MQTT / Webhook / server adapters，单测在 `dispatch/tests.rs`） |
| `outbox_monitor.rs` | MQTT outbox 积压/失败率监控（暴露为指标与状态接口） |
| `server/registry.rs` | Server 注册表：按配置装配被动服务端（`NorthServerContext` / `NorthServerRegistry`） |
| `server/modbus_server.rs` | Modbus TCP Server，被动北向暴露点位 |
| `server/opcua_server.rs` | OPC UA Server，被动北向暴露点位 |
| `server/mqtt_packet_passthrough.rs` | MQTT 报文透传会话管理 |
| `server/modbus_gateway_bridge.rs` | Modbus 网关桥接（TCP↔RTU）：收 Modbus TCP 请求 → 去 MBAP 加 CRC 转 RTU → 经 BusManager 发串口 → 响应反向还原。只做帧格式转换、不做协议解析，配置键沿用 `modbus_passthrough` |
| `server_adapters/` | 数据推送适配器：`NorthDispatcher` 分发数据后主动喂给 server/ 下各服务端（区别于 server/ 的被动监听）；基类在 `base.rs` |
| `template/` | 上送 payload 模板渲染（解析 / 渲染 / 模板注册表） |

**三处 Modbus「服务端」能力的分工（勿误删）**：

| 模块 | 角色 | 数据来源 | 典型用途 |
|---|---|---|---|
| `server/modbus_server.rs` | Modbus TCP 从站 | 内存寄存器镜像（由 server adapter 写入） | 上位机/SCADA 以 Modbus 主站身份读网关 |
| `server_adapters/modbus_adapter.rs` | 数据推送侧通道 | `NorthDispatcher` 分发（源为北向 `NorthFeed`） | 把采集值持续写进上面那台从站的寄存器空间，是「点位 → 寄存器」的映射执行者 |
| `server/modbus_gateway_bridge.rs` | 协议桥接（网关中转） | TCP 客户端请求 | 上游用 Modbus TCP、现场设备只有 RTU 串口时做透明转换 |

前两者是「服务端 + 喂数者」的主从配对，第三个是独立的编译期可选桥接能力，三者不构成重复实现。

### 6.7 Web API 层（gateway-api crate 的 web/）

- axum Router 按鉴权分组（见 5.1）
- 中间件链（由外到内，对应 `build_router` 中 `.layer()` 的逆序叠加）：
  `request_id` → `CatchPanicLayer` → `CorsLayer` → `rate_limit` → `ConcurrencyLimitLayer(64)` → `TraceLayer` → `CompressionLayer` → `envelope`（统一响应信封兜底，必须挂在压缩层内层）
- `AppState` 共享：配置、队列、调度器、北向管理器、指标等
- 统一 `require_auth` / `require_auth_if_anon_disabled` 鉴权中间件
- 端点清单由 `scripts/gen_api_surface.py` 从各子域 `routes.rs` 生成到 [`docs/api-surface.md`](../api-surface.md)，CI 校验防漂移
- **前缀归属硬约束**：一个 URL 前缀（`/api/<seg>`）只能有一个 `routes.rs` 注册点；脚本的 `check_prefix_ownership` 会在 CI 拦下「同一前缀被多个子域注册」（历史上 `/api/devices*` 曾被 config / device_ops / devices 三个子域重复注册）。handler 可以留在原处，路由入口必须唯一
- **`/api/points` 下不允许静态段**：matchit 静态段优先于 `:id` 且不回落，任何 `/api/points/<static>` 都会让同名 id 的点位失去 PUT/DELETE（405）。点位工具与回收站因此各自独立前缀：`/api/point-tools/*`（批量写/虚拟点位/偏移/克隆/查找替换）、`/api/point-trash/*`。新增点位子功能时按此规则挑前缀，不要挂到 `/api/points` 下

**文件拆分**：`types.rs`（AppState + DTO + ApiResponse）、`error.rs`（ApiError + IntoResponse 转换）、
`middleware/`（原单文件 `middleware.rs` 约 1.4k 行已按职责拆分：`mod.rs` 统一 re-export 保持 `web::middleware::*` 对外路径不变，
子模块 `auth.rs` + `auth/`（鉴权）、`rate_limit.rs`（限流）、`request_id.rs`（请求 ID）、`envelope.rs`（统一信封）、`category.rs`（请求分类标记））

### 6.8 平台服务（platform/）

| 模块 | 职责 |
|---|---|
| `ntp.rs` | NTP 时间同步 |
| `ota/` | A/B 分区 OTA 升级与固件验签：**Ed25519**（公钥来自编译内置 + `WebConfig::ota_public_keys`，签名覆盖包头 + 载荷）为主，旧 HMAC-SHA256（`ota_signing_key`）仅兼容；**无签名/验签失败一律拒绝**（fail-closed），上传接口同步预校验并落审计，升级失败自动回滚 |
| `gpio.rs` | Linux GPIO 控制（gpiod） |
| `cellular/` | 蜂窝网络状态查询（含 manager/modem/stub） |
| `hardware_watchdog.rs` | 软件+硬件看门狗 |
| `watchdog_flush.rs` | 看门狗超时前的队列落盘冲刷（避免重启丢失未持久化数据） |

### 6.9 核心服务（engine/）

| 模块 | 职责 |
|---|---|
| `metrics/` | 指标采集（采集点数、队列深度、上送成功率等）；`scoped.rs` + `scoped/`（按作用域计数，O-6 拆分） |
| `alarm.rs` | 告警规则引擎（比较阈值 → ActiveAlarm） |
| `audit.rs` | 审计日志 |
| `error.rs` | 通用错误类型（GatewayError 等） |
| `logging.rs` | 日志初始化（tracing JSON + 滚动） |
| `supervision.rs` | 任务监督（spawn_supervised，panic 自动重启+指数退避） |
| `shutdown.rs` | 优雅停机信号 |
| `bus.rs` | 实时广播总线契约（`RealtimeBus` / `BusConsumerKind` / `LaggedCounts`），由 `pipeline/` 重导出 |
| `ports.rs` | 基础设施端口（Ports）契约：`AlarmPublisher`（报警上报）、`DiskHealthSink`（磁盘健康），实现分别在 `north/mqtt/` 与 `queue/` |
| `codec/` | 通用编解码工具：`crc.rs`（CRC 校验）、`encoding.rs`（base64 / 十六进制编码） |
| `formula.rs` | 点位级换算公式求值（evalexpr；原在 `pipeline::formula`，现已上提至中立的 `engine`，供南向 `decode.rs` 与处理层共用；与 `pipeline/vformula.rs` 的递归下降引擎区分） |
| `model.rs` | 公共领域模型（`DataPoint` 等） |
| `monitor/` | 监控与可观测性子模块（见下） |

**依赖方向（engine 是基础设施层）**：`engine` 只依赖 `config`，**不得**反向依赖 `north` / `queue` / `pipeline`。
这三条路径上的协作一律走「engine 定义契约、下游实现」的依赖倒置：

| engine 侧需求 | engine 定义的契约 | 实现方 |
|---|---|---|
| 报警事件上报 | `engine/ports.rs::AlarmPublisher` | `north/mqtt/alarm_publisher.rs`（`MqttClient`） |
| 磁盘健康回写队列 | `engine/ports.rs::DiskHealthSink` | `queue/mod.rs`（`DataQueue`） |
| 订阅实时广播总线 | `engine/bus.rs::RealtimeBus` | `pipeline/mod.rs` 发布（侧保留 `pub use` 重导出兼容既有调用点） |

该约束由 `scripts/deploy_lint.ps1` 的 C22 段守护（`engine/` 下出现 `crate::north|queue|pipeline` 即判失败）。

`engine/monitor/`：

| 模块 | 职责 |
|---|---|
| `health.rs` | 健康监控（磁盘/内存/CPU，更新 disk_full 标志；单测在同目录 `health/tests.rs`） |
| `host.rs` + `host/` | 主机信息采集（OS / CPU 核数 / 主机名 / 内存等，供状态接口与页面展示）；按平台拆分，见下表 |
| `heartbeat.rs` | 采集器心跳监控（超时置 disconnect） |
| `remote_diag.rs` + `remote_diag/` | 远程诊断通道（日志实时 tail 始终可用；只读白名单 shell 受 feature=`remote-shell` 门控，默认关闭，`POST /api/diagnostics/exec` 返回「功能未启用」）；按关注点拆分，见下表 |
| `otel.rs` | OTel metrics 桥接（OTLP 周期性导出，feature=`tracing-otlp`） |

`engine/monitor/host/`（O-6 拆分，原 `host.rs` 单文件 902 行）：

| 文件 | 职责 | 编译条件 |
|---|---|---|
| `host.rs` | 公共类型（`HostMetrics` / `NetInterfaceMetric`）、CPU 基线状态、跨平台入口 `collect()` | 全平台 |
| `host/parsers.rs` | `/proc/*` 文本 → 数值的纯解析函数（`parse_proc_stat` / `parse_meminfo` / `parse_loadavg` / `parse_uptime` / `parse_thermal_milli`） | 全平台 |
| `host/linux.rs` | Linux 采集实现（`/proc` + `/sys` + `getifaddrs`） | `target_os = "linux"` |
| `host/windows.rs` | Windows 采集实现（`GetSystemTimes` / `GlobalMemoryStatusEx` / `GetAdaptersAddresses` + `GetIfTable2`） | `windows` |
| `host/other.rs` | 其它平台：全部如实返回未知（`None` / 空列表），不编造数字 | 非 Linux 且非 Windows |
| `host/tests.rs` | 单元测试 | `cfg(test)` |

平台分发方式：`host.rs` 用 `use self::linux as platform;` / `use self::windows as platform;` / `use other as platform;` 三选一，
调用点统一写 `platform::xxx()`，各平台模块的函数签名保持一致。

`engine/monitor/remote_diag/`（O-6 拆分，原 `remote_diag.rs` 单文件 999 行）：

| 文件 | 职责 | 编译条件 |
|---|---|---|
| `remote_diag.rs` | `Diagnostics` 结构体与对外方法、日志缓冲与 tail 游标状态 | 全平台 |
| `remote_diag/log_archive.rs` | 日志归档切分与列举（`split_log_path` / `archive_path` / `list_log_archives_blocking`） | 全平台 |
| `remote_diag/tail.rs` | 日志 tail 读取与续传偏移计算（`read_tail` / `tail_consumable` / `tail_resume_offset`） | 全平台 |
| `remote_diag/shell.rs` | 只读白名单 shell：命令解析、白名单校验、输出截断 | `feature = "remote-shell"` |
| `remote_diag/tests.rs` | 单元测试 | `cfg(test)` |

### 6.10 目录层次与命名约定

**跨层同名概念**：以下同名是「配置」与「运行时实现」的刻意分层，不是重复代码。约定为——**配置结构体一律放 `config/`；运行时实现按能力层归属（`platform/` / `north/` / `south/`），运行时组件装配在 `runtime/`（`GatewayRuntime`），跨层纯工具与策略在 `shared/`（无 I/O、无业务层依赖）**。

| 概念 | 配置（config/） | 运行时实现 |
|---|---|---|
| ntp | `config/ntp.rs`（`NtpConfig`） | `platform/ntp.rs`（`NtpManager`） |
| webhook | `config/webhook.rs`（`WebhookConfig`） | `north/webhook/`（HTTP POST 上报，已拆为目录） |
| cellular | `config/cellular.rs`（`CellularConfig`） | `platform/cellular/`（拨号与状态查询） |
| mqtt | `config/mqtt.rs`（`MqttConfig`） | `north/mqtt/`（`MqttClient`） |
| opcua | `config/opcua.rs`（`OpcuaConfig`） | `south/protocol/opcua/`（采集）+ `north/server_adapters/opcua_adapter.rs`（被动暴露） |
| metrics | — | `engine/metrics/`（Prometheus `/api/metrics`） vs `engine/monitor/otel.rs`（OTLP 桥接） |

**DTO 类型存放约定**：

- 跨子域共享的 DTO / `AppState` / `ApiResponse` 放 `gateway-api` 的 `web/types.rs`；
- 仅单个子域使用的请求/响应 DTO 就近定义在 `gateway-api` 的 `web/api/<子域>/handlers/` 下：单文件 handler 写 `*.rs`，拆分成模块的写 `<模块>/mod.rs`（如 `protocol_tools/handlers/auto_detect/mod.rs`）；
- 需暴露给前端的类型经 ts-rs 导出到 `gateway-web/src/api/generated/`，其**类型名属对外契约，禁止随意重命名**。
---
