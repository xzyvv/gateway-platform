# 功能盘点与下阶段方案（2026-11）

> 定位前提：系统 Web 前端 = **参数配置** + **现场调试**入口。
> 本文结论基于对代码的实测核查（端点清单、前端引用链、测试规模、TODO 扫描），非文档推断。

## 0. 结论摘要

| 维度 | 核查结论 |
|---|---|
| 技术债 | **几乎为零**：后端全仓 `TODO/FIXME/HACK/todo!/unimplemented!` 仅 **1 处**（`gateway-core/src/north/server/opcua_server.rs`） |
| 冗余功能 | **无僵尸功能**：前端 161 个 API 封装中 **151 个被 UI 引用**，未引用的 10 个全是内部工具（inflight/token 迁移/Tauri adapter） |
| 端点浪费 | **无未消费端点**：169 个后端端点全部有前端入口或明确为内部/第三方 API |
| 测试厚度 | 后端 **1427** 个测试 + 66 个 test 模块；前端 **175** 个 vitest 用例文件 |
| 真正的短板 | ① **端到端/真实部署路径**未被测试覆盖；② **非 Modbus 协议无现场调试工具**（协议能力 13 族，调试工具只覆盖 Modbus RTU）；③ 3 处「演示数据混入生产」的功能污染 |

**因此下阶段不做重构、不做功能砍除**，主线是：**把「功能齐备」升级为「现场敢交付」**——用端到端回归护栏 + 交付验收包 + 调试能力补齐，把已有 10 万行 Rust / 9 万行前端的工程价值转成可交付、可验收、可复制的产品价值。

---

## 1. 现状盘点（实测数据）

| 项 | 数值 |
|---|---|
| 后端代码 | `gateway-core` 329 文件 / 80,902 行；`gateway-api` 129 文件 / 23,800 行；`gatewayd` 7 文件 / 2,207 行 |
| 前端代码 | 197 个 `.vue` / 33,311 行；519 个 `.ts` / 57,579 行 |
| 后端端点 | 18 个路由组 / 141 条唯一路径 / 169 个「方法+路径」端点（含 2 个 WebSocket） |
| 前端页面 | 路由表 24 条 = 13 个业务页面 + 登录/无权限 2 页 + 9 条旧址重定向 |
| 南向协议 | 13 族：modbus / dlt645 / cjt188 / gpio / bacnet / bacnet_mstp / fins / iec101 / iec104 / mc / opcua / s7comm / skeleton |
| 北向通道 | mqtt（含 broker/outbox/template）、webhook、server 族（modbus_server / opcua_server / mqtt 透传 / modbus 网关桥）、server_sinks、sim_consumer、conversion |
| 测试 | 后端 1427 个测试；前端 175 个 vitest 文件；最大单文件 647 行（`slave_scan.rs`） |

---

## 2. 完整功能清单与价值评估

### 2.1 后端 18 个路由组

