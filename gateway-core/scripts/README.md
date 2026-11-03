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

`.github/workflows/ci.yml` 在 `push` / `pull_request` 时自动运行：

- `check` job：fmt + check + clippy + test + build
- `linux-runtime` job：仅编译 Linux 版本（硬件 watchdog / gpiod）

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
