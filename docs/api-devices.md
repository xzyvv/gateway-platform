# 设备运维 API 契约

> 覆盖「设备运维」域的只读监控与写控制接口：通信质量、采集退避、健康分、设备影子、
> 拓扑、资产台账、远程控制、设备分组。
>
> 路由注册：[`gateway-api/src/web/api/devices/routes.rs`](../gateway-api/src/web/api/devices/routes.rs)
> （`/api/devices*` 唯一注册点：合并 `config` 的设备 CRUD、`device_ops` 的运维 handler 与本模块 control/topology/IO）。
> 设备配置 CRUD（`GET/POST /api/devices`、`PUT/DELETE /api/devices/{id}`）属配置域，不在本文档范围。

---

## 1. 通用约定

### 1.1 响应包装

所有端点返回 `ApiResponse<T>`（`gateway-api/src/web/types.rs`）：

```json
{ "code": 0, "message": "success", "data": <T> }
```

| 字段 | 说明 |
|---|---|
| `code` | `0` = 成功；`4000..5000` = 业务/参数错误（HTTP **400**）；其它非 0 = 服务端错误（HTTP **500**） |
| `message` | 成功固定 `"success"`，失败为可读原因 |
| `data` | 失败时为 `null` |

`ApiError` 走另一套响应体（`gateway-api/src/web/error.rs`），**没有 `data` 字段**：

```json
{ "code": 4004, "message": "device not found", "error": "device not found" }
```

| code | HTTP | 触发 |
|---|---|---|
| `4000` | 400 | 参数非法 / 业务校验失败（4xxx） |
| `4001` / `4002` / `4003` | 400 | 设备 / 点位 / 配置导入校验失败 |
| `4005` / `4006` / `4007` | 400 | 用户域：参数校验 / 用户不存在 / 角色非法 |
| `4004` | 404 | 资源不存在（`ApiError::not_found`） |
| `6001` / `6002` | 400 | 设备分组业务校验 / 分组不存在（6xxx，见 §9） |
| `5101` | 500 | 下位机写入失败（§5.3） |
| `8000` | 401 | 缺 token / token 无效 |
| `9999` | 500 | 服务端内部错误 |

> **状态映射的生效条件**：handler 必须**直接返回 `ApiResponse<T>`**（`ApiResponse` 自己实现
> `IntoResponse` 做区间映射）。若像 `Ok(Json(ApiResponse::err(..)))` 那样包一层 `Json`，
> 走的是 axum 默认 200，业务错误会以 **HTTP 200 + 非 0 code** 返回。
> 设备分组、配置域（设备/点位/配置导入）、用户域、`control/send` 均已改为直接返回。

> 例外：`POST /api/io/{index}`、`GET /api/io/status` 直接返回裸 JSON，
> **不套** `ApiResponse`（见 §10）；OTA / NTP 端点虽在同批 handler 中，但路由归属系统域
> （`api/system/routes.rs`），不在本文档范围。

### 1.2 鉴权

- 请求头二选一：`Authorization: Bearer <token>` 或 `X-API-Token: <token>`
  （`middleware.rs::extract_token_from_headers`）。
- **读端点**（本文档全部 `GET`）：`allow_anon_read=true` 时匿名可读，否则要求 token；
  配置缺 `web` 节时按 **fail-closed** 处理（`allow_anon_read()` 默认 false）。
- **写端点**（`POST` / `PUT` / `DELETE`）：**始终**要求 token（`require_auth`）。
- 缺 token 返回 401，`message` 为 `missing api token (use Authorization: Bearer <token> or X-API-Token)`。

### 1.3 数据来源与口径

- **设备状态真值源**：`Scheduler::device_status()` 快照（含设备模板展开的点位）。
  `cfg.points` 只有直配点位，用于反查模板化配置的归属会得到空值——本文档各端点均优先读快照，
  配置仅作兜底。点位归属语义见 [architecture/data-flow.md §7.1](architecture/data-flow.md)。
- **`device_id` 空串**：表示该点位在配置里未绑定设备（`DeviceConfig.id` 为空），
  前端应按 `bus` 归类。
