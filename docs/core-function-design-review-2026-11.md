# 核心功能设计级复核清单（2026-11-20）

> 目的：把「核心功能」从**存在性**盘点（有没有、是否冗余、是否有 TODO）切换到
> **行为正确性**复核（设计上是否成立、失效时是否可见、失效后能否自愈）。
>
> 触发原因：此前多轮优化集中在零散 bug、协议细节与错误码一致性上，读写这一
> 核心链路的设计从未被系统审视过；写路径的优化由用户主动提出才启动。
>
> 前序文档（**仍在仓库，但口径已过时，仅作历史记录**）：
> - [functional-review-and-next-phase.md](functional-review-and-next-phase.md)（2026-11-03 第一版功能盘点）
> - [functional-inventory-and-next-roadmap.md](functional-inventory-and-next-roadmap.md)（2026-11-03/04 第二版复评）
> - [architecture/read-write-invariants.md](architecture/read-write-invariants.md)（读写语义不变量清单，含 G-01~G-08 缺口）
>
> 本清单**不重复**上述文档的存量盘点，只补「设计层缺口」。

---

## 0. 为什么之前一直没主动推荐优化读写（根因复盘）

| # | 原因 | 证据 | 后果 |
|---|---|---|---|
| 1 | **盘点口径是「存在性」不是「行为正确性」** | 前序两份盘点以路由组 / 页面 / 端点为最小单位，结论落在「功能齐备、债务为 0、冗余为 0」 | 「功能存在」被当成了「功能成立」。读写全部端点都在、能跑、无 TODO，于是被判为已完成 |
| 2 | **读写是横切语义，单模块 review 天然看不见** | 读链路跨 `scheduler → bus → protocol → pipeline → queue → north`；写链路跨 `api → scheduler/bus → protocol → audit` | 每一段单独看都合理，组合起来才暴露问题（例：组级退避被 `min` 拉平，失败点位实际从未退避） |
| 3 | **既有 ROI 清单由「显式失败信号」驱动** | 历次 ROI 项均来自 bug 复现、错误码不一致、panic、编码规范告警 | 设计级缺口没有告警、没有 panic、没有测试红；不出现在「可复现清单」里 |
| 4 | **「功能面冻结」的结论形成负向约束** | 前序盘点结论为「功能面冻结、债务为 0 ⇒ 主战场是验证，不是开发」 | 后续扫描自动转向「补测试 / 补验证」，读写这种「设计是否合理」的问题没有触发器 |
| 5 | **缺少「设计缺口」的登记载体** | 缺口登记表 [read-write-invariants.md](architecture/read-write-invariants.md) 的 G-xx 是写路径优化启动后才成体系的 | 在此之前缺口无处登记，也就无法排 ROI |

> 结论：不是「没看见」，而是**评估框架里没有这一类问题**。修法不是「以后多注意」，
> 而是引入**不变量化审查**：每个核心功能域都有一张「必须成立的不变量」表，
> 改动前对照检查（该表在读写域已存在，本清单把同样的方法扩展到其它核心域）。

---

## 1. 核心功能域清单（10 域）

> 域的划分按**运行链路**（而非模块目录），保证每个域都有独立的「行为正确性」可评估面。

