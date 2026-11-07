# 协议工具 API 契约（`/api/protocol-tools`）

> 覆盖「协议工具」域：真实发帧调试、抓包、回放、通讯参数探测、从站扫描、寄存器扫描、
> 从站模拟、协议转换规则、协议能力清单。这些端点服务于**现场调试**，
> 按 license v2.0 全部免费（调试分析能力不受产能配额约束）。
>
> 路由注册：[`gateway-api/src/web/api/protocol_tools/routes.rs`](../gateway-api/src/web/api/protocol_tools/routes.rs)
> handler：[`gateway-api/src/web/api/protocol_tools/handlers/`](../gateway-api/src/web/api/protocol_tools/handlers/)
> 响应包装、`code` 取值与错误体口径同 [`api-devices.md`](api-devices.md) §1。

---

## 1. 鉴权

| 类别 | 策略 |
|---|---|
| GET | `allow_anon_read=true` 时可匿名，否则要求 token |
| POST / PUT / DELETE | 始终要求 token |
| 真实发帧 / 占用总线的写操作（`packet-editor/send`、`register-scan`、`auto-detect/run`、`packet-replay/*/execute`） | **仅主 token**（与系统域同级） |

总线占用约定（[`session.rs`](../gateway-api/src/web/api/protocol_tools/session.rs)）：

- 采集引擎正在使用的总线**不抢占、不自动暂停**，直接返回 `bus_busy` +
  「请先暂停该总线的采集后再试」；显式暂停过的总线视为空闲，允许调试独占。
- 同一总线同一时刻只允许一个调试会话；会话结束（含请求取消）即释放。
- 总线未注册/端口未打开返回 `bus_offline`，不去尝试自行开口（避免与采集抢串口）。

帧级失败的 HTTP 状态仍为 **200**，由响应体 `error` 字段表达（`timeout` / `crc` /
`exception` / `bus_busy` / `bus_offline`）；参数与资源类错误（`invalid_frame` /
`missing_bus` / `bus_not_found` / `unsupported_protocol`）才用 400/404。

---

## 2. 端点分组

| 分组 | 端点 | 说明 |
|---|---|---|
| 协议清单 | `GET /api/protocols`、`GET /api/protocols/:name/diag` | 已注册协议能力元数据；协议诊断（s7comm SZL / bacnet Who-Is / gpio 芯片信息） |
| 参数探测 | `POST /api/protocol-tools/auto-detect/run`、`POST …/run/:id/stop`、`GET …/run/:id` | 真实发只读帧，逐项判定连通性 / 从站地址 / 功能码 / 耗时 / 字节序；start 返回 id 后轮询 |
| 从站扫描 | `POST /api/protocol-tools/slave-scan` | 逐地址探测从站是否存在，请求内同步返回结果 |
| 寄存器扫描 | `POST /api/protocol-tools/register-scan`、`GET /api/protocol-tools/register-scan`、`GET …/:id`、`DELETE …/:id`、`POST …/:id/suggestions` | 逐地址读寄存器/线圈；DELETE 内含「先中止再移除」语义 |
| 抓包 | `GET /api/protocol-tools/packet-capture/packets`、`POST …/packet-capture/clear` | 读取/清空 frame_log 环形日志 |
| 回放 | `GET …/packet-replay/sessions`、`GET …/sessions/:id`、`POST …/sessions`、`DELETE …/sessions/:id`、`POST …/sessions/:id/execute` | 会话 CRUD 落盘（见 §5）；预置会话仅在 `web.demo_replay_sessions=true` 时存在且不可覆盖/删除；execute 为真实逐帧重发 |
| 报文编辑 | `GET …/packet-editor/presets`、`POST …/packet-editor/send` | 预置帧模板 + 真实下发单帧 |
| 协议转换 | `GET/POST …/convert/rules`、`PUT/DELETE …/convert/rules/:id`、`POST …/convert/rules/:id/toggle` | 值变换规则，落配置并热加载 |
| 从站模拟 | `GET …/simulator/registers`、`GET …/simulator/status`、`PUT …/simulator/registers/:addr`、`POST …/simulator/start`、`POST …/simulator/stop` | **真实 Modbus TCP 从站**：启动后在网关本机监听 TCP 端口，外部主站可连接读写寄存器；寄存器为内存态（见 §3） |
| IEC104 直读 | `POST /api/protocol-tools/iec104/ioa-read` | **未建点位也能按 IOA 读到实时值**：一次总召唤取全站快照，返回命中 IOA 与全量快照（见 §4） |