- **时间戳**：统一毫秒（`i64`），`0` 表示「从未发生」。
- **质量码**：对外取值为 `good` / `uncertain` / `bad` / `old`（映射规则见 §3.1）。

---

## 2. 端点总览

| 方法 | 路径 | handler | 鉴权 |
|---|---|---|---|
| GET | `/api/devices/point-quality` | `list_point_quality` | 读 |
| GET | `/api/devices/point-quality/stats` | `point_quality_stats` | 读 |
| GET | `/api/devices/backoff` | `list_point_backoff` | 读 |
| GET | `/api/devices/health` | `list_health` | 读 |
| GET | `/api/devices/shadows` | `list_shadows` | 读 |
| GET | `/api/devices/topology` | `device_topology` | 读 |
| GET | `/api/devices/assets` | `list_assets` | 读 |
| GET | `/api/devices/{id}/asset` | `get_asset` | 读 |
| PUT | `/api/devices/{id}/asset` | `update_asset` | 写 |
| GET | `/api/devices/control/writable` | `list_writable_points` | 读 |
| GET | `/api/devices/control/commands` | `list_commands` | 读 |
| POST | `/api/devices/control/send` | `send_command` | 写 |
| GET | `/api/device-groups` | `list_groups` | 读 |
| POST | `/api/device-groups` | `create_group` | 写 |
| PUT | `/api/device-groups/{id}` | `update_group` | 写 |
| DELETE | `/api/device-groups/{id}` | `delete_group` | 写 |
| POST | `/api/device-groups/{id}/devices` | `add_device_to_group` | 写 |
| DELETE | `/api/device-groups/{id}/devices/{did}` | `remove_device_from_group` | 写 |
| GET | `/api/io/status` | `io_status` | 读（裸 JSON） |
| POST | `/api/io/{index}` | `control_io` | 写（裸 JSON） |

> 路径参数 `{id}` / `{did}` 为 `DeviceConfig.id` / 分组 ID；两者在配置里均允许为空串，
> 但作为路径参数时不能为空，否则路由不匹配。

---

## 3. 通信质量与退避

### 3.1 GET `/api/devices/point-quality`

点位级质量快照。**无 Query 参数**，一次性返回全部点位（含模板展开点位）。

响应 `data: PointQuality[]`：

| 字段 | 类型 | 说明 |
|---|---|---|
| `point_id` | String | 点位 ID |
| `point_name` | String | 点位名；配置缺失时回退为 `point_id` |
| `device_id` | String | **优先取 scheduler 快照**，为空才回退配置；仍为空表示未绑定设备 |
| `device_name` | String | 由 `device_id` 反查设备名，查不到回退 `device_id` 本身 |
| `value` | Option\<f64\> | 最近一次成功采集值（`last_good_value`） |
| `quality` | String | 见下方质量码映射 |
| `consecutive_failures` | u32 | 连续失败次数 |
| `last_update_at` | i64 | 毫秒；真实最后成功采集时刻（scheduler 快照 `last_good_ts`），从未成功时回退为请求时刻 |

**质量码映射**（`map_quality`，注意与内部原始码不同）：

| 原始 `quality` | `consecutive_failures` | 对外返回值 |
|---|---|---|
| `good` | 任意 | `good` |
| `stale` | 任意 | `uncertain` |
| 其它 | ≥ 3 | `bad` |
| 其它 | < 3 | `old` |

### 3.2 GET `/api/devices/point-quality/stats`

响应 `data: QualityStats`：`good`、`bad`、`stale`、`unknown`、`total`（均 u32）。

统计口径：`stale` = 上面的 `uncertain` 数量；`unknown` = `old` 数量（失败 < 3 次）；
`total` = 快照内点位总数。

### 3.3 GET `/api/devices/backoff`

采集退避可视化数据（对应设计 §7.3）。无 Query 参数，返回全部点位。

响应 `data: PointBackoff[]`：