| # | 核心功能域 | 一句话职责 | 代码主战场 | 现状 | 本次复核 |
|---|---|---|---|---|---|
| D1 | **南向采集（读）** | 按周期把现场值读上来，判质量、判失效 | `south/scheduler` + `south/bus` + `south/protocol` | 12 协议族 / 1750 core 测试 | **4 项缺口**（R-1~R-4） |
| D2 | **写与控制下发** | 把人的意图写进现场，并能说清「写没生效」 | `api/points/handlers` + `north/server` + `south/protocol` 写路径 | 三态 outcome + 读回消歧 | **4 项缺口**（W-1~W-4） |
| D3 | **数据处理管道** | 边缘处理 → 转换 → 质量规则 → 虚拟点位 → 分发 | `pipeline/` | 单测覆盖较全 | 1 项观察项 |
| D4 | **本地持久化与队列** | 断网不丢、断电可续、满时有序淘汰 | `queue/`（SQLite） | WAL + in_flight + 淘汰 | 1 项缺口（N-3） |
| D5 | **北向上行** | 稳定送达 MQTT / Webhook / OPC UA Server / Modbus Server | `north/` | 4 通道 + 独立 outbox | 2 项缺口（N-1、N-2） |
| D6 | **配置与热重载** | 改配置能生效、不能写坏、不能半生效 | `config/` + `runtime/gateway.rs` | 三层校验 + 落盘回滚 | 3 项缺口（C-1~C-3） |
| D7 | **诊断与可观测** | 出问题时能定位到**哪个设备哪个点** | `engine/metrics` + `api/diagnostics` | ~75 个指标 | 2 项缺口（D-1、D-2） |
| D8 | **协议调试台** | 现场能连、能扫、能抓包、能回放 | `api/protocol_tools`（31 端点） | 能力位驱动 | 无新增 |
| D9 | **设备与点位管理** | 配得起来（CRUD / 模板 / 批量作业） | `api/devices` + `api/points` | 已有 | 无新增 |
| D10 | **部署交付链路** | 装得上、跑得起来、改了配置不被沙箱吞 | `deploy/` + `gateway.service` | 121 项 lint + CI 实跑 | 无新增 |

---

## 2. 设计级缺口清单（按 ROI 排序）

> 每条 = 现象 / 证据 / 现场后果。**「现场后果」是排序依据**：
> 凡是「数据静默错误」或「运维看不见」的排前面。

### 第一梯队：静默出错，现场查不出

