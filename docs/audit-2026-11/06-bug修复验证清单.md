# bug 修复验证清单（2026-11-22）

> **用途**：规定「让大模型修 bug」之后，**怎么确认它真的消除了 bug**，而不是「看起来改好了」。
> **适用范围**：本仓库所有 bug 修复（含 [04-ROI推荐清单.md](04-ROI推荐清单.md) 的各批次、评审提出的缺陷、日常 hotfix）。
> **配套脚本**：[scripts/verify_bugfix.ps1](../../scripts/verify_bugfix.ps1) —— 能机器化的部分（范围体检 / 证据链检查 / 全量门禁）已自动化；「复现输出」与「反证」两项必须人工给出。

---

## 0. 判据（先记住这一句）

> **证据链完整 = 复现输出 + 新增回归用例 + 全量门禁 + 一次反证。**
> 少任何一环，结论只能写「大概率修好了」，**不能写「已消除」**。

核心观念：**别问「你改了吗」，问「你拿什么证明你改对了」**。
能被推翻的结论才有价值 —— 所以必须包含「反证」这一步。

---

## 1. 五步闭环

### 步骤 1｜复现（Red）

- **要求对方交付**：**修复前**的失败输出 —— 报错栈 / 现场现象 / 接口返回的实际内容。
- **验收标准**：能稳定复现。拿不出来 = 对方可能是在改「它以为错的地方」，不是你报的那个 bug。
- **产物**：一个能稳定触发该 bug 的用例（测试 / 脚本 / 手工步骤）。
- **常见问题**：对方只在文字里说「这个 bug 是因为 X」，但没有可复现的失败输出 → 视为未复现，打回。

### 步骤 2｜最小修复

- **验收标准**：diff 范围**不超出 bug 边界**。
- **注水信号（看到就要打回）**：
  - 顺手重构、顺手升级依赖、顺手改格式；
  - 给失败分支加「兜底返回默认值 / 吞掉异常」；
  - 改了错误码、接口契约、文档口径却没说明。