| 字段 | 类型 | 说明 |
|---|---|---|
| `point_id` / `point_name` | String | 点位标识 / 名称 |
| `device_id` / `device_name` | String | 归属设备（快照优先，同 §3.1） |
| `bus` | String | 总线名（快照优先，配置兜底） |
| `base_interval_ms` | u64 | 配置的基础采集间隔 |
| `current_interval_ms` | u64 | 当前实际间隔（退避时放大） |
| `backoff_multiplier` | u32 | `current / base`，除零时取 1 |
| `consecutive_failures` | u32 | 连续失败次数 |
| `stale_count` | u32 | 失败期间用 `last_good` 重发次数 |
| `last_good_value` | Option\<f64\> | 上次成功值 |
| `last_good_ts` | i64 | 上次成功时间（毫秒，0 = 从未成功） |
| `quality` | String | 同 §3.1 映射 |
| `backing_off` | bool | `consecutive_failures >= 3` |

---

## 4. 设备健康

三个端点共用同一次计算（`build_all`），窗口固定**最近 24 小时**（`HEALTH_WINDOW_MS`）。
数据全部取真实运行时：成功率 / 延迟 / 失败数来自 `Metrics::scoped` 日桶，
离线次数取点位状态的 `offline_episodes`，最后在线取各点位 `last_good_ts` 最大值。

**点位归属规则**：设备点位 = 配置点位 ∪ 模板展开点位，满足任一：
`PointConfig.device_id == DeviceConfig.id`（设备 id 非空），或 `point_id` 以 `"{设备名}_"` 开头。
这是模板设备（id 为空）健康数据不丢失的关键。

### 4.1 GET `/api/devices/health`

无 Query 参数。返回配置中**全部设备**（无采集数据的设备也返回，`level` 为 `unknown`）。

响应 `data: DeviceHealth[]`：

| 字段 | 类型 | 说明 |
|---|---|---|
| `device_id` | String | 设备 `id`；为空时回退 `device.name` |
| `device_name` | String | 设备名 |
| `protocol` | String | 协议名 |
| `score` | Option\<i32\> | 健康分 0~100；窗口内无采集成 `null` |
| `level` | String | `excellent`(≥90) / `good`(≥70) / `warning`(≥50) / `critical`(<50) / `unknown` |
| `success_rate` | Option\<f64\> | 成功率百分比；无数据为 `null` |
| `avg_latency_ms` | Option\<i32\> | 平均延迟（四舍五入取整）；无数据为 `null` |
| `error_count` | i32 | 24h 内失败次数 |
| `offline_count` | i32 | 累计离线次数（「正常 → 首次失败」，成功不清零） |
| `last_seen_at` | Option\<i64\> | 最后成功采集时间（毫秒）；从未成功为 `null` |
| `details` | HealthDetail[] | 固定 4 项：`{label, value, score}` |

**评分算法**（`score_from`）：

```
score = 100
      − (100 − success_rate) × 0.6
      − clamp(avg_latency_ms, 0, 300) / 10
      − clamp(error_count, 0, 10) × 2
      − clamp(offline_count, 0, 4) × 5
```

结果四舍五入并截断到 0~100；成功率或延迟缺失时整体 `score = null`。
`details` 的四项分数为展示用子分（成功率 ×0.6、延迟 `30 − ms/10`、失败 `20 − n×2`、离线 `20 − n×5`），与总分算法不完全等价，仅用于进度条。

### 4.2 单设备健康快照（原 `GET /api/devices/{id}/health`，已删除）

第 1 期冗余收口：**该端点已删除**。其内容与 §4.1 列表同源，改为在列表结果内按
`device_id` / `device_name` 匹配取用（前端 `findHealthView()`）。

### 4.3 健康汇总（原 `GET /api/devices/health/summary`，已删除）

第 1 期冗余收口：**该端点已删除**。汇总口径由前端从 §4.1 列表派生（前端 `summarizeHealth()`），
后端只保留一份判据。各等级设备数含义不变（五项之和 = 设备总数），
`avg_score` 为「对有评分设备取整均值；全部无数据时为 `null`」。

---

## 5. 远程控制（点位写入）

### 5.1 GET `/api/devices/control/writable`

可写入点位白名单。**只遍历 `cfg.points`（直配点位）**，且仅 `writable == true` 的点位返回；
模板展开点位不在其中（`expand_devices()` 展开出的点位固定 `writable=false`）。

