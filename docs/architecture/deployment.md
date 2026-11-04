# 部署架构

> Gateway Platform 文档子页：原 [architecture.md](../architecture.md) 第 8 节独立成页，便于按主题检索与维护。

---

## 8. 部署架构

### 8.1 目录布局（Linux）

```
/etc/gateway/
  config.json              # 主配置
  templates/builtin/       # 内置设备模板
  templates/custom/        # 用户模板
/var/lib/gateway/
  data_queue.db            # 数据队列（SQLite WAL）
  mqtt_outbox.db           # MQTT 离线 outbox
  config_versions.db       # 配置版本快照
  ota/                     # OTA 固件暂存
  certs/                   # TLS 证书（自动生成）
/var/log/gateway/
  gateway.log              # 主日志（按天滚动，保留 7 天）
  audit.log                # 审计日志
/opt/gateway/
  gateway                  # 主程序二进制
  web/                     # 前端静态资源（ServeDir 托管，缺失时回退嵌入式 UI）
```

### 8.2 进程模型

- 单进程 `gatewayd`，Tokio 多线程运行时
- systemd 管理（`deploy/gateway.service`），开机自启 + 崩溃重启
- 看门狗：软件（任务喂狗）+ 硬件 `/dev/watchdog`

### 8.3 开发联调与浏览器直连

**开发联调（前端热更新）**

```powershell
# 后端：cargo run --release -p gatewayd  → 127.0.0.1:8080（默认仅回环，见 8.3.2）
# 前端：npm run dev          → http://localhost:5173（vite 代理 /api、/health 到 8080）
```

**8.3.1 浏览器直连（现场网线调试）**

笔记本用网线直连网关后，浏览器打开 `http://<网关IP>:8080/` 即可使用调试控制台：
前端页面由网关自身托管，登录页只要求输入用户名 / 密码，**无需填写网关地址**
（前端 API 走同源 `/api`，WebSocket 地址取 `window.location.host`）。
该模式不受「桌面端（Tauri）需要地址输入 + 局域网扫描」的限制。

现场启用需同时满足三件事：

1. `web.listen_addr` 设为 `0.0.0.0:8080`（默认 `127.0.0.1:8080` 只允许本机访问）；
2. `web.password_hash` 非空 —— 否则 `security_violations()` 触发 **fail-closed**，网关拒绝启动
   （`0.0.0.0` + 默认凭据 admin/admin = 硬性违规）。首次部署可先用默认凭据在回环地址登录，
   `POST /api/auth/password` 改密（哈希会持久化到 `config.json`），再改监听地址；
3. 前端产物已部署到 `gateway-core/ui-dist/`（见 8.3.2）。

> HTTP 访问属非安全上下文，`crypto.subtle` 不可用，前端 `secureStorage` 会降级为明文 localStorage
> 并记录降级告警；功能不受影响，仅安全等级下降。

**8.3.2 前端产物托管（ServeDir）**

生产部署：前端 `dist/` 静态文件由后端 `tower_http::services::ServeDir` 托管，无 CDN 依赖，支持网线直连离线调试。

- 部署命令：`cd gateway-web && npm run deploy:gateway` —— 先 `npm run build`，再清理并复制 `dist/` 到 `gateway-core/ui-dist/`
- 静态目录由环境变量 `GATEWAY_WEB_DIR` 控制（默认相对路径 `./ui-dist`，即以 `gateway-core/` 为工作目录启动）；
  生产环境由 `install.sh` 设为 `/opt/gatewayd/ui-dist`
- SPA fallback：未匹配的路由请求返回 `ui-dist/index.html`，由前端 Vue Router 接管
- 嵌入式兜底：若 `ui-dist/` 目录不存在或 `index.html` 缺失，返回编译时嵌入的内置 UI 页面，保证网关首次启动即可访问
- 产物为纯静态文件，ServeDir 每请求读取，替换 `ui-dist/` 内容后**无需重启**网关

**8.3.3 一键交付（安装 → 验证 → 交付检查单）**

发布包（`deploy/build-release.sh` 产物）内含 `install.sh` / `verify-deploy.sh` / `deliver.sh`，三者必须同目录：

