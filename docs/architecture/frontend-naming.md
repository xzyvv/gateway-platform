# 前端命名规范（gateway-web）

> 目标：**看路径知道在哪、看文件名知道是什么、看路由知道打开哪一页**。
> 三个层次必须同构：目录 = 业务域，文件 = 页面能力，路由 = 路径语义。

## 1. 三层命名模型

| 层 | 命名依据 | 例子 |
| --- | --- | --- |
| 目录（一级） | 业务领域（名词，单数） | `devices/` `points/` `acquisition/` |
| 文件（页面） | 对象 + 视图能力 | `DeviceTopology.vue` `PointHub.vue` |
| 路由 | `/<域>[/<能力>]` | `/devices/topology`、`/system/config` |

一句话：**目录回答"哪个业务"，文件名回答"哪种视图"，路由是两者的拼接**。

## 2. 一级模块（业务域）

| 目录 | 业务域 | 路由前缀 | 覆盖范围 |
| --- | --- | --- | --- |
| `overview/` | 概览 | `/overview` | 运行总览、实时数据 |
| `devices/` | 设备 | `/devices` | 设备管理、设备运维、设备拓扑、设备批量导入、批量命令 |
| `points/` | 点位 | `/points` | 点位管理、地址换算、点位模板 |
| `acquisition/` | 采集 | `/acquisition` | 读写测试、协议调试台（扫描/报文/模拟合并）、通信优化 |
| `data/` | 数据 | `/data` | 数据流水、数据质量、数据加密 |
| `uplink/` | 上行 | `/uplink` | 上行转发与测试 |
| `ops/` | 运维 | `/ops` | 性能监控、通信诊断、健康巡检、批量任务 |
| `system/` | 系统 | `/system` | 系统设置、固件与证书、运行日志、配置管理、运行参数 |
| `auth/` | 鉴权 | `/login` `/change-password` | 登录、改密（顶层路由，不进 Layout） |
| `layout/` | 框架外壳 | — | Sidebar / Topbar / 菜单数据源 |

顶层非页面路由：`/load-failed`（加载失败）、`/forbidden`（无权限）、`/not-found`（路径兜底）。

## 3. 文件命名

### 3.1 页面

`<对象><能力>.vue`，能力词取自固定词表：

| 能力 | 含义 | 示例 |
| --- | --- | --- |
| `Hub` | 多视图容器（tab 集合） | `DeviceHub` `ProtocolConsoleHub` |
| `List` | 列表 / 检索 | `DeviceList` |
| `Detail` | 单对象详情 | `PointDetail` |
| `Edit` / `Config` | 可编辑单对象 | `RuntimeConfig` |
| `Topology` | 拓扑视图 | `DeviceTopology` |
| `BatchImport` / `BatchCommand` | 批量写入类 | `DeviceBatchImport` |
| `Monitor` | 只读监控 | `RealtimeMonitor` |
| `Templates` | 模板管理 | `PointTemplates` |
| `Tools` / `Hub`（领域工具集） | 工具箱 | `OpsHub` |

规则：
- 页面**不留** `index.vue`，文件名必须自解释；
- Hub 容器统一 `*Hub.vue`，其 tab 页放在同目录（`tabs/` 子目录）且不带 `Tab` 后缀；
- 能力词只表达"视图做什么"，不表达技术栈（禁止 `*Page` `*View` `*Comp`）。

### 3.2 附属文件

| 类型 | 位置 | 命名 |
| --- | --- | --- |
| 对话框 | 页面同目录 `dialogs/` | `*Dialog.vue`；预览 `*Preview.vue`；确认 `*Confirm.vue` |
| Hub tab | 所属 Hub 目录或 `tabs/` | 与 tab 语义同名，如 `NetworkConfig.vue` |
| 子组件 | 归属页面所在目录 | 随 owner 走，**不为一个页面新建 `components/` 跨域目录** |
| 组合式函数 | `composables/<域>/` | `use<对象>.ts`，域目录与 `views/<域>` 对齐 |
| 测试 | 与源码 co-locate | `<被测文件>.spec.ts` |

只有 **≥2 个业务域复用** 的组件才进 `src/components/`（如 `HubShell`、`ErrorBoundary`）。

### 3.3 禁止项

| 反例 | 问题 | 正例 |
| --- | --- | --- |
| `dashboard/` | 与"运行总览"业务名不符 | `overview/` |
| `config/`（跨切） | 既含采集调参又含系统配置 | 拆到 `acquisition/` 与 `system/config/` |
| `protocol-tools/` | 按实现手段分类，非业务域 | `acquisition/` |
| `diagnostics/` | 与运维域重复 | `ops/` |
| `New2.vue` `Old.vue` `temp.vue` | 无语义 | 用能力词重命名 |
| `common/` `misc/` `utils/`（views 下） | 垃圾桶目录 | 归入对应业务域 |
| `DeviceDeviceList.vue` | 前缀重复 | `DeviceList.vue` |

## 4. 路由命名

