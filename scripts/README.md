# 脚本目录分工

本仓库存在两个 `scripts/` 目录，职责不同，请勿混用：

## 1. 仓库级：`/scripts/`（本目录）

服务于**整个仓库**的工具脚本，跨 `gateway-core` / `gateway-api` / `gatewayd` / `gateway-web` / `gateway-desktop` 多个子项目。

| 脚本 | 作用 |
|------|------|
| `dev-all.ps1` | 一键联调：同时启动 Rust 网关 (`cargo run -p gatewayd`) + Vite dev server (`npm run dev`)，Ctrl+C 同时终止（PowerShell 5+；`-Desktop` 一并启动 Tauri 壳） |
| `check_repo_health.ps1` / `check_repo_health.sh` | 仓库健康度扫描：bare CR 检测、rust-toolchain 与 ci.yml 版本一致性、ts-rs generated 完整性、CRLF/LF 一致性、构建产物泄漏检测 |
| `f2_supply_chain_check.ps1` / `f2_supply_chain_check.sh` | F2 供应链门禁本地等效：包装 `cargo deny` + `cargo audit`（与 CI `security.yml` 同口径），建议挂 pre-push；退出码 0 干净 / 1 工具安装失败 / 2 deny 失败 / 3 audit 失败 / 4 两者均失败 |
| `soak.ps1` / `soak.sh` | 浸泡测试驱动：串起 Rust `cargo test --test soak` 与进程级 RSS/句柄采样，产物写 `soak-out`（默认 24h，`.ps1` 用 `-DurationSecs`、`.sh` 用 `SOAK_DURATION_SECS` 缩为冒烟）。两侧环境变量同名同义；进程级采样在 Windows 取 `WorkingSet64`/`HandleCount`，在 Linux 取 `/proc/<pid>/statm` 与 `/proc/<pid>/fd`。CI 的 `soak smoke` 作业跑 `.sh` |
| `e2e_smoke.ps1` | 真实进程端到端冒烟：临时配置 + 独立端口拉起 `gatewayd.exe` → 探活 → 登录 → 状态/配置校验/回放会话/协议/诊断/健康 → 清理；退出码 0 通过 / 1 失败（`-Build` 先编译、`-Port` 换端口、`-KeepTemp` 保留日志） |
| `release_gate.ps1` | 发版前 Windows 一键门禁：按 CI 同口径串起仓库健康度 / api-surface / Rust fmt+check+clippy+test / gatewayd 构建 / 前端 lint+类型+构建+体积+单测覆盖率 / Playwright E2E / 真实进程冒烟，输出每步日志与 `summary.md`；默认 fail-fast，`-KeepGoing` 全跑汇总，`-Only` 重跑单项，退出码 0 通过 / 1 失败 |
| `gen_api_surface.py` | 从 `gateway-api/src/web/**/routes.rs` 解析路由，生成 `docs/api-surface.md`（端点/方法/鉴权/handler）；`--check` 只校验不一致并以退出码 1 失败（CI 门禁，防路由与文档漂移） |
| `check_coverage.py` | 后端覆盖率"防回落地板"：解析 `lcov.info` 并按目录分桶（core 各模块 / api/web / total），低于阈值即退出码 1（CI 门禁）；`--print` 只打印实测值。口径对标前端 `gateway-web/vitest.config.ts` 的分目录 thresholds——下取整留约 3pp 余量，补测试只许上调不许下调。后端 lcov 无分支数据（`BRF` 恒 0），故只对 lines / functions 设阈值 |

**何时用这里：** 需要同时操作多个子项目、或扫描整个仓库一致性时。

## 2. 核心专属：`/gateway-core/scripts/`

服务于**仅 gateway-core（Rust 后端）**的脚本，与前端/桌面端无关。

| 脚本 | 作用 |
|------|------|
| `ci.sh` | CI 5 阶段门禁：`cargo fmt --check` → `cargo check` → `cargo clippy -D warnings` → `cargo test --lib` → `cargo build`，任意失败即非 0 退出。Windows 请在 Git Bash / WSL 下执行，或直接用 `.github/workflows/ci.yml` 同款命令 |
| `perf_baseline.py` | 性能基线测试（采集延迟、吞吐量等指标） |

**何时用这里：** 只涉及 Rust 后端的 CI、性能、构建相关操作。

## 选择指南

```
脚本会同时碰 gateway-core 和 gateway-web 吗？
  ├─ 是 → 放 /scripts/
  └─ 否 → 只碰 gateway-core 吗？
              ├─ 是 → 放 /gateway-core/scripts/
              └─ 否 → 放到对应子项目自己的 scripts/（如未来新增）
```

## 跨平台约定

- `.ps1` 用于 Windows PowerShell 5+，`.sh` 用于 Linux/macOS/bash
- 同一脚本若同时提供 `.ps1` 与 `.sh`（`check_repo_health`、`f2_supply_chain_check`、`soak`），两侧逻辑保持等价；仅提供 `.ps1` 的（`dev-all`、`release_gate`）为 Windows 本地工具
- CI 统一调用 `gateway-core/scripts/ci.sh`（GitHub Actions 的 Linux runner），本地按需调用
