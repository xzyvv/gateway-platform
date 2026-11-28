# 前端第五轮债务 ROI 推荐清单（gateway-web / 2026-11-28）

> **来源**：[`17-前端第五轮技术债务摸底.md`](17-前端第五轮技术债务摸底.md) 的发现（架构 7 / 结构 5 / 语义 14 / 文档 24 条【P1 10 + P2 14】，另有 P3 约 10 条归入低价值档），**当前 76 条可机器复核的断言全部通过**。
> **口径**：只修现有的东西，**不新增功能面**。工作量与价值记号沿用 [`05-剩余项ROI推荐清单.md`](05-剩余项ROI推荐清单.md) §0 的统一口径。
> **编号**：`DD-64` 起接前序（现有最大 `DD-63`）；前端文档债沿用 `DOC-F` 前缀（现有 `DOC-F1`/`DOC-F2`）。
> **排序**：ROI = 价值 ÷ 工作量。价值优先看「现场误判 / 误操作 / 数据正确性」与「挡着别的活」，其次可维护性；同价值下工作量小的排前。
> **边界**：本清单只排前端 `gateway-web/`；后端遗留项见 `05` 清单。

---

## 0. 一句话总览

> **2026-11-28 更新（13:30）：A 档 14 项全部清空**（批次 0 + 批次 1 + 批次 2 + 批次 3(DOC) + 批次 4(DD-77 / DD-82 / DD-80) + DD-66，共 **16 项**），门禁全绿（lint `--max-warnings 0` EXIT=0 / type-check EXIT=0 / 单测 264 文件 2513 用例全过 / build EXIT=0），**静态环 5 → 0**、**构建警告清零**，健康语义 5 处重复收敛为 1 份共享源，死导出清零，0 字节垃圾文件清零，源码 UTF-8 残字节清零。剩余 **16 项**：**A 档 0** / B 档 3 / C 档 12 / D 档 1。**（16:40 补充：DOC-F8~F10 已完成，剩余 13 项）**　**（20:55 补充：DD-81 / DD-84 / DD-67 / DD-74 / DD-78 / DD-79 六项已完成，B 档清零，剩余 7 项）**　**（21:10 补充：DD-73 / DD-76 / DD-87 已完成，剩余 4 项）**　**（21:25 补充：DD-86 已完成，剩余 3 项）**　**（21:35 补充：DD-75 已完成，剩余 2 项：C 1 / D 1，断言 101/101 全绿）**

- **32 项待办**（已完成 30 项，剩余 2 项）：**A 档 14 / B 档 5 / C 档 1 / D 档 1**，无一 P0。**A 档与 B 档已全部完成。**
- **最该先做（A 档头牌）**：
  - ~~**DD-64**（lint 红灯）~~ ✅ **已完成**——当前 CI lint 门禁已恢复绿；
  - ~~**DD-65**（5 组静态环）~~ ✅ **已完成**——静态环 **5 → 1**，只需动一条边；
  - ~~**DD-68**（健康报告页取色反向）~~ ✅ **已完成**——`error` 档现显红，两页取色逐项一致。
  - ~~**DD-66**（最后一组静态环）~~ ✅ **已完成**——用户拍板「上移为 store」，静态环 **1 → 0**。
  - ~~**DD-77**（清死导出）~~ ✅ **已完成**——真死导出 **5 个**已删（详见下方勘误），全仓零残留。
  - ~~**DD-82**（3 个 0 字节垃圾文件）~~ ✅ **已完成**——删前逐项复核，删后 `src` 一级目录实测 **12 个**。
  - ~~**DD-80**（源码 UTF-8 残字节）~~ ✅ **已完成**——`concurrency.ts:30` 还原为完整句子，其余字节未动。
  - ~~**DOC-F3~F7**（5 项纯文档）~~ ✅ **已完成**——10 条 P1 文档债一次清空。
- **下一个该做**：~~DOC-F8~F10~~、~~DD-81 / DD-84 / DD-67~~、~~DD-74 / DD-78 / DD-79~~、~~DD-73 / DD-76 / DD-87~~、~~DD-86~~、~~DD-75~~ ✅ **均已完成（2026-11-28）**；剩余 C 档 1 项（DD-85 随 api 重构）与 D 档 DD-83。
- **最高单笔价值**：**DD-68**。`HealthReport.vue:145` 自建取色映射，把后端登记的 `error` 档显示成琥珀色"警告"而非红色"严重"，且绕开了全站唯一取色入口。
- **投入最大**：**DD-83**（`utils/` 71 文件分层）——L 档，回报只是结构收敛、无现场表现，**建议随其他改动捎带，不单独开工**。
- ~~**需要一个决策拍板**：**DD-66**（`useSessions` 归属）~~ ✅ **已拍板并完成**——用户定「上移为 store」，静态环 **1 → 0**，两份互相矛盾的文档已统一；顺带清掉基线最后一条 `INEFFECTIVE_DYNAMIC_IMPORT` 构建警告。
- **本文档纠正了摸底报告的一处代价估计**：DD-65 的工作量从「M（1~3 天）」下调为 **S（半天以内）**，依据见 §3 批次 1 的模拟实验。

> **DD-77 勘误（2026-11-28 实施后）**：原表把 DD-77 写成「清 **7** 个死导出」，实际读码核实为 **5 删 2 留**——
> `diagHint` 是**生产活代码**（被 `diagHints.ts:78` 的 `withDiagHint` 调用），原标「待复核」的判断有误，现纠正为**非死导出**；
> `clearDeviceStats` 是 spec 夹具**且有独立行为用例**，删除属无收益改动，**保留并加注释**。
> 另外原建议「`healthStatusTag` 保留并改为被使用」已由 **DD-68** 以更优路径兑现（不新造调用方，而是让 `HealthReport.vue` 走已有真源），故 DD-77 **直接删除**而非改造。
> 详见 [`17-前端第五轮技术债务摸底.md`](17-前端第五轮技术债务摸底.md) §4 的 E-11 勘误表。

---

## 1. 工作量与价值口径

**工作量**（与 `05` 清单一致）

| 记号 | 人话 | 本清单典型内容 |
|---|---|---|
| **S** | 半天以内，改一两处 | 改 import / 改常量 / 改一个字典 / 改文档数字 |
| **M** | 1~3 天，跨 2~3 个模块 | 搬文件 + 改多处引用 / 抽公共函数 + 多方接入 |
| **L** | 4~7 天，要动结构 | 目录重组 / 大范围重命名 |
| **XL** | 超过一周 | —— 本清单无 |

