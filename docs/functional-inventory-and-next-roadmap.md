# 功能全景复核与下一阶段路线（主线 C）

> 复评日期：2026-11-03（第二次复评：主线 B 的 B0 / B3 / B4 已落地后重新核查）
> 复核方式：代码级核查（能力位 + 调用点 + 端点清单 + 测试资产实测），非文档转述
> 前序：[functional-inventory-and-next-roadmap.md 第一版](functional-review-and-next-phase.md) / [functional-review-and-next-phase.md](functional-review-and-next-phase.md)
> 前提：Web 前端定位为**参数配置 + 现场调试入口**（不是经营看板、不是多租户平台、不是 BI）
> 本版结论：**功能面冻结、债务为 0、冗余为 0 ⇒ 下一阶段主战场是「验证」，不是「开发」。**

---

## 0. 一句话结论

| 复评项 | 第一版结论（2026-11-03 早） | 本次复评（2026-11-03 晚） | 依据 |
|---|---|---|---|
| 技术债务 | 无系统性债务 | **仍为 0**（Rust 全仓 `TODO/FIXME/HACK/todo!/unimplemented!` = 0 处） | 全仓 `*.rs` 实测无匹配 |
| 低价值功能 | 无需下线 | **仍为 0**（每个页面/端点都有真实后端与定位归属） | 端点清单 + 页面清单比对 |
| 重复冗余实现 | 4 处（E1–E4） | **已清零**（E1–E4 全部收敛为单一实现） | 4 个 composable + 4 份 spec |
| 协议能力缺口 | diag 4/12 族、扫描类工具仅 Modbus | **diag 10/12 族、扫描类工具注册表驱动**；剩余缺口见 §2.D | `registry/capabilities.rs` + `registry/tests.rs` |
| 交付门禁 | 散落靠人记 | **一条命令跑完 19 步并出结论**（`scripts/release_gate.ps1`） | 本次实跑：18 PASS / 1 SKIP |
| **下一步主战场** | 主线 B（补能力 + 补验证） | **主线 C：以测试与故障验证为主，能力补齐为辅** | 见 §4 |

**一句话**：功能已经"够用且不冗余"，再投入做"优化/砍功能/重构"的边际收益接近 0；
真正未被证明的是**这些功能在真实故障与真实交付场景下是否可靠**——因此下一阶段应做**测试**（含故障注入），
而不是再做功能或重构。

---

## 1. 完整功能清单（复评后的代码事实）

### 1.1 体量基线（实测刷新于 2026-11-04）

| 项 | 数量 | 依据 |
|---|---|---|
| Rust 源文件（core+api+gatewayd，含 tests/） | 468（其中非测试文件 367） | `Get-ChildItem -Recurse *.rs`（排除 target） |
| HTTP 端点 | 18 路由组 / 141 唯一路径 / **169**「方法+路径」 | [docs/api-surface.md](api-surface.md)（脚本生成，CI 校验） |
| 前端页面组件（.vue） | 199 | `Get-ChildItem -Recurse *.vue` |
| 前端 Hub / 页签 | 10 个 Hub / 72 个页签 | `views/**/*Hub.vue` 的 `tabs` 数组逐个数 |
| 南向协议族 | 12（+ common/registry/skeleton 三个基础设施模块） | `gateway-core/src/south/protocol/` |
| Rust 单元测试 | 1246（core lib，另 1 ignored）+ 245（api lib）+ 3（license-issuer） | `cargo test --lib` 三段汇总 |
| 集成 / 契约测试文件 | 22 个（`gateway-api/tests/*.rs`） | 目录实测 |
| 前端单测 | 219 个 `*.spec.ts`（vitest 实际收集 **215 文件 / 1978 用例**） | `Get-ChildItem -Recurse *.spec.ts` + CI 实测 |
| Playwright E2E | 4 套 / 16 用例（smoke、write-flows、routes-deep、debug-paths） | `gateway-web/e2e/` |
| chaos 故障场景 | **47** 个真路径恢复场景 | `gateway-core/tests/chaos/scenarios.rs` |
| 覆盖率门禁 | 分目录阈值"只许上调、不许下调" | `gateway-web/vitest.config.ts`:83–104 |

