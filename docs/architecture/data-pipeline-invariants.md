# 数据处理链路不变量清单

本文回答什么问题：一条采集数据从协议栈产出到北向社会，中间每一段「必须成立」的规则是什么；现场出现「数据对不上」时，如何判断问题出在采集、换算、质量规则、虚拟点还是分发；哪些规则已有测试守护，哪些只是代码约定、属于后续补测试的缺口。

适用范围：`gateway-core` 的数据处理主链路。读写两侧的持久化不变量见 `docs/architecture/read-write-invariants.md`，链路总览见 `docs/architecture/data-flow.md`，点位换算配置见 `docs/点位配置与换算.md`。

判定口径：
- 【有守护】表示仓库内存在对该条语义的自动化断言，且断言失败会直接暴露该条不变量被破坏。
- 【无（缺口）】表示仅靠代码约定或注释维持，没有直接断言；这是后续补测试的清单来源。
- 行号均相对于仓库根目录，随代码演进可能漂移，语义以函数名为准。

主链路顺序（详见 §6 的 I-53）：

```
采集（协议解码 + 公式/scale/offset/CT-PT）
  → EdgeProcessor（聚合 / 死区 / 变化）
  → ConvertEngine（raw / linear / enum 模板）
  → quality_rules（range / rate / nan）
  → 虚拟点位求值
  → RealtimeBus（可丢） + NorthFeed（不丢） + DataQueue（落盘）
  → NorthDispatcher（feed 型 connector）
  → MQTT 单独从 DataQueue 出队发布
```

---

## §1 采集与数据点

| 编号 | 不变量 | 代码依据 | 现有测试 / 守护用例 | 违反后的现场表现 |
|---|---|---|---|---|
| I-01 | 成功点的 `value` 必须等于 `convert_value(point, raw)`：先公式（存在则用公式，否则 `scale*raw+offset`），再按单位乘 CT/PT。 | `gateway-core/src/south/protocol/modbus/batch.rs:100-103`、`gateway-core/src/south/protocol/common/decode.rs:184-215` | 【有守护】`gateway-core/src/south/protocol/common/decode/tests.rs:161-237` | 上报工程量整体偏大/偏小，换算类故障与采集类故障无法区分。 |
| I-02 | 成功点的 `raw_value` 必须是协议直接解析出的原始值，`raw_type` 必须是协议数据类型（如 `u16`/`bool`/`f32`），二者不得被换算覆盖。 | `gateway-core/src/south/protocol/modbus/batch.rs:122-138`、`gateway-core/src/engine/model.rs:143-178` | 【部分守护】`gateway-core/src/south/protocol/s7comm/tests/e2e.rs` 的 `read_applies_scale_offset_and_ratio` 断言 `raw_value == 2.0`（原始寄存器值不被换算覆盖）；`raw_type` 与协议数据类型的对应关系仍无断言 | 报文里只有工程量，无法核对原始码值；现场无法区分「协议解析错」与「换算错」。 |
| I-03 | `DataPoint::good` 遇到 NaN/±Inf 必须把对应字段清零，并把质量降为 `Uncertain`；`value` 非法时 reason 为 `value:NON_FINITE`，仅 `raw_value` 非法时为 `value:NON_FINITE_RAW`。 | `gateway-core/src/engine/model.rs:213-240` | 【有守护】`gateway-core/src/engine/model.rs:400-435` | NaN 进入 MQTT payload（序列化为 null）或 SQLite REAL 列，下游拒收或比较异常。 |
| I-04 | `DataPoint::good` 构造的 Good 点必须满足 `timestamp == source_ts` 且 `stale_count == 0`。 | `gateway-core/src/engine/model.rs:241-253` | 【部分守护】`gateway-core/src/engine/model.rs` 的 `good_passes_through_finite_values` 断言 `source_ts == timestamp`；`stale_count == 0` 仍无断言 | 诊断时无法按该构造路径依赖「采样时刻=上送时刻」这一简化假设。 |
| I-05 | 协议批量构造成功点时 `timestamp` 与 `source_ts` 是两次 `DataPoint::now()` 调用，允许相差数毫秒，不保证严格相等。 | `gateway-core/src/south/protocol/modbus/batch.rs:134-135`、`gateway-core/src/south/protocol/modbus/collector.rs:296-297` | 【无（缺口）】 | 若有下游按 `source_ts == timestamp` 去重或判定，会漏掉部分点。 |
| I-06 | `DataPoint::failed` 只有在存在 last_good 时 `raw_type` 才为 `last_good` 且 `value` 复用该值；从未成功采集时 `raw_type` 必须为 `none`、`value=0`、`stale_count=0`。 | `gateway-core/src/engine/model.rs:261-298` | 【有守护】`gateway-core/src/engine/model.rs:370-397` | 把 0.0 当真实读数；平台无法识别「这个点从未采到过」。 |
| I-07 | 调度器 `Err` 路径的失败点必须满足：`source_ts` 为 last_good 的原始采集时刻，`stale_count` 随连续失败递增，`quality` 由设备状态递进得出。 | `gateway-core/src/south/scheduler/results.rs:115-134`、`:153-161`、`gateway-core/src/engine/model.rs:293-295` | 【有守护】`gateway-core/src/engine/model.rs:377-379`（`source_ts`/`stale_count`）、`gateway-core/src/south/scheduler/device_state/tests.rs:19-62`（质量递进与 stale 计数） | 失败点时间戳变成本轮时间，平台无法判断值已经陈旧了多久。 |
| I-08 | 采集失败以 `Ok(DataPoint{quality: Bad})` 返回时，必须与 `Err` 路径同口径：复用 last_good 值与采集时刻（`raw_type=last_good`）；从未成功才保持 `none`/0。不得再上送 value=0、source_ts=0。 | `gateway-core/src/south/scheduler/results.rs:26-54`（统一构造）、`:106-115`（接入两种降级路径） | 【有守护】`gateway-core/src/south/scheduler/tests.rs:834-911`（契约）、`:915-977`（经 `handle_results` 到上报队列的端到端接线） | 同一条「失败」在两条路径上字段语义不同：一条带 last_good，另一条是 0 值且 `source_ts=0`（1970），现场极易误读。 |
| I-09 | `Ok` 且 `quality != Good` 的结果必须计入采集失败指标，不能按 `result.is_ok()` 判定成功。 | `gateway-core/src/south/scheduler/spawning.rs:216-235` | 【有守护】`gateway-core/src/south/scheduler/tests.rs:140-210` | Modbus 整条总线故障但 `collect_failures` 为 0，监控显示零故障。 |
| I-10 | 所有非空 `quality_reason` 统一为 `<场景>:<稳定码>`（成功为空串）：`collect_failed:<CODE>`、`modbus_decode:DECODE`、`value:NON_FINITE[_RAW]`、`convert:NON_FINITE`、`quality_rule:<规则名>`、`virtual_point:ERROR`、`aggregate:EMPTY_WINDOW`、`opcua_status_<Status>:OTHER`。 | `gateway-core/src/south/protocol/mod.rs:224-226`、`gateway-core/src/engine/model.rs:223`、`gateway-core/src/south/protocol/modbus/batch.rs:129-133`、`gateway-core/src/pipeline/convert.rs:214`、`gateway-core/src/pipeline/virtual_point.rs:351`、`gateway-core/src/pipeline/nodes.rs:54`、`gateway-core/src/pipeline/quality_rules.rs:119` | 【有守护】`gateway-core/src/engine/model.rs` 的 `quality_reason_follows_scene_code_contract`（校验公共构造函数 + 扫描各阶段赋值字面量） | 按冒号切分统计错因的脚本会把解码失败、换算降级归入未知分类。 |