**价值**

| 记号 | 判据 |
|---|---|
| **高** | 涉及现场误判 / 数据正确性 / **挡着别的活**（门禁红） |
| **中高** | 正确性防线，消除「改一处漏另一处」的漂移风险 |
| **中** | 好排查、好用、结构收敛，无直接现场表现 |
| **低** | 记录用 / 规范对齐 |

**ROI 档**

| 档 | 含义 |
|---|---|
| **A** | 划算，马上做 |
| **B** | 本季做 |
| **C** | 有空再做 |
| **D** | 先放着，等外部条件 |

---

## 2. 全量一览表（按建议顺序）

> 「→ 同源」= 改的是同一处代码或需同一次决策，**一起做，工作量不重复算**。

| 序 | 编号 | 一句话（人话） | 工作量 | 价值 | 档 | 备注 |
|---|---|---|---|---|---|---|
| 1 | **DD-64** | lint 红灯：`sse.ts` 直接用了 `console` | **S** | **高** | **A** | ✅ **已完成（2026-11-28）** |
| 2 | **DD-65** | 5 组静态循环依赖 → 1 组 | **S** | **中高** | **A** | ✅ **已完成（2026-11-28）**，静态环 5 → 1 |
| 3 | **DD-68** | 健康报告页取色绕开真源，`error` 档显示反了 | **S** | **高** | **A** | ✅ **已完成（2026-11-28）**，`error` 现显红 |
| 4 | **DOC-F3** | 文档三处说"不登记 redirect"，实际有 11 条 + 2 个 `*Redirect` | **S** | **中高** | **A** | ✅ **已完成（2026-11-28）**，口径统一为「旧地址保留兼容」 |
| 5 | **DOC-F4** | `frontend-state.md` 写着 2 个**不存在**的 store | **S** | **中高** | **A** | ✅ **已完成（2026-11-28）**，清单换成真实 8 个 store |
| 6 | **DOC-F6** | `test/README.md` 覆盖率阈值 **8 个数字全错** | **S** | **中高** | **A** | ✅ **已完成（2026-11-28）**，改为实测 63/56/53/60 |
| 7 | **DOC-F5** | `frontend.md` 指向**不存在的** `utils/tauri.ts`、store 名单错、漏 `acquisition/` | **S** | **中高** | **A** | ✅ **已完成（2026-11-28）**，三处事实错误已改 |
| 8 | **DOC-F7** | `composables/README.md` 大面积过期（数量、目录、8 处文件名） | **S** | 中 | **A** | ✅ **已完成（2026-11-28）** |
| 9 | **DD-77** | 清死导出（只有 spec 消费 / 零引用） | **S** | 中 | **A** | ✅ **已完成（2026-11-28）**，**5 删 2 留**（`diagHint`/`clearDeviceStats` 保留），原表"7 个"已勘误 |
| 10 | **DD-70** → 同源 | 健康状态中文名两份字面量副本 | **S** | 中 | **A** | ✅ **已完成**，同源到 `overallStatusMeta` |
| 11 | **DD-71** → 同源 | 磁盘水位档位字典两份逐字相同 | **S** | 中 | **A** | ✅ **已完成**，同源到 `diskLevelMeta` |
| 12 | **DD-72** → 同源 | 健康问题取色第二、第三份实现 | **S** | 中 | **A** | ✅ **已完成**，同源到 `problemLevelMeta` |
| 13 | **DD-82** | 3 个 0 字节垃圾文件已进 git 基线 | **S** | 低 | **A** | ✅ **已完成（2026-11-28）**，删前逐项复核，`src` 一级目录现 12 个 |
| 14 | **DD-80** | `concurrency.ts` 注释里的 UTF-8 坏字节 | **S** | 低 | **A** | ✅ **已完成（2026-11-28）**，文件 4381→4375 字节，其余字节未动 |
| 15 | **DD-66** | 最后一组静态环 + `useSessions` 归属需拍板 | **M** | 中 | **B** | ✅ **已完成（2026-11-28 13:15）**，用户拍板「上移为 store」，静态环 **1 → 0** |
| 16 | **DD-69** → 同源 | 问题分级计数两份，store 那份**无人消费** | **M** | 中 | **B** | ✅ **已完成**，store 那份已被结论条真实消费 |
| 17 | **DD-81** | `parts/` 与 `components/` 同义词并存（6 处目录） | **M** | 中 | **B** | ✅ 已完成（2026-11-28），6 目录 git mv，29 处引用同步 |
| 18 | **DD-84** | `utils/deviceComms.ts` 绕过 ESLint #17 规则盲区 | **M** | 中 | **B** | ✅ 已完成（2026-11-28），规则扩到 utils + 显式豁免 |
| 19 | **DD-67** | 唯一的真层级倒挂：composable 引用 view 组件 | **M** | 中 | **B** | ✅ 已完成（2026-11-28），对话框改由 DeviceList 挂载 |
| 20 | **DOC-F8** | `frontend.md` 的 axios / Element Plus 归属错 + E2E 覆盖项过期 | **S** | 中 | **C** | ✅ 已完成（2026-11-28） |
| 21 | **DOC-F9** | `frontend-naming.md` §7 页面表过期（3 处） | **S** | 中 | **C** | ✅ 已完成（2026-11-28） |
| 22 | **DOC-F10** | `README.md`「13 个一级目录」计数错 + 优化方向滞后现实 | **S** | 中 | **C** | ✅ 已完成（2026-11-28） |
| 23 | **DD-73** | `api/diagnostics` barrel 双 `export *`，两套 API 只差 `View` 后缀 | **M** | 中 | **C** | ✅ 已完成（2026-11-28），删除 barrel，显式 raw/view |
| 24 | **DD-74** | 两个同名不同义的 `ConfigIssue` | **M** | 中 | **C** | ✅ 已完成（2026-11-28），改为 ConfigValidationIssue / ConfigHealthIssue |
| 25 | **DD-75** | 「会话」一词四义 + `critical` 两义 | **M** | 中 | **C** | ✅ 已完成（2026-11-28，产品拍板：三词限定 + critical=严重） |
| 26 | **DD-86** | `utils/` 反向依赖上层 + `api` ⟷ `utils` 双向 | **M** | 中 | **C** | ✅ 已完成（2026-11-28） |
| 27 | **DD-87** | `components/` 直接依赖 `stores/`（10 处） | **M** | 中 | **C** | ✅ 已完成（2026-11-28），容器组件迁入 views，components 零 store 导入 |
| 28 | **DD-76** | `use*` 命名但没有 `use*` 导出（2 个文件） | **S** | 低 | **C** | ✅ 已完成（2026-11-28），改名 pointFields / protocolMeta |
| 29 | **DD-78** | 导出的 `statusTagType` 与全站唯一取色入口同名 | **S** | 低 | **C** | ✅ 已完成（2026-11-28），改名 replayStatusTagType |
| 30 | **DD-79** | 命名违反"能力词不表达技术栈"（`*Page` / `*View`） | **S** | 低 | **C** | ✅ 已完成（2026-11-28），UplinkConfig / CurrentCertificate |
| 31 | **DD-85** | `api/` 分组维度不统一（37 平铺 + 4 子目录） | **M** | 低 | **C** | 动 `api/index.ts` 需回归 |
| 32 | **DD-83** | `utils/` 顶层平铺 71 个文件，主题簇无边界 | **L** | 中 | **D** | 随其他改动捎带 |