### 1.2 后端功能域（18 组 / 169 端点，B3 后变化见右列）

| 路由组 | 端点 | 主要能力 | 定位归属 | B3 后变化 |
|---|---|---|---|---|
| `protocol_tools` | 30 | 调试向导、地址映射、寄存器扫描、从站扫描、参数探测、报文捕获/编辑/回放、单步调试、从站模拟、协议转换 | **现场调试核心** | **由 Modbus-only 改为注册表能力位驱动** |
| `system` | 27 | 网关信息、看门狗、计划任务、OTA、证书、日志归档、蜂窝、GPIO、运行时参数 | 支撑 | — |
| `config` | 21 | 设备/点位 CRUD、回收站、总线暂停恢复、导入导出、重载、校验、**版本历史/对比/回滚** | **参数配置核心** | — |
| `device_ops` | 18 | 设备健康评分、影子快照、批量命令、远程控制、设备发现 | **现场干预核心** | — |
| `access` | 15 | API 密钥、限流规则/统计/日志 | 支撑（公网暴露时才高） | — |
| `diagnostics` | 11 | 总线负载、采集诊断、统计、调度时间线、网关自诊断、日志下载 | **诊断核心** | — |
| `points_ops` | 11 | 点位批量编辑/复制/查找替换/地址偏移、虚拟点位 | **批量工程作业** | — |
| `uplink` | 8 | 上行配置、状态、透传会话、上送测试 | 配置 + 调试 | — |
| `devices` | 5 | IO 控制、设备模板 | 配置 | — |
| `quality` | 4 | 数据质量规则 | 支撑 | — |
| `auth` | 3 | 登录、会话、令牌 | 基础设施 | — |
| `points` | 3 | 立即读、写点 | **现场调试核心** | 写值路径统一为 `usePointWrite` |
| `realtime` | 3 | 实时数据 / WebSocket | 支撑 | — |
| `sim_consumer` | 3 | 上行兜底消费者（启停/状态） | 支撑（队列排障） | — |
| `health` | 2 | live/ready 探针 | 基础设施 | — |
| `history` | 2 | 历史数据查询 | 支撑 | — |
| `license` | 2 | 许可证状态与签发 | 商业必需 | — |
| `audit` | 1 | 配置审计 | 支撑 | — |

### 1.3 前端功能清单（10 Hub / 72 页签，定位归属复核）