### 质量原因编码表（I-10 决策）

约定：`<场景>:<码>`，场景固定小写标识；成功时为空串。码尽量复用跨协议错误码
（`TIMEOUT` / `NO_REPLY` / `CRC` / `DECODE` / `IO` / `CONFIG` / `OTHER`）。

| 场景 | 码 | 产生位置 | 说明 |
|---|---|---|---|
| `collect_failed` | 跨协议错误码 | `CollectorError::quality_reason("collect_failed")` | 调度器采集失败的默认场景 |
| `modbus_read_failed` / `modbus_write_failed` | 跨协议错误码 | Modbus 读写路径 | 协议级失败场景 |
| `modbus_decode` | `DECODE` | `modbus/batch.rs` | Modbus 批量解码失败 |
| `value` | `NON_FINITE` / `NON_FINITE_RAW` | `DataPoint::good` | 非有限值降级（`value` 优先于 `raw_value`） |
| `convert` | `NON_FINITE` | `pipeline/convert.rs` | 线性换算结果非有限 |
| `quality_rule` | 规则名 | `pipeline/quality_rules.rs` | 第二段为用户配置的规则名（例外，但场景固定） |
| `virtual_point` | `ERROR` | `pipeline/virtual_point.rs` | 公式缺失依赖/循环引用等求值失败 |
| `aggregate` | `EMPTY_WINDOW` | `pipeline/nodes.rs` | 聚合窗口内无样本 |
| `opcua_status_<Status>` | `OTHER` | OPC UA 读路径 | 场景内嵌原始 StatusCode，便于北向分类 |

新增原因时必须带冒号场景前缀，`quality_reason_follows_scene_code_contract` 会扫描源码字面量拦截遗漏。

---

## §2 换算与质量

