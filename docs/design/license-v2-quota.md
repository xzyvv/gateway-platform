# License 模型 v2.0：配额兜底（硬配额 + 无 license 降级）

> 状态：**已实施（2026-10-12，Step 1–5 全部落地，测试全绿）** · 修订自 license.md v1.2（已归档，未随仓库保留）
>
> 本文是 v2.0 授权模型的**技术规范**：license 只做一件事——约束 4 类产能配额
> （采集点数 / 并发协议数 / 历史保留天数 / 北向上行通道数）。
> 调试、分析、批量类工具能力**全部免费**，不受 license 约束。
>
> 实施记录：Step 1 `Quota` + payload.quota + 签发 CLI 配额参数；Step 2 配置化 tier_quotas/absent_quota +
> `LicenseManager.current()` 读时解析；Step 3 解挂 v1.2 全部功能中间件 + `check_quota` 配额校验
> （保存配置时 HTTP 400 / code 4021 中文报错）；Step 4 删除 features 字段、license_middleware、
> 签发 --features 参数，前端 tier 门禁解除、授权弹窗展示配额；Step 5 storage_days 配额驱动历史数据
> FIFO 截断（§5.2：每小时配额对账任务 + 队列 maintenance 周期清理，不阻断写入，None = 不限）。

## 1. 设计目标

授权模型只解决一个问题：**限制网关的产能占用**。

具体而言，license 负责：

```
license 的全部工作：
  1. 验证这是不是有效授权文件（验签）
  2. 验证这文件是不是签给这台机器的（指纹绑机）
  3. 校验当前使用量是否在 license 允许的配额内（点数/协议/存储/北向并发）
  4. 超限如何处理（拒绝新增 vs 软降级）
```

license **不负责**：

- ❌ 控制哪些菜单/路由/API 可用
- ❌ 控制调试/分析/抓包等工具能力是否启用
- ❌ 任何形式的「功能解锁」

> 设计取舍：**工具能力免费，产能配额收费**。
> 这样现场调试永远不会被授权状态阻断，同时产能上限依然可控。
> 落地过程中的完整决策记录见 §10。

## 2. 授权模型的边界

授权文件与硬件规格是**两层独立**的约束：

- **硬件层**：由 SKU 物理规格决定（接口数、通道数等物理上限）；
- **license 层**：只做「超限保护」——防止在硬件规格之外跑更大的负载。

配额档位与硬件型号的映射关系属于产品侧配置（`tier_quotas`），**不在本文档描述**，
避免文档与实际 SKU 定义产生漂移；调整档位只需改配置，无需改代码。

**license 兜底场景**：
- 设备实际负载超过当前档位配额 → license 拒绝新增（保护硬件与运行稳定性）；
- 授权文件未携带配额、且配置中未定义该档位 → 退到兜底配额（见 §3）；
- 授权失效 → 按 §6 软降级处理（已配置数据继续采集，新加点位拒绝）。

## 3. 授权 payload 结构

### v1.2 → v2.0 payload 对照

| v1.2 字段 | v2.0 字段 | 说明 |
|---|---|---|
| `tier: Tier` | `tier: Tier` | 保留（前端仍按 tier 展示档位） |
| `features: Vec<String>` | **删除** | 调试功能全免费，无需许可清单 |
| - | `quota: Quota` | **新增**：4 项配额上限 |
| `machine_key / machine_parts` | 不变 | 绑机机制保留 |
| `license_id / customer / issued_at / expires_at` | 不变 | 审计字段保留 |

### Quota 结构

```rust
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct Quota {
    /// 最大点位总数（None = 无限）
    pub max_points: Option<u32>,
    /// 最大并发协议数（None = 无限）
    pub max_protocols: Option<u32>,
    /// 历史数据保留天数（None = 无限；超限按 FIFO 清理）
    pub storage_days: Option<u32>,
    /// 北向上行并发通道数（None = 无限）
    pub northbound_channels: Option<u32>,
}
```

