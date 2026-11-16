# 通信协议

> Gateway Platform 文档子页：原 [architecture.md](../architecture.md) 第 5 节独立成页，便于按主题检索与维护。

---

## 5. 通信协议

### 5.1 前后端通信

#### REST API（HTTP/JSON）

统一响应格式：

```json
{ "code": 0, "message": "success", "data": {} }
```

- `code === 0` 表示成功，前端拦截器自动解包 `data`
- 非 0 为业务错误，`message` 作为提示

鉴权：请求头 `Authorization: Bearer <api_token>`，token 存于前端 `localStorage.gateway_api_token`。

**路由分组**（`gateway-api/src/web/mod.rs` 的 `build_router`，下表顺序即源码 `merge` 顺序；每域自带鉴权分层）：

| 子域（`api::*_routes`） | 鉴权 | 说明 |
|---|---|---|
| `health_routes`（内联） | 无 | `/health/live`、`/health/ready`，供 K8s / 负载均衡探测 |
| `auth_routes` | 分层 | `POST /api/auth/login` 无鉴权；`whoami` / 改密始终需 token |
| `system_routes` | 分层 | 读：`allow_anon_read=false` 时需 token；写：始终需 token（状态 / 版本 / 指标 / 健康报告 / OTA / NTP / 服务与证书 / 上行与死信） |
| `audit_routes` | 读层 | 审计日志查询 |
| `history_routes` | 读层 | 历史数据查询与 CSV 导出（带截断标记响应头） |
| `realtime_routes` | 读层 | 实时数据 WebSocket |
| `license_routes` | 分层 | 授权状态只读 + 导入/激活写入 |
| `devices_routes` | 分层 | **`/api/devices*` 唯一注册点**：设备 CRUD（原 `config`）+ 运维（原 `device_ops`）+ IO 控制 / 设备模板 |
| `points_routes` | 分层 | **`/api/points*` 唯一注册点**：点位 CRUD / 回收站（原 `config`）+ 即时读写 + 虚拟点位 / 批量工具（原 `points_ops`） |
| `config_routes` | 分层 | **`/api/config*` 唯一注册点** + `/api/project/*` + `/api/buses/*`（导入导出 / 热重载 / 校验 / 版本与回滚） |
| `uplink_routes` | 分层 | 上送通道配置 + 上送测试 |
| `diagnostics_routes` | 分层 | **`/api/diagnostics*` 唯一注册点**：诊断聚合 / 日志 tail 与归档 / 只读 Shell（受 `remote-shell` 门控） |
| `access_routes` | 分层 | API 密钥与限流规则 |
| `protocol_tools_routes` | 分层 | 协议工具（报文解析/组帧/连通性测试等） |
| `fallback_service` | — | `gateway-web/dist/` 静态托管 + SPA fallback（`/api`、`/health` 未命中回 404 JSON） |

> 「分层」= 同一子域内读接口用 `require_auth_if_anon_disabled`、写接口用 `require_auth`。
> 全量端点清单见 [`docs/api-surface.md`](../api-surface.md)（由 `scripts/gen_api_surface.py` 生成并纳入 CI 校验）。

**主要端点**：