| 域 | 端点数 | 前端入口 | 价值档 | 现场价值判定 |
|---|---|---|---|---|
| `system` 系统与配置 | 27 | `/system` | **B** | OTA/NTP/证书/看门狗/版本为交付运维刚需，但集中在高级区，低频 |
| `config` 设备点位 CRUD | 21 | `/system/config` | **A** | 配置主入口：设备/点位/总线暂停/导入导出/校验/项目备份，定位正中靶心 |
| `protocol_tools` 协议工具 | 30 | `/acquisition/console` | **A** | 现场调试命脉：扫描/仿真/抓包/回放/组帧/转换规则/协议诊断，v2.0 全部免费 |
| `device_ops` 设备运维 | 18 | `/devices` | **A** | 资产/分组/健康/影子/拓扑/可写点位/命令下发，运维判断依据 |
| `access` 访问控制 | 15 | 系统设置高级区 | **C** | API 密钥与计划任务有真实价值（第三方采集系统接入、cron 自动采集）；限流规则面向公网暴露，现场局域网近乎不用 |
| `diagnostics` 诊断 | 11 | `/ops` | **A** | 总线负载/Modbus 错误归因/调度时间线/日志归档，现场排障第一入口 |
| `points_ops` 点位运维 | 11 | `/points` | **B** | 虚拟点位/偏移/克隆/查找替换：交付期批量搬迁工具，一次性高频、之后闲置 |
| `uplink` 上行配置 | 8 | `/uplink` | **A**（需 admin） | MQTT/Webhook/OPC UA Server/Modbus Server/透传 + 模板变量 + 上送测试 |
| `devices` IO 与模板 | 5 | `/devices` | **B** | IO 控制与设备模板复用，场景较窄但成本低 |
| `quality` 数据质量规则 | 4 | `/data` | **B** | 断链/超限/跳变规则，配置期常用 |
| `points` 点位读写 | 3 | `/acquisition/tester` | **A** | 现场读写验证，最核心的调试动作 |
| `realtime` 实时 / WS | 3 | `/overview/realtime`、`/ops` | **A** | 实时流 + 日志 WS + 远程执行（admin/exec scope） |
| `sim_consumer` 模拟消费者 | 3 | 系统设置 | **C** | 仅为消除「队列积压但无消费者」的告警歧义，排障用，非日常 |
| `history` 历史数据 | 2 | `/data` | **B** | 趋势与 CSV 导出，边缘网关场景价值中等 |
| `health` 探针 | 2 | 无（K8s/外部） | **A** | live/ready，部署必需 |
| `auth` 认证 | 3 | 登录页 | **A** | 登录/whoami/改密 |
| `license` 授权 | 2 | 系统设置 | **B** | 授权导入与查看 |
| `audit` 审计日志 | 1 | 系统设置 | **B** | 操作追溯，交付合规有要求时价值升档 |

### 2.2 前端 13 个页面

| 路径 | 页面 | 价值档 | 现场用途 |
|---|---|---|---|
| `/system/config` | 配置管理（admin） | **A** | 总线/设备/点位/模板/导入导出/版本回滚/校验/部署向导 |
| `/acquisition/console` | 协议调试台 | **A** | 选总线 → 选动作（扫描/仿真/抓包/回放/组帧/转换）→ 看结果 |
| `/acquisition/tester` | 点位读写 | **A** | 单点/批量读写与回读校验 |
| `/ops` | 运维监控 | **A** | 总线负载、Modbus 错误归因、调度时间线、日志、网关诊断 |
| `/devices` | 设备管理 | **A** | 资产/分组/健康评分/影子/拓扑/批量导入/批量命令 |
| `/overview` `/overview/realtime` | 运行总览 / 实时数据 | **A** | 5s 轮询总览；WS 实时流 + 收藏筛选 |
| `/uplink` | 上行转发（admin） | **A** | 北向通道配置与上送测试 |
| `/data` | 数据流水 | **B** | 边缘过滤/协议转换/Lua/虚拟点位公式/质量规则 |
| `/points` | 点位管理 | **B** | 虚拟点位、偏移、克隆、查找替换 |
| `/acquisition/comm-tuning` | 采集调优 | **B** | 总线暂停恢复、扫描周期、地址换算 |
| `/devices/topology` | 设备拓扑 | **B** | 拓扑可视化，排障加分项 |
| `/system` | 系统设置 | **B** | OTA/NTP/证书/看门狗/访问控制/审计 |
| （登录/改密） | 认证 | **A** | — |

**价值档含义**：A=定位核心，必须做到「稳定可交付」；B=支撑能力，保持可用即可；C=低频或面向其它部署形态，可收进高级区。

---

## 3. 低价值与重复冗余甄别（带证据）