---

## 3. 批次与执行顺序

> 每完成一项，回到 [`17-前端第五轮技术债务摸底.md`](17-前端第五轮技术债务摸底.md) 对应条目把状态改为「已修」。

### 批次 0 · 解门禁红灯（半天，先做） ✅ 已完成（2026-11-28）

**DD-64** —— `src/api/protocolTools/sse.ts:92` 用了 `console.warn`，撞上 `eslint.config.js:92` 的 `'no-console': 'error'`。

- **现在什么毛病**：`56fe70c`（2026-11-26）新增了半截 JSON 的容错留痕，用了 `console.warn`。项目规定业务代码日志**唯一出口是 `@/utils/logger`**（`eslint.config.js:91` 注释明写），`eslint.config.js:98-103` 的豁免只给 `logger.ts` 自己，`:122-128` 只给 `e2e/`、`scripts/`、`*.spec.ts`。
- **现场会看到什么**：不直接影响现场，但**CI lint 门禁当前是红的**，任何人提 PR 都过不了；且生产构建下 `console` 不受分级控制，与项目"生产静默"的设计相悖。
- **工作量**：**S**。1 行改动：
  ```ts
  import { logWarn } from '@/utils/logger'
  logWarn('protocolTools', '丢弃无法解析的 SSE 事件', event, dataLines.join('\n'))
  ```
  同域正确范式可抄：`api/http/responseInterceptor.ts:57`、`api/inflight.ts:56`。
- **价值**：**高**（挡着别的活）。
- **验收**：✅ `npm run lint` EXIT=0；`api/protocolTools/sse.spec.ts` 仍全过。

### 批次 1 · 断静态环（半天，性价比最高） ✅ 已完成（2026-11-28）

**DD-65** —— 全仓 5 组静态循环依赖。

- **现在什么毛病**：根因只有一处 —— `api/http/responseInterceptor.ts:14` **静态**引入了 `stores/auth.ts`。`stores/auth.ts:3` 又 `from '@/api'` 回指 api barrel，于是闭合成环。**同一个文件 `:186` 已经在用 `void import('../../router')` 断环，唯独这一行没改**。
- **现场会看到什么**：本身不直接产生现场故障，但它是后续所有 `api/http/` 重构的**地雷**——改传输层时容易踩到 `undefined` 初始化顺序问题，且新人排查时会被 5 组环带偏。
- **工作量**：**S**（已模拟验证，见下方实验）。
- **价值**：**中高**。消除"改传输层就可能炸"的漂移风险。
- **验收**：✅ `node scripts/audit-static-cycles.mjs` 输出从 5 组降到 **1 组**（剩的那组属 DD-66）；`npm run lint` / `type-check` / `npm test` 全绿。

> **落地记录（实际修法与最初设想的差异）**
>
> 最终**没有**用动态 import，而是新增 `src/api/http/authBridge.ts` 桥接模块：`app/main.ts` 在 pinia 安装后注册一个"登出补丁函数"，传输层只管调用。
>
> **原因**：`stores/auth.ts` 已被 `router/index.ts`、`views/layout/HubShell.vue` 等多处静态引入，与 `api/http/` 同在入口 chunk——动态导入会产生 `INEFFECTIVE_DYNAMIC_IMPORT` 构建警告（零拆包收益），正是项目当初为避免该警告而拆出 `api/inflight.ts` 的同一顾虑。桥接方案保持**同步**语义（登出时 `loggedIn` 立刻变 false），不引入微任务延迟。
>
> **改动清单**（5 个文件）：
> | 文件 | 改动 |
> |---|---|
> | `src/api/http/authBridge.ts`（新增） | `setAuthLogoutPatcher` / `applyAuthLogoutPatch` |
> | `src/api/http/responseInterceptor.ts` | 删静态 import `:14`；`:175` 的 `try { useAuthStore().$patch(...) } catch {}` → `applyAuthLogoutPatch()` |
> | `src/api/http/index.ts` | barrel 导出桥接函数 |
> | `src/app/main.ts` | `app.use(pinia)` 之后注册补丁函数 |
> | `src/api/http/authBridge.spec.ts`（新增） | 3 条回归用例（未注册不抛 / 补丁内容正确 / patcher 抛异常被吞） |
>
> **实测前后**（`scripts/audit-static-cycles.mjs`）：**5 组 → 1 组**。
>
> **修法选型的模拟实验**（`scripts/audit-cycle-fix-sim.mjs`，只读不改码）：
>
> | 方案 | 静态环组数 |
> |---|---|
> | 基线 | **5** |
> | 仅收窄 `stores/auth.ts` 的 3 条 import 路径 + `api/auth.ts` 1 条 | 3 |
> | **仅切断 `responseInterceptor.ts → stores/auth.ts` 这一条边** | **1** ✅ |
>
> 结论：**根因只有那一条边**，不必动任何其他文件的 import 路径。`stores/notifications` / `stores/requestLog` 两条边**不必动**——它们不回指 api，不是环的成因。
>
> **验证**：lint ✅ EXIT=0 · type-check ✅ EXIT=0 · 单测 ✅ **263 文件 / 2502 用例全过** · `verify-round5-claims.mjs` ✅ **61/61 PASS**（新增 7 条"已修状态"断言）。
>
> **遗留（既有问题，非本轮引入）**：构建仍有 1 条 `INEFFECTIVE_DYNAMIC_IMPORT` 警告，指向 `src/router/index.ts` 被 `responseInterceptor.ts` / `useSessions.ts` 动态引入、同时又被 `app/main.ts` 静态引入。两处动态 import 是**刻意保留**的断环手段（基线 `c6bb1e3` 即如此），改为静态会重新引入环——属另一笔独立小债。
>
> 👉 **已由 DD-66 顺带消除**（2026-11-28 13:15）：会话模块不再随 `stores/auth.ts` 进入入口 chunk，构建警告 **1 条 → 0 条**。