| 编号 | 不变量 | 代码依据 | 现有测试 / 守护用例 | 违反后的现场表现 |
|---|---|---|---|---|
| I-11 | 读路径换算顺序固定为「先工程量换算、后变比」：公式存在时用公式，否则 `scale*raw+offset`；随后按 `ratio_scope` 乘 CT/PT。 | `gateway-core/src/south/protocol/common/decode.rs:184-215` | 【有守护】`gateway-core/src/south/protocol/common/decode/tests.rs:161-237` | 变比与 offset 互相放大，读回值整体偏移。 |
| I-12 | 公式执行失败时必须回退到 `scale*raw+offset`，只记 warn，不改变质量码、不产出错误点。 | `gateway-core/src/south/protocol/common/decode.rs:186-202` | 【无（缺口）】 | 公式语法错误被掩盖，上报值「看起来正常」但口径错误。 |
| I-13 | 换算结果非有限时必须回退为 `raw`（不是 0、不是 `Uncertain`）。 | `gateway-core/src/south/protocol/common/decode.rs:216-237` | 【有守护】`gateway-core/src/south/protocol/common/decode/tests.rs:264-291` | 极端变比或公式错误时该点上报原始寄存器值，量纲与同设备其它点不一致。 |
| I-14 | CT/PT 生效范围必须由 `unit` 决定：电压只 PT、电流只 CT、功率/电量/空/未知单位 Both、功率因数 None；读写两侧共用同一判定。 | `gateway-core/src/config/point.rs:394-415`、`gateway-core/src/south/protocol/common/decode.rs:203-215`、`gateway-core/src/south/protocol/common/decode.rs:271-291` | 【有守护】`gateway-core/src/south/protocol/common/decode/tests.rs:182-231`、`:295-356` | 电压点误填 CT 后被放大 30 倍；或写入侧不还原。 |
| I-15 | 反向换算必须与读路径对偶：先除变比，再 `(v-offset)/scale`；`scale=0` 时不反解；公式不反推。 | `gateway-core/src/south/protocol/common/decode.rs:241-292` | 【有守护】`gateway-core/src/south/protocol/common/decode/tests.rs:295-356` | 下发值与回读值长期对不上，形成「写成功、读不符」。 |
| I-16 | convert 模板只作用于 Good 点：linear 结果非有限时降级 `Uncertain` 并置 reason `convert:NON_FINITE`；enum 只写 `value_text`，不得改 `value`。 | `gateway-core/src/pipeline/convert.rs:186-237` | 【有守护】`gateway-core/src/pipeline/convert.rs:458-473`、`:553-575` | 坏值继续上行；枚举点数值被文本覆盖后无法参与阈值/报警。 |
| I-17 | 同一 point 的多条 convert 规则必须按配置顺序应用（例如 linear 先算、enum 后查表）。 | `gateway-core/src/pipeline/convert.rs:51-65`、`:197-201` | 【有守护】`gateway-core/src/pipeline/convert.rs:381-420` | enum 用换算前的值查表，命中错误标签。 |
| I-18 | 质量规则只作用于 Good 点，按规则列表顺序首个命中即生效并终止；命中原因固定为 `quality_rule:<name>`。 | `gateway-core/src/pipeline/quality_rules.rs:68-123` | 【有守护】`gateway-core/src/pipeline/quality_rules.rs:208-300` | 已判 Bad/Uncertain 的点被「洗白」；多个规则叠加改写原始故障原因。 |
| I-19 | rate 规则必须与同 `point_id` 的上一次采样值比较，首样本永不命中；上值表上限 4096 点、TTL 1 小时，超限先按时间淘汰、仍超则整体清空。 | `gateway-core/src/pipeline/quality_rules.rs:27-31`、`:84-106`、`:150-158` | 【有守护】上一采样比较 `:239-258`；【无（缺口）】容量/TTL 清理 | 点位数逼近上限时短时丢失历史基准，rate 规则在部分点上静默失效。 |
| I-20 | `stale` 不得由质量规则判定，必须由调度器的连续失败/超时机制负责；质量规则不得产出 `Stale`。 | `gateway-core/src/pipeline/quality_rules.rs:12-16`、`:114-115` | 【无（缺口）】 | 两套口径互相覆盖，前端质量码与退避计数不一致。 |
| I-21 | 死区按 `abs(value-last) < deadband` 过滤；变化节点在 `deadband>0` 时复用死区阈值、否则用 `EPSILON`；NaN/Inf 直接放行且不更新基线。 | `gateway-core/src/pipeline/nodes.rs:308-327`、`:368-400` | 【有守护】`gateway-core/src/pipeline/edge/tests.rs:39-58`、`:103-121` | 异常值污染基线导致后续比较全失效；变化上报退化为每样本都传。 |
| I-22 | 死区/变化/聚合判定的对象是 convert 模板**之前**的 `value`（边缘处理先于 convert 执行）。 | `gateway-core/src/pipeline/mod.rs:291-312` | 【有守护】`gateway-core/src/pipeline/mod.rs` 的 `stage_order_is_edge_then_convert_then_virtual`（原始差 0.2 被死区过滤，若先换算则差 2.0 会漏过） | 同时配置 linear 模板与 deadband 时，阈值按转换前量纲判定，过滤量与配置直觉不符。 |
| I-23 | 聚合只对配置了 `aggregation` 的 Good 点生效，原始样本不上行；聚合点 `timestamp=source_ts=窗口右边界`、`raw_type="aggregated"`；空窗口必须产出 `Bad`/`empty_window` 占位点。 | `gateway-core/src/pipeline/nodes.rs:28-60`、`:153-163`、`:176-247` | 【有守护】`gateway-core/src/pipeline/edge/tests.rs:177-237` | 原始点与聚合点重复上行；设备离线期间平台收不到任何占位样本。 |
| I-24 | 聚合补窗最多连续 64 个窗口；超出后跳到当前窗口边界，不再逐窗产出占位点。 | `gateway-core/src/pipeline/nodes.rs:21`、`:207-235` | 【有守护】`gateway-core/src/pipeline/edge/tests.rs:309-349` | 长时间离线或时钟前跳后，一次性产出海量点撑爆内存与下游。 |
| I-25 | 聚合 flush 产物不得重新进入聚合缓冲（`process_flushed` 直通），但仍要经过后续死区/变化节点。 | `gateway-core/src/pipeline/edge.rs:150-159`、`gateway-core/src/pipeline/nodes.rs:165-173` | 【有守护】`gateway-core/src/pipeline/edge/tests.rs:238-308` | 同一聚合值每秒重复上报；或聚合值被死区静默吞掉。 |
| I-26 | 质量恢复样本（非 Good→Good 边沿）必须强制上行一次，并把死区/变化的基线更新为恢复值。 | `gateway-core/src/pipeline/edge.rs:120-137`、`gateway-core/src/pipeline/nodes.rs:329-337`、`:394-400` | 【有守护】`gateway-core/src/pipeline/edge/tests.rs:73-101` | 恢复后的首个同值样本被过滤，平台长期停留在坏值状态。 |