### 档位默认配额

| tier | max_points | max_protocols | storage_days | northbound_channels |
|---|---|---|---|---|
| core | 500 | 3 | 30 | 1 |
| standard | 1500 | 8 | 90 | 3 |
| pro | 5000 | 16 | 365 | 8 |
| ultra | None | None | None | None |
| **无 license** | 100 | 1 | 7 | 1 |

> **tier 名称以代码枚举为准**：`core` / `standard` / `pro` / `ultra`（`Tier` enum，serde 小写序列化）。
> 配置里写 `professional` / `enterprise` / `oem` 是早期草案旧称，会反序列化失败，不要沿用。
>
> **这张表是「配置目标值」，不是「代码内置默认值」**：仓库没有内置 tier 查表默认值，
> 真正生效的载体是 `config.json` 的 `license.tier_quotas`（`LicenseConfig::tier_quotas`）。
> 完整四档模板见 `gateway-core/config.json.example` 的 `license` 节，可直接复制。
> 三条生效优先级（`LicenseManager::current`）：
> **license 自带 `quota` → 配置 `tier_quotas` 查表 → `license.absent_quota` 兜底**。
> 因此**部署时必须显式写全 `tier_quotas` 四档**：若配置里没写、且 license 文件也没自带 quota，
> 则任何 tier（哪怕 ultra）都会退到末行的「无 license」配额（100 点 / 1 协议 / 7 天 / 1 通道）。
>
> **查表只在「license 有效」时发生**：无 license / 授权失效（`present = false`）一律走
> `absent_quota` 兜底（100/1/7/1），**不查 `tier_quotas`**——否则配置写全四档后，
> 无 license 设备会因 `tier = core` 而错误拿到 core 档的 500 点 / 3 协议 / 30 天 / 1 通道。
> 「core 档配额」与「无 license 配额」是两件事，前者是 core 授权，后者是完全没有授权。
>
> **降级可观测**：配置漏配某档时，配额会静默降到兜底值，现场只看配额数字无法察觉。
> 三处兜底：
> - 启动日志：`gatewayd` 启动时打印 `license.tier_quotas missing tiers: [...]` 的 WARN；
> - API：`GET /api/license` 响应新增 `quota_source` 字段；
> - 前端：授权弹窗在 `quota_source = tier_missing` 时展示黄色告警，提示补齐配置。

> **无 license 默认配额**意义：拿到网关即可跑最小化演示（100 点 / 1 协议 / 保留 7 天），
> 这是「开箱可体验」的基础约定——无授权也能验证链路，不必先走采购流程。

## 4. LicenseState 暴露给前端的字段

```rust
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct LicenseState {
    pub present: bool,         // 是否存在有效 license
    pub tier: Tier,            // 当前生效档位
    pub quota: Quota,          // 配额上限
    pub quota_source: QuotaSource, // 配额来源：license / tier_config / absent_fallback / tier_missing
    pub status: LicenseStatus, // ok / expiring / expired / mismatch / tampered / invalid
    pub expires_at: Option<i64>,
    pub license_id: Option<String>,
    pub customer: Option<String>,
}

/// 配额来源（serde 小写序列化）
pub enum QuotaSource {
    License,        // license 文件自带 quota
    TierConfig,     // 配置 tier_quotas 查表命中
    AbsentFallback, // 无 license / 授权失效 → absent_quota 兜底（预期行为）
    TierMissing,    // license 有效但 tier_quotas 缺该档 → 兜底（配置缺失，需告警）
}
```

**前端 GET /api/license 响应**：

```json
{
  "present": true,
  "tier": "pro",
  "quota": { "max_points": 5000, "max_protocols": 16, "storage_days": 365, "northbound_channels": 8 },
  "quota_source": "license",
  "status": "ok",
  "expires_at": 1814822400,
  "license_id": "LIC-2026-0001",
  "customer": "某某自动化",
  "machine": { "machine_key": "...", "onboard_mac": "...", "storage_serial": "...", "device_serial": "..." },
  "usage": { "points": 1820, "protocols": 8, "storage_days": 30, "northbound_channels": 3 },
  "available": { "points": 3180, "protocols": 8, "storage_days": 335, "northbound_channels": 5 }
}
```