| Hub | 页签 | 定位归属 | 复核结论 |
|---|---|---|---|
| **设备** | 设备清单 / 分组 / 批量命令 / 健康评分 / 远程控制 / 设备模板 / 批量导入 / 设备发现 | 配置 + 现场干预 | 核心；"批量导入"与系统配置侧导入共用校验/结果组件（见 §3.1 R1，入口保留） |
| **点位** | 点位清单 / 已删除 / 虚拟点位 / 点位克隆 / 查找替换 / 地址换算 / 地址偏移 | 配置 + 批量工程作业 | 核心 |
| **采集调优** | 通信参数 / 通信画像 / 调度优化 | **现场调优核心** | 核心（通信画像口径已并入 `healthVerdict`） |
| **协议控制台** | 调试向导 / 地址映射 / 寄存器扫描 / 从站扫描 / 参数探测 / 报文捕获 / 编辑 / 回放 / 单步调试 / 从站模拟 / 协议转换 | **现场调试核心** | 核心；入口可用性现由能力位驱动（B3） |
| **采集测试** | 点位读写 / 读取性能 | **现场调试核心** | 核心；读/写口径已统一（`useReadPointNow` / `usePointWrite`） |
| **数据流水** | 历史数据 / 区间对比 / 数据压缩 / 断连续传 / 链路追踪 / 质量概览 / 采集退避 | 支撑（判障回溯） | **C 档冻结**：不扩功能，只修 bug；退避口径已并入 `usePointBackoff` |
| **运维监控** | 总线负载 / 采集诊断 / 采集统计 / 运行指标 / 队列排障 / 采集甘特图 / 连接测试 / 磁盘健康 / 自诊断 / 健康报告 / 交付验收 | 观测 + 交付验收 | 核心（验收）；健康报告为唯一结论页，自诊断/网关信息改为摘要+深链（R2 已收敛） |
| **上行转发** | 上行配置 / 状态 / 透传会话 / 上行测试 | 配置 + 调试 | 核心 |
| **系统** | 网关信息 / 计划任务 / 看门狗 / Webhook 测试 / 固件升级 / 蜂窝 / GPIO / 实时日志 / 日志归档 / API 密钥 / 证书签发 / 配置审计 / 限流规则 | 支撑 | B 档：保留不扩张 |
| **系统配置** | 部署向导 / 运行参数 / 版本历史 / 配置体检 / 导入导出 / 片段导入 | **配置核心** | 核心；与设备批量导入共用校验/结果组件（R1 已收敛，入口按风险等级保留） |

### 1.4 南向协议能力矩阵（复评：新增 2 个能力位 + diag 6 族）

| 协议 | 批量读 | 帧提取 | 写 | 从站扫描 | 寄存器扫描 | 参数探测 | 多点写 | **diag** |
|---|---|---|---|---|---|---|---|---|
| modbus | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | **✗（缺口）** |
| dlt645 / cjt188 | ✗ | ✓ | ✓ | ✗ | ✗ | ✗ | ✗ | **✓（B3 新增）** |
| mc / fins | ✓ | ✗ | ✓ | ✗ | **✓（B4 新增）** | ✗ | ✗ | ✓（B3 新增） |
| iec104 | ✓ | ✗ | ✓ | ✗ | ✗ | ✗ | ✗ | ✓ |
| iec101 | ✓ | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ | **✓（B3 新增）** |
| s7comm | ✓ | ✗ | ✓ | ✗ | ✗ | ✗ | ✓ | ✓ |
| opcua | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | **✓（B3 新增）** |
| bacnet_ip | ✓ | ✗ | ✓ | ✗ | ✗ | ✗ | ✗ | ✓ |
| bacnet_mstp | ✗ | ✓ | ✗ | ✗ | ✗ | ✗ | ✗ | **✓（B4 新增）** |
| gpio | ✗ | ✗ | ✓ | ✗ | ✗ | ✗ | ✗ | ✓ |

- 能力位扩展为 8 位（新增 `supports_register_scan` / `supports_auto_detect`），前端按钮随能力自动可用；
- `registry/tests.rs` 有两条回归钉子：①`supports_diag` 必须与真实 `diag` 实现一致（10 族）；②"声明 true ⇒ 真的实现了"。
- **剩余缺口**：**Modbus 族无 diag**——现场最常用的协议反而没有"建链+握手+一次真实交互"的诊断入口（§2.D 最高优先缺口）。

### 1.5 质量与门禁资产（复评）