---

## §3 虚拟点

| 编号 | 不变量 | 代码依据 | 现有测试 / 守护用例 | 违反后的现场表现 |
|---|---|---|---|---|
| I-27 | 虚拟点只能读到「未过期」的缓存值；只有 Good 采集点写缓存；TTL 取 `max(30s, 3×interval)`。 | `gateway-core/src/pipeline/virtual_point.rs:138-150`、`gateway-core/src/pipeline/point_cache.rs:21-22`、`:51-84` | 【有守护】`gateway-core/src/pipeline/point_cache.rs:111-160` | 设备离线后仍用陈旧值算出「正常」结果，且永不报引用缺失。 |
| I-28 | 虚拟点缓存只写「最终上报口径」值（边缘过滤 → convert → 质量规则后的 `final_points`）；被过滤/降级的点保留上一条上报口径值，绝不回退到 convert 前的 `items.value`。 | `gateway-core/src/pipeline/virtual_point.rs:130-150`、`gateway-core/src/pipeline/mod.rs:312-323` | 【有守护】`gateway-core/src/pipeline/virtual_point.rs:552`、`:566`、`:584` | 同一采集点在不同批次给虚拟点提供的量纲不一致，派生值抖动或与上报值对不上。 |
| I-29 | 虚拟点可引用其它虚拟点，按固定点迭代最多 8 轮；同级解出后必须立即可被后续规则看到。 | `gateway-core/src/pipeline/virtual_point.rs:171-229` | 【有守护】`gateway-core/src/pipeline/virtual_point.rs:434-498` | 级联只算一层，或结果依赖规则书写顺序而不稳定。 |
| I-30 | 无进展（缺引用/存在环）时，剩余规则必须产出 `Uncertain`、`value=0`、`quality_reason="virtual_point:ERROR"`；具体错因只保存在内部 state，文案固定为「引用点位不可用（未采集 / 已过期 / 存在循环引用）」。 | `gateway-core/src/pipeline/virtual_point.rs:213-229`、`:342-356` | 【部分守护】`gateway-core/src/pipeline/virtual_point.rs` 的 `error_point_has_zero_value_and_fixed_reason` 断言 `value==0`、`raw_value==0`、`quality_reason=="virtual_point:ERROR"`、`raw_type=="virtual"`、`stale_count==0`；`missing_reference_reports_error_point`、`circular_reference_is_detected` 断言 `Uncertain`；内部 state 的固定文案仍无断言 | 现场无法区分「输入没采到」「缓存过期」与「公式配成环」。 |
| I-31 | 派生点只在数值变化、首次求值或错误状态变化时产出一次。 | `gateway-core/src/pipeline/virtual_point.rs:231-264` | 【有守护】`gateway-core/src/pipeline/virtual_point.rs:400-412`、`gateway-core/tests/virtual_point_pipeline.rs:193` | 虚拟点每个周期重复上报，淹没真实变化并放大北向流量。 |
| I-32 | 派生 Good 点必须 `raw_type="virtual"`、`timestamp=source_ts=求值时刻`、`stale_count=0`；派生点在边缘处理之后追加，不经过死区/变化节点。 | `gateway-core/src/pipeline/virtual_point.rs:326-340`、`gateway-core/src/pipeline/mod.rs:320-325` | 【有守护】`gateway-core/tests/virtual_point_pipeline.rs:118-192` | 虚拟点被死区误过滤，或每个周期都重复上行。 |
| I-33 | 虚拟点热重载必须原子替换规则；按 id 保留 `value`/`error` 状态；被删除规则的状态清理；点位缓存不随规则重载重置。 | `gateway-core/src/pipeline/virtual_point.rs:112-122` | 【部分守护】`gateway-core/src/pipeline/virtual_point.rs` 的 `hot_reload_replaces_rules_and_keeps_state` 断言规则集被替换、新公式立即生效；按 id 保留 `value`/`error`、删除规则的状态清理、点位缓存不重置均无断言 | 重载后所有虚拟点重新上报一次；或已删除点位的状态长期残留。 |
| I-34 | 禁用虚拟点不得参与求值、不得产出数据点；其状态 `error` 固定为 `disabled`。 | `gateway-core/src/pipeline/virtual_point.rs:235-246` | 【无（缺口）】 | 已禁用点位仍上行；或静默消失且 Web 端无状态可查。 |