1. **path**：`/<域>` 或 `/<域>/<能力>`；能力段用短横线小写（`batch-import`、`runtime-config`）。
2. **Hub tab**：一律 `?tab=<name>`，**不再为每个 tab 登记 redirect 别名**；tab 名与 Hub 内 `tabs[].name` 一致。
3. **name**：与页面组件名完全一致（PascalCase），便于从路由反查文件；不使用 `Index`、`*Redirect`。
4. **meta.title**：中文页面名，必须与侧边菜单 `label`、搜索索引文案一致。
5. **meta.requireAdmin** 等权限只写在 `meta`，不在页面内二次判断。
6. **旧地址不做兼容**：删除或改名即断链，未匹配路径落 `/not-found`（携带 `?from=`），不静默回首页。

## 5. 菜单与侧栏搜索索引

- 菜单分组文件 `views/layout/menu/groups/<域>.ts`，`name` 为该域中文名，分组顺序与第 2 节表格一致；
- 菜单项 `target` 必须是 `routes/index.ts` 中已登记的 path（含 `?tab=`）；
- 侧栏搜索索引 `navIndex.ts` 中导航项统一 `nav-` 前缀，`target` 与菜单同源；
- 上述一致性由 `src/router/routes/navTargets.spec.ts` 与 `src/views/layout/menu/navTargets.spec.ts` 静态校验，新增入口漏登记会直接失败。

## 6. 新增页面清单

1. **文件**：`views/<域>/<对象><能力>.vue`
2. **路由**：`routes/index.ts` 登记 `path` / `name`（= 组件名）/ `meta.title`
3. **入口**：`menu/groups/<域>.ts` 加菜单项 + `navIndex.ts` 加 `nav-` 项
4. **Hub tab**：若属某个 Hub，只在该 Hub 的 `tabs[]` 登记，不新增路由

## 7. 中文显示名（菜单 label = 页面 title = 搜索索引 title）

规则：**对象 + 能力**，同一能力词全站只表达一种视图；页面名与其 Hub 内同名 tab 必须区分（如页面「设备管理」→ tab「设备清单」）。

因为两者刻意不同名，Hub 落地页必须同时呈现父子两层（页头显示页面名 + 当前 tab 名），否则用户点完菜单只看到 tab 名，会误以为点错了入口。实现见 `src/components/HubShell.vue` 的 `hubTitle` / `.hub-body-parent`。

Hub tab 的 `subtitle` 必须描述该页**真实存在**的区块，不得承诺页面上没有的内容（如页面没有组件健康表，就不能写「组件健康总览」）。页面结构增删区块时同步改副标题。

| 页面 | 中文名 | 同域 tab 命名要点 |
| --- | --- | --- |
| `Overview` | 运行总览 | — |
| `RealtimeMonitor` | 实时数据 | — |
| `DeviceHub` | 设备管理 | 设备清单 / 分组 / 批量命令 / 健康评分 / 状态快照 / 远程控制 / 模板 / 批量导入 / 设备发现（原「设备运维」Hub 已并入） |
| `DeviceTopology` | 设备拓扑 | — |
| `DeviceBatchImport` | 设备批量导入 | — |
| `DeviceBatchCommand` | 批量命令 | — |
| `PointHub` | 点位管理 | 点位清单 / 回收站 / 虚拟点位 / 批量导入 / 点位克隆 / 进阶工具（查找替换 / 地址换算 / 地址偏移，原「地址换算」Hub 已并入） |
| `PointTemplates` | 点位模板 | — |
| `TesterHub` | 点位读写 | 点位读写 / 读取性能 |
| `ProtocolConsoleHub` | 协议调试台 | 扫描与探测：地址映射 / 寄存器扫描 / 从站扫描 / 参数探测；报文查看与下发：报文捕获 / 报文编辑 / 报文回放 / 单步调试；模拟与扩展：从站模拟 / 协议转换。合并口径：原「协议扫描 / 协议仿真 / 报文调试」三个 Hub 合并为一台，页签 name 与旧 Hub 保持一致以兼容旧链接 |
| `CommTuningHub` | 通信优化 | 通信参数 / 通信画像（表格·矩阵） / 调度优化 |
| `DataPipelineHub` | 数据流水 | 历史数据 / 区间对比 / 数据压缩 / 断连续传 / 链路追踪 / 质量与治理（质量概览 / 采集退避，原「数据质量」Hub 已并入） |
| `DataEncryption` | 数据加密 | — |
| `UplinkHub` | 上行转发 | 转发规则 / 通道参数 / 上行服务状态 / 透传会话 / 上行测试 |
| `OpsHub` | 运维监控 | 运行观察：总线负载 / 采集诊断 / 采集统计 / 运行指标；时序分析：采集甘特图；连通诊断：连接测试；资源：磁盘健康；自诊断与报告：自诊断 / 健康报告。合并口径：原「性能监控 / 通信诊断 / 健康巡检」三个 Hub 合并为一台 |
| `SystemHub` | 系统设置 | 运维：网关信息 / 计划任务 / 看门狗 / Webhook 测试 / 固件升级 / 蜂窝网络（无蜂窝模块时隐藏）/ GPIO 控制（非 Linux 平台隐藏）/ 模拟消费者；日志：实时日志 / 日志归档（原「运行日志」Hub 已并入）；安全：API 密钥（内含 API 限流）/ 证书签发（内含当前生效证书与详情）/ 配置审计 |
| `ConfigHub` | 配置管理 | 部署向导 / 运行参数（内含参数同步）/ 版本历史（内含对比与撤销）/ 网络配置 / 配置体检 / 配置锁定 / 导入导出 / 片段导入 |
| `RuntimeConfig` | 运行参数 | 总线 / Web / MQTT / NTP / Telemetry / Webhook / 原始 JSON |