| # | 判定 | 证据 | 建议 | 工作量 |
|---|---|---|---|---|
| R1 | **生产返回演示数据（功能污染）** | `handlers/packet_replay/sessions.rs` 硬编码 `s1 电力仪表#1 正常采集` / `s2 PLC#4 离线故障` 两条假会话，生产环境与真实会话同列表返回，前端无法区分 | 预置会话改为「demo 开关启用时才注入」（或直接删除，保留用户自建会话能力） | 0.5 天 |
| R2 | **从站地址探测重复** | `slave_scan`（扫 1..247 从站）与 `auto_detect` 第 1/2 步（连通性 + 从站地址）覆盖同一现场问题；两者都在协议调试台内 | 明确分工：`slave_scan` 保留（找未知地址），`auto_detect` 聚焦参数确认（已知地址）；两者入口文案区分 | 1 天 |
| R3 | **设备影子与健康评分重叠** | `device_ops/shadows.rs` = 期望周期 vs 实测快照；`/api/devices/health` = 评分；两者都在设备管理页签内 | 影子并入健康页签作为「期望 vs 实测」子表，不做独立页签 | 1–2 天 |
| R4 | **限流规则面向错配场景** | `access` 15 端点中 `rate-rules`（4）+ `rate-stats`/`rate-logs`（2）= 6 端点服务「网关被公网/大流量访问」；现场网关多为局域网直连 | 保留代码，前端收进系统设置 › 高级（不占一级入口），不删 | 0.5 天 |
| R5 | **模拟消费者占独立入口** | `north/sim_consumer.rs` 785 行 + 3 端点，作用是排障时消歧 | 归入运维监控页签「队列排障」；不单列一级入口 | 0.5 天 |
| R6 | **协议调试能力只覆盖 Modbus** | `auto_detect`/`register_scan`/`slave_scan` 三处均硬判 `UnsupportedProtocol`（非 modbus 前缀直接 400） | 见方案主线 A3：优先为现场主力协议补「连通性诊断」最小工具 | 3–5 天/族 |

> 结论：**没有需要删除的功能**。R1 是唯一带「生产污染」性质的问题，必须修；R2–R5 是信息架构收敛（入口归位），不删能力；R6 是缺口而非冗余。

---

## 4. 技术债核查明细（为什么不做重构）

| 检查项 | 结果 |
|---|---|
| `TODO/FIXME/HACK/todo!/unimplemented!` | 全后端 **1 处**（`north/server/opcua_server.rs`），非阻塞 |
| 超大单文件 | 最大 647 行（`slave_scan.rs`），其余 < 400 行；本轮已把 930/805/776 行三文件拆分完 |
| 死代码 / 僵尸端点 | 0（169 端点全部有消费方） |
| 前端未使用 API 封装 | 10 个，均为内部工具函数，非功能 |
| 测试规模 | 后端 1427 测试 + 66 test 模块；前端 175 vitest 文件 |
| 一致性护栏 | `cargo fmt` / `clippy -D warnings` / `api-surface --check` 已在用 |

---

## 5. 下阶段方案

### 5.1 推荐主线：**现场交付化（Deliverable-Ready）**