---

## §4 队列入队

| 编号 | 不变量 | 代码依据 | 现有测试 / 守护用例 | 违反后的现场表现 |
|---|---|---|---|---|
| I-35 | Bad/Uncertain/Stale 点必须与 Good 点一样进入 DataQueue，失败点不得被队列层过滤。 | `gateway-core/src/pipeline/mod.rs:294-299`、`:351-353` | 【有守护】端到端落库断言 `gateway-core/src/pipeline/mod.rs` 的 `failed_points_reach_queue`（另有 Edge 层透传断言 `edge/tests.rs:61-71`） | 平台看不到设备离线；现场表现为「整条总线静默、队列却为空」。 |
| I-36 | 队列唯一键必须是 `(point_id, timestamp)`；重复行以 `ON CONFLICT DO NOTHING` 跳过（保留最早 id），不得计入 depth。 | `gateway-core/src/queue/storage/sqlite/db_init.rs:302-315`、`gateway-core/src/queue/storage/sqlite/limit.rs:376-381`、`:470-480` | 【有守护】`gateway-core/src/queue/tests.rs:331-365` | 重发/重放产生重复计费；队列深度指标虚高。 |
| I-37 | 出队必须按 `id` 升序（入库顺序 FIFO），不得按 `timestamp`/`source_ts` 排序；`in_flight=1` 或 `delivered=1` 的行不得被再次选中。 | `gateway-core/src/queue/storage/sqlite/limit.rs:483-499` | 【有守护】`gateway-core/src/queue/tests.rs:23-48`、`:618-650` | 迟到的旧样本覆盖新值；已送达行重复上报。 |
| I-38 | 容量淘汰只选 `in_flight=0` 的行，按 `timestamp ASC, id ASC` 删除；在途（待 ACK）行受保护。 | `gateway-core/src/queue/storage/sqlite/db_init.rs:239-247` | 【部分守护】`gateway-core/src/queue/storage/sqlite/limit/limit_tests.rs` 的 `in_flight_rows_are_never_pruned`、`mixed_rows_pruned_oldest_first` 断言在途行受保护、最旧优先；同一 `timestamp` 下的 `id ASC` 次序仍无断言 | 淘汰把正在发送的行删掉，ACK 失败后再也找不到原行。 |
| I-39 | `enqueue()` 返回 Ok 只代表命令已交给队列 actor，不代表已落盘；SQLite 写失败在 actor 内部回退 fallback，不向调用方返回错误。 | `gateway-core/src/queue/mod.rs:346-355`、`gateway-core/src/queue/core/task.rs:380-453` | 【有守护】`gateway-core/src/queue/tests.rs:1311-1453` | 调用方误以为已持久化；重启后最后一批数据丢失且无错误日志。 |
| I-40 | `persist_failed` 是「持久化失败总数」：队列入队失败（actor 退出 / 通道发送失败）与 SQLite 读写失败都计入同一计数；**成功入队不递增**（决议 A，2026-11-26）。 | `gateway-core/src/pipeline/mod.rs:183-192`、`:371-387`、`gateway-core/src/queue/mod.rs:351-355` | 【有守护】`pipeline::tests::successful_enqueue_does_not_increment_persist_failed` 断言成功入队不递增；`enqueue_failure_increments_persist_failed_counter`、`every_enqueue_path_is_checked` 守护入队通道失败计数；`queue/core/task/tests.rs` 的 `dequeue_db_error_is_reported_not_masked_as_empty_batch`、`release_db_error_increments_failure_metric` 守护 SQLite 读写失败同样计入 | 指标语义已统一；若现场看板需要区分「通道失败 / 落盘失败」两类，再拆独立指标（当前无依赖）。 |
| I-41 | fallback 容量为 5000；超容必须按优先级淘汰：Good(3) > Uncertain(2) > Stale(1) > Bad(0)，同质量下 `stale_count` 越大优先级越低。 | `gateway-core/src/queue/mod.rs:44`、`gateway-core/src/queue/helpers.rs:19-30`、`:47-88` | 【有守护】`gateway-core/src/queue/tests.rs:90-131`、`:1111-1147` | 真正的最新 Good 数据被旧的 Bad 数据挤掉，或反过来丢坏值掩盖故障。 |
| I-42 | 非磁盘满错误反复回写失败的批次最多重试 5 轮，超限整批丢弃并计入 `persist_dropped`；磁盘满/unsent overflow 路径使用哨兵计数，不消耗重试预算。 | `gateway-core/src/queue/mod.rs:46-52`、`:297-304`、`gateway-core/src/queue/core/task.rs:390-452`、`gateway-core/src/queue/helpers.rs:53-58` | 【有守护】`gateway-core/src/queue/tests.rs:133-198`、`:1311-1453` | 病态错误导致写失败→回退→再写的无限反弹，占满 CPU/IO；或磁盘满恢复后数据被误丢。 |
| I-43 | 入队前必须按 10ms / 500 点做 micro-batch 合并；退出信号与定时 tick 都必须 flush 残留数据。 | `gateway-core/src/pipeline/mod.rs:206-235`、`:244-260` | 【部分守护】`gateway-core/src/pipeline/mod.rs` 的 `stage_order_is_edge_then_convert_then_virtual` 以不足 500 点的批次轮询 `queue.len() >= 2`，间接证明 10ms tick 会 flush 残留；500 点满批立即入队、退出信号 flush 均无断言 | 末尾不足 500 点的数据长期滞留内存，进程退出前不落库。 |