响应 `data: WritablePoint[]`：

| 字段 | 类型 | 说明 |
|---|---|---|
| `id` | String | 点位 ID |
| `name` | String | 点位名；为空回退 `id` |
| `data_type` | String | 数据类型；为空回退 `"float"` |
| `device_id` | String | 所属设备 ID；为空表示未绑定 |
| `device_name` | String | 设备名；设备查不到时回退该点位的 `bus` 名 |

### 5.2 GET `/api/devices/control/commands`

控制命令历史，来源是**审计日志回读**（`action = "point.write"`，最多 50 条，无分页参数）。

响应 `data: ControlCommand[]`。字段为审计记录映射：

| 字段 | 说明 |
|---|---|
| `id` | `cmd-{audit.ts}` |
| `device_id` / `device_name` | 均为审计 `target`（下发时写入设备 ID 或总线名） |
| `point_id` / `point_name` | 均为审计 `detail` 拆出的点位名（审计未存 point_id） |
| `write_value` | 从 `detail` 的 `{点位名}={值}` 拆出的真实写入值 |
| `operator` | 审计 `source`（接口下发为 `"api"`） |
| `status` | `"success"` / `"failed"`（§5.3 写入失败同样落审计，detail 带 ` [失败: ...]` 后缀） |
| `created_at` / `executed_at` | 审计时间戳，两者相同 |
| `result` | 审计 `detail` 原文 |

> 该端点反映审计库中已记录的写入（含失败）；命令下发的同步结果仍以 §5.3 响应为准。

### 5.3 POST `/api/devices/control/send`

请求体：

```json
{ "point_id": "sim-meter.voltage", "value": 220.0 }
```

`SendCommandRequest`：`point_id: String`（必填）、`value: f64`（必填）。

处理流程与错误：

| 场景 | 返回 |
|---|---|
| 点位不存在 | 404，`code=4004`，`point {id} not found` |
| 点位 `writable != true` | 400，`code=4000`，提示需在点位配置将 `writable` 置为 true |
| 找不到该点位 `bus` 对应的 collector | 404，`code=4004`，`collector for bus {bus} not found` |
| 下位机写入失败 | **500**，`code=5101`，`写入失败：{原因}` |
| 成功 | 200，`data: ControlCommand`（`status="success"`、`operator="api"`、`result="写入成功：{value}"`） |

> `5101` 按服务端执行失败映射为 HTTP 500，响应体仍是 `{code,message,data:null}`。
> 前端 axios 拦截器会同时按 HTTP 状态与 `code != 0` 走错误提示，不会把失败误报为「下发成功」。

---

## 6. 拓扑

### 6.1 GET `/api/devices/topology`

返回 `总线 → 设备 → 点位` 三层图。响应 `data: TopologyData`：`{ nodes: TopoNode[], links: TopoLink[], stats: TopoStats }`。

`TopoNode`：`id`、`name`、`node_type`（`bus` / `device` / `point`）、`status`（可空）、`meta`（可空）。

| 节点类型 | `id` / `name` | `meta` | `status` |
|---|---|---|---|
| bus | `bus.port_name` / 同 | `"{protocol} @ {baud_rate} baud"` | 恒为 `null` |
| device | `device.id` / `device.name` | `"slave: {slave_id}"`，缺省 0 | `online`（任一点位 good）/ `warning`（无 good 但有 stale）/ `offline`（其余，含无点位设备） |
| point | `point.id` / 点位名（空回退 id） | `"addr: {address}"` | `online`（quality == good）/ `offline`（其余）；快照缺失为 `null` |

`TopoLink`：`{ source, target }`。连线规则：`device.bus → device.id`；`point.device_id → point.id`
（`device_id` 为空时按 `"{设备名}_"` 前缀反查模板展开点位的归属设备；仍查不到则不连线，点位成为游离节点）。

> 数据来源是**配置**（`cfg.buses` / `cfg.devices` / `cfg.points`）+ **模板展开点位**
> （`templates.expand_devices`）+ 调度器质量快照。模板展开点位会出现在拓扑里并参与
> 设备状态判定（与 device_ops/health.rs 同口径）。设备级健康/质量统计请看 §4 / §3。