**DD-66** —— 剩下的最后一组静态环 + 归属口径需拍板。**M 档，但建议在批次 2 之后**。 ✅ **已完成（2026-11-28 13:15）**

- **现在什么毛病**：`stores/auth.ts:5` 静态引 `composables/system/useSessions`，后者 `:9` 又静态引 `stores/auth.ts`。更本质的问题不是环，而是**归属反了**：
  - `composables/README.md:114-126`（自称"Store vs Composable 边界（唯一真源）"）判定：跨页面共享、需长期驻留 → 进 store。登录会话正是这种（消费方含 `stores/auth.ts:119`、`composables/system/useSystem.ts:70`、`views/system/overview/SystemOverview.vue:269`）；
  - 但 `frontend-state.md:62` 把 `useSessions` 登记在 `@singleton` 表（即认定为 composable）。
  - **两份文档互相矛盾**，且现状是"store 反向依赖 composable"，与 README 的判定表相反。
- **现场会看到什么**：不直接产生现场故障。
- **工作量**：**M**（新建 `stores/sessions.ts` + 改 3 个消费方 + 同步 `frontend-state.md` 登记表 + 迁 spec）。
- **价值**：**中**（消除最后一组环 + 消除两份文档的矛盾）。
- **依赖**：⚠️ **需先拍板**：`useSessions` 到底算 store 还是 composable。建议**上移为 store**（符合 README 判定表，且 `frontend-state.md` 的 `@singleton` 表本就是为"模块级单例"准备的登记表，改起来是删一行而不是改语义）。
- **验收**：`audit-static-cycles.mjs` 输出 0 组；`frontend-state.md` 与 `composables/README.md` 对 `useSessions` 的归属表述一致。

> **落地记录（2026-11-28 13:15，用户拍板「上移为 store」）**：
> - **归属上移**：`composables/system/useSessions.ts` → `src/stores/sessions.ts`（Pinia store `useSessionsStore`），`LoginSession` 类型一并迁移。
> - **断 `auth → sessions`**：`stores/auth.ts` 不再引会话模块；`login()` 里的 `useSessions().markLogin()` 移出，改由调用方 `views/auth/Login.vue:292` 在登录成功后显式调 `useSessionsStore().markLogin()`。
> - **留 `sessions → auth`**：强制下线当前会话要真登出（Bug 180），该依赖单向、无环。
> - **调用方同步**：`composables/system/useSystem.ts:14` 改引 `@/stores/sessions`；`views/system/overview/SystemOverview.vue:269` 类型引用改 `@/stores/sessions`。
> - **测试迁移**：`useSessions.spec.ts` → `stores/sessions.spec.ts`（改用 `createPinia()`，用例与断言逐条保留）；`stores/auth.spec.ts` 删掉对会话模块的 mock。
> - **文档同步**：`frontend-state.md` store 清单 8 → 9（新增 `sessions`）且 `@singleton` 表删行；`composables/README.md` 删 composable 行 + spec 数 43 → 42 + 边界表补「登录会话」；`frontend.md` store 目录树 8 → 9。
> - **实测**：`audit-static-cycles.mjs` **1 组 → 0 组**；`npm run build` 的 **`INEFFECTIVE_DYNAMIC_IMPORT` 警告 1 条 → 0 条**（基线最后一条构建警告顺带清掉）；lint EXIT=0 · type-check EXIT=0 · 单测 264 文件 / 2513 用例全过 · `verify-round5-claims.mjs` **85/85 PASS**（A-2 四条翻转为「已修状态」+ 新增 DD-66.1~DD-66.9）。

### 批次 2 · 健康语义收敛（1~2 天，五处重复一起改） ✅ 已完成（2026-11-28）

**DD-68 / DD-70 / DD-71 / DD-72 / DD-69**（→ 同源，**一起做**）

- **现在什么毛病**（五处重复，详见 §4 的表）：

  | 编号 | 重复点 | 关键位置 |
  |---|---|---|
  | DD-68 | 组件状态取色自建映射，绕开全站唯一真源 `utils/statusType.ts:81` | `views/ops/health/HealthReport.vue:145-146` |
  | DD-70 | 健康状态中文名两份字面量副本 | `stores/healthReport.ts:24-28` ↔ `composables/dashboard/useHealthSignals.ts:97-101` |
  | DD-71 | 磁盘水位档位字典两份逐字相同 | `HealthReport.vue:149-154` ↔ `DiskHealthChart.vue:130-135` |
  | DD-72 | 健康问题分级取色第二、第三份实现 | `HealthProblemList.vue:45-49`、`console/parts/debugWizardProbes.ts:156-159` |
  | DD-69 | 问题分级计数两份，store 那份**无人消费** | `stores/healthReport.ts:95-101` ↔ `HealthConclusionBar.vue:46-52` |