---

## §5 北向分发

| 编号 | 不变量 | 代码依据 | 现有测试 / 守护用例 | 违反后的现场表现 |
|---|---|---|---|---|
| I-44 | 每批必须先投递 RealtimeBus（允许丢帧），再投递 NorthFeed（不丢批）；两路共享同一份 `Arc<Vec<DataPoint>>`。 | `gateway-core/src/pipeline/mod.rs:327-349`、`gateway-core/src/north/feed.rs:1-10`、`:76-95` | 【无（缺口）】`gateway-core/src/pipeline/mod.rs` 的用例全部 `realtime_bus: None`；`gateway-core/tests/north_feed_backpressure.rs` 只测 NorthFeed 自身、`gateway-core/src/north/dispatch/tests.rs` 只测 fan-out，bus→feed 投递次序与两路共享同一 `Arc` 均无断言 | 为保 WS 实时性而丢北向数据；或两路各拷一份导致内存翻倍。 |
| I-45 | NorthFeed 容量默认 1024 批；队列满时 `send().await` 必须挂起，不得主动丢批；仅在 dispatcher 已退出（通道关闭）时发送失败，pipeline 只记 warn 并丢弃该批。 | `gateway-core/src/north/feed.rs:21-22`、`:81-95`、`gateway-core/src/pipeline/mod.rs:336-348` | 【有守护】`gateway-core/src/north/feed.rs:159-185`、`gateway-core/tests/north_feed_backpressure.rs:45-109` | 停机窗口的最后一批对 feed 型 connector 静默缺失；MQTT 仍可从队列补，webhook 无队列 replay。 |
| I-46 | MQTT 不得从 NorthFeed 取数，必须直接从 DataQueue 出队发布；`MqttClient::publish_batch` 必须是幂等空实现。 | `gateway-core/src/north/mqtt/mod.rs:831-845`、`gateway-core/src/north/mqtt/publish.rs:159-166` | 【有守护】`gateway-core/tests/north_single_delivery.rs:123-198` | 同一批点在 `data_queue` 出现两行、broker 收到两条 PUBLISH，形成系统性重复上报。 |
| I-47 | 所有点位必须发布到同一 `config.topic`，且 `retain=false`；默认 payload 按 `device_id` 分组，组内点序等于出队顺序，组间顺序不保证；自定义模板模式为每点一行 JSON Lines。 | `gateway-core/src/north/mqtt/publish.rs:199-206`、`:240-243`、`gateway-core/src/north/mqtt/payload.rs:41-57`、`:184-215` | 【有守护】`gateway-core/src/north/mqtt/payload/tests.rs:66-121` | 消费方按设备订阅时收不到数据；或按报文中设备顺序做差分会误判抖动。 |
| I-48 | 报文顶层 `ts` 是构建时刻（`DataPoint::now()`），采样时刻只在每点的 `source_ts`；顶层 `ts` 不得被当作采样时刻。 | `gateway-core/src/north/mqtt/payload.rs:193-213` | 【无（缺口）】 | 平台按顶层 `ts` 对齐时，把积压补发的历史数据当成「当前值」，曲线出现平台期。 |
| I-49 | MQTT 上行固定 QoS1（AtLeastOnce）：`qos` 配置项只允许 1，0/2 在配置校验阶段直接拒绝（`invalid_qos`），发布与 outbox 记录的 `qos` 恒为 1。 | `gateway-core/src/north/mqtt/publish.rs:199-206`、`:240-243`、`gateway-core/src/config/validate/field.rs:64-76`、`gateway-core/src/config/mqtt.rs:69-74` | 【有守护】`gateway-core/src/config/mqtt.rs:320`（`roi23_qos_only_accepts_1`） | 配置 `qos=0/2` 不再被静默按 1 发布，而是在写入时明确报错，避免按 QoS2 设计幂等链路的语义偏差。 |
| I-50 | snapshot 模式下每个上报周期每个 `(device_id, point_id)` 只出队最新一行（按 `MAX(id)`，不按 `source_ts`）；`cutoff` 必须排除本周期排空期间新到的样本；stream 模式逐行按 id 顺序出队。 | `gateway-core/src/north/mqtt/publish.rs:159-166`、`gateway-core/src/queue/storage/sqlite/limit.rs:549-579`、`gateway-core/src/config/mqtt.rs:129-142` | 【有守护】`gateway-core/src/queue/tests.rs:1493-1560` | 同一周期同一点上行两次；或迟到的旧样本覆盖新值。 |
| I-51 | NorthDispatcher 必须对启用 connector 并发 fan-out；单个 connector 失败只记 warn，不得阻塞或影响其它 connector；停机时必须先 drain 已积压批次再退出。 | `gateway-core/src/north/dispatch.rs:99-131`、`:139-158` | 【有守护】`gateway-core/src/north/dispatch/tests.rs:35-84`、`:126-165` | 慢 connector 拖死快 connector；停机丢最后积压批次。 |