已下线的能力（**不再有路由，勿按旧文档调用**）：`packet-capture/buses`（总线列表统一走
`GET /api/config`）、`register-scan/:id/stop`（DELETE 已含中止语义）、`step-debug`
（前端每步直调 `packet-editor/send`）、`protocol-test`（协议一致性自测，结论现场无法处置）。

---

## 3. 从站模拟（真实 Modbus TCP 从站）

`POST …/simulator/start` 启动后，网关在 `0.0.0.0:<port>` 监听 TCP，外部主站可直接连接
并读写寄存器；`stop` / 进程退出即关闭监听。**仅 Modbus TCP（MBAP），不含 RTU/ASCII
串口从站模拟**。

### 3.1 端点与字段（路由不变，仅字段扩展）

| 端点 | 请求体 | 响应体 |
|---|---|---|
| `POST …/simulator/start` | `{ "slave_id": 1, "port": 1502 }`；`port` 可省略（缺省 `1502`），`0` 表示由系统分配（测试用） | `data: null`；绑定失败返回 HTTP 400 + 可读消息（如端口占用） |
| `POST …/simulator/stop` | 无 | `data: null`；abort 监听任务并等待退出，端口立即释放 |
| `GET …/simulator/status` | — | `{ running, slave_id, port, listen_addr, requests, errors, started_at_ms }` |
| `GET …/simulator/registers` | — | 寄存器表（含外部主站写入后的当前值） |
| `PUT …/simulator/registers/:addr` | `{ "value": 500 }` | `data: null` |

`status` 字段：`port` 为当前（或最近一次）监听端口，`port=0` 启动时为系统**实际分配**
的端口；`listen_addr` 为运行中的监听地址（如 `0.0.0.0:1502`），停止后 `null`；
`requests` / `errors` 为本次启动以来的请求数与解析失败/异常响应数；`started_at_ms` 为
启动毫秒时间戳，停止后 `null`。

### 3.2 端口与 unit_id 语义

- 默认端口 `1502`，范围 `1–65535`；`0` 仅用于测试（系统分配）。
- MBAP 事务号、协议号（0）、长度、单元号按请求原样回填；协议号非 0 / 长度非法时
  计数 `errors` 并断开该连接（帧边界不可信）。
- `unit_id` 必须与启动时的 `slave_id` 一致；不一致时**不应答**（TCP 端口可能被多个从站
  共用，抢答会让主站把错误从站的数据当成目标从站响应），由主站自行超时重试。

### 3.3 功能码支持范围

| 功能码 | 能力 | 说明 |
|---|---|---|
| `0x03` / `0x04` | 读保持 / 输入寄存器 | 共用同一份寄存器表；单次最多 125 个 |
| `0x06` | 写单个寄存器 | 写入立即反映到 `GET …/simulator/registers` |
| `0x10` | 写多个寄存器 | 先整体校验地址，任一地址不存在则整体拒绝（不部分写入） |
| `0x01` / `0x02` | 读线圈 / 读离散量 | 映射口径：线圈地址 = 寄存器地址，取寄存器值 **bit0**（非 0 即 ON） |
| 其它 | — | 异常码 `0x01`（非法功能） |

异常码：地址不在寄存器表 → `0x02`（非法数据地址）；数量为 0 / 超上限 → `0x03`（非法数据值）。
地址必须存在于寄存器表（默认 6 条：`0–3`、`10`、`11`），不支持隐式扩展。

### 3.4 内存态约束