#### R-1 ✅ 已修（2026-11-21） 采集失败原因被压扁成 `collect_failed`，真实错因只进 debug 日志
- **现象**：读失败时上送的质量原因是硬编码字符串 `"collect_failed"`；真实错误（超时 / CRC / 帧格式 / 设备拒绝）只写到 `tracing::debug!`。
- **证据**：[scheduler/results.rs](../gateway-core/src/south/scheduler/results.rs#L108-L124) —— `DataPoint::failed(..., "collect_failed", ...)` 紧跟 `tracing::debug!("point {} read failed: {} ...")`。
- **现场后果**：北向拿到的报文只有「坏了」，没有「怎么坏的」；运维在平台侧无法区分「从站不在线」与「报文解析错」，只能回现场抓包。同一批点位如果原因是「帧格式错」被当成「设备离线」，排查方向直接跑偏。
- **修法**：新增 `CollectorError::quality_reason(prefix)`，统一输出 `<场景>:<稳定错误码>`（如 `collect_failed:TIMEOUT`、`modbus_read_failed:CRC`、`modbus_decode_failed:DECODE`）。改 [scheduler/results.rs](../gateway-core/src/south/scheduler/results.rs) 与 Modbus 采集器 4 处失败点；测试 `quality_reason_carries_stable_error_code` 锁定「后缀可被 `CollectErrorCode::from_code` 还原」。

#### R-2 ✅ 已修（2026-11-21） 读失败时继续用「上次成功值」上送，且无新鲜度标识
- **现象**：失败点构造的数据点 `value = last_good_value`、`source_ts = last_good_ts`、`raw_value = 0.0`，质量位标 Bad/Stale。
- **证据**：[scheduler/results.rs](../gateway-core/src/south/scheduler/results.rs#L74-L116) + [engine/model.rs](../gateway-core/src/engine/model.rs#L262-L285)（`failed()` 注释明写「value=last_good」）。
- **现场后果**：消费方若只看 `value` 不看 `quality`，会把**数小时前的旧值当实时值**用（电表冻结、流量累计错乱）。质量位存在，但没有任何机制强制消费方区分。
- **修法**：`DataPoint::failed` 把 `raw_type` 由固定 `"unknown"` 改为语义标记 —— 复用旧值时为 `"last_good"`，从未采到有效值时为 `"none"`，并把契约写进 `DataPoint` 字段注释与前端 `types.ts`；前端实时表「更新时间」列改用 `source_ts`（原来用 `timestamp`，会把旧值显示成「刚刚更新」），`stale_count>0` 时给出「采集失败 N 次，显示上次成功值」提示。测试 `failed_marks_reused_value_as_last_good` / `failed_without_last_good_is_marked_none`。

#### R-3 ★★☆ 组级退避被 `min` 拉平，单点失败实际**从未退避**
- **现象**：退避倍率（1→5→15→60）写在**点位**的 `DeviceState.current_interval` 上，但调度间隔取的是**组内所有点位 current_interval 的最小值**。只要组内还有一个健康点位，组的周期就保持最短 → 故障点位仍然每轮被读、每轮超时。
- **证据**：[scheduler/spawning.rs](../gateway-core/src/south/scheduler/spawning.rs#L62-L77)（`interval_fn` 取 `min`）对比 [scheduler/device_state.rs](../gateway-core/src/south/scheduler/device_state.rs#L72-L96)（`record_failure` 放大 `current_interval`）。
- **现场后果**：一个掉线的从站会**长期占据总线时隙**，把整条总线的有效采集率拖下来；日志与超时告警按周期反复刷；现场感觉「总线上其它设备时好时坏」，实际是**一个坏点拖垮一条线**。

#### W-1 ★★★ 写前无值域校验，`writable` 是唯一闸门
- **现象**：写入前只校验「点位是否 `writable`」，没有值的上下限/类型/量程校验；点位模型里也没有写值域字段（`alarm_high/low` 是**报警阈值**，不是**写入限幅**）。
- **证据**：[api/points/handlers/mod.rs](../gateway-api/src/web/api/points/handlers/mod.rs#L479-L495)（`ensure_writable` 只看 `point.writable`）；[config/point.rs](../gateway-core/src/config/point.rs#L166-L186)（点位无写限幅字段）。
- **现场后果**：任何拿到 exec scope 的上位机/MES/脚本，一次误写就能把 PLC 的设定值写越界（如功率设定 3000kW、压力上限 999bar）；写下去的是真实设备动作，事后只能靠人工复位。

#### W-2 ★★☆ 批量写是「部分成功」语义，但接口不声明，重试即重复动作
- **现象**：批量写按 `bus` 分组执行，逐点返回 `outcome`，成功与失败混在同一 `results[]` 里；接口没有「整批原子」或「可安全重试整批」的声明，也没有幂等键。
- **证据**：[api/points/handlers/mod.rs](../gateway-api/src/web/api/points/handlers/mod.rs#L321-L476)（分组写 + 逐点 outcome）。
- **现场后果**：上游看到 200 + 部分失败后重试整批 → **已成功的点被写第二次**。对累加型/脉冲型点位就是重复动作；这与 W-07（`WriteUncertain` 禁止重试）的结论方向一致，但批量层没继承这条规则。

#### W-3 ★★☆ 写缺少「写属性」建模（沿用 G-04，未关闭）
- **现象**：点位无法声明「设定值 / 增量 / 脉冲」，读回消歧只覆盖设定值语义。
- **证据**：[architecture/read-write-invariants.md](architecture/read-write-invariants.md) W-09（❌）/ G-04；[config/point.rs](../gateway-core/src/config/point.rs#L74-L187)（无对应字段）。
- **现场后果**：增量点位被上层用多次写模拟时，读回比对必然落空 → 长期停在「结果未知」，操作员每次都要人工读回核实。

### 第二梯队：会咬人，但有兜底或可事后发现

#### R-4 ★★☆ 超时是总线级配置，慢设备无法单独放宽
- **现象**：`timeout_ms` 挂在总线上，一个慢设备拖慢同总线全部从站的应答预算；组级超时按点位数估算，与设备实际速度无关。
- **证据**：[config/bus.rs](../gateway-core/src/config/bus.rs#L43-L46)（`timeout_ms` 在 BusConfig）；[scheduler/spawning.rs](../gateway-core/src/south/scheduler/spawning.rs#L104-L106)（`batch_timeout_budget(points.len())`）。
- **现场后果**：现场常见「一个老设备特别慢」，工程师只能**把整条总线的超时调大**，代价是所有从站的故障发现变慢。配合 R-3，坏点反复占用放大后的超时窗口。

#### C-1 ★★☆ 热重载非原子：运行态可能半新半旧，磁盘却已回滚
- **现象**：`reload_config` 按分节增量应用（buses / points / mqtt / ntp / convert / virtual_points），任一节失败时返回 Err，**已应用的分节不会回退**；Web 层只做**磁盘字节级回滚**，运行态不回滚。
- **证据**：[runtime/gateway.rs](../gateway-core/src/runtime/gateway.rs) `reload_config`（注释明写部分替换语义）；[web/config_apply.rs](../gateway-api/src/web/config_apply.rs) `persist_then_apply_inner`（reload 失败 → `rollback_saved_config` 字节回滚 → `store`）。
- **现场后果**：磁盘是旧配置、内存是新配置（或反之）。重启后行为改变，不重启行为也变 —— 最难查的一类「偶发」。

#### C-2 ★★☆ 配置文件改动不触发重载（无文件监听）
- **现象**：reload 触发路径只有 Web 保存与自愈流程；改磁盘上的 `config.json` 不会生效。
- **证据**：`config/loader.rs` 无 watcher 依赖；全仓无 `notify::/RecommendedWatcher/inotify` 匹配。
- **现场后果**：现场工程师直接改配置文件 → 重启前完全没效果，误以为「改了没用」而反复改；也可能改完直接重启，撞上 C-1 的半生效窗口。

#### D-1 ★★☆ 指标没有设备/总线维度，只有「失败最多的前 N 个点」
- **现象**：指标约 75 个 `gateway_*`，带标签的仅 `gateway_collect_failures_scoped{point=...}`，且**只导出失败数最多的前 N 个点**；无 `device=` / `bus=` / `slave=` 维度。
- **证据**：[engine/metrics/scoped.rs](../gateway-core/src/engine/metrics/scoped.rs#L324-L358)；全仓 `gateway-core/src` 无 `trace_id|correlation_id|request_id` 匹配。
- **现场后果**：一条总线有 200 个点时，第 50 名以后的坏点**在指标里完全不可见**；且无关联 ID，一次采集的日志无法串联 → 排查单设备故障只能人工翻日志。

### 第三梯队：已知取舍，确认保留

| # | 项 | 证据 | 结论 |
|---|---|---|---|
| N-1 | 北向 feed 满时**背压反压 pipeline**（容量 1024 批） | [north/feed.rs](../gateway-core/src/north/feed.rs) `DEFAULT_NORTH_FEED_CAPACITY` / `send` 满时 `await` | 有意为之（无损优先），代价是 broker 长挂会顶到采集。**保留，但需在文档中写明「上行堵 → 采集停」这条传导链** |
| N-2 | MQTT 与 Webhook 走**两套独立 outbox 与重试** | `north/mqtt/outbox` vs `north/webhook/client.rs` `outbox protocol='http'` | 双写风险低（分流明确），但两套重试/死信语义不一致，排障要分两套口径 |
| N-3 | 队列满时淘汰**最旧**未在途行 | [queue/storage/sqlite/limit.rs](../gateway-core/src/queue/storage/sqlite/limit.rs) `ORDER BY timestamp ASC, id ASC` | 语义正确且可观测（evicted 计数）。**保留** |
| N-4 | Modbus **同从站组内并发、组间串行** | [protocol/modbus/collector.rs](../gateway-core/src/south/protocol/modbus/collector.rs#L242-L353) | 组内并发已用满 `tcp_concurrent`（Bug 217 修复），组间串行即 S2 候选，收益依赖现场从站数 |
| C-3 | `queue.max_rows` 淘汰线**重启才生效**，水位判定立即生效 | [config/queue.rs](../gateway-core/src/config/queue.rs) 注释明写 | 配置与运行时短暂不一致，可接受，需在装机清单提示 |
| D-2 | 健康报告与队列/连接健康**同源** | `engine/monitor/health.rs` + `queue/core/limit.rs` `QueueHealth` | 口径一致，保留 |

---

## 3. 建议的执行顺序

| 序 | 项 | 价值 | 工作量 | 依赖 |
|---|---|---|---|---|
| 1 | ~~**R-1** 失败原因随数据上送（`quality_reason` 带真实错因）~~ ✅ 已完成 | 高 | 小 | 无 |
| 2 | **W-1** 写值域校验 + 点位写限幅字段 | 高 | 中 | 需产品口径（限幅/拒绝/告警三种策略） |
| 3 | **R-3** 退避按点位生效（组周期取组内**最大**失败退避，或拆组） | 高 | 中 | 与 R-4 一起设计 |
| 4 | ~~**R-2** 失败值的「新鲜度/陈旧」语义固化（写进 API 契约与前端）~~ ✅ 已完成 | 中高 | 小 | 与 R-1 同批 |
| 5 | **D-1** 指标加 `device` / `bus` 维度（或至少导出全量失败点位） | 中高 | 中 | 指标基数评估 |
| 6 | **W-2** 批量写原子性/幂等声明 | 中 | 中 | 需上游接口约定 |
| 7 | **R-4 + C-1** 逐点超时配置 + 重载原子性 | 中 | 中~大 | 改动面大 |
| 8 | **W-3 / N-4** 写属性建模、组间并行 | 中 | 中~大 | 需需求确认 |

---

## 4. 流程改进（防止再次漏掉）

### 4.1 评审口径：功能存在 ≠ 功能成立

**两个正交维度，必须分开评估：**

| 维度 | 问题 | 判定依据 |
|---|---|---|
| **功能面**（是否存在） | 这个功能该不该有？有没有重复实现？ | 需求匹配度 + 是否与已有能力重叠 |
| **功能成立**（行为正确性） | 已有的功能在真实现场是否成立？ | 不变量 + 横切语义 + 失效可见性 |

**「功能面冻结」只约束第一个维度**：不重复开发冗余功能、不做当前不需要的能力
（如双机热备 / 云平台 / 数据分析平台）。

**它不约束第二个维度**。优化补全现有功能（改行为、补语义、加可见性）**不是新功能开发**，
不受冻结约束，可正常排期、正常做 ROI 评估。

### 4.2 具体做法

1. **每个核心功能域维护一张「必须成立的不变量」表**（读写域已有：
   [architecture/read-write-invariants.md](architecture/read-write-invariants.md)）。
   新增语义必须补一行 + 尽量补测试；改动先对照勾选。
2. **不变量表要能表达「失效时是否可见」**——这一列是过去完全缺失的维度，
   正是 R-1/R-2/D-1 这类问题长期未被发现的直接原因。
3. **设计级缺口与 bug 分开登记**：bug 清单来自「复现」，设计缺口清单来自
   「假设这条不变量在真实现场成立吗」，两者不得互相替代。
4. **评审口径从「功能齐备」转为「功能成立」**：功能存在 ≠ 功能成立。
   存量盘点结论（功能齐备、债务 0）不再作为「不做优化」的依据。
5. **横切语义单独过一遍**：跨模块的组合行为（读路径的调度×退避×质量、
   写路径的语义×并发×审计）不会在单模块 review 里暴露，
   需按「链路」而非「模块」复核一次。
6. **失效可见性是硬门槛**：任何「失效后无人知道」的设计（静默降级、
   静默丢数、静默用旧值）在未修复前不得标记为完成。