**前端配额可视化**（设置页 / 状态栏）：

```json
{
  "tier": "pro",
  "quota": { "max_points": 5000, ... },
  "usage": { "points": 1820, "protocols": 8, "storage_days": 30, "northbound_channels": 3 },
  "available": { "points": 3180, "protocols": 8, "storage_days": 335, "northbound_channels": 5 }
}
```

`usage` / `available` 挂在 `LicenseSnapshot` 层（不在 `LicenseState` 层）——配额用量来自
**运行态**（当前生效配置 + 队列实际保留的历史），不属于授权文件本身状态；放 `LicenseState`
会让授权状态被运行态数据污染。

各字段口径（**与配额校验同源**，避免"显示的数"和"拦截的数"两套算法互相打架）：

| 字段 | 上限来源 | 已用量（usage）口径 | 剩余（available）口径 |
|---|---|---|---|
| `points` | `quota.max_points` | 展开后的采集点数（模板展开口径，同 `check_config_quota`） | 上限 − 已用，饱和到 0 |
| `protocols` | `quota.max_protocols` | 展开后点位 protocol 字段去重计数 | 上限 − 已用，饱和到 0 |
| `storage_days` | `quota.storage_days` | 队列实际保留的历史跨度（now − 最旧数据时间戳，向上取整到天） | 上限 − 已用，饱和到 0 |
| `northbound_channels` | `quota.northbound_channels` | 已启用的北向通道数（mqtt / webhook / cellular / opcua / modbus_server / modbus_passthrough / telemetry，共 7 类） | 上限 − 已用，饱和到 0 |

约定：
- 上限为 `null` → 该项**不限**，`available` 对应字段也是 `null`（不能退化成 0）
- 已用量 > 上限（存量配置超出当前档位）→ `available` 饱和到 0，前端标红提示升级
- `storage_days` 已用量取 `data_queue.timestamp` 与 `data_queue_aggregated.window_start`
  两者的最小值——超过 24h 的原始行会被压成分钟桶搬进聚合表，只看原始表会低估历史跨度

**超限提示策略**：
- 仪表盘/状态栏展示档位 + 配额使用进度条（无 license 不展示，只显示"未授权"）
- 配额超限时**配置保存接口直接拒绝**（如增加点位时返回 4021 `quota_exceeded` + 提示"当前档位支持最多 5000 点，当前已 5000 点，请升级档位或减少点位"）
- **不影响采集连续性**（已有数据继续运行）

**导入低档位 license 的超限告警**：

场景：现场已有 2000 点位配置，运维导入了一份 `max_points=100` 的低档位 license。
存量配置**瞬间**就超出了新档位配额，但导入接口此前只做「验签 + 指纹比对 + 落盘」，
不做存量配置与新配额的对比 → **零提示**，用户要等到下次保存配置才收到 4021，
而那时的报错指向"加点位"而非"导入 license 降了档"，排查方向被带偏。

策略（`POST /api/license/import`）：

| 动作 | 行为 |
|---|---|
| 是否阻断导入 | **不阻断**。license 已落盘生效、配额是既定事实，阻断只会造成"导入失败"的误解 |
| 是否告警 | **必须告警**。写 `tracing::warn!` + 审计日志 `license.quota_exceeded`（含超限项中文名） |
| 响应 | 返回 `usage` / `available`，`available` 饱和到 0；前端据此弹红色告警 |
| 判定范围 | 只判 `points` / `protocols` / `northbound_channels` 三项（保存配置时会被拒绝的项）。`storage_days` 由运行时 FIFO 截断保证，不拒绝配置保存，拿它报警只制造噪音 |