### 6.2 拓扑统计（原 `GET /api/devices/topology/stats`，已删除）

第 1 期冗余收口：**该端点已删除**，统计随 §6.1 的 `stats` 字段一并返回（`data.stats: TopoStats`）：

| 字段 | 类型 | 说明 |
|---|---|---|
| `bus_count` | usize | 配置总线数 |
| `device_count` | usize | 配置设备数 |
| `online_count` / `warning_count` / `offline_count` | usize | 按 §6.1 同口径聚合的设备数，三项之和 = `device_count` |
| `point_count` | usize | 点位总数（`cfg.points` + 模板展开点位） |

---

## 7. 设备影子

### 7.1 GET `/api/devices/shadows`

无 Query 参数，返回 `cfg.devices` 中每台设备的影子对象。响应 `data: DeviceShadow[]`。

### 7.2 单设备影子（原 `GET /api/devices/{id}/shadow`，已删除）

第 1 期冗余收口：**该端点已删除**。改为在 §7.1 列表结果内按 `device_id` 精确匹配取用
（前端 `findShadowView()`；原端点也不支持设备名匹配）。

`DeviceShadow` 字段：

| 字段 | 类型 | 说明 |
|---|---|---|
| `device_id` / `device_name` / `protocol` | String | 来自设备配置 |
| `expected_interval_ms` | i64 | 期望采集间隔 = `device.interval`（秒）× 1000，缺省 5 秒 |
| `last_value_hint` | Option\<String\> | 该设备最近一次成功采集值（快照 `last_good_value`，取 `last_good_ts` 最大者）；无成功记录为 `null` |
| `last_quality` | String | 设备点位质量最差值（`good` / `uncertain` / `bad`）；无点位快照（未采集）为 `uncertain` |
| `last_update_at` | i64 | 最近一次成功采集时刻（毫秒）；从未成功为 `0`（前端显示「—」） |
| `points` | ShadowPoint[] | 该设备的真实点位列表（`{point_id, quality, last_value}`），来自 scheduler 快照 |

> **数据来源**：scheduler 点位状态快照（含模板展开点位，按 `device_id` 匹配 +
> `{设备名}_` 前缀兜底，与 §6 拓扑 / §4 健康同口径）。历史 V1 的伪随机占位已移除。

---

## 8. 资产台账

一设备一资产，`device_id` 既作主键也作分组键；`PUT` 即 upsert（无则创建），持久化到
`GatewayConfig.device_assets`。

### 8.1 GET `/api/devices/assets`

响应 `data: DeviceAsset[]`（配置中 `device_assets` 全量，`null` 时返回空数组）。

`DeviceAsset` 字段：`device_id`、`asset_no`、`vendor`、`model`、`location`、`owner`、
`purchased_at`（采购日期，ISO8601 字符串）、`calibrated_at`（上次校准日期）、`tags: string[]`。

### 8.2 GET `/api/devices/{id}/asset`

按 `device_id` 精确匹配，未命中返回 404 `"asset not found"`。响应 `data: DeviceAsset`。

### 8.3 PUT `/api/devices/{id}/asset`

请求体（`UpdateAssetRequest`，所有字段可选，缺省为空串/空数组）：

```json
{
  "asset_no": "GW-2024-001",
  "vendor": "Acme",
  "model": "M-100",
  "location": "1F 配电间",
  "owner": "张三",
  "purchased_at": "2024-01-15",
  "calibrated_at": "2024-06-01",
  "tags": ["关键", "计量"]
}
```

响应 `data: DeviceAsset`：`device_id`（取路径参数）+ 上述字段。
覆盖写入：已存在则整体替换（未传的字段被置为空值，不是部分更新）。

---

## 9. 设备分组

持久化到 `GatewayConfig.device_groups`；所有写操作都在 `config_reload_lock` 下克隆配置、
改完调用 `reload_then_save`（热重载运行时配置并写入 `config.json`）。

`DeviceGroup` 结构：`id`、`name`、`description`、`color`（hex）、`device_ids: string[]`、
`created_at`（毫秒）。