- **现场会看到什么**（**只有 DD-68 是真缺陷**）：`healthStatus.ts:49` 明确登记后端组件状态词 `error` → severity `error`（应显红），但 `HealthReport.vue:145` 的三元映射把它落到 `warning`（琥珀）。**同一台网关，运行总览页显示红色"异常"、健康报告页显示琥珀色"警告"** —— 现场按颜色判断严重度时会判反。其余四处无直接现场表现，是"改一处漏一处"的漂移风险。
- **工作量**：**S**（DD-68/70/71/72 四项各 S）+ **M**（DD-69，需在"组件 props 驱动"与"改读 store"之间做一次设计选择）。
- **价值**：DD-68 = **高**；其余四项 = 中（合计**中高**）。
- **依赖**：无外部依赖。DD-69 需先定设计（见下）。
- **验收口径**（建议写成守护用例）：
  1. `HealthReport.vue` 的组件状态**中文名 + 徽章色**与 `views/overview/HealthStatus.vue` **逐项一致**；
  2. `error` 档位在健康报告页显红（danger），不是琥珀；
  3. `stores/healthReport.ts` 的 `STATUS_META` 与 `useHealthSignals.ts` 的 `STATUS_LABEL` **由同一来源派生**（改一处另一处必变）；
  4. `problemCounts` 要么被真实消费、要么从公开 API 移除——不留"有但不接"的僵尸出口。

> **落地记录（实际修法）**
>
> 五处重复全部收敛到 **`src/utils/healthStatus.ts`**（该文件此前已是组件健康词表的所在地，本次扩为健康语义总源），新增 4 组共享语义：
>
> | 新增 | 替代了 | 消费方 |
> |---|---|---|
> | `overallStatusMeta(status)` | store 的 `STATUS_META` + `useHealthSignals` 的 `STATUS_LABEL` | 健康报告 store · 总览异常信号 |
> | `problemLevelMeta(level)` | `HealthProblemList.levelMeta` + `debugWizardProbes.levelTagType` | 健康报告问题清单 · 协议调试台探针 |
> | `diskLevelMeta(level)` | `HealthReport.DISK_LEVEL_LABEL` + `DiskHealthChart.LEVEL_LABEL` | 健康报告页 · 磁盘健康图表页 |
> | `countProblemLevels(problems)` | store 的 `problemCounts` reduce + `HealthConclusionBar` 的本地 reduce | 健康报告 store · 健康结论条 |
>
> **DD-69 的设计选择（已落地）**：采纳「**props 驱动 + 可选 counts 覆盖**」——`HealthConclusionBar` 新增可选 `counts?: ProblemCounts` prop，不传则用共享纯函数自算；`GatewayDiag.vue` 与 `SystemOverview.vue` 把 store 里算好的 `problemCounts` 传进去。两条路径结果必然一致（同一纯函数），且 **store 的 `problemCounts` 从此被生产代码真实消费**，不再是僵尸出口。
>
> **DD-68 的界面变化**：健康报告页的组件状态胶囊由「`mqtt：ok`」改为「**`MQTT 客户端：正常`**」——中文组件名 + 中文状态名，英文原词不再漏到界面（与运行总览页 `HealthStatus.vue` 逐项一致）。
>
> **改动清单**（10 个文件）：
>
> | 文件 | 改动 |
> |---|---|
> | `src/utils/healthStatus.ts` | 新增 4 组共享语义（`overallStatusMeta` / `problemLevelMeta` / `diskLevelMeta` / `countProblemLevels`） |
> | `src/stores/healthReport.ts` | 删 `STATUS_META`；`overallStatus` 与 `problemCounts` 改走共享源；re-export `ProblemCounts` 类型（保持既有 import 路径） |
> | `src/composables/dashboard/useHealthSignals.ts` | 删 `STATUS_LABEL`，改 `overallStatusMeta(report.status).label` |
> | `src/views/ops/health/HealthReport.vue` | 删 `componentType` 与 `DISK_LEVEL_LABEL`/`diskTagType`；组件状态改走 `healthStatusInfo`+`healthComponentLabel` |
> | `src/views/ops/health/DiskHealthChart.vue` | 删 `LEVEL_LABEL`，改 `diskLevelMeta` |
> | `src/views/ops/health/HealthProblemList.vue` | `levelMeta` 委托 `problemLevelMeta` |
> | `src/views/acquisition/console/parts/debugWizardProbes.ts` | `levelTagType` 委托 `problemLevelMeta` |
> | `src/views/ops/health/HealthConclusionBar.vue` | 新增可选 `counts` prop；本地 reduce 改用 `countProblemLevels`（DD-87 后归入 health 目录） |
> | `src/composables/system/useSystem.ts` | 透出 `healthProblemCounts` |
> | `src/views/system/overview/SystemOverview.vue` · `src/views/ops/health/GatewayDiag.vue` | 给结论条传 `:counts` |
>
> **新增回归用例**：`src/views/ops/health/healthSemantics.spec.ts`（13 条），钉住上面 4 条验收口径 + 词表本身不被改动。
>
> **既有用例同步**：`HealthReport.spec.ts` 原有一条断言旧行为（`text` 含 `mqtt：ok`）已改为断言新行为（中文名 + 中文状态，英文原词不出现）。
>
> **验证**：lint ✅ EXIT=0 · type-check ✅ EXIT=0 · 单测 ✅ **264 文件 / 2515 用例全过**（+1 文件 / +13 用例） · `verify-round5-claims.mjs` ✅ **68/68 PASS**（新增 14 条"已修状态"断言）。
>
> *（后续 DD-77 删掉 2 个随之失效的 `it` 块，当前为 **2513 用例**、断言 **76 条**，见批次 4。）*
>
> **行为变化提示（唯一一处用户可见）**：`error` 组件状态在健康报告页由琥珀变红（修复）；`running` 组件状态由琥珀变绿（修复）。其余档位取色与改前一致。

### 批次 3 · 纯文档修正（1 天，零代码风险）—— **DOC-F3~F10 全部已完成（2026-11-28）**

> **这批是"防误判"投资**：照着当前文档改代码会踩空。**8 项全是纯文档，合计约 1 天。**
>
> ✅ **DOC-F3 / F4 / F5 / F6 / F7 五项已于 2026-11-28 落地**（P1 文档债 10 条一次清空）；**DOC-F8 / F9 / F10 也已于 2026-11-28 落地**（断言 92 → 99 条，99/99 全绿）。