---

## §6 跨阶段不变量

| 编号 | 不变量 | 代码依据 | 现有测试 / 守护用例 | 违反后的现场表现 |
|---|---|---|---|---|
| I-52 | 阶段顺序必须固定为：采集解码/换算 → 边缘（聚合/死区/变化）→ convert 模板 → 质量规则 → 虚拟点 → 广播/NorthFeed → 入队 → dispatcher → connector；MQTT 另从 DataQueue 出队。 | `gateway-core/src/pipeline/mod.rs:288-353` | 【有守护】`gateway-core/src/pipeline/mod.rs` 的 `stage_order_is_edge_then_convert_then_virtual`（用死区×换算的敏感场景锁定「边缘→换算→虚拟点」；质量规则段仍未单独断言） | 任一阶段换序都会改变「哪些数据被过滤、虚拟点看到什么值」，且现象分散、难定位。 |
| I-53 | 非 Good 点不得进入边缘节点、convert、质量规则、虚拟点缓存；必须原样透传到广播/队列/北向。 | `gateway-core/src/pipeline/mod.rs:294-299`、`gateway-core/src/pipeline/edge.rs:128-130`、`gateway-core/src/pipeline/convert.rs:193-196`、`gateway-core/src/pipeline/quality_rules.rs:69-71`、`gateway-core/src/pipeline/virtual_point.rs:142-145` | 【有守护】`gateway-core/src/pipeline/edge/tests.rs:61-71`、`gateway-core/src/pipeline/convert.rs:458-473`、`gateway-core/src/pipeline/quality_rules.rs:277-295` | 故障样本被换算或质量规则「洗白」，平台丢失真实故障原因。 |
| I-54 | 上游已判定的非 Good 质量与原因不得被后续阶段覆盖；convert 与质量规则只允许修改 Good 点。 | `gateway-core/src/pipeline/convert.rs:193-196`、`gateway-core/src/pipeline/quality_rules.rs:68-71` | 【有守护】`gateway-core/src/pipeline/convert.rs:458-473`、`gateway-core/src/pipeline/quality_rules.rs:277-295` | 第一个失败原因被第二个规则覆盖，现场按 reason 排查时指向错误模块。 |
| I-55 | `value_text` 必须由 pipeline 的 convert 在入队前固化；北向优先读持久化值，缺失时才按当前规则回查，保证报文标签与历史查询同源。 | `gateway-core/src/pipeline/mod.rs:312`、`gateway-core/src/north/mqtt/payload.rs:105-127`、`:196-207` | 【有守护】`gateway-core/src/north/mqtt/payload/tests.rs:247-330` | 历史查询显示「运行」，MQTT 报文显示「故障」（或反之），同一时刻两种标签。 |
| I-56 | 时间语义必须一致：`timestamp`=上送/产出时刻；`source_ts`=原始采集时刻；聚合与虚拟点 `source_ts=timestamp`；失败点（`Err` 与 `Ok+Bad` 两条路径）`source_ts` 均为 last_good 时刻，无成功历史时为 0 / `none`。 | `gateway-core/src/engine/model.rs:293-295`、`gateway-core/src/pipeline/nodes.rs:28-43`、`gateway-core/src/pipeline/virtual_point.rs:326-340`、`gateway-core/src/south/scheduler/results.rs:26-54` | 【有守护】`gateway-core/src/south/scheduler/tests.rs:834-911`、`:915-977`（两条失败路径 + 上报队列） | 平台把补发数据当作实时数据，或把 1970 年时间戳渲染成异常曲线。 |
| I-57 | 一条采集批在「管道 → DataQueue → MQTT」链路上必须恰好投递一次，不得因 connector 二次入队而重复。 | `gateway-core/src/north/mqtt/mod.rs:831-845` | 【有守护】`gateway-core/tests/north_single_delivery.rs:123-198` | 同一采样重复计费/重复入库，历史曲线出现重复点。 |
| I-58 | 采集失败或非 Good 的批次必须至少有一项可观测计数（`collect_failures` / `pipeline_dropped` / `persist_dropped`），不得静默消失。 | `gateway-core/src/south/scheduler/spawning.rs:216-235`、`gateway-core/src/pipeline/mod.rs:161-181`、`gateway-core/src/queue/mod.rs:306-321` | 【有守护】`gateway-core/src/south/scheduler/tests.rs:140-210` | 全链路「看起来正常」但北向无数据，排障只能靠逐层加日志。 |