前端表现：导入成功后 `LicenseDialog` 立即显示红色告警条，列出超限项（如"采集点数"），
提示用户升级档位或清理存量配置。

## 5. 代码调整范围

### 5.1 必须改

| 模块 | 改动 |
|---|---|
| `gateway-core/src/license/verify.rs` | `LicensePayload` 删除 `features: Vec<String>`，新增 `quota: Quota` 字段；新增 `QuotaUsage` / `QuotaAvailable` 结构体与 `Quota::available()` / `Quota::exceeded_kinds()` 计算 |
| `gateway-core/src/license/mod.rs` | `LicenseState` 替换为新结构；`allows(min_tier, feature)` 替换为 `check_quota(quota_kind, current_value) -> Result<(), QuotaError>`；新增默认配额映射（core/standard/pro/ultra 各自的 Quota）；`LicenseSnapshot` 增加 `usage` / `available` 字段 |
| `gateway-core/src/license/fingerprint.rs` | 无改动 |
| `gateway-core/src/queue/storage/mod.rs` | `QueueStorage` 新增 `oldest_data_timestamp()`（默认 `None`） |
| `gateway-core/src/queue/storage/sqlite/sqlite_storage.rs` | 实现 `oldest_data_timestamp()`：查 `data_queue.timestamp` 与 `data_queue_aggregated.window_start` 两表 MIN |
| `gateway-core/src/queue/core/task.rs` | `QueueCommand` 新增 `OldestTs` 变体（真实 DB 查询走 `spawn_blocking`） |
| `gateway-core/src/queue/mod.rs` | `DataQueue` 新增 `oldest_data_timestamp()` async 方法 |
| `tools/license-issuer/src/main.rs` | CLI 参数：`--features` 删除，新增 `--max-points / --max-protocols / --storage-days / --northbound-channels` |
| `gateway-api/src/web/api/license/handlers/mod.rs` | GET / POST 响应装配 `usage` / `available`；配额计数复用 `config_apply::count_*`；导入成功后对存量配置做超限判定（不阻断 + `license.quota_exceeded` 审计告警） |
| `gateway-api/src/web/config_apply.rs` | `count_points` / `count_protocols` / `count_northbound_channels` 提升为 `pub(crate)`（供用量统计复用同源口径） |
| `gateway-api/src/web/traits.rs` | `DataQueueTrait` 新增 `oldest_data_timestamp()`（默认 `None`） |
| `gateway-web/src/api/generated/` | ts-rs 自动重新生成（`Quota.ts`, `QuotaUsage.ts`, `QuotaAvailable.ts`, `LicenseSnapshot.ts`） |
| `gateway-web/src/stores/tier.ts` | 删除 `features` 相关逻辑，改为 `quota + usage` 跟踪（`licenseUsage` / `licenseAvailable`） |
| `gateway-web/src/views/layout/Topbar.vue` | 徽章文案从"Ultra"改为"5000 点（已用 1820）"；tooltip 追加配额明细 |
| `gateway-web/src/views/layout/LicenseDialog.vue` | 现状展示从"档位+功能清单"改为"档位+配额表+使用量"（新增"配额使用"行：已用 / 上限 / 剩余，超限标红） |

### 5.2 feature gate 替换为配额校验

| 原 v1.2 feature gate | v2.0 配额校验挂接点 |
|---|---|
| `require_tier_pro` / `require_tier_ultra` 等档位中间件 | **删除**（调试功能全免费，不需要 tier 拦截） |
| `require_packet_capture` (ultra) | **删除**（抓包免费） |
| `require_step_debug` (ultra) | **删除**（单步免费） |
| `require_slave_sim` (ultra) | **删除**（从站模拟免费） |
| `require_perf_monitor` (pro) | **删除**（性能监控免费） |
| `require_batch_cmd` (pro) | **删除**（批量操作免费） |
| `require_protocol_scan` (standard) | **删除**（协议扫描免费） |
| - | **新增** `point_registry` 保存时调用 `check_quota(Points, current_count + 1)` |
| - | **新增** `protocol_config` 保存时调用 `check_quota(Protocols, current_count + 1)` |
| - | **新增** `northbound_config` 保存时调用 `check_quota(Channels, current_count + 1)` |
| - | **新增** 存储清理任务每日检查 `storage_days`，超限数据按 FIFO 清理（不阻断写入） |