| 编号 | 改什么 | 位置 | 状态 |
|---|---|---|---|
| **DOC-F3** | 三处文档 + 一处源码注释统一口径：**旧地址保留兼容**（11 条 redirect 是有意保留旧书签），删掉"不再为 tab 登记 redirect 别名""旧地址不做兼容""不使用 `*Redirect`"三句 | `frontend.md:94`、`frontend-naming.md:83,84,87`、`routes/index.ts:7` | ✅ 已完成 |
| **DOC-F4** | Store 清单换真（删 `config`/`devices`，补 `healthReport` 等 5 个）；`useRequestLog` 改为 Pinia store | `frontend-state.md:24-32,63` | ✅ 已完成 |
| **DOC-F5** | `utils/tauri.ts`→`api/env.ts`；store 名单改真；composables 补 `acquisition/` 域 | `frontend.md:108,52-53,46-47` | ✅ 已完成 |
| **DOC-F6** | 覆盖率阈值按 `vitest.config.ts:110-122` 实测改写（63/56/53/60；views 58 / utils 76 / composables 61）；文件名错 5 处；指错的文件 | `src/test/README.md:98-99,19,21,106` | ✅ 已完成 |
| **DOC-F7** | spec 数 12→**43**；子目录 9→**10**；"应用页面"列 8 处不存在的文件名改真；`useSessions` 职责描述改真 | `src/composables/README.md:4,6,19,37-39,46,54-55,64,74` | ✅ 已完成 |
| **DOC-F8** | axios 拦截器归属→`api/http/`；Element Plus 改"按需引入"而非"全局注册"；E2E 覆盖项去掉已移 vitest 的两条 | `frontend.md:16,20,102` | ✅ 已完成（2026-11-28） |
| **DOC-F9** | §7 页面表删 `PointTemplates`/`DataEncryption`；`CommTuningHub` 改"采集调优"；`.hub-body-parent`→实际类名 | `frontend-naming.md:107,115,119,120,123,125` | ✅ 已完成（2026-11-28） |
| **DOC-F10** | "13 个一级目录"→**12**；"优化方向"改为"已完成项 + 剩余项" | `gateway-web/README.md:43,64-75` | ✅ 已完成（2026-11-28） |

- **工作量**：合计 **S × 8 ≈ 1 天**（DOC-F3 因涉及 4 处联动口径，稍多半天）。
- **价值**：**中高**（DOC-F3/F4/F5/F6）/ 中（其余）。
- **验收**：每条改完后用 `scripts/verify-round5-claims.mjs` 复跑——**该脚本会把这批文档断言直接判 FAIL**，是现成的回归钉子。

> **DOC-F3~F7 落地记录（2026-11-28）**：
> - **DOC-F3**：统一口径为「**旧地址保留兼容**」，并**新增允许 redirect 别名路由用 `*Redirect` 命名**的说明（原文档只说"不使用"，与 `QuickSetupRedirect`/`BatchMonitorRedirect` 两条现存路由直接冲突）。
> - **DOC-F4**：`frontend-state.md` store 清单换成真实 **8 个**（auth / gateway / healthReport / notifications / requestLog / theme / tier / websocket），删 `useRequestLog` 的 composable 登记，补 `useStepSequences`，并修 2 处过期举例。
> - **DOC-F5**：`utils/tauri.ts` → `api/env.ts::isTauri()`；store 名单去掉不存在的 `config`；composables 目录补 `acquisition/`（10 个子目录）。
> - **DOC-F6**：覆盖率阈值改为实测值；修 5 处文件名；删掉指错文件的注；co-locate 示例改 `DeviceList.vue`。
> - **DOC-F7**：spec 数 12→**43**、子目录 9→**10**、新增 `acquisition/` 域 7 个 composable 条目、修 8 处不存在的文件名、修 `useSessions` 职责描述（→ `SystemOverview.vue`）、修相对路径示例。
> - **数值反漂移**：数量易变处沿用「不写死静态数字 + 指向实测命令」的约定（spec 覆盖列表改为「43 个 /约 35 个」）。
> - **验证**：lint ✅ EXIT=0（`--max-warnings 0`）· type-check ✅ EXIT=0 · 单测 ✅ **264 文件 / 2513 用例全过** · `verify-round5-claims.mjs` ✅ **76/76 PASS**（13 条文档断言由"断言旧缺陷"翻转为"断言已修状态"）。

### 批次 4 · 结构收敛（2~3 天）—— **DD-77 / DD-82 / DD-80 / DD-81 / DD-84 全部已完成（2026-11-28）**

| 编号 | 做什么 | 工作量 | 备注 |
|---|---|---|---|
| **DD-77** | 清死导出。**实际 5 删 2 留**：`hasBlockingIssues`、`healthStatusTag`、`healthRowsFrom`、`invalidateHealthReport`、`getDeviceStat` 已删；`clearDeviceStats`、`diagHint` 保留 | S | ✅ **已完成（2026-11-28）**。⚠️ 原写「7 个」有误：`diagHint` 是**生产活代码**（被 `diagHints.ts:78` 的 `withDiagHint` 调用），`clearDeviceStats` 有独立行为用例；`healthStatusTag` 的"改为被使用"已由 **DD-68** 兑现（走已有真源），故直接删 |
| **DD-82** | `git rm gateway-web/src/c gateway-web/src/r gateway-web/src/v`（3 个 0 字节，已在基线提交 `d748f7b` 里） | S | ✅ **已完成（2026-11-28）**。删前逐项复核：存在 / 长度 0 / 非目录 / 由 `d748f7b` 引入 / 全仓零引用；删后 `src` 一级目录实测 **12 个** |
| **DD-80** | 修 `utils/concurrency.ts:30` 的 UTF-8 坏字节（"已␦在飞的任务不被打断"） | S | ✅ **已完成（2026-11-28）**。坏处为 `已` 后 2 个 U+FFFD（6 字节）；删除后整句还原为「已在飞的任务不被打断」，文件 4381 → 4375 字节、换行与 BOM 不变，**其余字节未动**；全仓 `src/` 再扫 U+FFFD 零命中 |
| **DD-81** | `parts/` → `components/` 全量统一（6 处目录、**29 处 import**） | M | ✅ **已完成（2026-11-28）**：6 目录 `git mv`，19 个文件 29 处引用同步，`parts/` 零残留；`dialogs/` 保留 |
| **DD-84** | `utils/deviceComms.ts` 二选一：迁 `stores/`，或把 ESLint #17 的 `files` 从 `src/composables/**/*.ts` 扩到 `src/utils/**/*.ts` 并加显式豁免注释 | M | ✅ **已完成（2026-11-28）**：采用扩规则方案，`files` 增加 `src/utils/**/*.ts`，`STATS` 加显式豁免注释；lint 保持零告警 |