| 阶段 | 内容 | 价值 | 工作量 | 验收口径 |
|---|---|---|---|---|
| **P0 半日** | 修 R1：预置演示会话加 demo 开关，生产不再返回假数据 | 高（消除误导） | 0.5 天 | 生产构建 `GET /api/protocol-tools/packet-replay/sessions` 只返回真实会话；demo 模式仍可见模板 |
| **P1 一周** | **端到端冒烟测试**：① 进程内 API 主链路（`e2e_journey`，假总线注入）；② 真实二进制进程冒烟（`scripts/e2e_smoke.ps1`：启动 `gatewayd` → 探活 → 登录 → 配置读取/校验 → 回放会话 → 诊断/健康报告）；③ 前端页面主流程（13 页）另行取证 | **最高**（防回归、护住 10 万行已有功能） | 5–7 天 | 后端两条命令跑完（见 §5.4）；失败定位到具体步骤 |
| **P2 三天** | **交付验收包**：① 默认配置模板（Modbus 电力仪表 / IEC104 站各一套）；② 一键自检（诊断页汇总「协议/总线/北向/时钟/证书/看门狗」六项，输出可复制报告）；③ 交付检查单（前端导出 JSON/Markdown）。**已落地，见 §5.6** | **最高**（现场工程师直接用） | 3 天 | 现场工程师不看文档即可完成一次交付 |
| **P3 按需** | R2/R3/R5 入口归位 + R4 高级区收敛。**已落地，见 §5.7** | 中 | 2–3 天 | 13 页导航深度下降，入口无重复语义 |
| **P4 按协议排期** | 协议调试补齐到主力协议族（IEC104 / BACnet / S7 任选，先做现场占比最高的）。**IEC104 已落地，见 §5.8** | 高（决定能否进新行业） | 3–5 天/族 | 新协议具备「连通性诊断 + 单点读值」最小工具 |

### 5.2 备选方案对比

| 方案 | 适用前提 | 收益 | 风险 |
|---|---|---|---|
| **A 现场交付化（推荐）** | 无待修 bug 清单，功能已齐 | 把工程量转成可复制交付能力 | 需要真实现场/仿真环境做 E2E |
| **B 修 bug** | 有现场 issue 清单 | 直接解决阻塞问题 | **当前无输入**：仓内无 bug 单、无 issue 追踪，先收集再动手，避免凭猜测改代码 |
| **C 纯优化重构** | 存在可度量的性能/可维护性问题 | — | **不成立**：TODO 近零、无死代码、无超大文件，重构边际收益极低 |
| **D 补测试** | 单元测试缺口明显 | — | 单元测试已 1427 个；真正缺的是 **E2E/集成层**（P1），故并入 A 而非单列 |

### 5.3 建议执行顺序

1. **P0**（半日）：清掉唯一的功能污染 R1。
2. **P1**（一周）：E2E 冒烟护栏 —— 这是后续所有改动的安全网，先建网再动刀。
3. **P2**（三天）：交付验收包 —— 直接提升现场人效与交付成功率。
4. **P3**（按需）：入口归位，纯前端改动，风险低。
5. **P4**（按协议排期）：决定产品能进哪些行业的关键路径。

### 5.4 全局验收口径（沿用现有护栏）

```
cargo fmt --check
cargo clippy -p gateway-api --all-targets -- -D warnings
cargo test --workspace --quiet
npx vitest run            # 前端
python scripts/gen_api_surface.py --check
# 端到端冒烟（P1 落地，两条都跑）
cargo test -p gateway-api --test e2e_journey   # 进程内：8 步 API 主链路
powershell -File scripts/e2e_smoke.ps1         # 真实进程：临时配置 + 独立端口 + 自动清理
```

### 5.5 P1 落地记录（2026-11-03）

| 产物 | 覆盖 | 运行方式 |
|---|---|---|
| `gateway-api/tests/e2e_journey.rs` | 探活 → 登录 → `/api/status` → 读配置 → 配置校验 → 真实寄存器扫描（假总线 3 点全响应）→ 网关诊断 → 健康报告 | `cargo test -p gateway-api --test e2e_journey` |
| `scripts/e2e_smoke.ps1` | 真实 `gatewayd.exe` 启动 + 登录 + 配置校验 + R1 回归（回放会话为空）+ 协议注册表/诊断/健康报告 | `powershell -File scripts/e2e_smoke.ps1`（`-Build` 先编译、`-Port` 换端口、`-KeepTemp` 留日志） |

脚本隔离约定：工作目录设为 `%TEMP%\gateway-e2e-smoke-*`，清空继承的 `GATEWAY_*` 路径覆盖、
`GATEWAY_CONFIG_PATH` 指向临时配置，其余默认路径（队列库/日志/证书）随工作目录落在此处，
不触碰仓库根的 `config.json` / `data_queue.db` / `logs`；结束后强制停进程并删目录。