| 端点 | 请求体 / 参数 | 响应 `data` |
|---|---|---|
| GET `/api/device-groups` | 无 | `DeviceGroup[]`（`device_groups` 为 null 时返回 `[]`） |
| POST `/api/device-groups` | `{ id, name, description?, color?, device_ids? }` | 新建的 `DeviceGroup` |
| PUT `/api/device-groups/{id}` | `{ name?, description?, color?, device_ids? }` | `null`（只更新传入字段） |
| DELETE `/api/device-groups/{id}` | 无 | `null` |
| POST `/api/device-groups/{id}/devices` | 路径 `{id}` + 请求体 `{ "device_id": "..." }` | `null` |
| DELETE `/api/device-groups/{id}/devices/{did}` | 无 | `null` |

> `POST /api/device-groups/{id}/devices` 的 `device_id` 走**请求体**（`AddDeviceToGroupRequest`），
> 不是第二段路径。前端 `deviceOps.ts::addDeviceToGroup` 与本文一致。
> 重复加入同一设备幂等（不会产生重复项）。

**业务错误**（`ApiResponse::err`，直接返回 `ApiResponse` 走状态映射）：

| code | 场景 | HTTP |
|---|---|---|
| `6001` | 组名为空 / 组名重复 | **400** |
| `6002` | 分组不存在（update / delete / 加设备 / 移设备，以及 `device_groups` 为 null） | **400** |

> 加设备不校验设备是否存在；重复加入同一 `device_id` 不会产生重复项（幂等）；
> 移除不存在的设备也返回成功（幂等）。

---

## 10. IO 控制

这两个端点注册在 [`gateway-api/src/web/api/devices/routes.rs`](../gateway-api/src/web/api/devices/routes.rs)，
与 §3~§5 的 `devices` handler 同模块，鉴权策略相同（GET 看 `allow_anon_read`，POST 必须 token）。
**返回裸 JSON，不套 `ApiResponse`**。

> **设备发现端点已下线**：原 `POST /api/discover` 与 `POST /api/protocol-tools/slave-scan`
> 是同一套扫描引擎的等价实现（同一校验、同一错误语义），现已统一为 slave-scan，
> 契约见 [api-protocol-tools.md](api-protocol-tools.md)。前端的设备发现向导、
> 设备扫描弹窗与单从站连通性探测均改调 slave-scan，不再保留第二套封装。

### 10.1 GET `/api/io/status`

返回 GPIO 原始状态（`IoStatus`），4 路固定长度数组：

```json
{ "di": [false, true, false, false], "di_quality": [true, true, false, false], "do_state": [false, false, true, false] }
```

`di` = 数字输入电平；`di_quality` = 该路 DI 读数是否有效；`do_state` = 数字输出当前状态。

### 10.3 POST `/api/io/{index}`

请求体：`{ "action": "on" | "off" }`（`IoControlRequest`，必填）。

| 场景 | 返回 |
|---|---|
| 成功 | 200，`{ "status": "ok", "index": <index>, "on": true/false }` |
| `action` 不是 `on` / `off` | 400，`code=4000`，`invalid action: {action}` |
| 写 GPIO 失败 | 由 `GatewayError` 映射为 `ApiError`（错误码随底层错误，通常 4xx/5xx） |

---

## 11. 已知偏差与维护约定

**维护约定**：修改上述 handler 的字段或语义时，同一 PR 内更新本文档对应小节；新增端点
同步补进 §2 总览表。

**待办（源码现状与契约不一致，需修代码而非改文档）**：无。

> 已修复（2026-10-27）：拓扑模板展开点位缺失（§6）、point-quality `last_update_at`
> 恒为请求时刻（§3.1）、discover 桩未扫描（§10.1）、control/commands 占位值（§5.2）、
> 设备影子伪随机值（§7）。

**相关文档**：

- 端点总入口与文档地图：[INDEX.md](INDEX.md)
- 点位归属 `device_id` 语义：[architecture/data-flow.md §7.1](architecture/data-flow.md)
- 认证与配置域接口：[api-access.md](api-access.md)