```bash
scp -r deploy/out/ gateway@<target>:/tmp/gw-out
ssh gateway@<target> 'cd /tmp/gw-out && sudo bash deliver.sh'
```

- `install.sh`：落盘二进制/前端/模板/systemd 服务，注入 `GATEWAY_WEB_DIR=/opt/gatewayd/ui-dist`，启动后轮询 `/health/live`；升级场景先停服再替换二进制（避免运行中覆盖 `ETXTBSY`）
- 监听地址：未显式 `--port`/`--host` 时，`install.sh`、`deliver.sh`、`verify-deploy.sh` 都从 `/etc/gateway/config.json` 的 `web.listen_addr` 探测（gatewayd 不读 `GATEWAY_PORT` 环境变量），自定义端口不会被 8080 默认值误判
- `verify-deploy.sh`：7 组检查（服务/端口/健康/API 契约/前端/硬件/资源），每项 PASS/FAIL/WARN，失败项附诊断命令
- 端口检查区分绑定范围：`0.0.0.0`/`*`/`[::]` 记 PASS；仅 `127.0.0.1`/`[::1]` 记 **WARN**（本机验证可用，但车间 LAN 打不开），检查单与处置建议同步提示改为 `web.listen_addr=0.0.0.0:<port>`
- 交付检查单：由 `verify-deploy.sh` 自动导出 Markdown 到 `<包目录>/out/delivery-checklist-<时间>.md`，含构建信息、逐项结果与处置建议，可直接签字留档；`--report <path>` 可改路径
- `deliver.sh` 编排三段：安装失败时**仍继续验证**，把失败项写进检查单；最终退出码非 0 一律不得交付
- 完整 `config.json` 不随包分发：现场按需提供 `/etc/gateway/config.json`（`web.listen_addr` / `api_token` / `password_hash`）；缺失时 gatewayd 以安全默认值启动（仅回环 + 自动生成 token）

### 8.4 时区与计划任务

计划任务（系统中心 → 计划任务）的 cron 表达式按**网关设备本地时区**计算：
「每天 02:00」指设备本地凌晨 2 点，不是 UTC 2 点，也不是浏览器所在时区的 2 点。

设备时区按以下优先级解析：

| 优先级 | 来源 | 形式 | 依赖 tzdata | 含夏令时 |
|---|---|---|---|---|
| 1 | `GATEWAY_TZ` 环境变量 | 固定 UTC 偏移：`+08:00` / `+0800` / `UTC+8` / `Z` | 否 | 否 |
| 2 | `TZ` 环境变量 | IANA 时区名：`Asia/Shanghai` | 是 | 是 |
| 3 | 操作系统 | `/etc/localtime` | 是 | 是 |
| 4 | 兜底 | UTC（启动日志打告警） | — | — |

- `GATEWAY_TZ` 是**运维可控的显式项**：固定偏移不依赖时区数据库，工业现场常用的精简镜像（无 `tzdata`）也能正确执行「每天凌晨 2 点」
- `TZ` 走 IANA 时区名，含夏令时切换；`TZ` 指向的时区文件缺失时日志会告警（否则会静默退化成 UTC）
- 兜底到 UTC 的场景：既没有 `GATEWAY_TZ`，也没有 `TZ`，且系统既无 `/etc/localtime` 也无 zoneinfo 目录 —— 这种情况**一定会在日志里出现告警**，不允许静默
- 生效时区在启动日志里打印，运维可据此核对「每天 02:00」到底是哪儿的 02:00：

```text
device timezone: Asia/Shanghai (UTC+08:00)
device timezone: 固定偏移 UTC+08:00（来源 GATEWAY_TZ）
```

- **容器/Docker 部署必须显式设置时区**（镜像默认多为 UTC），否则计划任务整体偏移：

```bash
# docker
docker run -e TZ=Asia/Shanghai ...
# docker-compose
environment:
  - TZ=Asia/Shanghai
# systemd 单元
[Service]
Environment=TZ=Asia/Shanghai
# 无 tzdata 的精简镜像：用固定偏移，不依赖时区数据库
Environment=GATEWAY_TZ=+08:00
```