已知边界（不影响结论，但取证时需注意）：
- 冒烟只验证「起得来 + 主链路可读」，不覆盖真实串口/PLC（现场总线由假总线或 `soak.ps1` 覆盖）；
- Windows 下退出走 `Stop-Process` 强杀，**优雅停机**由 Rust 侧 `graceful_shutdown` 单测覆盖；
- 前端 13 页 UI 主流程未纳入本脚本（需 Playwright + dev server），列为可选后续。

### 5.6 P2 落地记录（2026-11-03）

统一入口：**运维监控 → 现场交付 → 交付验收**。现场按页内顺序即可完成一次交付，无需读文档。

| 产物 | 位置 | 说明 |
|---|---|---|
| 默认配置模板 | `gateway-web/public/delivery-templates/modbus-power-meter.json`、`iec104-station.json` | 工程文件格式（kind=gateway-project），页内可「下载 JSON」或「应用模板」一键导入；只覆盖总线/北向/变量映射三层，不动 Web 鉴权与时钟 |
| 配套设备模板 | `templates/builtin/universal_modbus_power_meter.json`、`universal_iec104_station.json` | 通用三相电表 12 点、IEC104 标准站 12 点（IOA 1001~1010 / 2001~2002） |
| 一键自检 | `gateway-web/src/views/ops/delivery/selfCheck.ts` + `DeliveryCheck.vue` | 并行调用 `/api/protocols`、`/api/config`、`/api/health/report`、`/api/north/status`、`/api/ntp/status`、`/api/certificates`、`/api/watchdog/services`，六项分级为 通过/提醒/未通过，生成可复制、可下载的 Markdown 报告 |
| 交付检查单 | 同上 | 六项自检 + 4 项人工确认（接线/地址/首点值/恢复验证）+ 交付人，导出 JSON / Markdown |

顺带修复：IEC104 设备模板展开时，`address` 改用点位自身 IOA（`default_address` / `address_map`），
不再复用设备地址（此前同一站所有点位挤在一个 IOA，采集值互相覆盖）。

回归：
- `cargo test -p gateway-api --test project_io delivery_templates` —— 两套模板真实走工程导入端点；
- `npx vitest run src/views/ops/delivery/selfCheck.spec.ts` —— 六项判定分级与报告/检查单导出。

已知边界：自检是**当下快照**，刚应用模板或刚重启时总线统计可能为空，会给出「提醒」而非误判失败；
证书项在未启用 TLS 时为「提醒」（内网明文可交付，公网需补证书）。

### 5.7 P3 落地记录（2026-11-03）

原则：**只收敛入口、不删能力**；所有原功能仍可从新入口到达。

| 项 | 改动 | 落点 |
|---|---|---|
| R2 | 「从站扫描」与「参数探测」分工写进页签副标题与页内提示：前者找**未知从站号**（只判断有无应答），后者确认**已知地址设备**的通信参数；两页互相指路 | `ProtocolConsoleHub.vue`、`scan/SlaveScan.vue`、`scan/AutoDetect.vue` |
| R3 | 「设备状态快照」不再单列页签，并入「设备健康评分」作为「期望 vs 实测」子表：列含期望周期、最后上报、相对期望（超 2 个周期判超期）、最近质量、点位质量分布；逐点明细改为「查看点位」抽屉 | `views/devices/HealthScore.vue`（`DeviceShadow.vue` 及独立 spec 已删除，快照断言迁移至 `HealthScoreShadow.spec.ts`） |
| R4 | 限流规则从「API 密钥」页内折叠卡片收进「系统设置 › 高级」分组（默认收起）；API 密钥页只保留密钥管理 | `views/system/SystemHub.vue`、`access/ApiKeys.vue` |
| R5 | 模拟消费者从系统域迁入「运维监控 › 队列排障」，与队列长度指标同一排查动线；页内写明「先看积压，再用兜底消费者消歧」 | `views/ops/queue/QueueTriage.vue`（原 `system/maintenance/SimConsumer.vue` 删除，spec 同步迁移） |