| 方法 | 路径 | 说明 |
|---|---|---|
| POST | `/api/auth/login` | 用户名密码登录，返回 token |
| POST | `/api/auth/password` | 修改密码 |
| GET | `/api/config` | 获取完整配置 |
| POST | `/api/config` | 保存配置（热加载） |
| GET/POST | `/api/config/versions` | 配置版本列表 / 回滚 |
| GET | `/api/config/versions/:id/compare` | 版本差异对比 |
| POST | `/api/config/diff` | 配置差异计算 |
| GET | `/api/status` | 网关运行状态 |
| GET | `/api/metrics` | Prometheus 风格指标 |
| GET | `/api/realtime` | **WebSocket** 实时数据推送 |
| GET | `/api/diagnostics/logs` | **WebSocket** 诊断日志流 |
| GET | `/api/history` | 历史数据查询 |
| GET | `/api/history/export` | 历史数据 CSV 导出 |
| GET | `/api/points/{id}/read` | 立即读取单个点位 |
| POST | `/api/point-tools/batch` | 点位批量导入 |
| GET | `/api/point-tools/export` | 配置点位批量导出（CSV/JSON 的前端另存数据源） |
| POST | `/api/point-tools/write` | 点位统一写入口 |
| POST | `/api/discover` | 设备发现 |
| GET/POST | `/api/io/status`、`/api/io/{index}` | GPIO 状态 / 控制 |
| GET/POST | `/api/ota/status`、`/api/ota/upload`、`/api/ota/rollback` | OTA 升级 |
| GET/POST | `/api/ntp/status`、`/api/ntp/sync` | NTP 状态 / 同步 |
| GET | `/api/audit/logs` | 审计日志 |
| GET/POST/DELETE | `/api/templates` | 设备模板管理 |
| GET | `/api/north/status` | 北向服务统一状态 |
| GET | `/api/cellular/status` | 蜂窝网络状态 |
| GET | `/api/certificates` | X.509 证书信息（列表项含 `days_remaining` / `cert_path`） |
| GET | `/api/passthrough/sessions` | 透传会话列表 |
| GET | `/api/health/disk/history` | 磁盘健康历史 |

#### 测点批量导出 / 导入的往返语义

测点的导出与导入共用同一套列名与转换规则（见前端 `src/utils/pointFields.ts`），保证「导出 → 改 → 导回」不丢字段。

| 环节 | 档位 | 说明 |
|---|---|---|
| 导出 | 精简 CSV | 10 列（id / 名称 / 设备 / 总线 / 协议 / 地址 / 从站 / 地址偏移 / CT / PT），与导入页精简模板逐列一致 |
| 导出 | 完整 CSV | 全部可平铺字段（含 scale、offset、unit、公式、告警阈值、可写标记等）；`aggregation` 是嵌套结构，CSV 装不下，不在其中 |
| 导出 | JSON | `PointConfig[]` 原样快照，可整体粘贴到导入页的「粘贴 JSON」 |
| 导入 | 合并（默认） | 以现场现有点位为基底，只覆盖 CSV 里**出现过**的列；没出现的列保留原值，出现的列留空表示清空该字段 |
| 导入 | 覆盖 | 整点替换，CSV 没提到的字段一律重置为默认值（历史行为） |

两个语义要点：

- **不可清空字段**：留空表示保留原值，而不是写默认值。`scale` / `offset` / `interval` / `deadband` 属于这一类——若把「空单元格」当成 0，现场测点值会被直接乘成 0。
- **导出范围**：只含配置点位（`cfg.points`），不含模板设备展开出来的点位；展开点位导回去会被固化成显式点位，与运行期的模板展开结果重复。

#### 单位的取值口径

单位是上行数据的量纲标识，同义写法（`℃` / `C`、`kWh` / `KWH`、`m3` / `m³`）会让上行分组与统计出现重复维度。

- **表单控件**：点位表单、虚拟点位、设备模板三处统一为纯下拉，禁止自由输入，清单见前端 `src/composables/devices/useUnits.ts`。清单必须覆盖内置模板里实际出现的单位，否则模板建出来的点位在下拉里显示空白。
- **存量兼容**：早期版本允许自由输入，配置里可能存在清单外的值。下拉会把当前值补进选项（`unitChoices`），保证老点位看得见、改得动、不被静默改写。
- **CSV 导入**：unit 列仍接受任意文本——「导出 → 改 → 导回」的无损往返是硬约束，若导入侧按白名单拒绝，存量点位导回会被打回。
- **变比适用范围**：单位同时决定 CT/PT 是否参与计算（`V`/`kV`/`mV` 只吃 PT，`A`/`kA`/`mA` 只吃 CT，`PF`/`cosφ` 都不吃，其余按 CT × PT）。前端 `useRatio.ts` 与后端 `config/point.rs::ratio_scope` 是同一套判定，改动需同步。

