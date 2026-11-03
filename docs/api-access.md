# 访问控制 API 契约（/api/access/*）

覆盖限流规则、API 密钥、计划任务、访问日志与统计五组接口。本文档字段名与
`gateway-api/src/web/api/access/`（handlers / routes / executor）、
`gateway-core/src/config/store/`（rate_rules / api_keys / scheduled_tasks）一致，
请求/响应示例取自 `gateway-api/tests/access_api.rs`。

所有 JSON 字段均为 **camelCase**。时间戳除特别说明外均为 **epoch 毫秒**。

## 1. 通用约定

### 1.1 响应信封

成功（HTTP 200）：

```json
{ "code": 0, "message": "success", "data": { } }
```

失败（HTTP 非 200）：

```json
{ "code": 8000, "message": "invalid api token", "error": "invalid api token" }
```

失败响应同时带 `error` 与 `message`（内容相同，`error` 用于向后兼容），`data` 字段不存在。

### 1.2 鉴权

凭证按优先级从两处提取：`Authorization: Bearer <token>`，或 `X-API-Token: <token>`。
token 可以是主 API token，也可以是本模块签发的 API 密钥（`gwk_…`）。

| 分层 | 中间件 | 语义 |
|---|---|---|
| 读接口（GET） | `require_auth_if_anon_disabled` | `web.allow_anon_read=true` 时匿名可读；否则要求主 token 或任意启用中的 API 密钥（不校验 scope） |
| 写接口（POST/PUT/PATCH/DELETE） | `require_auth` | 要求主 token，或持有**该路径所属域**写 scope 的 API 密钥（域映射见 §3 作用域表）；系统域写操作仅主 token 可执行 |

API 密钥不存在 / 已停用 / 已过期与「错误 token」返回完全相同的 401，不对外区分（防枚举）。
密钥被停用后立即失效；每次鉴权成功会累计该密钥的 `callCount` 并刷新 `lastUsedAt`。

### 1.3 错误码

| HTTP | code | 含义 |
|---|---|---|
| 200 | 0 | 成功 |
| 400 | 4000 | 参数校验失败（`message` 含具体原因） |
| 401 | 8000 | 未携带凭证 / token 无效 / 密钥停用或过期 |
| 403 | 8003 | API 密钥缺少该路径所属域的写权限 scope（`api key lacks write scope`） |
| 404 | 4004 | 目标对象不存在 |
| 429 | — | 触发限流，响应头固定带 `Retry-After: 1`（秒），无 JSON 信封 |