| 资产 | 现状 | 结论 |
|---|---|---|
| Windows 本地发布门禁 | `scripts/release_gate.ps1`，19 步（`-IncludeChaos` 为 20），失败项可定位到具体步骤，日志落 `target/release-gate/<时间戳>/` | **已闭环**（本次实跑 18 PASS / 1 SKIP：git 不在 PATH） |
| CI 门禁 | check / dead_code / 协议 feature 矩阵 / SKU 矩阵 / 集成契约 / bundle 预算 / 覆盖率 / 仓库健康 / chaos / e2e / 加密 / 跨平台 / **ARM 交叉编译** | 覆盖完整；ARM 已纳入（见下） |
| ARM 交叉编译 | CI `cross-arm`：aarch64-unknown-linux-gnu + armv7-unknown-linux-gnueabihf，`-p gatewayd --features sku-standard` 与出厂包同口径，`file` 校验 ELF 架构 | **已闭环（G2）**：不再依赖开发机手工产包 |
| 交付链路 | `deploy/deliver.sh` 一条命令：install.sh → 等待就绪 → verify-deploy.sh → 交付检查单（Markdown，含构建信息/逐项 PASS/FAIL/诊断命令） | **已闭环（G5）**：检查单由 verify 脚本生成，判定逻辑单一来源 |
| 桌面端 | `gateway-desktop` 已冻结：不再构建/打包/发布，CI `desktop-build` job 移除，代码保留（workspace 仍编译兜底） | 冻结（现场调试统一走 Web 控制台） |
| 前端覆盖率 | 分目录阈值"只许上调"；**默认仅统计被测试加载的文件**（`COVERAGE_FULL=1` 才全量口径） | **盲区**：新增无测试文件不会拉低覆盖率（§4.C1.4） |
| chaos | **47 场景**（源码口径，`gateway-core/tests/chaos/scenarios.rs`）；默认发行集实跑 **45**——另 2 条 `bacnet-mstp` 场景随该协议「退役观察」不在默认 feature 集内 | C1.3 的 6 类现场故障（证书过期、许可证到期、NTP 失步、看门狗触发、网络切换、南向长时无响应）**已落地**，四段结论见 `docs/故障处置卡.md` |
| E2E 覆盖 | 登录/核心路由巡游/配置保存/点位增删/license 端点/登出 | **缺口**：现场调试主路径（协议控制台、采集调优、点位读写）无浏览器级验证（§4.C1.2） |

### 1.6 SKU 与授权分层现状（编译期裁剪 vs 运行时配额）

两层解耦，各管一件事：

| 层 | 载体 | 决定什么 | 现状 |
|---|---|---|---|
| 行业 SKU | `gateway-core/Cargo.toml` 的 `sku-*` 聚合 feature（**编译期**） | **二进制体积**：装不装得下（flash / RAM） | 5 档（互为并行切片，不是叠加）：standard（modbus/dlt645/cjt188/iec104/opcua）、building（modbus/bacnet/cjt188/opcua）、power（modbus/dlt645/iec104/iec101/opcua）、plc（modbus/s7comm/mc/opcua）、minimal（modbus/dlt645/cjt188 三件套）；`fins` / `bacnet-mstp` 为退役观察；`--protocols` 可自定义。**实测 5 档产物仅 25.9~26.7 MiB、依赖 crate 均为 335，裁剪当前基本无效（见下方实测基线）** |
| 授权档 | License `tier` + `Quota`（**运行时**） | **卖多少钱**：点位 / 并发协议数 / 历史保留天数 / 北向通道数 | tier = core / standard / pro / ultra；配额数值载体是 `config.json` 的 `license.tier_quotas`，**仓库未内置默认表**，出厂配置必须写全四档 |

分工原则（与主流厂商一致）：**编译期只裁体积，License 只卖规模**——不用 License 去卖「二进制里已经没有的东西」
（客户付了钱发现内存没降会直接投诉），也不用 License 做功能开关（v2.0 已删除 6 个 feature gate，调试功能全免费）。
详见 [docs/design/license-v2-quota.md](design/license-v2-quota.md)。

**实测体积基线**（CI 作业 `sku size matrix`，Linux x86_64 release 构建，commit `4e02b47`，OPC UA 双向拆分后）：

