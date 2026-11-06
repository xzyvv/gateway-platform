# CI Gate (T0.1)

防"注释吞代码 / 配置损坏 / clippy 警告堆积" 类 bug 进入主干。

## 5 阶段门禁

1. **`cargo fmt --check`** — 代码格式
2. **`cargo check --all-targets`** — 编译检查
3. **`cargo clippy --all-targets -- -D warnings`** — clippy 警告视为错误
4. **`cargo test --lib`** — 单元测试
5. **`cargo build`** — 调试构建

## 本地执行

### Linux / macOS

```bash
./scripts/ci.sh
```

### Windows

在 Git Bash / WSL 下执行同一脚本（PowerShell 版 `ci.ps1` 已随死代码清理删除）：

```bash
./scripts/ci.sh
```

任意一阶段失败立即退出，非 0 退出码。

## GitHub Actions

`.github/workflows/ci.yml` 在 `push` / `pull_request` 时自动运行（`ci` workflow 共 15 个 job）：

- `check / clippy / fmt / test`：fmt + check + clippy + test + build + 前端 lint/构建/单测 + ts-rs 校验 + API surface 校验 + **时区矩阵**（UTC / Asia/Shanghai / America/New_York 下重跑时间敏感单测）（**其它 job 全部依赖它**）
- `linux runtime checks`：仅编译 Linux 版本（硬件 watchdog / gpiod）
- `cross build (aarch64-unknown-linux-gnu)` / `cross build (armv7-unknown-linux-gnueabihf)`：ARM 交叉编译 **+ QEMU 运行时验证**（ARM 产物装载冒烟 + 在 ARM 上真跑架构敏感单测与采集→北向端到端接线测试）
- `encryption feature build` / `chaos scenarios` / `backend coverage (baseline)` / `performance benchmarks`
- `e2e smoke`：Playwright 端到端；`deploy smoke (install + verify)`：出厂包安装 + 部署校验；`deploy script lint`：交付脚本静态检查
- `soak smoke (memory leak gate)`：短时浸泡 + 判定，兜住内存 / 句柄泄漏回退（Linux 走 `scripts/soak.sh`）
- `sku size matrix`：release 构建 5 个 SKU 出体积表（纯观测，仅 `push` / 手动触发时跑，不阻塞 PR）
- 跨平台矩阵：`check on windows-latest`（Windows 侧有平台专属代码——license 指纹走注册表 MachineGuid）

`.github/workflows/security.yml` 另跑供应链门禁：`cargo audit (RustSec)` / `npm audit (frontend deps)` / `cargo deny (licenses / bans / sources)`。

## 强制要求

任何 PR 合入必须：

1. ✅ CI 全绿
2. ✅ 新增代码附带单元测试（happy + sad path）
3. ✅ 修改公共接口更新 `mod.rs` 顶部 `//!` 注释

## 为什么这是必需项

历史教训：本项目曾出现：
- `queue.rs` 注释吞掉 `if` 语句，导致 `enqueue` 函数结构错乱
- `config.rs` `pub modbus_mode: String,`n`n    pub tcp_addr:` 损坏换行

rustfmt + check 是这类损坏的最快检测手段。