429 由全局限速（`web.rate_limit_per_sec`）或限流规则触发，按客户端 IP 分桶计数。
`X-Forwarded-For` 仅在 TCP 对端为回环地址时采纳首跳，详见
[部署架构 §8.5](architecture/deployment.md#85-反向代理与限流)。

## 2. 限流规则

规则匹配语义：`path` 为精确路径（`/api/devices`）或以 `*` 结尾的前缀（`/api/points*`）；
`method` 为 `GET/POST/PUT/DELETE/PATCH` 或 `*`（全部，大小写不敏感，入库统一归一化为大写）。
限流窗口语义：分钟窗口允许 `limitPerMinute + burst` 次，小时窗口允许 `limitPerHour` 次；
多条规则命中同一请求时任一超限即拒（最严生效）。

### 2.1 规则对象（RateRule）

| 字段 | 类型 | 说明 |
|---|---|---|
| id | string | 规则 ID，`rr_` 前缀 |
| name | string | 名称 |
| path | string | 匹配路径 |
| method | string | `GET/POST/PUT/DELETE/PATCH/*` |
| limitPerMinute | u32 | 每分钟限额 |
| limitPerHour | u32 | 每小时限额 |
| burst | u32 | 分钟窗口内额外允许的突发数 |
| enabled | bool | 是否启用 |
| createdAt | i64 | 创建时间（毫秒） |
| updatedAt | i64 | 更新时间（毫秒） |

### 2.2 GET /api/access/rate-rules（读）

返回全部规则（按创建时间升序）。

```json
{
  "code": 0,
  "message": "success",
  "data": [
    {
      "id": "rr_18f3c2a1b0001",
      "name": "点位读取",
      "path": "/api/points",
      "method": "GET",
      "limitPerMinute": 30,
      "limitPerHour": 1000,
      "burst": 0,
      "enabled": true,
      "createdAt": 1729900000000,
      "updatedAt": 1729900000000
    }
  ]
}
```

```bash
curl http://127.0.0.1:8080/api/access/rate-rules \
  -H "Authorization: Bearer <token>"
```

### 2.3 POST /api/access/rate-rules（写）

请求体：

| 字段 | 类型 | 必填 | 说明 |
|---|---|---|---|
| name | string | 是 | 非空且 ≤ 64 字符 |
| path | string | 是 | 以 `/` 开头且 ≤ 256 字符 |
| method | string | 是 | `GET/POST/PUT/DELETE/PATCH/*`，小写自动转大写 |
| limitPerMinute | u32 | 是 | ≥ 1 且 ≤ 1 000 000 |
| limitPerHour | u32 | 是 | ≥ 1 且 ≤ 100 000 000 |
| burst | u32 | 否 | 默认 0 |
| enabled | bool | 否 | 默认 true |

附加约束：**规则 path 不允许覆盖 `/api/access/*` 管理入口**（如 `/api/access*`、
`/api/*`、`/*`），否则误配后无法进入面板解除限流，服务端返回 400
（`限流规则不能覆盖 /api/access/*（限流/密钥/任务管理入口）…`）。

响应：`data` 为创建后的完整规则对象（见 2.1），HTTP 200。

```bash
curl -X POST http://127.0.0.1:8080/api/access/rate-rules \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"name":"点位读取","path":"/api/points","method":"GET","limitPerMinute":30,"limitPerHour":1000,"burst":0,"enabled":true}'
```

### 2.4 PUT /api/access/rate-rules/{id}（写）

请求体同 2.3（全量替换可变字段，更新 `updatedAt`）。规则不存在返回 404/4004。
响应 `data` 为更新后的规则对象。

### 2.5 DELETE /api/access/rate-rules/{id}（写）

响应 `data: true`；规则不存在返回 404/4004。

```bash
curl -X DELETE http://127.0.0.1:8080/api/access/rate-rules/rr_18f3c2a1b0001 \
  -H "Authorization: Bearer <token>"
```

## 3. API 密钥

- 完整 Key 格式为 `gwk_` + 40 位小写 hex（共 44 字符），由操作系统 CSPRNG 生成；
- **完整 Key 仅在创建接口返回一次**，服务端只保存 SHA-256 摘要，无法找回；
- 列表/其他接口只返回 `keyPreview`：前 8 字符 + `…` + 后 4 字符（如 `gwk_ab12…9f0e`）；
- 密钥停用后**立即失效**，且失效表现与错误 token 相同（401，不区分原因）。

scope 白名单（创建时逐项校验，未知值返回 400 `未知权限范围: <scope>`）：

```
devices:read  devices:write  points:read  points:write
config:read   config:write   history:read
```

写接口按**路径所属域**校验 scope（仅写方法生效，GET 只要求密钥有效）：

| 路径前缀 | 需要的写 scope |
|---|---|
| `/api/devices`、`/api/device-groups`、`/api/io`、`/api/discover`、`/api/diagnostics`（诊断写）、`/api/quality`（数据质量规则写） | `devices:write` |
| `/api/points`（含 `/api/points/virtual`、`/api/points/clone` 等） | `points:write` |
| `/api/config`、`/api/buses`、`/api/uplink`、`/api/templates`、`/api/project`、`/api/protocol-tools/conversion`、`/api/sim_consumer` | `config:write` |
| `/api/protocol-tools` 中**真实发帧 / 占线**的写操作（`packet-editor/send`、`register-scan`、`test/run`、`auto-detect/run`、`packet-replay/*/execute`） | **仅主 token**（与系统域同级） |
| `/api/protocol-tools` 其余写操作（presets、抓包清理、回放会话 CRUD） | `devices:write` |
| `/api/access`、`/api/auth`、`/api/system`、`/api/ota`、`/api/ntp`、`/api/certificates`、`/api/network` 等系统域 | **仅主 token**（无可授予 scope） |
| 其余未归类路径 | 任一 `*:write` |

`GET /api/points/:id/read` 与 `POST /api/points/virtual/:id/recalc` 均属**写层**
（前者真实发起一次现场采集，后者会同步求值回写），匿名模式下返回 401。

系统域刻意不提供可授予的 `system:write`：若允许密钥持有系统域写权限，
持有者可通过 `POST /api/access/api-keys` 将权限提升到管理员，
按域授权随之失效。

### 3.1 密钥记录（ApiKeyRecord）

| 字段 | 类型 | 说明 |
|---|---|---|
| id | string | 密钥 ID，`ak_` 前缀 |
| name | string | 名称 |
| keyPreview | string | 预览片段（前 8 + `…` + 后 4） |
| scopes | string[] | 权限范围 |
| enabled | bool | 是否启用 |
| expiresAt | i64 \| null | 过期时间（毫秒）；null = 永不过期 |
| createdAt | i64 | 创建时间（毫秒） |
| lastUsedAt | i64 \| null | 最近一次鉴权成功时间（毫秒） |
| callCount | u64 | 累计鉴权成功次数 |

### 3.2 GET /api/access/api-keys（读）

返回全部密钥记录（按创建时间升序），**不含完整 Key**。

```json
{
  "code": 0,
  "message": "success",
  "data": [
    {
      "id": "ak_18f3c2a1b0001",
      "name": "对接方A",
      "keyPreview": "gwk_ab12…9f0e",
      "scopes": ["points:read"],
      "enabled": true,
      "expiresAt": 1732492000000,
      "createdAt": 1729900000000,
      "lastUsedAt": null,
      "callCount": 0
    }
  ]
}
```

### 3.3 POST /api/access/api-keys（写）

请求体：

| 字段 | 类型 | 必填 | 说明 |
|---|---|---|---|
| name | string | 是 | 非空且 ≤ 64 字符 |
| scopes | string[] | 否 | 默认 `[]`；每项必须在白名单内 |
| expiresDays | u32 \| null | 否 | 有效期（天）；0 或缺省 = 永不过期；最大 3650 |

响应 `data` 为密钥记录 **外加 `key` 字段（完整 Key，仅此一次返回）**：

```json
{
  "code": 0,
  "message": "success",
  "data": {
    "id": "ak_18f3c2a1b0001",
    "name": "对接方A",
    "keyPreview": "gwk_ab12…9f0e",
    "scopes": ["points:read"],
    "enabled": true,
    "expiresAt": 1732492000000,
    "createdAt": 1729900000000,
    "lastUsedAt": null,
    "callCount": 0,
    "key": "gwk_ab12cd34ef56ab78cd90ef12ab34cd56ef789f0e"
  }
}
```

创建密钥并用它调接口的完整两段式示例：

```bash
# 1) 用主 token 创建密钥（立即保存返回的 key）
curl -X POST http://127.0.0.1:8080/api/access/api-keys \
  -H "Authorization: Bearer <主 token>" \
  -H "Content-Type: application/json" \
  -d '{"name":"对接方A","scopes":["points:read"],"expiresDays":30}'

# 2) 用返回的完整 key 调用业务读接口
curl http://127.0.0.1:8080/api/points \
  -H "Authorization: Bearer gwk_ab12cd34ef56ab78cd90ef12ab34cd56ef789f0e"
```

注意：第 2 步若换成写接口（如 POST /api/access/rate-rules），本例只有
`points:read`，将返回 403/8003；写操作需要密钥持有**目标路径所属域**的写 scope
（如写点位需 `points:write`，跨域不通用；系统域写操作仅主 token 可执行）。

### 3.4 PUT /api/access/api-keys/{id}（写，启停）

请求体：

| 字段 | 类型 | 必填 | 说明 |
|---|---|---|---|
| enabled | bool | 是 | false 即停用，立即生效 |

响应 `data: true`；密钥不存在返回 404/4004。

```bash
curl -X PUT http://127.0.0.1:8080/api/access/api-keys/ak_18f3c2a1b0001 \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"enabled":false}'
```

### 3.5 DELETE /api/access/api-keys/{id}（写）

响应 `data: true`；密钥不存在返回 404/4004。

## 4. 计划任务

- `cron` 为 5 段标准 cron 表达式（分 时 日 月 周），按**网关设备本地时区**解释，详见
  [部署架构 §8.4](architecture/deployment.md#84-时区与计划任务)；
- `nextRunAt` 为 epoch 毫秒；`enabled=false` 时恒为 null（停用任务不排程），任务的
  增删改会立即按 cron 重算；
- 后台调度循环每 15s 扫描一次到期任务并串行执行，单任务超时 60s；每次执行后回写
  `lastRunAt / runCount / lastStatus / lastError` 并重算 `nextRunAt`。

任务类型（`type`）：

| 值 | 含义 | target |
|---|---|---|
| read_point | 读取单个点位 | 必填，点位 ID |
| read_device | 读取设备全部点位 | 必填，设备 ID（回退按总线名匹配） |
| backup_config | 备份当前配置（写一条配置版本快照） | 不需要 |
| sync_ntp | 立即同步 NTP 时间 | 不需要 |

### 4.1 任务对象（ScheduledTask）

| 字段 | 类型 | 说明 |
|---|---|---|
| id | string | 任务 ID，`st_` 前缀 |
| name | string | 名称 |
| type | string | 任务类型（见上表） |
| target | string \| null | 目标 ID |
| cron | string | 5 段 cron 表达式 |
| enabled | bool | 是否启用 |
| lastRunAt | i64 \| null | 上次执行时间（毫秒） |
| nextRunAt | i64 \| null | 下次执行时间（毫秒）；停用时为 null |
| runCount | u64 | 累计执行次数 |
| lastStatus | string \| null | `success` / `error` |
| lastError | string \| null | 上次失败原因 |
| createdAt | i64 | 创建时间（毫秒） |

### 4.2 GET /api/access/scheduled-tasks（读）

返回全部任务（按创建时间升序）。

### 4.3 POST /api/access/scheduled-tasks（写）

请求体：

| 字段 | 类型 | 必填 | 说明 |
|---|---|---|---|
| name | string | 是 | 非空且 ≤ 64 字符 |
| type | string | 是 | 任务类型，未知值返回 400 `未知任务类型: <type>` |
| target | string | 条件必填 | `read_point` / `read_device` 必填，否则 400 `读取类任务必须填写目标 ID` |
| cron | string | 是 | 非法表达式返回 400 `cron 表达式无效: <cron>` |
| enabled | bool | 否 | 默认 true |

响应 `data` 为创建后的任务对象（启用时已算好 `nextRunAt`）。

```bash
curl -X POST http://127.0.0.1:8080/api/access/scheduled-tasks \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"name":"五分钟读点","type":"read_point","target":"point_1","cron":"*/5 * * * *","enabled":true}'
```

### 4.4 PUT /api/access/scheduled-tasks/{id}（写）

请求体同 4.3（全量更新）；`enabled` 缺省时保持原值。任务不存在返回 404/4004。
响应 `data` 为更新后的任务对象。

### 4.5 DELETE /api/access/scheduled-tasks/{id}（写）

响应 `data: true`；任务不存在返回 404/4004。

### 4.6 POST /api/access/scheduled-tasks/{id}/run（写，立即执行）

手动触发一次任务，**同步等待执行结果**（最长 60s），与后台调度共用同一执行路径，
执行结果同样回写任务记录（`runCount`、`lastStatus`、`lastRunAt`、`nextRunAt`）。
任务不存在返回 404/4004。

响应（HTTP 200 仅代表执行流程完成，业务成败看 `data.status`）：

```json
{ "code": 0, "message": "success", "data": { "status": "error", "error": "点位 missing-point 不存在" } }
```

成功时 `data` 为 `{ "status": "success", "error": null }`。

```bash
curl -X POST http://127.0.0.1:8080/api/access/scheduled-tasks/st_18f3c2a1b0001/run \
  -H "Authorization: Bearer <token>"
```

## 5. 访问日志

### GET /api/access/rate-logs（读）

返回最近的 API 访问日志，最新在前。日志由限流中间件对 `/api/*` 请求旁路写入，
库内仅保留最近 5000 行。

查询参数：

| 参数 | 类型 | 默认 | 说明 |
|---|---|---|---|
| limit | usize | 100 | 返回条数，服务端钳制到 1..=500 |

日志条目（ApiAccessLog）：

| 字段 | 类型 | 说明 |
|---|---|---|
| id | i64 | 自增 ID |
| ts | i64 | 请求时间（毫秒） |
| method | string | HTTP 方法 |
| path | string | 请求路径 |
| status | u16 | HTTP 状态码 |
| latencyMs | u64 | 处理耗时（毫秒） |
| clientIp | string | 客户端 IP（无法确定时为 `-`） |
| limited | bool | 是否被限流拒绝（HTTP 429） |

```json
{
  "code": 0,
  "message": "success",
  "data": [
    {
      "id": 102,
      "ts": 1729900000123,
      "method": "GET",
      "path": "/api/points",
      "status": 429,
      "latencyMs": 0,
      "clientIp": "192.168.1.10",
      "limited": true
    }
  ]
}
```

```bash
curl "http://127.0.0.1:8080/api/access/rate-logs?limit=50" \
  -H "Authorization: Bearer <token>"
```

## 6. 统计

### GET /api/access/rate-stats（读）

基于当前保留窗口（最近 5000 行访问日志）聚合：

| 字段 | 类型 | 说明 |
|---|---|---|
| totalRequests | u64 | 总请求数（含被限流的） |
| limitedRequests | u64 | 被限流拒绝的请求数（totalRequests 的子集） |
| avgLatencyMs | u64 | 平均延迟（毫秒）；**只统计未被限流的请求**，429 请求不过 handler、延迟≈0，混入会拉低均值 |

```json
{ "code": 0, "message": "success", "data": { "totalRequests": 10, "limitedRequests": 1, "avgLatencyMs": 14 } }
```

```bash
curl http://127.0.0.1:8080/api/access/rate-stats \
  -H "Authorization: Bearer <token>"
```