导航索引同步：`devices?tab=health-score` 增加「设备影子 / 设备状态快照」别名；`system?tab=api-rate-limit` 指向新页签；
`ops?tab=queue-triage` 承接「模拟消费者」旧叫法。旧的深链（`device-shadow`、`sim-consumer`）不再路由，
但侧栏搜索仍能通过别名命中新入口。

回归：
- `npx vue-tsc --noEmit`、`npx eslint <改动文件>` 通过；
- `npx vitest run`：175 文件 / 1567 用例通过；
- `src/stores/theme.spec.ts` 在全量并发下会因 echarts 动态 import 超 5s 而偶发超时（与本次改动无关），
  单独运行 `npx vitest run src/stores/theme.spec.ts` 6/6 通过。

### 5.8 P4 落地记录：IEC104 协议调试（2026-11-03）

选型：先做 **IEC104**（国内电力/新能源现场占比最高，且 P2 已补齐 IEC104 标准站模板）。
BACnet 与 S7 此前已有 `diag`（Who-Is / SZL），本次不重复实现。

| 能力 | 实现 | 入口 |
|---|---|---|
| 连通性诊断 | `Iec104Collector` 覆写 `diag_supported` / `diag`：惰性 TCP 建链 + STARTDT 确认 + 一次总召唤，报告本轮返回的 IOA 数量；总召唤无应答时如实报「TCP 已连接但无数据」，不冒充成功 | 采集调试台「协议诊断」按钮（`GET /api/protocols/iec104/diag`，调用前需先暂停该总线） |
| 单点读值 | 复用既有 `Iec104Collector::read_point`（惰性建链 + 总召唤刷新快照 + 按 IOA 取值）；`POST /api/points/:id/read` 与 Tester「读取」按钮已可用 | 采集调试台选中设备/点位后「读取」 |
| 能力声明 | 新增 `ProtocolCapabilities::telecontrol_tcp_write()`（`supports_diag = true`），仅 `iec104` 使用；`mc` / `fins` 仍用 `tcp_batch_write()`（`supports_diag = false`），避免共享构造器造成假能力位 | `/api/protocols` 能力元数据 |

测试：
- `iec104/tests/diag.rs`：假从站完成 STARTDT + 对时 + 总召唤数据帧，断言报告「1 个 IOA」；
  从站沉默时断言「未返回任何数据」；
- `registry/tests.rs` 的 N-8 能力位钉子加入 `iec104`；
- `cargo fmt --all` / `cargo clippy -p gateway-core --all-targets -- -D warnings` 通过；
- `cargo test -p gateway-core` 1222 passed / 1 ignored；`cargo test -p gateway-api` 全通过；
- `python scripts/gen_api_surface.py --check` 通过（无新增路由）。

已知边界：单点读值仍以「已配置点位」为前提（Tester 选设备 → 选点位），
未提供「未建点位、直接按 IOA 读」的独立调试入口；如需该场景，后续可基于
`read_point` 构造临时 `PointConfig` 增加一个调试端点。

### 5.9 后续：功能全景复核与主线 B 路线（2026-11-03）

P0–P4 收口后做了一次代码级全量复核（功能清单、价值评估、低价值/冗余甄别、技术债实测），
结论与下一阶段路线见 **[functional-inventory-and-next-roadmap.md](functional-inventory-and-next-roadmap.md)**：
技术债实测为 0（TODO 1 处、`todo!`/`unimplemented!` 0 处），无僵尸功能；
后续主线定为「交付规模化 + 可靠性验证 + 协议能力补齐」，而非继续加功能或重构。