- 寄存器为**进程内内存态**，与页面寄存器表是同一份数据；不落盘，进程重启回到 6 条
  默认寄存器；监听器不随开机自启，网关停止即断开。
- 需要长期保留的取值请记录到点位/设备配置，或按 §3.6 留证据。

### 3.5 其它进程内临时态

以下 3 处为**进程内 static 存储**（W3a-7 明确标注），**重启即清空**，不参与任何备份：

| 存储 | 位置 | 内容 | 生命周期与上限 |
|---|---|---|---|
| 探测结果表 | [`auto_detect/mod.rs`](../gateway-api/src/web/api/protocol_tools/handlers/auto_detect/mod.rs) `RESULTS` | `id → { steps, progress, status }` | 进程内；容量 64 条，超限只淘汰**已结束**的最旧条目 |
| 扫描结果表 | [`register_scan/mod.rs`](../gateway-api/src/web/api/protocol_tools/handlers/register_scan/mod.rs) `RESULTS` | `id → { cells, responded, progress }` | 进程内；容量 64 条（同上口径）；DELETE 立即移除 |
| 总线调试互斥表 | [`session.rs`](../gateway-api/src/web/api/protocol_tools/session.rs) `LOCKS` | `port_name → tokio::Mutex` | 随会话 `Drop` 释放；重启清空 |

（模拟寄存器表同属进程内 static，见 §3.4。）

读取口径：进程重启后再查 `GET /auto-detect/run/:id` 或 `GET /register-scan/:id`
得到 `data: null`（不是 404），前端按「结果已随重启丢失」处理即可。

**为什么这些不落盘**（决策，非遗漏）：

1. 探测/扫描结果是**当场即用的调试快照**，不是业务数据；落盘会在工业现场引入
   磁盘写放大、半写损坏、容量回收三类新故障点，收益不成比例。
2. `LOCKS` 是运行期互斥量，本身不可持久化——持久化一个锁没有语义。
3. 需要「事后留证据」的场景，走下面 §3.6 更可靠的通道。

### 3.6 需要留证据时的正确做法

| 诉求 | 留存方式 | 跨重启 |
|---|---|---|
| 这次扫描/探测**做过什么、结论如何** | 审计日志（每次收尾必写一条） | 是 |
| 具体**帧内容**（HEX / 时序 / 响应） | 先 `packet-capture` 抓包，再把关键会话 `POST /packet-replay/sessions` 保存为回放会话 | 是（会话落盘） |
| 模拟寄存器的取值配置 | 无持久化；需要长期保留请记录到点位/设备配置 | — |

审计 action 取值（字段：`action | target | detail | source=api`）：

- `debug.auto_detect` / `debug.auto_detect.abort`：含 device、bus、slave、成功步数、status、duration_ms
- `debug.register_scan` / `debug.register_scan.abort`：含 slave、reg_type、地址区间、responded/total、status
- `debug.slave_scan` / `debug.slave_scan.abort`
- `debug.frame.send`：每帧真实下发
- 租约失败也会写一条（detail 为 `auto-detect bus_busy` / `register-scan unsupported_protocol` 等）

审计落 `audit.log`（`GATEWAY_AUDIT_LOG` 覆盖，默认 `./audit.log`），由
`gateway_core::engine::audit` 维护。

---

## 4. IEC104 按 IOA 直读（未建点位调试）

`POST /api/protocol-tools/iec104/ioa-read`

现场常见场景：设备刚上电、点位配置表里**还没有**那个 IOA，但要先确认「这个点到底
有没有值、值是多少」。本端点就是这条最短路径：给一条总线 + 一组 IOA，直接返回实时值。

### 4.1 请求 / 响应

| 端点 | 请求体 | 响应体 |
|---|---|---|
| `POST /api/protocol-tools/iec104/ioa-read` | `{ "bus": "iec104_1", "ioas": [100, 101], "timeout_ms": 3000 }`；`bus` 必填，`ioas` 必填（≤ 256 个），`timeout_ms` 可选 | `{ bus, source, entries[], snapshot[] }` |