| SKU | 协议数 | 协议清单 | 唯一依赖 crate | 产物体积 | 相对精简版 |
|---|---|---|---|---|---|
| `sku-standard` | 5 | modbus,dlt645,cjt188,iec104,opcua-client | 328 | 18.6 MiB | +19.0% |
| `sku-building` | 4 | modbus,bacnet,cjt188,opcua | 335 | 26.7 MiB | +70.8% |
| `sku-power` | 5 | modbus,dlt645,iec104,iec101,opcua | 335 | 26.7 MiB | +70.9% |
| `sku-plc` | 4 | modbus,s7comm,mc,opcua | 335 | 26.7 MiB | +70.8% |
| `sku-minimal` | 3 | modbus,dlt645,cjt188 | 291 | 15.7 MiB | 基准 |

**对比拆分前（commit `5947fcd`，全部 335 crate）**：

| SKU | 拆分前 | 拆分后 | 变化 |
|---|---|---|---|
| `sku-minimal` | 25.9 MiB | **15.7 MiB** | **-10.2 MiB（-39%）**：彻底不链接 `async-opcua` |
| `sku-standard` | 26.7 MiB | **18.6 MiB** | **-8.1 MiB（-30%）**：只带 `opcua-client`，不再带 server/nodes/core-namespace |
| `sku-building` / `sku-power` / `sku-plc` | 26.7 MiB | 26.7 MiB | 无变化（仍为 `opcua` 双向） |

结论：拆分前 5 个 SKU 依赖树完全相同（335 crate）、体积只差 ≤11 KB，**SKU 裁剪基本无效**；
拆分后精简版与标准版分别下降 39% / 30%，SKU 之间体积差从 ≤11 KB 拉开到 **11 MiB**，
「按行业切体积」第一次真正成立。楼宇 / 电力 / PLC 三版尚未切 client-only，是本杠杆的剩余空间。

**待办（不阻塞主线 C）**：

1. ~~**缺真实体积数据**~~ **已闭环**：CI 新增 `sku-size-matrix` 作业（`if: push || workflow_dispatch`，
   纯观测不阻塞 PR），每次 main 合并后自动跑 5 个 SKU 的 release 构建并出表，
   产物见 Actions artifact `sku-size-matrix`。数据来源见上表。
2. ~~**`opcua` feature 同时打包南向 Client（`south/protocol/opcua`）与北向 Server（`north/server/opcua_server.rs`）**~~ **已闭环**：
   `async-opcua` 改为 optional，拆成 `opcua-client`（`async-opcua/client`）与
   `opcua-server`（`async-opcua/server`）两个 feature，`opcua` 保留为合并别名（既有构建命令不变）。
   北向 `opcua_server` / `opcua_sink` / 注册表工厂 / 相关测试与 chaos 测试服务端全部挂
   `#[cfg(feature = "opcua-server")]`；裁剪构建若配置里启用北向 UA Server，启动时打 `warn`
   而不是静默不启动（同 §8.4 时区隐患的处理口径）。
   实测：`sku-minimal` 依赖树 **335 → 288 crate**，不再链接 `async-opcua` 及其服务端子 crate；
   `sku-*` 体积新数值由下一次 `sku size matrix` 作业回填。
3. **行业 SKU 与授权档目前完全独立**（`sku-minimal` 也能配 ultra 授权），缺推荐组合；
   `tier_quotas` 的默认值也未随 SKU 场景给建议值。

---

## 2. 逐项价值评估（三因子：现场价值 × 维护成本 × 定位契合）

| 档位 | 功能域 | 理由 | 状态 |
|---|---|---|---|
| **A 核心（继续加固）** | 参数配置（config）、设备/点位管理、采集调优、协议控制台、点位读写、诊断运维、上行配置与测试、交付验收 | 直接对应"配得起来 + 调得通 + 交得掉" | 已具备；下一步靠**验证**加固（§4.C1） |
| **B 支撑（保留不扩张）** | 访问控制、证书、审计、看门狗、计划任务、Webhook 测试、OTA、蜂窝、GPIO、透传会话 | 频次低但缺失会造成交付事故；已用权限/分组收口 | 保持 |
| **C 边缘（冻结，只修 bug）** | 历史数据、区间对比、数据压缩、断连续传、质量概览、采集退避、链路追踪 | 现场判"数据不对"要回溯，但**定位不是数据分析平台** | 冻结 |
| **D 能力缺口（不是冗余）** | ①**Modbus 无 diag**；②mc/fins/iec101/串口计量无扫描类调试工具（需先判定"不适用"还是"未实现"）；③opcua/iec101/bacnet_mstp 无写 | 缺口才是阻塞现场交付的东西 | **本阶段做 C2（仅 ① 必做）** |
| **E 重复实现** | 第一版 4 处（E1–E4） | 维护成本翻倍 | **已清零**（§3.1） |

