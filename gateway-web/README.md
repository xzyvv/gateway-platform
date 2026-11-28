# gateway-web

网关管理平台的 Web 前端：Vue 3 + TypeScript + Vite，Element Plus 组件体系。以 10 个 Hub、70+ 页签组织设备接入、协议调试、数据管道、系统监控等功能。

## 快速开始

**环境要求**：Node.js **22**（CI 与 vitest 5 的 engines 均要求 `^22.12`，Node 20 会导致测试 worker 崩溃）。

```bash
cd gateway-web
npm install
cp .env.example .env.local      # 按需修改；.env.local 不入库
npm run dev                     # http://localhost:5173
```

开发服务器监听 `0.0.0.0:5173`，将 `/api` 与 `/health/` 反向代理到 `VITE_API_BASE_URL`（默认 `http://127.0.0.1:8080`，即本地 gatewayd）。**笔记本网线直连网关**的调试场景下，把该变量改成网关实际 IP（见 `vite.config.ts:17` 的说明）。生产构建移除 proxy，由 nginx 反代 `dist/` 与 `/api/*`。

## 常用命令

| 命令 | 用途 |
|---|---|
| `npm run dev` | 启动开发服务器（:5173，带 API 代理） |
| `npm run build` | `vue-tsc` 类型检查 + Vite 生产构建，产物在 `dist/`（700KB 分包预算） |
| `npm run preview` | 本地预览构建产物 |
| `npm run deploy:gateway` | 构建并把 `dist/` 拷入 `gateway-core/ui-dist`，供后端内嵌 |
| `npm run type-check` | 仅跑 Vue TS 类型检查 |
| `npm run lint` / `npm run lint:fix` | ESLint 检查 / 自动修复（含 800 行文件预算、冻结账本门禁） |
| `npm run format` | Prettier 格式化 |
| `npm test` / `npm run test:watch` / `npm run test:coverage` | Vitest 单测（一次性 / 监听 / 覆盖率） |
| `npm run e2e` | Playwright 浏览器测试，自动编排隔离网关 + dev server |
| `npm run e2e:install` | 首次运行前安装 Chromium（只需一次） |
| `npm run types:export` | 从 Rust 后端导出 TS 类型到 `src/api/generated/`（需 cargo 环境） |
| `npm run types:check` | 导出后校验 `generated/` 无 diff，防止前后端契约漂移 |

## 测试

- **单测**：Vitest + jsdom，spec 与被测源码同目录（`src/**/*.spec.ts`），数量与用例数以实际运行输出为准——执行 `npm test` 查看汇总。约定与 mock 手法见 [`src/test/README.md`](src/test/README.md)。
- **E2E**：Playwright，用例在 `e2e/`。`npm run e2e` 会自动起一个**空配置的隔离网关**（默认 :18080）和独立 dev server，结束自动回收；`reuseExistingServer: false` 确保不会误打到真实设备或真实配置。覆盖路由巡游、写链路、调试路径等；首次使用先跑 `npm run e2e:install`。详见 [`docs/architecture/frontend.md`](../docs/architecture/frontend.md) 3.5 节。
- **类型契约**：`src/api/generated/` 由后端 Rust struct 生成，**不要手改**。后端类型变动后跑 `npm run types:export` 同步。

## 目录结构

`src/` 下按职责分 12 个一级目录（`api` / `app` / `assets` / `components` / `composables` / `mock` / `router` / `stores` / `test` / `types` / `utils` / `views` 等）。各目录的职责边界、命名约定与分层规则见 **[`docs/architecture/frontend.md`](../docs/architecture/frontend.md) 3.3 / 3.4 节**（权威来源，此处不重复以免漂移）；组合式函数的分类与边界另见 [`src/composables/README.md`](src/composables/README.md)。

一个值得注意的约定：`src/utils` 顶层平铺了大量工具文件，改动前先全局搜索同名/近义实现（历史上出现过字节工具三套并存，DD-47）。

## 环境变量

见 [`.env.example`](.env.example)（含逐项注释）：

| 变量 | 默认 | 说明 |
|---|---|---|
| `VITE_USE_MOCK` | `false` | 仅用于无真实后端端点的演示功能（数据压缩、协议调试等）；有真实 API 的功能一律走真实请求 |
| `VITE_API_BASE_URL` | `http://127.0.0.1:8080` | 仅 dev server 代理目标；生产由 nginx 反代，无需配置 |

## 更多文档

- 前端架构、目录与 E2E 约定：[`docs/architecture/frontend.md`](../docs/architecture/frontend.md)
- 功能摸底台账（10 Hub / 72 页签）：[`docs/audit-2026-11/01-功能摸底台账.md`](../docs/audit-2026-11/01-功能摸底台账.md)
- 前端技术债务摸底与 ROI 清单：[`09-前端专项技术债务摸底.md`](../docs/audit-2026-11/09-前端专项技术债务摸底.md) · [`10-前端债务ROI推荐清单.md`](../docs/audit-2026-11/10-前端债务ROI推荐清单.md)

---

## 优化方向

**已完成（截至 2026-11-28）**：

- **菜单与页面**：8 次 Hub 合并，收敛为 10 个 Hub / 72 个页签（合并记录见 `src/router/routes/index.ts` 注释）；
- **架构**：传输层收敛到 `api/http/` 12 个模块；路由拆为 `router/index.ts` + `router/routes/index.ts`；composables 按 10 个业务域分组；
- **目录**：`src/` 收敛为 12 个一级目录、10 个业务域目录，并以 800 行门禁 + 冻结账本防止巨型文件回潮。

**剩余（结构性收敛，不新增功能面）**：C 档 6 项（DD-73 diagnostics 双 barrel、DD-75「会话」四义、DD-76 `use*` 命名、DD-85 `api/` 分组、DD-86 `utils/` 反向依赖、DD-87 `components` 依赖 stores）与 D 档 DD-83（`utils/` 顶层 71 文件分层，随其他改动捎带），逐项与 ROI 见 [`18-前端第五轮债务ROI推荐清单.md`](../docs/audit-2026-11/18-前端第五轮债务ROI推荐清单.md)。目录同义词（DD-81）与 `deviceComms` 规则盲区（DD-84）已分别于 2026-11-28 收口。

整体原则：**易用、易理解、易上手**；可参考成熟工业物联网平台与主流管理后台的交互设计作为对标。