- **判断法**：如果 diff 里出现了与 bug 无关的改动，一律要求拆开。
- **邻居债当场登记（强制）**：本批触碰的模块里，凡是被新暴露、但不在本批范围内的问题，**必须在同一动作里**登记进 [05-剩余项ROI推荐清单.md](05-剩余项ROI推荐清单.md#10-邻居债登记簿常驻规则) 的 §10 登记簿（编号 / 暴露位置 / 一句话 / 类型 / 工作量 / 价值）。不许只写在对话或提交说明里 —— 不登记就会在下一轮以「又发现新债」的形式回流，且分不清是老债还是新债。

### 步骤 3｜回归（Green）

- **验收标准**：
  1. 步骤 1 的用例**由红转绿**；
  2. **只有它变绿** —— 把新加的用例单独跑一遍，确认它真的覆盖了原路径（不是碰巧通过）；
  3. **其他用例没有被改软** —— 检查测试文件里被修改的断言（见脚本的「断言强度」检查）。
- **常见问题**：对方把失败断言改成 `assert!(true)` / `toBeTruthy()` → 直接判负。

### 步骤 4｜全量门禁

- **验收标准**：**整个仓库**的门禁全跑，不是只跑相关模块。
- **本仓库门禁命令**（= 本项目 CI 口径）：

  ```powershell
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  cargo fmt --all -- --check
  cd gateway-web; npm run lint; npm run type-check; npm run test
  cd ..; powershell -ExecutionPolicy Bypass -File scripts/deploy_lint.ps1
  python scripts/check_api_test_coverage.py
  python scripts/gen_api_surface.py --check
  ```

- **说明**：以上 8 条已由 `scripts/verify_bugfix.ps1` 串成一条命令。

### 步骤 5｜反证（唯一能证伪的一步）

- **做法**：**主动破坏这次修复** —— 把修复回退 / 改错一个字节，看新加的用例**会不会重新变红**。
- **验收标准**：用例**必须变红**。不会变红 = 这是个**假测试**，根本没测到修复点。
- **补充手法**：极端输入试边界值、并发压一压、故意制造超时 —— 看它是不是只绕开了你报的那条路径。

---

## 2. 三个高性价比的补充手段

1. **让模型自己扮演反方**
   同一轮再问一遍：「如果这个修复是错的，最可能错在哪？写一个能证明它错的测试。」——它自找的漏洞通常比你去猜要准。

2. **看它有没有留下根因解释**
   只改代码不给「哪一行、什么条件下、为什么会走到错误分支」的分析，多半是猜的。

3. **检查它是否偷改了契约**
   本项目尤其要盯：`docs/api-*.md`、`gateway-web/src/api/generated/`、错误码表、前后端字段 —— 契约被悄悄改宽，是最隐蔽的回归。

---

## 3. 证据归档模板（每修一个 bug，填一份）

| 项 | 内容 |
|---|---|
| bug 编号 / 来源 | 例：`DD-33`、或评审条目链接 |
| 现象 | 修复前现场看到什么（**贴原始输出**） |
| 根因 | 哪一行 / 什么条件 / 为什么错 |
| 复现用例 | 文件:行 + 用例名；**修复前红**的输出 |
| 修复文件 | `file:line` 列表（**只列与 bug 相关的**） |
| 回归用例 | 文件:行 + 用例名；**修复后绿**的输出 |
| 反证记录 | 我怎么破坏的 → 用例如何变红（贴输出） |
| 门禁结果 | 8 条命令逐条结果 |
| 结论 | 「已消除」/「大概率修好」/「未消除」+ 理由 |

> 归档位置：随该批次的台账（如 [04-ROI推荐清单.md](04-ROI推荐清单.md)）的「实施说明」表一起提交。

---

## 4. 自动化脚本

### 用法

```powershell
# 默认：基线 HEAD（只看工作区未提交改动），跑范围体检 + 证据链检查 + 全量门禁
powershell -ExecutionPolicy Bypass -File scripts/verify_bugfix.ps1

# 对比某个分支（如合并前 review）
powershell -ExecutionPolicy Bypass -File scripts/verify_bugfix.ps1 -Base origin/main

# 只跑体检与证据链（快速自检）
powershell -ExecutionPolicy Bypass -File scripts/verify_bugfix.ps1 -SkipRust -SkipWeb -SkipExtra

# 纯文档改动：允许没有测试文件
powershell -ExecutionPolicy Bypass -File scripts/verify_bugfix.ps1 -AllowNoTest

# 只采集门禁结果、不因门禁失败判负（证据链缺证据仍判负）
powershell -ExecutionPolicy Bypass -File scripts/verify_bugfix.ps1 -AllowGateFailure

# 输出汇总文件
powershell -ExecutionPolicy Bypass -File scripts/verify_bugfix.ps1 -SummaryPath target/verify-summary.md
```

### 参数与退出码

| 参数 | 说明 |
|---|---|
| `-Base <ref>` | 对比基线，默认 `HEAD`（只看未提交改动），可给 `origin/main` 等 |
| `-SkipRust` / `-SkipWeb` / `-SkipExtra` | 跳过对应门禁段（快速自检用） |
| `-AllowNoTest` | 允许本次改动没有测试文件（纯文档 / 纯配置） |
| `-AllowGateFailure` | 门禁失败降级为警告；**证据链缺证据仍然判负** |
| `-SummaryPath <path>` | 把汇总写成 Markdown 文件 |

退出码：`0` = 通过 / `1` = 有阻断项（证据链缺证据，或门禁失败且未加 `-AllowGateFailure`）。

### 脚本查什么（能机器化的那部分）

| 检查项 | 判定 |
|---|---|
| 吞错模式 | 新增行里的 `unwrap_or_default()` / `unwrap_or(0)` / `.ok()` / 空 catch / 裸 `except:` / `pass` → 警告（**把错误藏起来 = 把证据藏起来**） |
| 门禁口径变更 | 改动了 `check_coverage.py` / `vitest.config.ts` / `line_budget_frozen.txt` / `deploy_lint.ps1` / `Cargo.toml` / `docs/api-surface.md` / CI workflow → 警告 |
| 契约面变更 | 改动了 `gateway-web/src/api/generated/` / `docs/api-*.md` / API model → 警告（确认不是把契约改宽来让测试通过） |
| 回归网 | 本次改动**没有任何测试文件** → **判负**（除非 `-AllowNoTest`） |
| 断言强度 | 测试文件的断言数量比基线少 → 警告（可能被改软） |
| 全量门禁 | Rust 3 条 + 前端 3 条 + 附加 3 条，逐条记录退出码 |

### 脚本**不**覆盖的部分（必须人工做）

1. **复现输出** —— 脚本不会替你构造失败现场；
2. **反证** —— 脚本不会替你回退修复看用例是否变红；
3. **根因解释是否成立** —— 需要人读 diff 判断；
4. **契约变更是否合理** —— 脚本只报「改了」，不判断「改得对不对」。

---

## 5. 与 ROI 台账的衔接

[04-ROI推荐清单.md](04-ROI推荐清单.md) 里每一项 DD 都写清了「现在的毛病 / 现场会看到什么」—— **那正是复现用例的素材**。

例（DD-35「结果未知的写被显示成成功」）：

- 修复前：构造一次超时写入 → 控制历史显示「成功、写入值 0」；
- 修复后：同一构造 → 必须显示为「结果未知」；
- 这条能写成断言，就是最硬的证据 —— **它同时是步骤 1 的复现用例和步骤 5 的反证对象**。

---

## 6. 关联文件

- [scripts/verify_bugfix.ps1](../../scripts/verify_bugfix.ps1) —— 自动化脚本
- [scripts/README.md](../../scripts/README.md) —— 脚本目录分工
- [02-技术债务清单.md](02-技术债务清单.md) —— DD 条目与 file:line 证据
- [04-ROI推荐清单.md](04-ROI推荐清单.md) —— 批次台账（实施说明表填这份清单的归档）