### 5.3 不变（骨架完全保留）

- 验签实现（Ed25519 + ring 0.17）
- 指纹分层比对（主键优先 + 动态阈值）
- 时钟回拨防护（48h + NTP 重置）
- 失效软降级语义（降级 + 采集不停）
- 注入通道（POST /api/license/import + 文件路径 + 每日重校验）
- 授权管理对话框（设备码复制 + 文件导入 + 中文错误）
- NTP→ClockGuard 接线（spawn_license_ntp_resync_watch）
- ts-rs 类型导出

## 6. 失效软降级语义

### v1.2 行为

- 无 license / 失效 → 降级 core，采集不停
- 调试类功能（ultra）→ 失效时不可用

### v2.0 行为

- 无 license / 失效 → 配额**回退到无 license 默认配额**（100/1/7/1）
  - 已配置但未超限的数据**继续运行**（如原本有 200 点，失效后仍在跑，不会突然停）
  - 任何**新增/修改配置**操作（加点位 / 加协议 / 加北向）**被拒绝**（返回 4021 quota_exceeded + 提示"授权失效，仅可使用默认 100 点限制"）
- 用户感知："我现在不能用满配了，要么导入有效 license，要么精简到 100 点"

### 这个调整的设计意图

- 超限有明确提示 → 使用者清楚知道当前上限，不会"莫名其妙配不上去"；
- 不会突然失能（在运数据链路继续跑）→ 工业现场红线不破；
- 语义一句话可概括：**授权失效 = 新增受限，不是断采**。

## 7. 注入通道（不变）

- POST /api/license/import（dry-run 校验 + 中文错误反馈）
- 文件路径约定（GATEWAY_LICENSE_PATH → config → ./license.lic）
- 每日自动重校验
- 网页授权管理对话框

> 注入通道的设计已经满足"任何能写文件的位置都能导入"，不需要为 v2.0 调整。

## 8. 迁移路径（已按原计划完成）

> **状态：全部落地（2026-10-12）**，以下 Step 描述保留作实施过程的历史参考；
> 实际落地细分记录见本文档头部「实施记录」（Step 1–5）。

### Step 1：扩展 payload（双轨 1 周）

- `LicensePayload` **同时保留** `features` 字段（已废弃但解析）和新增 `quota` 字段
- 校验逻辑：features 忽略，quota 启用
- license-issuer CLI 双参数支持（features 转为警告 + quota 必填）
- 旧 license 文件仍可加载（features 字段忽略）

### Step 2：配额校验挂接（2 周）

- 删除 6 个 feature gate 中间件
- 调试类功能（packet-capture、step-debug、slave-sim、perf-monitor、batch-cmd、protocol-scan）路由不再受 license 控制
- 配额校验挂接到 point_registry / protocol_config / northbound_config
- 集成测试：配额超限配置保存返回 4021

### Step 3：前端改造（1 周）

- 重新生成 ts-rs 类型
- Topbar 徽章 + LicenseDialog 改造为展示配额 + 使用量
- 删除 tier store 的 features 相关状态

### Step 4：清理（1 周）

- 移除 `features: Vec<String>` 字段
- 移除 `feature_min_tier` / `implies` 等档位隐含逻辑
- 移除 license-issuer 的 `--features` 参数
- 文档更新（v1.2 文档归档，本 v2.0 文档作为现行版本）

**总计**：原计划约 5 周完成从 v1.2 到 v2.0 的平滑迁移——实际已按计划完成，期间双轨运行未破坏存量设备。

## 9. 风险与决策点

### 风险

