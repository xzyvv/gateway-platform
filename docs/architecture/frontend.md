# 前端技术栈（gateway-web/）

> Gateway Platform 文档子页：原 [architecture.md](../architecture.md) 第 3 节独立成页，便于按主题检索与维护。

---

## 3. 前端技术栈（gateway-web/）

### 3.1 核心框架

| 技术 | 版本 | 用途 |
|---|---|---|
| **Vue 3** | (由 `@vitejs/plugin-vue` 引入) | UI 框架，所有页面为 `.vue` 单文件组件，Composition API + `<script setup>` |
| **TypeScript** | ~6.0.2 | 全量类型约束，`tsc && vite build` 构建前类型检查 |
| **Vite** | ^8.3.0 | 构建工具 + 开发服务器（5173 端口），`vite.config.ts` 配置 `/api`、`/health` 代理到后端 8080 |
| **Element Plus** | ^2.14.6 | UI 组件库，`app/main.ts` 全局注册，配中文语言包 zh-cn |
| **@element-plus/icons-vue** | ^2.3.2 | Element Plus 图标集 |
| **Pinia** | ^4.0.3 | 状态管理，如 `stores/auth.ts` 管理登录 token、登录态、会话 |
| **Vue Router** | ^5.3.1 | 路由，`router/index.ts` 定义页面路由与登录鉴权守卫 |
| **Axios** | ^1.20.0 | HTTP 客户端，`api/index.ts` 封装拦截器（自动 Bearer token、统一解包 `{code,message,data}`、401 跳登录、请求日志） |
| **ECharts** | ^6.1.0 | 图表渲染，`utils/echarts.ts` 按需引入（LineChart/CanvasRenderer），用于磁盘健康、指标大盘等 |

### 3.2 工程化

| 技术 | 用途 |
|---|---|
| **Vitest** ^5.0.3 | 单元测试（`npm run test`） |
| **@vue/test-utils** ^2.5.1 | Vue 组件测试工具 |
| **jsdom** ^29.1.1 | 测试环境（DOM 模拟） |
| **@playwright/test** ^1.64.0 | E2E 冒烟（`npm run e2e`，仅 chromium；用例在 `e2e/`，详见 3.5） |
| **unplugin-auto-import** | Vue/Element Plus API 自动导入 |
| **unplugin-vue-components** | Element Plus 组件自动按需导入 |

### 3.3 前端目录结构

```
gateway-web/src/
├── api/                # REST 接口封装（40+ 模块：auth/devices/points/uplink/diagnostics...）
│   ├── generated/      # ts-rs 从 Rust struct 自动生成的 TS 类型（勿手改，cargo test export_ts_types 重新生成）
│   ├── http/           # 传输层：instance/request/响应拦截器/日志脱敏/登出门（barrel 出口 index.ts）
│   └── protocolTools/  # 协议工具按业务域分组的接口模块
├── assets/             # 静态资源（全局样式等）
├── components/         # 通用组件（HubShell/StatCard/ErrorBoundary/RequestMonitor...）
│   ├── dataTable/      # 表格子组件（普通表 / 虚拟滚动 / 分页器 / 错误条），DataTable.vue 为组合入口
│   └── industrial/     # 工业场景控件（波特率/校验位/Modbus 地址输入等）
├── composables/        # 组合式函数（30+ 个，按域分子目录：core/auth/devices/config/dashboard/
│                       #   data/system/layout/uplink；详见 composables/README.md 分类与边界约定）
├── mock/               # 仅放「模拟后端」模块（现仅 1 个：用户权限演示数据，dev 模式下按需
│                       #   动态加载、生产构建被整链消除）；mocks.spec.ts 做孤儿检测，
│                       #   页面接入真实 API 后强制删除对应 mock
├── router/             # 路由配置（index.ts 装配 + routes/index.ts 页面路由，按业务域登记）
├── stores/             # Pinia 状态（auth/config/gateway/notifications/requestLog/theme/
│                       #   tier/websocket 共 8 个 store；仅放跨页面共享状态，
│                       #   页面内数据由 composables 持有，见 composables/README.md 边界约定）
├── test/               # 测试 setup（vitest，测试文件 .spec.ts 与源码 co-locate）
├── types/              # 全局类型（角色、RouteMeta 增强）
├── utils/              # 工具函数（echarts/formulaEngine/frameParser/exportCsv/secureStorage/
│                       #   addrCalculator/configFragment/batchImportCsv...）
│   └── websocket/      # WebSocket 客户端（连接/心跳/重连/分发/订阅/连接池），index.ts 为出口
├── views/              # 业务页面（140+ .vue：页面 + 同域子组件，一级目录 = 业务域：
│                       #   overview/devices/points/acquisition/data/uplink/ops/system/auth 等）
│   └── layout/         # 框架外壳（Sidebar/Topbar 及其组合式函数）
│       └── menu/       # 侧边菜单分组与侧栏搜索索引数据源
├── app/                # 应用装配层（入口与根组件）
│   ├── App.vue         # 根组件
│   └── main.ts         # 入口（Pinia/Router/ElementPlus 注册）
```