### 批次 5 · 需拍板 / 可延后

| 编号 | 做什么 | 需要谁拍板 |
|---|---|---|
| **DD-67** | 唯一的真层级倒挂：`composables/devices/useDevices/index.ts:3` 引用 `views/.../DeviceDiscoverDialog.vue`。改法：对话框由 `DeviceList.vue` 挂载，composable 只管开关状态与提交回调 | ✅ 已修（2026-11-28） |
| **DD-73** | `api/diagnostics/index.ts` 同时 `export * from './raw'` 与 `'./view'`，两套 API 只差 `View` 后缀。改法：删一层强制显式选，或统一加 `Raw` 后缀 | ✅ 已修（2026-11-28，删 barrel） |
| **DD-74** | 两个同名 `ConfigIssue`（`configValidation.ts:15` vs `configHealth.ts:19`）→ 改名即可，风险低 | ✅ 已修（2026-11-28） |
| **DD-75** | 「会话」四义（登录/回放/透传/"连接会话"）+ `critical` 两义（设备域"异常" vs 网关域"严重"）→ 改文案要过现场确认 | ✅ 已修（2026-11-28） |
| **DD-76** | `usePointFields.ts` / `useProtocol.ts` 用 `use*` 命名但无 `use*` 导出 → 需裁定"目录即 composable"（`usePoints/`、`useDevices/` 已是此范式） | ✅ 已修（2026-11-28） |
| **DD-78** | `usePacketReplay.ts:60` 导出的 `statusTagType` 与 `utils/statusType.ts:81` 同名不同义 → 改名即可 | ✅ 已修（2026-11-28） |
| **DD-79** | `UplinkConfigPage.vue` / `CertView.vue` 违反"能力词不表达技术栈" → 改名即可 | ✅ 已修（2026-11-28） |
| **DD-85** | `api/` 分组维度统一（37 平铺 + 4 子目录混放），动 `api/index.ts` 需全量回归 | 开发负责人 |
| **DD-86** | `utils/` 反向依赖上层 + `api` ⟷ `utils` 双向 → 牵动较广 | ✅ 已修（2026-11-28） |
| **DD-87** | `components/` 直接依赖 `stores/`（10 处）→ 影响通用组件可复用性 | ✅ 已修（2026-11-28） |
| **DD-83** | `utils/` 顶层 71 文件按主题分域 → **L 档，建议随其他改动捎带，不单独开工** | 排期 |

---

## 4. 附：32 项与摸底报告的对照

| 编号 | 摸底报告条目 | 标题 | 工作量 | 价值 | 档 |
|---|---|---|---|---|---|
| DD-64 | A-6 | lint 门禁已被并发提交打破 | S | 高 | A |
| DD-65 | A-1 | 静态循环依赖 5 组 | S | 中高 | A |
| DD-66 | A-2 | `stores/auth.ts` ⟷ `useSessions.ts` 2 元环（**已修**：会话上移为 store） | M | 中 | B |
| DD-67 | A-3 | 唯一的真层级倒挂 | M | 中 | B |
| DD-68 | E-1 | 组件状态取色 + `error` 档位反向 | S | 高 | A |
| DD-69 | E-2 | 问题分级计数两份 | M | 中 | B |
| DD-70 | E-3 | 健康状态中文名两份 | S | 中 | A |
| DD-71 | E-4 | 磁盘水位档位字典两份 | S | 中 | A |
| DD-72 | E-5 | 健康问题取色第二三份 | S | 中 | A |
| DD-73 | E-6 | diagnostics 双 `export *` | M | 中 | C |
| DD-74 | E-7 | 两个同名 `ConfigIssue` | M | 中 | C |
| DD-75 | E-8 / E-9 | 「会话」四义 + `critical` 两义 | M | 中 | ✅ 已修 |
| DD-76 | E-10 | `use*` 命名但无 `use*` 导出 | S | 低 | C |
| DD-77 | E-11 | 7 个候选死导出（**实为 5 个**） | S | 中 | A |
| DD-78 | E-12 | `statusTagType` 同名撞车 | S | 低 | C |
| DD-79 | E-13 | 命名违反"能力词"约定 | S | 低 | C |
| DD-80 | E-14 | 源码 UTF-8 残字节（**已修**） | S | 低 | A |
| DD-81 | S-1 | `parts/` 与 `components/` 同义词 | M | 中 | B |
| DD-82 | S-2 | 3 个 0 字节垃圾文件（**已修**） | S | 低 | A |
| DD-83 | S-3 | `utils/` 顶层平铺 71 文件 | L | 中 | D |
| DD-84 | S-4 | `deviceComms.ts` 绕过 ESLint #17 | M | 中 | B |
| DD-85 | S-5 | `api/` 分组维度不统一 | M | 低 | C |
| DD-86 | A-4 | `utils` 反向依赖 + `api` ⟷ `utils` 双向 | M | 中 | ✅ 已修 |
| DD-87 | A-5 | `components/` 直接依赖 `stores/` | M | 中 | C |
| DOC-F3 | D-4 / D-5 / D-10 | redirect 别名口径矛盾 | S | 中高 | A |
| DOC-F4 | D-6 / D-7 | `frontend-state.md` store 清单 | S | 中高 | A |
| DOC-F5 | D-1 / D-2 / D-3 | `frontend.md` 三处事实错误 | S | 中高 | A |
| DOC-F6 | D-9 / D-22 / D-23 | `test/README.md` 覆盖率与文件名 | S | 中高 | A |
| DOC-F7 | D-8 / D-11~D-14 | `composables/README.md` 过期 | S | 中 | A |
| DOC-F8 | D-16 / D-17 | `frontend.md` axios/EP/E2E | S | 中 | C |
| DOC-F9 | D-18 / D-19 / D-20 | `frontend-naming.md` §7 页面表 | S | 中 | C |
| DOC-F10 | D-21 / D-24 | `README.md` 计数与优化方向 | S | 中 | C |

---

## 5. 不排期（明确不做，防误改）