- `install.sh` 会自动把时区写进 systemd 单元（`GATEWAY_TZ` > `TZ` > 探测系统时区），探测不到时打印告警并提示修复命令
- 验证：设备上执行 `date`，确认输出的时区缩写/偏移符合预期；再查 `journalctl -u gateway | grep "device timezone"` 确认网关实际采用的时区
- 前端「下次执行时间」显示的是同一时刻在**浏览器时区**下的墙钟读数；设备与浏览器时区不一致时读数不同属正常现象，实际触发时刻不变

### 8.5 二进制兼容性（glibc 与静态链接）

gnu 动态链接的产物会把**构建机的 glibc 版本**变成隐藏的运行要求。CI 与常见的构建机是 Ubuntu 22.04/24.04（glibc 2.35/2.39），而现场设备常见 Debian 10（2.28）、Debian 11（2.31）、Buildroot/Yocto（musl）——这类设备上二进制连启动都到不了，直接 `GLIBC_2.39 not found`。

**交付优先用静态链接**（musl target，产物不依赖目标机任何 libc）：

| 构建目标 | 链接方式 | 适用 |
|---|---|---|
| `--target linux-x86_64-musl` | 静态 | x86_64 网关（首选） |
| `--target linux-arm64-musl` | 静态 | ARM64 网关（首选） |
| `--target linux-armv7-musl` | 静态 | ARMv7 网关（首选） |
| `--target linux-arm64` / `linux-armv7` / `linux-x86_64` | 动态（gnu） | 仅当确认目标系统 glibc ≥ 构建机版本 |

```bash
# 静态包（推荐）
./deploy/build-release.sh --target linux-arm64-musl
# 目标系统与构建机 glibc 一致时才用动态包
./deploy/build-release.sh --target linux-arm64
```

构建结束会自动做兼容性自检，超基线直接失败：

- 静态：打印「不依赖设备 glibc」并写入 `BUILD-INFO.txt` 的 `link_mode=static`
- 动态：解析产物引用的最高 `GLIBC_x.y` 符号，与基线 `MAX_GLIBC`（默认 **2.31** ≈ Debian 11）比较，超出则报错并提示改用 musl target 或显式放宽 `MAX_GLIBC`
- 设备侧 `verify-deploy.sh` 的 `[1c] 二进制兼容性` 会核对已安装产物与设备 glibc，结论写入交付检查单

CI 中的 `static musl (*)` 作业会为三个架构实际构建静态包，并**启动产物探 `/health/live`**（ARM 走 QEMU user-mode），确保"静态能编出来"和"静态能跑起来"都被验证。

### 8.6 反向代理与限流

访问日志的 `client_ip` 与按 IP 分桶的限流规则，默认取 TCP 对端地址。网关前面挂反向代理时行为如下：

- **反代与网关同机**（对端为回环地址 `127.0.0.1`/`::1`）：自动采纳反代传来的 `X-Forwarded-For` 首跳作为真实客户端 IP，按 IP 限流与访问日志正常生效
- **反代在其他机器**：按 IP 限流退化为「全局限流」（所有客户端共享反代地址这一个桶），访问日志的 `client_ip` 全部为反代地址。此时如需按 IP 限流，应在反代层做，或让客户端直连网关
- **直连场景**：`X-Forwarded-For` 一律忽略，防客户端伪造该头绕过限流

nginx 同机反代示例：

```nginx
location / {
    proxy_pass http://127.0.0.1:8080;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_set_header Host $host;
}
```

---

### 8.7 Windows 部署的指标限制

工业现场主部署形态为 Linux。在 Windows 上运行时，「运行指标」页有三项**如实显示未知**，不是缺陷：

- **网卡收发量 / 接口列表**：需 `GetAdaptersAddresses` 的复杂链表结构，暂未接入，返回空列表并显示「暂无接口」（`gateway-core/src/engine/monitor/host.rs` 的 `net_interfaces()`）
- **1 分钟负载（loadavg）**：Windows 无此概念，返回 `None`
- **CPU 温度**：需厂商 SDK，未接入

排查现场性能问题请在 Linux 网关上看完整指标。

---
