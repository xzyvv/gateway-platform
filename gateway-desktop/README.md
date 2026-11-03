# Gateway Desktop

> **已冻结（2026-11）**：桌面端不再构建 / 打包 / 发布。
> 现场调试统一走 Web 控制台（`gateway-web` 的「协议控制台 / 采集调试」页签），
> 不再分发 Tauri 安装包；CI 的 `desktop-build` job 已移除。
> 代码保留以支持历史分支合并，冻结期间不接受新功能；如需恢复，见
> `.github/workflows/ci.yml` 的 gateway-desktop 冻结说明。

工业网关调试控制台 - 桌面客户端（Tauri）。

## 架构

```
gateway-desktop/          # 本项目（Tauri 桌面壳）
├── src-tauri/            # Rust 端
│   └── src/main.rs       # 入口：窗口 + HTTP 代理 + 本地配置存储
└── package.json

gateway-web/              # 前端（复用，不复制）
gateway-core/             # 网关引擎（lib：配置/采集/北向/队列）
gateway-api/              # Web API 面（axum + tower，HTTP/WS handler）
gatewayd/                 # 网关守护进程（bin：启动编排 + main，跑在设备上）
```

前端代码直接复用 `../gateway-web`，通过 `tauri.conf.json` 的 `frontendDist` 指向其 `dist/` 目录。

## 功能

1. **Tauri 窗口壳** - 加载 gateway-web 前端
2. **网关 HTTP 代理** - 前端通过 `invoke('proxy_request')` 转发请求到远端网关，绕开 CORS
3. **本地配置存储** - 网关地址列表存于 `appConfigDir/config.json`

## 开发

```bash
npm install
npm run dev       # 启动 Vite dev + Tauri 窗口
```

## 构建

```bash
npm run build     # 生成 .exe / .msi / .nsis 安装包
```

产物位于 `src-tauri/target/release/bundle/`。

## Tauri 命令

前端可调用的命令：

| 命令 | 说明 |
|---|---|
| `get_config` | 获取网关列表和当前激活项 |
| `get_config_load_error` | 读取配置文件失败时的错误信息（供前端提示） |
| `save_config` | 保存配置 |
| `set_active_gateway` | 切换激活的网关 |
| `proxy_request` | 代理 HTTP 请求到网关 |
| `scan_gateway_lan` | 局域网扫描发现网关 |
| `check_gateway_health` | 探测已配置网关的健康状态 |
| `check_url_health` | 探测任意 URL 可达性 |
| `forward_webhook` | 转发 Webhook 回调（绕过 CORS） |
| `get_diagnostic` | 采集桌面端诊断信息 |