| 项 | 为什么不排 |
|---|---|
| `views/` 的 `dialogs/` 目录 | 语义清晰（弹窗专用）；`parts/` 已于 DD-81 全部并入 `components/`，仅保留 `dialogs/` |
| `utils/` 的 `echarts*` 9 文件 | 分层清晰（按需引入 / 实例生命周期 / 主题），注释逐项写明 |
| `utils/` 的 `hex*` 3 文件 | 已收敛为单一实现（DD-47 已销账），另两个是 re-export 兼容层 |
| `utils/` 的 `datetime`/`format`/`realtimeFormat`/`historyChart` | 职责不同：时间本源 / 容量时长 / 实时页纯函数 / 图表 option |
| `utils/concurrency.ts` ↔ `api/inflight.ts` | 前者批量任务并发池，后者 HTTP 在飞请求注册中心 |
| `api/token.ts` ↔ `utils/secureStorage.ts` 同名 `getToken` | 前者是 API 层薄封装 |
| `composables/auth/usePermission.ts` 动态引 mock | 有意为之，生产构建整链 tree-shake |
| `stores/healthReport.ts` vs `stores/gateway.ts` | 两个不同端点、两种语义，不构成重复所有权 |
| `api/index.ts` 的 barrel 兼容层 | 37 个业务模块在用，barrel 有存在必要（问题在 `api/` 分组维度，不在 barrel） |

---

## 6. 验收与回归

| 手段 | 命令 | 期望 |
|---|---|---|
| 门禁 | `npm run lint`（CI 口径加 `--max-warnings 0`） | EXIT=0 |
| 门禁 | `npm run type-check` | EXIT=0 |
| 门禁 | `npm test` | 全过 |
| 门禁 | `npm run build` | EXIT=0，**`INEFFECTIVE_DYNAMIC_IMPORT` 警告 0 条**（DD-66 后已清零） |
| 静态环 | `node scripts/audit-static-cycles.mjs` | **0 组** ✅（DD-65 5→1，DD-66 1→0） |
| 倒挂 | `node scripts/audit-layer-violations.mjs` | DD-67 后 `composables → views` 归零 |
| 断言 | `node scripts/verify-round5-claims.mjs` | 改文档后原 FAIL 项转 PASS；改代码后原 PASS 项不得变 FAIL |

> **最后一行是最重要的回归钉子**：`verify-round5-claims.mjs` 把摸底报告的可机器判定断言固化成可执行检查，**当前 92 条**，**任何一项修复后都应复跑一次**——既确认修好了，也确认没改坏别的地方。
>
> ⚠️ **维护约定**：脚本里原本钉的是「缺陷仍存在」（如"文档仍写 30% 覆盖率"）。修完以后这些断言会**正确地 FAIL**——此时**不要删断言**，要把它翻转为「断言已修状态」（如"覆盖率阈值已改为 63%"）。这样脚本才同时具备**回归**与**防回退**双重作用。DD-77 与 DOC-F3~F7 两批共翻转 13 条、新增 8 条；DD-66 翻转 4 条、新增 9 条；DD-80 / DD-82 翻转 4 条（`S-2.c/r/v`、`E-14.1`）、新增 7 条。
>
> ⚠️ **另一条维护约定（DD-66 踩出来的）**：做「符号残留」类断言时**必须剥离注释**再匹配。修 DD-66 时源码注释里会写明"原先是 `composables/system/useSessions.ts`"，正则不剥注释就会把注释里的历史路径误判成残留 import。脚本已加 `stripComments()` 辅助函数，新增此类断言请复用。

---

## 7. 一页纸版

```
✅ 已完成（2026-11-28，共 30 项，A 档 14 项全部清空）：
   DD-64 lint红灯      → 已修，lint 恢复 EXIT=0
   DD-65 静态环        → 已修，5 组 → 1 组
   DD-66 静态环        → 已修，1 组 → 0 组（会话上移为 store，用户已拍板）
   DD-68 取色反向      → 已修，error 档显红，两页逐项一致
   DD-69 计数归位      → 已修，store 那份已被真实消费
   DD-70/71/72        → 已修，三处语义同源到 utils/healthStatus
   DD-77 死导出        → 已修，5 删 2 留（diagHint/clearDeviceStats 保留），全仓零残留
   DD-80 坏字节        → 已修，concurrency.ts 还原为完整句子，其余字节未动
   DD-82 垃圾文件      → 已修，删 src/c、src/r、src/v，src 一级目录现 12 个
   DOC-F3~F7 文档      → 已修，10 条 P1 文档债一次清空（redirect 口径 / store 清单 /
                         tauri.ts 指向 / 覆盖率阈值 / composables 数量与文件名）
   DOC-F8~F10 文档     → 已修，axios/EP 归属、§7 页面表、README 计数与优化方向；断言 99/99
   DD-81 目录统一      → 已修，6 个 parts/ 并入 components/，29 处引用同步，零残留
   DD-84 规则盲区      → 已修，ESLint #17 覆盖 src/utils，STATS 显式豁免，lint 零告警
   DD-67 层级倒挂      → 已修，对话框改由 DeviceList.vue 挂载，composables → views 归零
   DD-74 ConfigIssue   → 已修，拆分 ConfigValidationIssue / ConfigHealthIssue
   DD-78 statusTagType → 已修，回放侧改名 replayStatusTagType
   DD-79 命名违例      → 已修，UplinkConfig / UplinkTemplate / CurrentCertificate
   DD-73 双 barrel     → 已修，删除 diagnostics/index.ts，调用方显式选 raw / view
   DD-76 use* 命名     → 已修，改名 pointFields.ts / protocolMeta.ts
   DD-87 store 耦合    → 已修，容器组件迁 views/layout 与 views/ops/health
   DD-86 utils 倒挂    → 已修，utils 向上依赖 0；api→stores 事件化后分层倒挂 36→0
   DD-75 术语漂移      → 已修，critical 统一「严重」；会话三词限定写入命名文档

本季做（1~3天）：     无强制项；DD-85 随下一次 api 重构捎带

有空再做：            DD-85（随 api 重构） · DD-83（只捎带）

别单独开工：          DD-83 utils分层(L档，随其他改动捎带)
```

> **剩余 2 项分布**：**A 档 0** / B 档 0 / C 档 1 / D 档 1，无一 P0。
> **当前门禁实测（2026-11-28 21:35）**：lint `--max-warnings 0` EXIT=0 · type-check EXIT=0 · 单测 264 文件 / 2513 用例全过 · build EXIT=0（**构建警告 0 条**） · **静态环 0 组** · 断言 **99/99 PASS**。