已消歧义：`运行总览`（唯一“总览”）、`实时数据`（不再与运维侧“监控”重名）、`配置管理` vs `运行参数`（原“配置中心/运行配置”最易混）、`运维监控`（原“性能监控 / 链路诊断 / 健康巡检”三个 Hub 合并）、`批量任务`（原“批量监控”）、`协议调试台`（原“协议扫描 / 协议仿真 / 报文调试”三个 Hub 合并）、`固件与证书`（原“证书与升级”）、`运行日志`（原“日志中心”，现为系统设置 Hub 的日志分组）。

## 8. 页内文案术语表

页面内的按钮、弹窗主按钮、空状态、提示语共用一套动词，同一个动作全站只用一种说法。

| 场景 | 统一用词 | 禁用 / 慎用 | 说明 |
| --- | --- | --- | --- |
| 新建对象（按钮、弹窗主按钮、菜单项） | 新增 | 添加、新建 | 新增设备 / 新增点位 / 新增一行 / 新增网关 |
| 保存表单编辑 | 保存 | 提交、应用 | 只写库、不下发 |
| 把配置下发到设备或批量生效 | 应用 | 保存 | 应用失败 / 应用到 N 个点位 |
| 放弃本次编辑 | 取消 | 关闭 | |
| 关闭纯信息弹窗 | 关闭 | 取消 | 弹窗只有“知道了”性质的单按钮 |
| 恢复默认参数 | 重置 | 清空 | 恢复的是默认值 |
| 清除内容（不可恢复） | 清空 | 重置 | 需二次确认，并写明“不可恢复” |
| 二次确认主按钮 | 确定 | 确认 | 确认句本身可用“确认”（如“确认导入 N 条？”） |
| 彻底删除 | 彻底删除 | 删除 | 回收站场景 |
| 从列表移除关联 | 移除 | 删除 | 只解除关联，不删对象 |
| 生成并保存文件 | 导出 | 下载 | 导出配置 |
| 获取已有模板 / 文件到本地 | 下载 | 导出 | 下载模板 |
| 失败提示（toast） | …失败 | …错误 | 技术异常详情才用“错误” |
| 空状态 | 暂无数据 | 无数据、暂无记录 | 业务对象可细分：暂无点位 / 暂无规则 / 暂无枚举映射 |
| 周期字段名 | 采集周期 | 采集间隔 | |

补充规则：
- 空状态默认用 `EmptyState` / `DataTable` 的 `empty-text`，不手写“无数据”；
- 提示语中的页面名必须是第 7 节的现行中文名（如“请先在**运行参数**中新增总线”），不得出现“运行配置 / 配置中心 / 实时监控页”等旧称；
- 页内 `SectionHeader` 标题与所属 tab 名保持一致（如 `ConfigVersions` → 版本历史、`Wizard` → 部署向导）。

### 状态词与徽章色

- **徽章色**：`src/utils/statusType.ts` 的 `statusTagType()` 是全站唯一取色入口；页面不再各写 `xxxType()`。确需例外时用「本地覆盖表 + 委托通用表」，并在注释里写明为何不能直接委托。
- **组件健康状态**（`/health/ready` 的 `components`）：`src/utils/healthStatus.ts` 登记后端真实取值（组件 `process/mqtt/queue/ota/storage`，状态 `ok/down/backpressure/degraded`），中文名、严重度、徽章色同源；后端新增取值时只改这一个文件。
- **严重度与徽章色必须同档**：`ok→success`、`warning→warning`、`error→danger`，不允许出现「表格红徽章、汇总黄底色」的同页矛盾。
- **聚合口径同源**：仪表盘总览、异常信号、连通性横幅、诊断报告「健康检查」分节按同一份严重度判定；不直接使用后端聚合的 `status` —— 它对任何非 `ok` 组件都判 `degraded`，直接沿用会与组件明细口径不一致。
- **一词一义，不重复报**：组件故障只由「N 个组件异常 / 未就绪」表达；「网关状态」卡片与「健康检查未通过」信号只负责探针可达性，两者不叠加同一件事。
- **报告保留原始状态词**：诊断报告是现场发给研发的凭据，中文名后必须保留后端原始状态词（如「不可用（down）」），便于对网关日志核对。

## 9. 校验

```bash
npm run type-check   # vue-tsc
npm run lint         # eslint
npx vitest run       # 含 navTargets.spec.ts（菜单/搜索索引目标合法性）
```

参考实现：[routes/index.ts](../../gateway-web/src/router/routes/index.ts)、[frontend.md](./frontend.md#L66-L72)。
