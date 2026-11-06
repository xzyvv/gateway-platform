# 贡献指南

感谢你愿意为这个工业网关项目出力。

这个仓库是**边缘数据采集网关**：向下对接现场仪表 / PLC / 空调等设备（Modbus RTU、DLT645、IEC104、OPC UA、BACnet、S7 等 12 个协议族），
向上把数据送到 MQTT / Webhook / OPC UA Server / Modbus Server。

所以贡献的第一原则是：**改动不能只在"功能跑通"这一层成立，必须在"现场能诊断"这一层也成立。**

---

## 目录

- [这个仓库最看重什么](#这个仓库最看重什么)
- [快速上手](#快速上手)
- [提交前必须跑的门禁](#提交前必须跑的门禁)
- [代码风格](#代码风格)
- [改动协议族](#改动协议族)
- [改动 API](#改动-api)
- [改动前端](#改动前端)
- [提交信息与 PR](#提交信息与-pr)
- [Issue 怎么写](#issue-怎么写)
- [安全漏洞](#安全漏洞)
- [许可](#许可)

---

## 这个仓库最看重什么

按优先级排：

1. **稳定性第一**。现场网关挂掉一条产线，代价远高于少一个功能。
   任何影响长时间运行（内存增长、句柄泄漏、线程饿死、重连风暴、状态机卡死）的改动都要在 PR 里说明影响面。
2. **失败必须如实报告**。不能把"连不上"包装成"成功"，不能把"没数据"写成"值为 0"。
   宁可让现场看到一个明确的错误，也不要给一个看起来正常的假象。
3. **能力缺失要说"不适用"**。协议本身没有这个概念时（比如 IEC104 没有寄存器扫描语义），
   界面应显式标注"不适用（不是尚未实现）"，而不是留一个 `✗` 让运维以为缺功能。
4. **不加没有工具化的协议**。只加采集、不加调试工具，等于把问题推给现场。

---

## 快速上手

### 环境要求

| 组件 | 版本 | 说明 |
|---|---|---|
| Rust | stable（建议 1.85+） | edition 2021 |
| Node.js | 20+ | CI 用 Node 22 |
| Python | 3.9+ | 仅用于 `scripts/gen_api_surface.py` |
| MSVC Build Tools | — | Windows 上编译 SQLite/sqlcipher 需要 |

> Linux 上 `encryption` feature 依赖 OpenSSL；Windows 上 `--all-features` 会带上 bundled-sqlcipher，
> 需要本机 OpenSSL 构建环境。CI 把这条路径单独放在 `encryption-check` job。

### 构建与测试

```bash
# 后端（workspace 根）
cargo build
cargo test --lib

# 后端完整五阶段门禁：fmt → check → clippy → test → build
bash gateway-core/scripts/ci.sh

# 前端
cd gateway-web
npm ci
npm run lint          # ESLint
npm run type-check    # vue-tsc
npm run test          # vitest
npm run e2e           # Playwright（需先 npm run e2e:install）
```

### 一次性跑完全部门禁（推荐）

Windows 上有一条和 CI 同口径的一键脚本：

```powershell
.\scripts\release_gate.ps1                 # 全量门禁（fail-fast）
.\scripts\release_gate.ps1 -KeepGoing      # 全部跑完再汇总
.\scripts\release_gate.ps1 -ListOnly       # 只看步骤清单
.\scripts\release_gate.ps1 -Only fmt,web-lint   # 只重跑失败项
```

日志默认写在 `target/release-gate/<时间戳>/`（`target/` 已在 `.gitignore` 里，不会污染仓库）。

仓库一致性检查（单独可跑）：

```powershell
pwsh scripts/check_repo_health.ps1     # 仓库健康度扫描
pwsh scripts/deploy_lint.ps1           # 部署脚本与 systemd 配置一致性
python scripts/gen_api_surface.py --check   # API 面快照是否与代码一致
```

---

## 提交前必须跑的门禁

CI 与本地门禁是同一套口径，`gateway-core/scripts/ci.sh` 是 Rust 五阶段的**唯一实现**，
GitHub Actions 直接调用它（不在 `ci.yml` 里内联同一串命令），避免两份门禁漂移。

| 门禁 | 命令 | 覆盖什么 |
|---|---|---|
| fmt | `cargo fmt --all -- --check` | 格式 |
| check | `cargo check --all-targets --all-features` | 编译 |
| clippy | `cargo clippy --all-targets --all-features -- -D warnings` | 警告即失败 |
| 单测 | `cargo test --lib --quiet` | 单元测试 |
| license-issuer | `cargo test -p license-issuer --quiet` | 签发侧单测（该 crate 只有 `[[bin]]`，`--lib` 覆盖不到） |
| 死代码 | `cargo check --all-targets --all-features` | CI 里用 `-D dead_code` 再跑一遍 |
| 前端 lint | `npm run lint` | ESLint |
| 前端类型 | `npm run type-check` | vue-tsc |
| 前端单测 | `npm run test:coverage` | vitest + 覆盖率阈值（**默认全量口径**，新增无测试文件同样计入） |
| 包体积 | `npm run check:bundle` | 前端产物体积预算 |
| API 面 | `python scripts/gen_api_surface.py --check` | 新增/删除端点需同步快照 |
| ts-rs 绑定 | `npm run types:check` | Rust 导出的 TS 类型需同步 |
| 仓库健康度 | `pwsh scripts/check_repo_health.ps1` | 仓库卫生 |

还有两个 CI 专属 job，本地一般不跑：

- **协议 feature 矩阵**：每个协议族单独 `cargo check --all-targets`，确保加一个 feature 不会破坏其它协议。
- **SKU feature 矩阵**：每个 SKU 组合单独编译，确保裁剪构建可用。

---

## 代码风格

### Rust

- `cargo fmt` 的输出就是标准，不做手工调整。
- clippy 警告一律当错误（`-D warnings`）。
- 注释写**为什么**，不写**是什么**。尤其这几类必须写清楚理由：
  - 协议帧解析里的偏移量与字节序假设
  - 超时、重试、退避的取值依据
  - 为什么某个能力位是 `false`（尤其"不适用"和"没实现"的区别）
- 涉及现场状态的错误信息，用运维能看懂的话（"TCP 已连接但从站无应答"），
  不要用内部术语（"recv timeout"）。

### 前端

- ESLint + Prettier，配置见 `gateway-web/eslint.config.js`。
- 组件用 `<script setup lang="ts">`。
- 业务逻辑放 `src/composables/`，页面组件只做编排。
- API 调用统一走 `src/api/`，不要在组件里直接 fetch。

### 通用

- 中文注释、中文文档、中文提交信息（与仓库现状一致）。
- 缩进、换行、文件末尾换行由编辑器 / 格式化工具保证，不要手工调。

---

## 改动协议族

新增或修改一个协议族，请严格按 [docs/adding-a-protocol.md](docs/adding-a-protocol.md) 的清单走。
这个文档里的 8 个 `- [ ]` **不是待办清单，是每次新增协议时的自检项**，逐条勾。

最关键的三条：

1. **Collector 实现** —— 帧解析、CRC、超时重试、退避。
2. **能力位声明** —— 在 `gateway-core/src/south/protocol/registry/capabilities.rs` 里如实声明能力。
   **声明 `true` 必须真的有实现**，否则 `registry/tests.rs` 的钉子会挂。
3. **调试工具** —— 协议没有寄存器扫描语义时（如 IEC104、DLT645），
   在前端显式标注"不适用（不是尚未实现）"，不要留 `✗`。

新增协议族后至少要补：

- 单元测试（正常帧 + 异常帧 + 超时）
- 「不可达端点如实报错」用例 —— 这是 `C1.5` 定下的验收口径
- 能力位钉子（`registry/tests.rs`）

---

## 改动 API

后端 API 变更必须同步 [docs/api-surface.md](docs/api-surface.md)（由脚本生成，CI 校验）：

```bash
python scripts/gen_api_surface.py            # 重新生成
python scripts/gen_api_surface.py --check    # CI 用的校验模式
```

改了 Rust 结构体并加了 `ts-rs` 导出时，前端类型也要同步：

```bash
cd gateway-web && npm run types:check
```

API 设计上的几条硬规矩：

- **参数校验失败返回 4xx + 明确错误信息，不要返回 500**。
- **协议不支持时返回明确的不支持错误**（如 `UnsupportedProtocol`），不要伪装成"没数据"。
- 鉴权接口不要在错误信息里回显 api_token / 密码。

---

## 改动前端

- 新增页面组件要同步 [docs/INDEX.md](docs/INDEX.md) 与功能清单文档（页签数会变）。
- 涉及按钮可用性的，**以能力位为准**：`useProtocolCaps` 是唯一判据，不要在组件里硬判协议名。
- 覆盖率门槛是**默认全量口径**（`vitest.config.ts` 的 `include: ['src/**/*.{ts,vue}']`），
  新增文件不写测试会直接拉低覆盖率并被门禁拦下。这是刻意的：
  **不要为了刷数字写无意义测试**，也不要因为"新文件没测试会被拦"就绕过门禁。
- 错误分支要有回归钉子，区分**协议错误**（后端明确返回的业务错误）与**传输错误**（网络/超时）。
  这两者在现场排查时的处置完全不同，不能混为一谈。

---

## 提交信息与 PR

### 提交信息

用中文，第一行不超过 72 字符，说明**做了什么**：

```
feat(protocol): 为 modbus 补 diag 能力（FC03 实读探针）
fix(collector): 修正 dlt645 跨帧边界的时间戳取值
fix(web): useProtocolCaps 重试成功后未复位 failed
docs: 同步路线图文档到当前代码事实
test(api): 补能力位路由契约测试
```

常用前缀：`feat` / `fix` / `docs` / `test` / `refactor` / `chore` / `perf`。

### Pull Request

PR 描述请覆盖这几项：

- **改了什么、为什么**（不是"怎么改"）
- **影响面**：是否改动了协议能力位 / API 端点 / 配置格式 / 产物体积
- **验证方式**：跑了哪些门禁、贴出结果
- **是否需要更新文档**：`docs/api-surface.md`、`docs/INDEX.md`、路线图状态标注

如果改动涉及**协议帧解析**或**状态机**，请在 PR 里说明失败模式：
出错时会发生什么、会不会留下脏状态、能不能自愈。

### 提交信息里的"开源许可"

提交 PR 即表示你同意你的贡献按 **Apache-2.0** 授权（见 [LICENSE](LICENSE)）。
不需要在每次提交里额外声明。

---

## Issue 怎么写

请用仓库的 Issue 模板（`.github/ISSUE_TEMPLATE/`），它会引导你填**复现所需的信息**。

对工业网关来说，下面这些信息缺一不可，否则基本无法定位：

| 必填 | 为什么 |
|---|---|
| 协议族 + 总线类型 | 12 个协议族的帧格式、校验、超时策略完全不同 |
| 网关版本 / commit | 不同版本的能力位和配置格式有差异 |
| 部署形态（裸机 / systemd / 容器） | 影响配置路径、环境变量、证书路径 |
| 现场现象 | 采集值不动？值跳变？连不上？重连风暴？ |
| 复现步骤 | 最小可复现路径 |
| 相关日志 / 诊断包 | 网关前端「系统总览」页可导出一键诊断包（zip：report.html / requests.har / packets.csv / meta.json / gateway.log / config.snapshot.json / health.json / north.json），**优先附这个** |

**不确定是不是 bug？** 提 issue 就行，不要因为"可能是我配置错了"就不提。
配置问题往往说明配置的表达力不够，那也是要修的。

**功能建议**请先到 Discussions 提，避免和 bug 混在一起。

---

## 安全漏洞

**不要用公开 issue 报告安全问题。**

请按 [SECURITY.md](SECURITY.md) 的说明走私密上报渠道（GitHub 私密漏洞上报 / 备用邮箱）。

特别提醒：授权与签名相关的安全问题（私钥泄露、授权绕过、指纹伪造）也在此列，请走同一通道。

---

## 许可

本项目代码以 **Apache-2.0** 授权，见 [LICENSE](LICENSE) 与 [NOTICE](NOTICE)。

关于"代码许可"与"官方发行版的产品授权（配额）"的区别，见 [README 的开源许可章节](README.md#开源许可)——
两者是**独立**的：用本仓库源码自行编译部署，不需要任何授权文件。