---

## 3. 低价值 / 重复冗余甄别（复评，带证据）

### 3.1 结论：需下线 0 处、重复实现 0 处；剩余 2 处"入口分散"（可选，低优先）

| # | 现象 | 复评状态 | 证据 |
|---|---|---|---|
| E1 | 立即读点位 7 处实现 | **已收敛** | 统一为 [useReadPointNow.ts](../gateway-web/src/composables/data/useReadPointNow.ts)（+6 个 spec）；`useDataFlowTrace` / `useDiagnosticExport` / `values` / `connectivity` / `useVerifyStep` / `BatchReadTest` / `Tester` / `DeviceBatchCommand` 全部改为调用 |
| E2 | 写值入口 4 处 | **已收敛** | 统一为 [usePointWrite.ts](../gateway-web/src/composables/data/usePointWrite.ts)（批量/单点/确认文案一体） |
| E3 | 退避数据 3 处各拉一次 | **已收敛** | 统一为 [usePointBackoff.ts](../gateway-web/src/composables/data/usePointBackoff.ts)（含设备反解、同刻去重） |
| E4 | 健康口径 5 套 | **已收敛（阈值层）** | 统一为 [healthVerdict.ts](../gateway-web/src/utils/healthVerdict.ts)，被 `HealthScore` / `CommProfile` / `HealthReport` / `selfCheck` 共用 |
| R1 | **导入路径 4 条**：设备批量导入（`DeviceBatchImport` + `batchImport/*`）/ 设备模板（`DeviceTemplates`）/ 配置导入导出（`ConfigIo`）/ 片段导入（`ConfigFragmentImport`）+ 部署向导 | **已收敛（校验+结果层）** | 入口仍分散（导入是高危操作，分散反而降低误操作），但「JSON 文本解析」统一为 [jsonTextValidator.ts](../gateway-web/src/utils/jsonTextValidator.ts)、「导入结果」统一为 [ImportOutcome.vue](../gateway-web/src/components/ImportOutcome.vue)；`ConfigIo` 两处、`ConfigFragmentImport` 一处均改为共用，片段导入失败由 toast 改为行内展示 |
| R2 | **网关级健康结论 5 处入口**：设备健康评分 / 运维健康报告 / 网关自诊断 / 网关信息 / 交付验收自检 | **已收敛** | `运维 › 健康报告` 为唯一结论页；自诊断与网关信息改用 [HealthConclusionBar.vue](../gateway-web/src/components/HealthConclusionBar.vue)（一行结论 + 分级计数 + `查看健康报告 →` 深链），不再各自渲染结论卡；问题清单仍共用 `HealthProblemList`。设备健康评分是**设备级**口径、交付自检是**交付六项**口径，语义不同，保留 |

> 判定：R1/R2 属"体验冗余"而非技术债，收敛后两者均已闭环；下一步仍以真机验证为先。

### 3.2 技术债务复测（结论：仍为 0）

| 指标 | 实测 | 判断 |
|---|---|---|
| `TODO`/`FIXME`/`HACK`/`todo!`/`unimplemented!`（全仓 `*.rs`） | **0** | 无半成品分支 |
| `panic!` / `unsafe` | 52 / 16（多在测试与平台封装） | 与第一版持平，无新增 |
| 演示数据混入生产 | 无 | 已闭环 |
| 大文件 | 4 个 >800 行，最大 912 行 | 已被测试与 CI 覆盖，不构成风险 |
| 覆盖率盲区 | 默认口径只统计被测试加载的文件 | **唯一实质缺口**（不是债，是门禁口径问题，见 §4.C1.4） |