**大文件拆分约定（W2-#15/#16）**：单文件超过 ~500 行即视为「巨型文件」，必须按职责拆为
同域子目录 + barrel 出口（`index.ts` 原样再导出全部对外符号），调用方 import 路径不变；
组件拆分为「主组件（状态与编排）+ 子组件（渲染）」，composable 拆分为「index 组合根 +
按职责分模块」。拆分只改结构不改行为，既有 spec 必须原地继续通过。

**约定已 lint 化（O-8）**：`eslint.config.js` 的 `max-lines`（500 行）把它变成机器断言。
存量超限文件登记在 `LINE_BUDGET_EXEMPT` 清单里豁免，**新文件超限即 warning**——
CI 用 `--max-warnings 0` 跑 lint，所以等于阻断。拆到 500 行以内后请把对应行从清单删掉，
否则它会一直享受豁免。测试文件（`*.spec.ts`）与自动生成的 `ep-icons-compact.ts` 不纳入。
后端同规但采用「只减不增」的棘轮：`scripts/deploy_lint.ps1` 的 C24 段扫描
`gateway-core` / `gateway-api` / `gatewayd` 的 `src/**/*.rs`，基线在
[`scripts/line_budget_frozen.txt`](../../scripts/line_budget_frozen.txt)。基线每行带单文件行数上限，
取「登记时当前行数 + 20 行余量」：未登记的新文件 > 500 行、或已登记文件超过该上限，
都直接判 FAIL（不再只是 WARN），所以存量只能瘦身、不能继续长。

### 3.4 路由 meta 与命名约定

- 命名分层：**目录 = 业务域（名词）**，**页面 = 对象 + 视图能力**（`DeviceTopology` / `PointHub` / `DataPipelineHub`），Hub 容器统一 `*Hub.vue`；
- 路径语义 = `/<业务域>[/<视图能力>]`（如 `/devices/topology`、`/system/config`），Hub 内 tab 一律用 `?tab=<name>` 表达，不再为 tab 登记 redirect 别名；
- 路由 `name` 与页面组件名一致，便于从路由反查文件；页面路由统一在 [routes/index.ts](../../gateway-web/src/router/routes/index.ts) 维护，`title` / `requireAdmin` 直接写在记录的 `meta` 中；
- v2.0 起路由不再携带 `tier`：调试/分析/批量功能对所有档位（含失效态）免费，`tier` 不参与访问控制（历史 tier 守卫与 `promoteMeta` 提升逻辑已移除）；
- 消费方（面包屑 `Layout.vue`、`document.title` 的 `main.ts` afterEach、admin 守卫）一律从 `meta` 读取。

### 3.5 E2E 冒烟（Playwright）

- 运行：`npm run e2e` 一条命令，webServer 自动编排**空配置 gatewayd**（`scripts/e2e/start-gateway.mjs`：临时目录写配置 + /health/live 轮询 + 退出清理）与 **vite dev server :5199**，测试结束自动回收；
- 范围刻意克制：仅 chromium、串行（workers=1）、retries=0、6 条只读核心链路（栈健康 / 登录 / 路由巡游 / 暗色持久化 / 404 与降级页 / 登出），不做写操作、不碰串口 / MQTT / OTA；
- 用例约定：禁止固定 sleep，只用 web-first 断言；定位优先 role / placeholder / 可见文本；浏览器形态下登录页不再触发局域网扫描（`isTauri()` 为 false 时直接跳过 discovery），`e2e/helpers.ts::stubLanDiscovery` 仍拦截 `/health/live` 作为兜底；
- CI：`.github/workflows/ci.yml` 的 `e2e` job（ubuntu）为**强制门禁**——用例失败即阻塞合并；失败报告 / trace 见 Actions artifact `e2e-playwright-report`（`if: always()` 上传，失败时也能取到现场）。

### 3.6 运行形态：桌面端 vs 浏览器直连

`utils/tauri.ts::isTauri()` 区分两种运行形态，决定登录页字段与网络地址来源：

| 形态 | 登录页 | API / WS 地址来源 |
|---|---|---|
| 桌面端（Tauri，`gateway-desktop/`） | 网关地址输入 + 局域网发现扫描 | 用户填写 / 发现结果写入 `stores/gateway` |
| 浏览器直连（网关 `ui-dist` 自托管） | 只填用户名 / 密码，顶部展示当前直连主机 | 同源：API `baseURL: '/'`，WS `ws(s)://<window.location.host>/ws/...` |

浏览器直连是现场「笔记本网线直连网关」的主用形态；网关侧需开启 `listen_addr`、设置 `password_hash`
并部署 `ui-dist`，详见 [deployment.md §8.3](deployment.md)。

---