| 风险 | 缓解 |
|---|---|
| 调试类功能全放开后，可能出现「现场无人再深入使用高级调试能力」的情况 | 属于产品运营范畴，不是 license 能解决的问题；调试能力作为工具链的一部分持续交付 |
| 6 个 feature gate 全部删除，回归测试覆盖范围缩小（v1.2 有 21 条 feature gate 集成测试） | 新增配额校验集成测试（点数超限 / 协议超限 / 北向超限）+ 调试功能免 gate 测试（约 15 条） |

### 决策点与结论

以下四个决策点已评审通过（2026-10-12 随实施生效），结论如下：

1. **档位配额数值：core / standard / pro / ultra 四档，点数配额 500 / 1500 / 5000 / 无限制。**
   （档位名以代码 `Tier` 枚举为准：`core` / `standard` / `pro` / `ultra`；
   早期草案里的 professional / enterprise / oem 是旧称，serde 无法反序列化，已作废。）
   数值定义于 `tier_quotas` 配置（gateway-core LicenseConfig），后续如需调整档位数值，**改配置即可，无需改代码**。
   ⚠️ 仓库未内置这些数值：**出厂/部署配置必须显式写全 `license.tier_quotas` 四档**
   （完整模板见 `gateway-core/config.json.example` 的 `license` 节，可直接复制），
   否则 license 未自带 quota 字段时，各 tier 一律退到 `absent_quota`（100 点 / 1 协议 / 7 天 / 1 通道），
   与本节「已落地」的预期不符。
   该漏配不再静默：启动时打 WARN 列出缺失档位，API 的 `quota_source` 返回 `tier_missing`，
   前端授权弹窗展示告警提示补齐配置。

2. **无 license 默认配额：100 点位 / 1 并发协议 / 7 天历史保留 / 1 北向通道。**
   对应 `Quota { max_points: 100, max_protocols: 1, storage_days: 7, northbound_channels: 1 }`
   —— 注意 `storage_days` 是历史保留天数，不是北向通道数。
   已按此数值落地实施。数值定义于 `absent_quota` 配置，调整同样无需改代码。该默认值保证无 license 时设备可调试、可演示，但不足以支撑正式项目运行。

3. **配额与硬件 SKU 解耦：档位数值与具体硬件型号的映射关系由产品侧维护。**
   配额与 SKU 已解耦，SKU 定义变化只改配置、不改代码。硬件侧确认后仅需同步 `tier_quotas` 中各档配额数值。
   此项**不阻塞 license 实现**。

4. **失效软降级语义：license 失效后现有配置继续运行，新增/修改配置受配额限制（"新增受限 vs 现有继续跑"）。**
   已按此语义落地实施，行为详见 §6 v2.0 行为。该语义遵守工业现场红线——不因 license 问题中断在运数据链路。

## 10. 决策记录（已通过，2026-10-12 随实施生效）

| 项 | v1.2 决策 | v2.0 提案 | 重新评估触发条件 |
|---|---|---|---|
| 调试功能是否受 license 约束 | 是（ultra/pro 档） | **否（全部免费）** | 无（工具能力免费是模型前提） |
| License 维度 | 按功能分档（tier + features） | **按配额分档（tier + quota）** | 若改用按调用量计费模型（届时改为"按月调用次数计费"） |
| 硬件分层 | 同款软锁分档 | **配额与硬件规格解耦** | 单 SKU 规格差异过小（不值得分开） |
| 高级分析是否受 license 约束 | 是 | **否** | 分析能力作为云端服务提供时（届时按云服务计费） |
| 失效时现有数据 | 全部继续跑 | **继续跑，新增受限** | 明确要求"全停以促续费"（不推荐，违反工业红线） |

---

> **状态**：本文档已评审通过并完成实施（2026-10-12）。后续配额数值调整走 `tier_quotas`/`absent_quota` 配置；
> 若触发 §10「重新评估触发条件」，另起 v3 文档修订，不在本文档上原地改写。
