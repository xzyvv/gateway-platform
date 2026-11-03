# Gateway Platform

工业协议网关统一仓库 — Vue 3 调试前端 + Rust 核心引擎，同仓库 / 分目录协作。

```
gateway-platform/
├── gateway-web/       Vue 3 + Element Plus 调试前端（源码）
├── gateway-core/      Rust 工业网关核心引擎（lib：配置/采集/北向/队列）
├── gateway-api/       Web 调试面（axum + tower，HTTP/WS handler，拆自 gateway-core）
├── gatewayd/          网关守护进程（bin：启动编排 + main 入口）
├── gateway-desktop/   Tauri 桌面端（封装 gateway-web 为桌面应用）
├── docs/              架构文档与索引
├── scripts/           平台级脚本（dev-all / repo health）
├── .github/           CI 工作流
└── Cargo.toml         Cargo workspace 根
```

## 快速开始

### 1. 启动后端（Rust daemon）

```bash
cd gateway-core
cargo run --release -p gatewayd   # 默认监听 0.0.0.0:8080
```

> 二进制由 `gatewayd` crate 提供；在 `gateway-core/` 目录运行可让默认 `./config.json`
> 解析到 `gateway-core/config.json`（历史 dev 约定）。也可在仓库根直接 `cargo run -p gatewayd`。

启动前需配置 `web.api_token`（≥16 字符），未配置则 Web 服务拒绝启动（fail-closed）。

### 2. 启动前端（开发模式）

```bash
cd gateway-web
npm install
npm run dev                  # 默认 5173 端口，自动代理 /api 到 8080
```

打开 http://localhost:5173 ，登录用后端 config.json 里的 username/password。

### 3. 一键联调（PowerShell）

```powershell
.\scripts\dev-all.ps1
```

同时启动 Rust daemon + Vite dev server，Ctrl+C 同步停掉。

## 测试

```bash
# 前端（gateway-web/ 下）
npm run test          # Vitest 单元测试
npm run type-check    # vue-tsc 类型检查
npm run lint          # ESLint
npm run e2e           # Playwright E2E 冒烟（自动拉起空配置 gatewayd + vite :5199）

# 后端（workspace 根）
cargo test --lib
```

E2E 为 6 条只读冒烟链路（栈健康 / 登录 / 路由巡游 / 暗色 / 404 / 登出），CI 中为**强制门禁**（见 `.github/workflows/ci.yml` 的 `e2e` job），失败即阻塞合并，报告见 Actions artifact `e2e-playwright-report`。约定细节见 [`docs/architecture/frontend.md`](docs/architecture/frontend.md)。

## 仓库约定

| 子项目 | 语言 | 构建 | 产物 |
|---|---|---|---|
| gateway-web | TypeScript / Vue 3 | `npm run build` | `gateway-web/dist/` |
| gateway-core | Rust 2021 | `cargo build --release -p gateway-core` | `target/release/libgateway_core.rlib`（引擎库） |
| gateway-api | Rust 2021 | `cargo build --release -p gateway-api` | `target/release/libgateway_api.rlib`（Web 面库） |
| gatewayd | Rust 2021 | `cargo build --release -p gatewayd` | `target/release/gatewayd`（可执行） |
| gateway-desktop | Rust (Tauri) | ~~`cd gateway-desktop/src-tauri && cargo build`~~ | **已冻结**，不再构建/发布（现场调试用 Web 控制台） |

三个子项目独立编译。gateway-web 与后端通过 REST + WebSocket 通信（`gateway-api` 提供 HTTP/WS 面，
`gatewayd` 组装并启动引擎与 Web 服务）；gateway-desktop 封装 gateway-web 为桌面应用。

## 配置 & 数据

- 主配置：`gateway-core/config.json`（设备 / 点位 / 总线 / 转发 / 鉴权）
- 历史缓冲 + MQTT 离线队列：`gateway-core/data_queue.db` (sqlite, WAL)
- 配置历史快照：`gateway-core/config_versions.db` (sqlite)

## API 鉴权

前端 axios 自动附加 `Authorization: Bearer <token>`，token 存于 `localStorage.gateway_api_token`。

## 文档

- [`docs/INDEX.md`](docs/INDEX.md) — 文档索引（所有文档入口）
- [`docs/architecture.md`](docs/architecture.md) — 架构图 + 数据流
- [`gateway-core/README.md`](gateway-core/README.md) — Rust 引擎细节
- [`gateway-core/scripts/README.md`](gateway-core/scripts/README.md) — CI / 性能脚本说明
- [`gateway-web/VISUAL_SPEC.md`](gateway-web/VISUAL_SPEC.md) — 前端视觉规范
- [`gateway-desktop/README.md`](gateway-desktop/README.md) — Tauri 桌面端说明

