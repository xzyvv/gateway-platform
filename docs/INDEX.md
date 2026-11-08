# 文档索引

Gateway Platform 仓库所有文档的统一入口。按主题分类，便于快速定位。

## 顶层文档

| 文档 | 说明 |
|---|---|
| [`README.md`](../README.md) | 仓库总览、快速开始、仓库约定 |
| [`docs/architecture.md`](architecture.md) | 系统架构总览（项目概述 + 整体架构图 + 文档目录） |

## 架构文档（`docs/architecture/`）

`architecture.md` 按主题拆分到子目录，按需检索：

| 主题 | 文档 | 内容 |
|---|---|---|
| 前端技术栈 | [`architecture/frontend.md`](architecture/frontend.md) | Vue 3 + Vite + Element Plus + Pinia 技术栈与目录结构 |
| 前端状态管理 | [`architecture/frontend-state.md`](architecture/frontend-state.md) | Pinia store 设计、composable 职责边界与测试策略 |
| 前端命名规范 | [`architecture/frontend-naming.md`](architecture/frontend-naming.md) | 组件 / 文件 / Hub 页签 / 侧栏搜索索引命名与路由约定 |
| 后端技术栈 | [`architecture/backend.md`](architecture/backend.md) | Rust + Tokio + Axum + rusqlite + 北向 sink 技术栈与目录结构 |
| 通信协议 | [`architecture/protocols.md`](architecture/protocols.md) | REST/WS 前后端通信 + 南向采集协议 + 北向上报通道 |
| 核心模块说明 | [`architecture/modules.md`](architecture/modules.md) | 启动流程 / config / south / pipeline / queue / north / Web API / platform / engine / 目录约定 |
| 数据流 | [`architecture/data-flow.md`](architecture/data-flow.md) | 设备→采集→处理→队列→分发 完整链路 |
| 部署架构 | [`architecture/deployment.md`](architecture/deployment.md) | 目录布局 / 进程模型 / 开发联调 / ServeDir 静态托管 |
| 现场 TLS 决策 | [`architecture/tls-decision.md`](architecture/tls-decision.md) | 各链路加密现状 / 现场必答 4 问 / 分场景配置 / 已知不一致与排障速查 |
| 关键设计约束 | [`architecture/constraints.md`](architecture/constraints.md) | 稳定性 / 安全 / 资源占用 相关硬性约束 |

## 开发指南

| 文档 | 说明 |
|---|---|
| [`docs/adding-a-protocol.md`](adding-a-protocol.md) | 新增南向采集协议的分步指南 + 可复制模板（注册、帧提取器、feature gate、测试清单） |

## API 契约

| 文档 | 说明 |
|---|---|
| [`docs/api-surface.md`](api-surface.md) | **全量端点清单**（REST + WebSocket，含鉴权层级与 handler）：由 `scripts/gen_api_surface.py` 从 `gateway-api/src/web/**/routes.rs` 生成，CI `--check` 校验，禁止手改 |
| [`docs/api-access.md`](api-access.md) | 访问控制域（`/api/access/*`）REST 契约：限流规则 / API 密钥 / 计划任务 / 访问日志 / 统计 |
| [`docs/api-devices.md`](api-devices.md) | 设备运维域（`/api/devices/*`、`/api/device-groups/*`、`/api/io/*`、`/api/discover`）REST 契约：通信质量 / 退避 / 健康分 / 影子 / 拓扑 / 资产 / 控制 / 分组，含已知偏差清单 |
| [`docs/api-protocol-tools.md`](api-protocol-tools.md) | 协议工具域（`/api/protocol-tools/*`、`/api/protocols/*`）REST 契约：发帧 / 抓包 / 回放 / 探测 / 扫描 / 从站模拟 / 转换规则；**§3 明确 4 处进程内临时态及留证据方式** |

## 现场运维

| 文档 | 说明 |
|---|---|
| [`docs/装机前准备清单.md`](装机前准备清单.md) | **面向现场工程师**的装机前准备清单：软件产物 / 系统前置 / 硬件接法（串口驱动、稳定名、DeviceAllow、OTA 分区）/ 完整配置 / fail-closed 安全顺序 / 许可证前置，附「看着正常其实没生效」速查表 |
| [`docs/容量规划.md`](容量规划.md) | **点位规模 → 磁盘占用换算参考表**：点位 × 采集周期 → 每天行数 → `queue.max_rows` 建议值 → eMMC/SD 卡选型；含原始队列/聚合表/备份/WAL/journald 五项占用总账、磁盘水位四档、现场自查命令与常见误配 |
| [`docs/config-reload-matrix.md`](config-reload-matrix.md) | **配置生效矩阵**：每个配置分节保存后是热更、需重启还是纯元数据；含持久化/落盘失败语义与「新增分节必须做的 4 步」 |
| [`docs/license-签发流程.md`](license-签发流程.md) | **许可证签发的顺序依赖**：公钥落地 → 装机取码 → 签发 → 校时 → 导入 → 配点，共 8 条「必须先于否则」依赖，逐条给出代码依据与现场可见现象 |
| [`docs/故障处置卡.md`](故障处置卡.md) | 现场 6 类常见故障（证书过期 / License 到期 / NTP 失步 / 看门狗触发 / 网络切换 / 南向长时无响应）的判定入口、处置步骤与数据完整性核对口径 |
| [`docs/linux-branch-review.md`](linux-branch-review.md) | **Linux 专属分支审查报告**（部署前风险排除）：14 处 `cfg(target_os="linux")` 代码块清单、已修 **19 项** P0/P1 缺陷（两轮：看门狗 / OTA 变砖 / GPIO EBUSY / failover 静默断网 / 密钥权限竞态 / 优雅停机误复位 / A/B 回滚失效 / 并发写分区交错 / 北向静默无数据 / 心跳僵死检测失效 / 许可证指纹漂移）、跨平台纯函数化验证手段、长周期 soak 回归验证（3h 基线 + 2h post-fix，判定两次全绿）、真机验收清单 |