---

## 缺口汇总（后续补测试清单）

以下编号为【无（缺口）】条目，即当前只靠代码约定维持、没有直接自动化断言的部分。按 §1–§6 顺序汇总，便于后续逐条补测试：

| 阶段 | 缺口编号 | 缺口摘要 |
|---|---|---|
| §1 采集与数据点 | I-02 | 无断言 `raw_value`/`raw_type` 与协议类型的对应关系 |
| §1 采集与数据点 | I-04 | 无断言 `good()` 的 `timestamp == source_ts`、`stale_count == 0` |
| §1 采集与数据点 | I-05 | 无断言协议构造器两次 `now()` 允许的时间差 |
| §1 采集与数据点 | I-10 | 已补 `<场景>:<码>` 契约测试与编码表（ROI-27 第二批） |
| §2 换算与质量 | I-12 | 无公式失败回退 `scale*raw+offset` 的测试 |
| §2 换算与质量 | I-19 | 无 rate 上值表容量/TTL 清理测试 |
| §2 换算与质量 | I-20 | 无「质量规则不得产出 Stale」测试 |
| §2 换算与质量 | I-22 | 已补「死区作用于 convert 之前」断言（ROI-27 第一批，与 I-52 同一用例） |
| §3 虚拟点 | I-30 | 无「虚拟点失败具体原因」断言 |
| §3 虚拟点 | I-33 | 无 error 状态保留、删除规则状态清理、缓存不重置的测试 |
| §3 虚拟点 | I-34 | 无禁用虚拟点不产出、状态为 disabled 的测试 |
| §4 队列入队 | I-35 | 已补端到端「失败点也落库」测试（ROI-27 第一批） |
| §4 队列入队 | I-40 | 已补（决议 A，2026-11-26）：`persist_failed` 统一为「持久化失败总数」，新增「成功入队不递增」断言 |
| §4 队列入队 | I-43 | 无 micro-batch 残留 flush 的测试 |
| §5 北向分发 | I-48 | 无「顶层 ts=构建时刻」契约测试 |
| §6 跨阶段 | I-52 | 已补「边缘→换算→虚拟点」顺序断言（ROI-27 第一批）；质量规则段仍待补 |

最值得优先补测的三条：

1. I-10 → 已补（ROI-27 第二批）：原因统一为 `<场景>:<码>` 并加源码字面量契约测试；新增原因必须遵守编码表。
2. I-52 → 已补（ROI-27 第一批）；质量规则段仍可单独加断言。
3. I-35 → 已补（ROI-27 第一批）；I-40 → 已补（决议 A，2026-11-26）；剩余可补 I-43（micro-batch 残留 flush）。