### 3.3 明确不要动（防止再次误判）

- 数据流水 7 个页签：现场判障工具，不是分析需求；
- `unwrap` 616 处 / `panic!` 52 处：无证据表明生产路径可触发，无证据表明是债；
- Rust 核心不重构：无债证据，重构只引入回归；
- 不为覆盖率数字刷测试：覆盖率 job 出报告不设阈值是对的（**但口径要修，见 C1.4**）。

---

## 4. 接下来怎么做：主线 C（以验证为主）

前提判断：**功能面冻结 + 债务 0 + 冗余 0** ⇒ 再投入"优化/砍功能/重构"是负收益；
剩余风险全部集中在"**功能在真实故障与真实交付场景下是否可靠**"。因此主线 C 的排序是：
**先补验证（C1）→ 再补最后的硬缺口（C2）→ 最后沉淀现场知识（C3）**。

### C1. 验证补强（3–4 天，价值最高 / 成本中）★ 本阶段主体

| 子项 | 做什么 | 验收 |
|---|---|---|
| **C1.1 新能力端到端验证** | ①`gateway-api` 增加能力位路由的 API 契约测试：`slave_scan` / `register_scan` / `auto_detect` 对"声明支持的协议"返回可执行结果、对"未声明的协议"返回明确错误（不是 500）；②前端 `useProtocolCaps` 单测 + 加载失败降级；③`useReadPointNow` / `usePointWrite` / `usePointBackoff` 的**协议错误 vs 传输错误**分支各补一条回归钉子 | 3 个新 spec 落地且进门禁 |
| **C1.2 浏览器级覆盖现场调试主路径** | 新增 Playwright 用例：协议控制台入口按能力位可用/禁用、采集调优参数修改→生效、点位读写→写值确认弹窗→结果回显（复用现有隔离端口与登录助手） | `e2e/` 新增 1 套 spec，纳入 `release_gate.ps1` |
| **C1.3 chaos 补 6 类现场故障** | 证书过期、许可证到期、NTP 失步、看门狗触发、网络切换（网卡 down/up）、南向长时无响应（超时风暴）；每条要求"**故障 → 现象 → 自愈动作 → 恢复后数据完整性（不丢/不重/有序）**"四段结论，落 `docs/故障处置卡.md` | ✅ 已落地：chaos 由 41 → 47 场景（6 条已补），处置卡 6 张成文 |
| **C1.4 覆盖率口径修盲区** | 跑一次 `COVERAGE_FULL=1` 全量实测 → 以 `coverage-summary.json` 重定分目录阈值 → 改为默认全量口径（`include: ['src/**/*.{ts,vue}']`），使"新增无测试文件"不再免疫门禁 | 阈值重定后 CI 仍绿；新增盲区文件会被覆盖率拦下 |
| **C1.5 调试工具的"非 Modbus"真机验证** | 用 `mc`/`fins`/`iec101`/`dlt645` 的 diag 路径各做一次"连不上就如实报错"的验证（本地无设备时用不可达端点断言错误文案），避免 B3 新代码只被单测覆盖 | 每族至少 1 条"不可达端点如实报错"的用例 |

> 为什么先做 C1 而不是 C2：新能力（diag、工具协议化）刚落地，**回归窗口就在现在**；
> 等功能再变一轮，验证资产就过期了。

### C2. 最后的能力缺口（1–2 天，价值高 / 成本小）

1. **Modbus 族 diag**（最高优先）：Modbus 是现场主力协议，却只有"扫从站"没有"诊断"；
   按 iec104/opcua 的做法补：建链 + 一次真实读（FC03）+ 报寄存器数量，失败如实报。