## 性能基线报告

`gateway-core/scripts/perf_baseline.py` 压测脚本产出的基线报告。解读 RSS / 队列
深度 / 吞吐数据前，**必读** [`docs/perf-rss-explanation.md`](perf-rss-explanation.md)，
否则易把「未配置北向消费 + page cache 持有」误判为泄漏。

| 文档 | 内容 |
|---|---|
| [`docs/perf-baseline.md`](perf-baseline.md) | 13 个场景的**汇总基线**（50 ~ 20 000 点）：参数与结果总表、分场景结论、读数口径（队列上限截断 / p99 饱和 / 整轮失败） |
| [`docs/perf-knee.md`](perf-knee.md) | 150 000 点**拐点**压测：总线数 × tcp 并发矩阵；结论「吞吐 ≥ 150k 点/秒，瓶颈在测试机 CPU」 |
| [`docs/perf-rss-explanation.md`](perf-rss-explanation.md) | RSS 增长**归因**：未配置北向消费 → SQLite page cache 持有，附运维判据与复现命令 |
| [`docs/bench-baseline.md`](bench-baseline.md) | **微基准基线**（criterion 0.5）：CRC / Modbus 帧构建与校验 / 内存队列 / SQLite 周期 / DataQueue 往返的单点成本与回归判定口径 |

## 功能盘点与路线图

| 文档 | 说明 |
|---|---|
| [`docs/functional-review-and-next-phase.md`](functional-review-and-next-phase.md) | 上一阶段功能评审结论：实测体量基线、三因子价值评估、去留决策与各轮落地记录 |
| [`docs/functional-inventory-and-next-roadmap.md`](functional-inventory-and-next-roadmap.md) | 全量功能清单（后端 16 路由组 / 前端 10 Hub）+ SKU 与授权分层现状 + 下一阶段路线图（§4 行动项、§5 验收口径） |

## 设计文档（`docs/design/`）

未实施或实施中的功能设计方案，实施完成后请更新文档内状态标注：

| 文档 | 说明 |
|---|---|
| [`docs/design/license-v2-quota.md`](design/license-v2-quota.md) | **现行模型** v2.0：license 仅约束 4 类产能配额（点数/协议数/存储天数/北向通道），调试分析功能全部免费 |
| [`tools/license-issuer/README.md`](../tools/license-issuer/README.md) | 离线授权签发工具（Ed25519）：keygen / issue 用法、私钥红线、自签发自用说明 |

## 后端（Rust：gateway-core / gateway-api / gatewayd）

| 文档 | 说明 |
|---|---|
| [`gateway-core/README.md`](../gateway-core/README.md) | Rust 引擎细节、配置说明、编译产物 |
| [`gateway-core/scripts/README.md`](../gateway-core/scripts/README.md) | CI 脚本、性能基线脚本说明 |
| [`gateway-core/tests/chaos/README.md`](../gateway-core/tests/chaos/README.md) | 混沌测试说明 |

## 前端（gateway-web）

| 文档 | 说明 |
|---|---|
| [`gateway-web/VISUAL_SPEC.md`](../gateway-web/VISUAL_SPEC.md) | 前端视觉规范、设计系统 |

## 桌面端（gateway-desktop）

**已冻结（2026-11）**：桌面端不再构建 / 打包 / 发布，代码保留供历史分支合并。

| 文档 | 说明 |
|---|---|
| [`gateway-desktop/README.md`](../gateway-desktop/README.md) | Tauri 桌面端结构说明与冻结声明（构建与打包流程已停用） |

## 目录命名约定

为避免混淆，仓库中与"web"相关的目录含义如下：

| 路径 | 含义 | 是否进 git |
|---|---|---|
| `gateway-web/` | Vue 前端**源码** | 是 |
| `gateway-api/src/web/` | Rust Web API **源码模块** | 是 |
| `gateway-api/src/web/builtin_ui.html` | 编译进二进制的**兜底 UI** | 是 |
| `gateway-core/ui-dist/` | `npm run deploy:gateway` 生成的前端 dist **部署目录**（gatewayd 默认 `./ui-dist`，可用 `GATEWAY_WEB_DIR` 覆盖） | 否（gitignore） |