- `entries`：与请求 `ioas` 一一对应、保持请求顺序；`value` 为 `null` 表示**本轮总召唤未
  上报该 IOA**（**绝不返回 0**：0 是合法工程量，静默 0 会把「从站没这个点」误读成
  「值为 0」）。
- `snapshot`：本轮总召唤返回的**全量** IOA 快照（按 IOA 升序）——现场据此就能看到从站
  到底上报了哪些点。
- `source`：`collector` = 复用了该总线已有的采集器实例；`standalone` = 自建连接直读。

### 4.2 取数来源与协议约束

IEC 60870-5-104 **没有**「读单个 IOA」的请求帧，唯一取数机制是总召唤（C_IC_NA_1 /
ASDU 100）一次返回全站快照。因此「按 IOA 直读」= 一次总召唤 → 从快照取目标 IOA。

| 场景 | 取数方式 | 说明 |
|---|---|---|
| 该总线**已建点位**（采集器实例在跑） | 复用采集器实例 | 不新建第二条 TCP 连接（IEC104 从站通常只接受一条主站连接）。此时若总线仍在采集，返回 `bus_busy` + 提示先 `POST /api/buses/:name/pause`——直读与采集循环会争用同一链路 |
| 该总线**未建点位**（无采集器实例） | 自建连接直读 | 读完即断开（采集器 `Drop` 中断 keepalive 并关闭 TCP 连接），不影响采集面 |

- 仅支持 `protocol = iec104` 的总线，其它协议返回 `unsupported_protocol`。
- 本端点**不占用调试租约**（不做裸帧收发），因此不受 §1「同一总线同一时刻只允许一个
  调试会话」约束；采集占用判定由本端点自行完成。
- 总召唤窗口：缺省取总线 `timeout_ms`（并抬到 2000ms 下限——总召唤返回全站数据，大站点
  下 500ms 偏紧）；显式指定时按 `[100, 60000]` ms 截断。
- 审计：每次调用写一条 `debug.frame.send`（detail 含 IOA 数量与取数来源）。

### 4.3 错误码

| `error` | 含义 |
|---|---|
| `missing_bus` | 请求缺少 `bus` |
| `bus_not_found` | 总线不在当前配置中 |
| `unsupported_protocol` | 该总线协议不是 iec104（或当前构建未启用 iec104） |
| `bus_busy` | 该总线正在采集中，需先暂停采集 |
| `timeout` / `bus_offline` | 总召唤无应答 / 链路不可用 |

参数非法（`ioas` 为空、IOA 越界、单次超过 256 个）走 HTTP 400 + `code 4000`。

---

## 5. 已落盘状态

| 文件 | 位置 | 写入方式 |
|---|---|---|
| `replay_sessions.json` | 网关配置目录（`config.json` 同目录） | 保存/删除时原子写（`tmp` + `rename`，跨盘失败回退直接覆盖）；启动后首次访问懒加载，文件缺失/损坏按空列表处理 |

预置回放会话（`s1`/`s2`）由 handler 内置返回，不写盘，因此不可覆盖、不可删除。
**仅在 `web.demo_replay_sessions=true` 时注入**；默认 false，生产环境的会话列表只含真实落盘会话，
避免演示数据被误认为历史回放记录。

---

## 6. 页面本地态（不经过后端，勿当接口能力）

| 位置 | 说明 |
|---|---|
| 协议转换页「本地试跑」 | 浏览器内换算预览，**不写回网关**；真正生效走 `convert/rules` 保存 |
| 单步调试执行日志 | 页面内存，刷新即失；下发本身走 `packet-editor/send` 真实发帧 |

---

## 7. 已知偏差

- `auto-detect` 只输出「当前总线参数下已确认的项」，**不做未知波特率/校验位扫描**：
  运行期没有改串口参数的接口（`DebugBusTrait` 按 `BusConfig` 打开时固定参数）。
  真正的参数扫描属后续独立能力。
- `register-scan` 单次地址跨度上限 65536、总线协议仅 Modbus；非 Modbus 总线返回
  `unsupported_protocol`。