#### WebSocket

| 端点 | 用途 | 推送内容 |
|---|---|---|
| `/api/realtime` | 实时数据 | 采集到的 DataPoint 流（点位值、质量、时间戳） |
| `/api/diagnostics/logs` | 诊断日志 | 结构化日志行（target/level/message） |

### 5.2 南向协议（设备采集）

通过 `ProtocolRegistry` 插件化注册，新增协议只需 `register()`，无需改调度器/总线。

| 协议 | 传输 | 采集器 | 说明 |
|---|---|---|---|
| **Modbus** | RTU (串口) / TCP / ASCII | `ModbusCollector` | 批量合并读，功能码 01/02/03/04/05/06/15/16 |
| **DLT645** | 串口 | `Dlt645Collector` | 电表协议，广播校时、事件记录 |
| **CJT188** | 串口 | `Cjt188Collector` | 水表/燃气表协议 |
| **IEC104** | TCP | `Iec104Collector` | 远动规约（IEC 60870-5-104） |
| **IEC101** | 串口 | `Iec101Collector` | 远动规约串行链路（FT1.2 帧格式） |
| **BACnet** | UDP/IP | `BacnetCollector` | 楼宇自控（BACnet/IP） |
| **BACnet MSTP** | 串口 | `BacnetMstpCollector` | BACnet 主从令牌传递（RS485） |
| **S7Comm** | TCP | `S7CommCollector` | 西门子 PLC |
| **OPC UA** | TCP | `OpcUaCollector` | OPC UA Client 采集 |
| **三菱 MC** | TCP | `McCollector` | QnA 3E 二进制帧 |
| **欧姆龙 FINS** | TCP | `FinsCollector` | FINS/TCP |
| **GPIO** | 本地 | `GpioCollector` | 点位级协议（DI/DO），不经总线、始终编译 |

> 上表 12 个协议中，除 `gpio` 外的 11 个均受 Cargo feature gate 控制（编译期剥离），
> feature 名与裁剪方式见仓库根目录 `PROTOCOL_FEATURE_GATES.md`。

### 5.3 北向协议（数据上报）

| 通道 | 技术 | 说明 |
|---|---|---|
| **MQTT** | rumqttc | QoS1 ACK 确认 + SQLite outbox 离线续传，支持 TLS/WS |
| **Webhook** | reqwest (HTTP POST) | JSON 回调，rustls TLS |
| **OPC UA Server** | async-opcua | 北向暴露点位给 OPC UA Client |
| **Modbus Server** | 自研 | 北向暴露点位给 Modbus 主站 |
| **透传 (Passthrough)** | MQTT | 跳过边缘聚合+队列，低延迟直发 |
| **模拟消费者 (SimConsumer)** | 内置 | 无 MQTT 时的兜底消费者：按批 `dequeue → 处理 → ack` 消费队列但不外发数据，用于消除 `has_consumer=false` 的告警歧义（配置项 `sim_consumer`，默认关闭，与 MQTT 互斥）；**运行时可启停**（`POST /api/sim_consumer/start|stop`，状态见 `GET /api/sim_consumer/status`），指标 `gateway_sim_consumer_consumed_total` / `gateway_sim_consumer_batches_total` / `gateway_sim_consumer_running` |

`NorthDispatcher` 消费北向专用无损队列 `NorthFeed`，将采集数据分发给所有已注册的 `NorthConnector`。

> **写回与总线超时的关系**：OPC UA Server 的写回调会同步等待真实写结果（下限 3s，
> 按 `3×buses[].timeout_ms+500ms` 推导，5s 封顶）。等待期间 async-opcua 持有地址空间
> 写锁，该 Server 的读与订阅通知都会被阻塞。因此参与 OPC UA 写回的总线建议
> `timeout_ms ≤ 1000`；写频繁的站点请让客户端串行写，避免多个写请求在锁上排队、
> 把阻塞时长按倍数叠加。

---