2. **扫描类工具的"不适用"判定**（不是全做）：mc/fins 有寄存器模型 → `register_scan` 可实现；
   iec101 是遥测遥信 → 无寄存器概念，应显式标注"不适用"而不是留着 `✗` 让人误以为缺功能；
   dlt645/cjt188 同理（表计无地址段扫描语义）。

### C3. 现场知识与远程支持（1–2 天，价值中 / 成本小）

- **故障处置卡**（依赖 C1.3 产出）：按"现象 → 页面 → 判断 → 动作"组织，如
  "采集值不动" → 采集诊断 → 看退避/质量 → 强制重连/暂停总线；
- **一键诊断包扩展**：`useDiagnosticExport` 扩成"一次打包：配置快照 + 最近日志 + 健康报告 + 北向状态"，供远程支持。

### 排序（价值 ÷ 成本）

| 序 | 项 | 价值 | 成本 | 依赖 | 本阶段 |
|---|---|---|---|---|---|
| 1 | **C1.1 新能力端到端验证** | 高 | 小 | 无 | ✅ 立即开工 |
| 2 | **C1.2 调试主路径 E2E** | 高 | 中 | 无 | ✅ |
| 3 | **C2 Modbus diag** | 高 | 小 | 无 | ✅ |
| 4 | **C1.3 chaos 补现场故障 + 处置卡** | 高 | 中 | 无 | ✅ |
| 5 | **C1.4 覆盖率口径修盲区** | 中高 | 小 | 无 | ✅ |
| 6 | **C3 诊断包 + 处置卡** | 中 | 小 | C1.3 | ✅ |
| 7 | R1/R2 入口收敛 | 中低 | 小 | 无 | ✅ |
| 8 | ~~B1 多网关批量下发~~ | — | 大 | — | ❌ **降级为不做**（见下） |
| 9 | ~~B6 万点位 soak~~ | — | 大 | 规模驱动 | ❌ 暂缓 |

### 明确不做（防止跑偏）

1. **不做多网关批量下发**：第一版把它列为 B1 首选，复评后**降级**——单台边缘网关没有设备台群，
   跨网关下发属于上位机/云平台职责，硬做会让网关背上不属于它的状态管理（仓库里也找不到设备台群模型）。真正的"批量工程作业"是**单台内的点位批量作业**（`points_ops` 已覆盖）。
2. **不重构 Rust 核心**：无债证据，只引入回归。
3. **不砍数据流水页签**：现场判障工具。
4. **不做多租户 / 大屏 / 报表平台**：偏离定位。
5. **不新增协议而不做工具化**：只加采集不加调试工具 = 把问题推给现场。
6. **不为覆盖率数字刷测试**：只修口径（C1.4），不设"刷覆盖率"任务。

---

## 5. 验收口径与决策点

| 目标 | 验收标准 |
|---|---|
| C1.1 | 能力位路由有 API 契约测试；前端 3 个新 composable 的错误分支有回归钉子 |
| C1.2 | Playwright 新增 1 套"现场调试主路径"用例并进门禁 |
| C1.3 | ✅ chaos 47 场景（默认发行集实跑 45，2 条 bacnet-mstp 随退役观察不在默认集）；6 类现场故障各在 `docs/故障处置卡.md` 有"现象/自愈/数据完整性"结论 |
| C1.4 | 覆盖率默认全量口径，阈值重定后 CI 绿；新增无测试文件会被拦下 |
| C2 | Modbus 在采集调试台可做"连通性诊断 + 单点读值"，与其它族口径一致 |
| C3 | 一键诊断包一次产出：配置快照 + 日志 + 健康报告 + 北向状态 |

**决策点**：若业务方确认"现场协议固定为 Modbus、设备规模长期 <10 台"，
则本阶段只保留 **C1.1 + C2 + C1.3**（≈2 天），C1.2/C1.4/C3 可推后——
这是本方案允许的最简路径。
