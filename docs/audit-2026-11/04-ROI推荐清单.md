# ROI 推荐清单（2026-11 功能摸底新增项）

> **来源**：[audit-2026-11/02-技术债务清单.md](02-技术债务清单.md) 的 63 条新发现债务，按 ROI 体例重排为可排期的清单。
> **编号**：接续既有 [remaining-roi-backlog-2026-11.md](../remaining-roi-backlog-2026-11.md) 的 ROI-01~27，本清单从 **ROI-28** 起，避免冲突。
> **口径**：只排「**优化补全现有功能**」（正确性 / 横切语义 / 失效可见性 / 门禁可信度），**不涉及新增功能面**（双机热备 / 云平台 / 数据分析平台仍冻结）。
> **体例**：沿用既有清单 —— 每项写清「现在的毛病 / 现场会看到什么 / 工作量 / 价值 / 依赖」，末尾标出对应的 DD 编号。
> **执行状态**：摸底阶段只读；第 1~7 批（ROI-28~34、ROI-36~42、ROI-45~49）已实施并通过门禁，见上方「实施状态」。

---

## 实施状态

| 批次 | ROI | 状态 | 验证证据 |
|---|---|---|---|
| 第 1 批 | ROI-28 / ROI-29 / ROI-30 | ✅ **已完成** | `cargo test -p gateway-core --test chaos` → 45 passed / 0 failed；`npm run type-check` → 通过；`cargo clippy --workspace --all-targets -- -D warnings` → 通过；`cargo fmt --check` → 通过；`npm run test` → 253 files 全通过 |
| 第 2 批 | ROI-31 / ROI-32 / ROI-33 | ✅ **已完成** | `cargo test --workspace` → 全部 ok（gateway-core lib 1815 passed / 0 failed，chaos 45 passed）；`cargo clippy --workspace --all-targets -- -D warnings` → 通过；`cargo fmt --check` → 通过；`npm run type-check` → 通过；`npm run test` → 253 files / 2420 tests 全通过 |
| 第 3 批 | ROI-34 / ROI-40 / ROI-41 / ROI-42 | ✅ **已完成** | `cargo test --workspace` → 全部 ok（chaos 45 passed / 0 failed）；`cargo clippy --workspace --all-targets -- -D warnings` → 通过；`cargo fmt --check` → 通过；`npm run type-check` → 通过；`npm run test` → 253 files / 2424 tests 全通过 |
| 第 4 批 | ROI-36 / ROI-37 / ROI-38 / ROI-39 | ✅ **已完成** | `cargo test --workspace` → 全部 ok（chaos 45 passed / 0 failed）；`cargo clippy --workspace --all-targets -- -D warnings` → 通过；`cargo fmt --check` → 通过；`npm run type-check` → 通过；`npm run test` → 253 files / 2424 tests 全通过 |
| 第 5 批 | ROI-48 / ROI-49（护栏专项） | ✅ **已完成** | `cargo test --workspace` → 全部 ok（chaos 45 passed / 0 failed）；`cargo clippy --workspace --all-targets -- -D warnings` → 通过；`cargo fmt --check` → 通过；`npm run lint` → 通过；`npm run type-check` → 通过；`npm run test` → 253 files / 2424 tests 全通过；`scripts/deploy_lint.ps1` → 通过 180 项 / 失败 0 项；接口覆盖 176/176 |
| 第 6 批 | ROI-46 / ROI-47（历史查询口径 + 北向 Modbus） | ✅ **已完成** | `cargo test --workspace` → 全部 ok（chaos 45 passed / 0 failed；新增 `stored_counts` 2 例、FC16 边界 1 例）；`cargo clippy --workspace --all-targets -- -D warnings` → 通过；`cargo fmt --check` → 通过；`npm run lint` → 通过；`npm run type-check` → 通过；`npm run test` → **256 files / 2436 tests** 全通过；`scripts/deploy_lint.ps1` → 通过 180 项 / 失败 0 项；接口覆盖 176/176；`gen_api_surface.py --check` → 与源码一致 |
| 第 7 批 | ROI-45（锁序规范 + 缓存淘汰） | ✅ **已完成** | `cargo test --workspace` → 全部 ok（gateway-core lib **1822 passed** / 0 failed；chaos 45 passed / 0 failed；新增 prune 3 例）；`cargo clippy --workspace --all-targets -- -D warnings` → 通过；`cargo fmt --check` → 通过；`npm run lint` → 通过；`npm run type-check` → 通过；`npm run test` → **256 files / 2436 tests** 全通过；`scripts/deploy_lint.ps1` → 通过 180 项 / 失败 0 项；接口覆盖 176/176；`gen_api_surface.py --check` → 与源码一致 |

**第 1 批实际改动（5 个文件）**

| 文件 | 改动 |
|---|---|
| `gateway-core/tests/chaos/harness.rs` | 桩回帧的 unit_id 由「功能码 `pdu.first()`」改为「`header[6]` 请求从站地址」 |
| `gateway-core/tests/chaos/scenarios.rs` | 串口截断回显断言 `NoReply` → `WriteUncertain`；BACnet WriteProperty Abort 断言 `InvalidFrame` → `DeviceRejected` |
| `gateway-core/src/config/point.rs` | `timeout_ms` 补 `skip_serializing_if` + `#[ts(optional)]`（保留 `#[ts(type = "number")]`），与同结构 `write_min`/`write_max` 及仓库既有惯例一致 |
| `gateway-web/src/api/generated/PointConfig.ts` | 重生成：`timeout_ms: number` → `timeout_ms?: number`（前端 4 处 TS2322/TS2741 随之消除，未改业务代码） |
| `gateway-core/src/runtime/gateway.rs` | 串口独占重建：先把 `Arc` 从锁中取出并释放 guard，再 `await` 关闭旧 task |

**第 2 批实际改动**

| 文件 | 改动 |
|---|---|
| `gateway-web/src/utils/configDefaults.ts` | 顶层已知键清单改为 `as const satisfies readonly (keyof GatewayConfig)[]` + `_AssertNoMissingTopKey` 编译期断言（以后加节漏登记直接编译失败）；补入 `convert_rules` |
| `gateway-web/src/composables/config/useConfigJson.spec.ts` | 新增回归用例：`convert_rules` 不再被误报为未知键，整段导入后原样保留 |
| `gateway-core/src/engine/metrics/queue_persist.rs` | 新增 `gateway_queue_fallback_depth` / `gateway_queue_degraded` 两个 gauge 并接入 render |
| `gateway-core/src/queue/mod.rs` | 降级入队、flush 成功/失败、`set_disk_ok` 各路径同步两个 gauge（降级不再「接口 200 但数据只在内存」） |
| `gateway-core/src/queue/core/task.rs` | `disk_full` / `disk_full_retry_at` 真正被消费：退避期内 `Enqueue` 直接进 fallback 且不再撞存储；tick 冲刷成功解除暂停、失败重新退避；同步 gauge |
| `gateway-core/src/engine/metrics/mqtt.rs` | 新增 `gateway_mqtt_template_missing_vars_total` / `gateway_mqtt_payload_empty_lines_dropped_total` 两个 counter 并接入 render |
| `gateway-core/src/engine/metrics/counter.rs` | 对应 `inc_mqtt_template_missing_vars` / `inc_mqtt_payload_empty_line_dropped` 写入方法 |
| `gateway-core/src/north/mqtt/payload.rs` | 缺变量改为按行计数 + `error!`（日志仍只打一次防刷屏）；空串丢行改为计数 + `error!`，不再静默 `continue` |
| `gateway-core/src/north/webhook/client.rs` | `persist_batch` / `persist_one_chunk` 返回 `GatewayResult<()>`；序列化 / 落库 / Join 失败均计 `webhook_data_lost` 并向上返回（dispatch 按 connector 告警） |
| `gateway-core/src/queue/tests.rs`、`queue/core/task/tests.rs`、`engine/metrics/tests.rs` | 新增 DD-17 / DD-18 / DD-23 回归用例（gauge 生命周期、退避真暂停写库、模板丢数据计数可导出） |

**第 3 批实际改动**

| 文件 | 改动 |
|---|---|
| `gateway-web/src/utils/frameParser.ts` | CJT188 帧结构对齐后端 `68 + ADDR(7) + C + L + DATA + CS + 16`（删除多算的 1 字节「仪表类型」）；长度/校验和/帧尾任一不符即 `valid=false`；样例帧按新口径重建 |
| `gateway-web/src/utils/frameParserError.spec.ts` | 用例帧同步新口径；新增 DD-01 回归（非对称地址锁字段偏移、坏校验和不得显示「通过」） |
| `gateway-api/.../register_scan/scan.rs` | 失败/中止时不再把 `progress` 置 100，保留已扫比例（clamp 99） |
| `gateway-api/.../auto_detect/detect.rs`、`auto_detect/tests.rs` | 同一规则；中止断言由 `progress==100` 改为 `0 < progress < 100` |
| `gateway-web/src/composables/acquisition/useRegisterScan.ts` | 轮询以「非 running」为终态立即收尾；failed/aborted 给出已扫帧数提示；历史状态区分完成/进行中/失败/已中止 |
| `gateway-web/src/views/acquisition/scan/RegisterScan.vue` | 结果卡状态标签与进度条区分 done/failed/aborted，不再一律显示「扫描中」 |
| `gateway-api/.../packet_replay/replay_run.rs`、`engine.rs` | `ReplayExecuteResult` 新增 `terminal` / `planned`；阻塞模式中止/失败也如实返回终态与计划帧数 |
| `gateway-web/src/api/protocolTools/types.ts` | View 类型与转换同步 `terminal` / `planned` |
| `gateway-web/src/composables/acquisition/usePacketReplay.ts` | 删除本地 `summarizeFrames`；以服务端终态 / aborted / failed / 用户停止为准，非 completed 不再伪造「回放完成」 |
| `gateway-web/src/views/acquisition/packet/components/ReplayRunStatus.vue`、`PacketReplay.vue`、`PacketReplay.spec.ts` | 非 completed 显示「未跑完 / 已停止」告警与「已执行 N / M 帧」；新增流式/阻塞中止回归用例 |
| `gateway-core/src/queue/storage/sqlite/wal.rs` | DD-19：WAL size gauge 在生产路径回写（低于阈值也写、文件不存在归零），不再只在单测赋值 |
| `gateway-api/src/web/api/system/handlers/watchdog.rs` | DD-37：`ServiceInfo.restarts` 拆为 `panic_count` / `publish_fail_count` / `collect_fail_count` / `heartbeat_stall_count` 四个具名字段 |
| `gateway-web/src/api/watchdog.ts` | DD-39：改用生成的 `ServiceInfo` 类型，API 边界把 bigint 字段转 number，删除手写接口与 `.restarts` |
| `gateway-web/src/views/system/maintenance/Watchdog.vue`、`views/ops/delivery/selfCheckEvals.ts`、对应 spec | 列名与自检汇总同步四个异常计数（不再把心跳停顿算成队列重启） |
| `gateway-core/src/platform/hardware_watchdog.rs`、`engine/metrics/prometheus.rs`、`lifecycle.rs` | DD-38：导出 `gateway_watchdog_hw_write_errors_total`（此前只写不读）；`gateway_watchdog_timeouts_total` HELP 澄清为心跳停顿 |
| `gateway-api/src/web/api/realtime/handlers/limit.rs`、`handlers/mod.rs` | DD-62：日志 WS 独立连接上限；读写分离 + 有界发送缓冲 + 发送超时；Lagged / 慢客户端丢行计指标并节流告警 |
| `gateway-core/src/engine/metrics/{mod,counter,prometheus,tests}.rs` | 新增 `gateway_log_ws_dropped_total` 计数与导出断言 |
| `gateway-api/src/web/middleware/auth.rs`、`auth/tests.rs` | DD-63：死信 list/replay/purge 写操作归 `system:write`，可授予的 `config:write` 不再能重投/清理 |

**第 4 批实际改动**

| 文件 | 改动 |
|---|---|
| `gateway-core/src/south/scheduler/mod.rs` | DD-07：`point_config_equal` 由字段白名单改为整结构 `a == b`（旧白名单漏掉 timeout/CT/PT/告警/可写区间/device_id/protocol，改这些配置热加载判不出差异）；DD-08：`start()` 分组键由 `by_bus` 改为 `by_group`，与 reload 的 `protocol:bus` 口径一致 |
| `gateway-core/src/south/scheduler/spawning.rs`、`reload.rs` | DD-08：`spawn_collector_task` 显式接收分组键与总线名；reload 走同一分组口径，避免「启动按总线、热加载按协议」两套分组导致重复/漏采 |
| `gateway-core/src/south/scheduler/tests.rs` | DD-07 回归：`point_config_equal_covers_all_fields` 覆盖 protocol/device_id/timeout_ms/ct_ratio/pt_ratio/writable/write_min/write_max/alarm_high/alarm_low |
| `gateway-core/src/south/bus/tcp_task.rs`、`tcp_task/tests.rs` | DD-09：重连退避在「连接失败」时也推进计数，已连过但本轮通信失败才归零；消除「假在线设备退避永远停在 1 秒」的忙等 |
| `gateway-core/src/pipeline/convert.rs` | DD-15：`RuleSet::compile` 与 `RuleCompiled::compile` 返回逐条错误，`ConvertEngine` 保存 `issues` 并暴露 `compile_issues()`（转换规则编译失败不再静默丢失） |
| `gateway-core/src/runtime/gateway.rs` | 转发 `convert_issues()` 供 API 层读取 |
| `gateway-api/src/web/traits.rs`、`tests/common/mock_state.rs` | `GatewayRuntimeTrait` 新增 `convert_issues`；`SchedulerTrait` 新增 `merge_gap` 默认实现（Modbus 取运行时自适应 gap，其它协议回退 5） |
| `gateway-api/src/web/services.rs` | DD-41：`install_quality_rules` 返回 `Result`，安装失败不再吞错；DD-11：新增 `MergeGroupDto` / `merge_plan` 桥接 `gateway_core` 的合并算法（不违反 api 层禁用 south 的架构测试） |
| `gateway-api/src/web/api/quality/handlers/mod.rs` | DD-41：新增/更新/删除质量规则后若重装采集管道失败，接口返回明确错误（「规则已保存，但当前未生效」），不再 200 假成功 |
| `gateway-api/src/web/api/protocol_tools/handlers/protocol_convert.rs` | DD-15：列表与创建回包带 `error`，运行期编译失败的规则前端可直接看到原因 |
| `gateway-api/src/web/api/diagnostics/handlers/stats.rs` | DD-12：`DeviceStat` 增加 `loss_rate` / `current_period_ms` / `suggested_period_ms` / `health` / `suggestion`，由后端按成功率与配置周期统一计算 |
| `gateway-api/src/web/api/diagnostics/handlers/schedule_optimize.rs`（新增）、`handlers/mod.rs`、`routes.rs` | DD-11：新增只读端点 `GET /api/diagnostics/schedule-optimize`，复用 `merge_batches` 与运行时自适应 gap，返回按从站/功能码分组的真实合并计划 |
| `gateway-core/src/south/protocol/mod.rs`、`modbus/{trait_impl,batch,mod}.rs` | `Collector::merge_gap` 默认实现 + Modbus 读取自适应 gap；新增 `MergeGroupInfo` / `merge_plan`（按从站、功能码、超时分组的真实合并结果） |
| `gateway-api/src/web/types.rs` | DD-22：新增 `export_ts_types_web_defaults`，从 serde 默认值生成 `configDefaults.generated.ts`（Web/MQTT/Sim/Queue/总线退避默认值同源） |
| `gateway-api/src/web/api/system/handlers/config.rs` | DD-21：配置差异由手抄小节改为序列化后比较全部顶层键（新增/删除小节也能报出），附单测 |
| `gateway-web/src/utils/configDefaults.ts`、`backoffPresets.ts`、`api/generated/configDefaults.generated.ts` | DD-22：默认值改为 import 后端生成常量；退避预设由 `DEFAULT_MQTT.reconnect_backoff` 派生，标签按实际秒数/抖动生成 |
| `gateway-web/src/views/system/config/tabs/BusConfigTab.vue` | DD-10：总线退避预设回落后端默认值，不再前端硬编码 |
| `gateway-web/src/composables/config/useConfigJson.ts`、`useConfigJson.spec.ts` | DD-50：JSON 反向同步改为对整份 `config` 深度 watch，新增小节自动纳入，不再逐节手写 watch 清单 |
| `gateway-web/src/api/diagnostics/{types,convert,raw}.ts`、`api/generated/{DeviceStat,index}.ts` | DD-12：前端只做字段映射，健康度/建议周期直接读后端字段（周期 bigint → number） |
| `gateway-web/src/api/generated/{ScheduleOptimizeDevice,ScheduleOptimizeGroup,ScheduleOptimizeReport}.ts` | DD-11：新端点类型生成与 barrel 导出 |
| `gateway-web/src/views/acquisition/ScheduleOptimize.vue`、`CommProfile.vue` | DD-11/DD-12：删除前端自算合并与健康度逻辑，改读后端报告；缺失周期显示「—」 |
| `gateway-web/src/api/protocolTools/{types,convertRules}.ts`、`convertRules.spec.ts`、`views/acquisition/simulate/ProtocolConvert.vue` | DD-15：转换规则视图带 `error`；列表状态列对不生效规则显示红色「不生效」及原因 tooltip |
| `gateway-web/src/api/generated/ConvertRule.ts` | 重新生成，补 `error: string \| null` |

**第 4 批实施说明（如实记录）**

- DD-14（转换规则全局静态表 → 运行时实例）本批**未做结构性重构**：保留现有原子替换语义，只把「编译失败」变成可通过 `compile_issues()` / 列表 `error` 字段看到的可见失败。原因：重构涉及 `ConvertEngine` 生命周期与配置热加载路径，超出「只补全现有功能、稳定性第一」的本批边界，留待后续单独立项。
- DD-10/11/12 的「前端别自己算」已收口：诊断健康度、建议周期、合并计划均以后端返回为准；前端仅保留展示层格式化。

**第 5 批实际改动（ROI-48 注释文档对齐 + ROI-49 护栏专项）**

| 文件 | 改动 |
|---|---|
| `gateway-core/src/south/protocol/registry/capabilities.rs` | DD-59：订正 `telecontrol_tcp_write` 的过期注释（原文写「mc / fins 尚未实现 diag，不能把 tcp_batch_write 的 diag 改为 true」，实际 mc / fins 早已实现 diag——诊断 = 读一个字元件——且 `tcp_batch_write` 的 `supports_diag` 已是 true）；`tcp_batch_write` 文档改为「批量读 + 写 + 寄存器扫描 + 真实诊断」，并写清两者能力差异只剩 `supports_register_scan` |
| `gateway-core/src/pipeline/mod.rs` | DD-60：文件头与 `ProcessingPipeline::process_batch` 的步骤顺序由「入队 → 广播」订正为「广播 → 入队」，并补一段说明：广播在 `process_batch` 内逐批发出，入队在 `run()` 里攒批落库，两者并列投递同一份数据，不存在「入队成功才算广播成功」的依赖 |
| `gateway-api/src/web/api/system/handlers/ota_ntp.rs`（新建）、`system/handlers/mod.rs` | DD-44：OTA（`/api/ota/upload|status|rollback`）与 NTP（`/api/ntp/sync|status`）handler 由 `devices/handlers/discovery.rs` **按调用方归位**到系统域（它们操作的是网关自身，不是设备），并把 `NtpStatusResponse` 及其单测一并迁入 |
| `gateway-api/src/web/api/devices/handlers/discovery.rs` | DD-44：移出 OTA/NTP，只留设备域真正的能力（IO 控制 / IO 状态），文件头补归位说明 |
| `gateway-api/src/web/api/devices/handlers/mod.rs`、`devices/routes.rs` | DD-44：订正过期的路由清单（原文仍列 `/api/devices/ota`、`/api/devices/ntp/*`——这些路径从来不存在，实际是 `/api/ota/*`、`/api/ntp/*`） |
| `gateway-api/src/web/api/devices/handlers/control.rs` | DD-55：孤儿端点 `POST /api/devices/control/send` **标注废弃**（不删，删即破坏外部脚本契约）：响应统一带 `Deprecation: true` 与 `Warning: 299 - "…改用 POST /api/point-tools/write"` 头；写业务仍走统一核心 `points::write_points_core`，失败码语义不变 |
| `gateway-api/tests/protocol_tools_write_api.rs` | DD-57：补 `POST /api/protocol-tools/iec104/ioa-read` 用例——覆盖缺 bus / ioas 为空 / 总线不存在 / 协议不匹配四个分支，全部在触碰网络前返回，断言 HTTP 状态码与机器可读 `error` 分类 |
| `gateway-api/tests/read_api_coverage.rs` | 补第 4 批新增端点 `GET /api/diagnostics/schedule-optimize` 的用例（合并计划由后端算，断言 `original_reads` / `merged_reads` / 分组数与总览自洽）；接口覆盖由 175/175 变为 **176/176** |
| `scripts/line_budget_frozen.txt` | DD-58：账本头写明**前端与后端共用同一份账本、同一套规则**（前端条目写 `gateway-web/src/...`）；补登记两个基线建立后涨过 800 行却漏登记的存量巨型文件（`gateway-core/src/runtime/gateway.rs` 上限 1112、`gateway-core/src/south/bus/serial_task.rs` 上限 905）——补登记后它们同样受硬上限约束，不得再悄悄长 |
| `scripts/deploy_lint.ps1` | DD-58：C24 段由「只扫三个后端 crate」扩到**同时扫描 `gateway-web/src`**（排除 `src/api/generated/`、`src/utils/ep-icons-compact.ts`、`*.spec.ts` / `*.test.ts`），与后端同一账本、同一上限规则 |
| `gateway-web/eslint.config.js` | DD-58：删掉写死的 `LINE_BUDGET_EXEMPT` 清单，改为**读取同一份 `scripts/line_budget_frozen.txt`**：未登记文件 > 800 行 → warning（CI `--max-warnings 0` 即阻断）；已登记文件按账本里的**单文件硬上限**判定，超出 → error |
| `gateway-core/src/engine/metrics/scoped.rs`、`scoped/error_codes.rs`（新建） | 拆出错误码与失败原因分桶（`CollectErrorCode` / `REASON_LABELS` / `ReasonCounters`），主文件 532 → 383 行（< 500，恢复 C23 的拆分守护）；对外路径用 `pub use` 原样再导出，引用方无需改动 |
| `docs/architecture/frontend.md` | 同步前端行数预算的实际口径（共用账本 + 仓库级 C24 也扫前端），与代码一致 |

**第 5 批实施说明（如实记录）**

- 本批只做 ROI-48 / ROI-49（用户指定「小投入护栏项」）。ROI-35（写路径统一）、ROI-43（OTA 验签）、ROI-44~47（调试台边界 / 锁序 / 历史口径 / 北向 Modbus 写语义）**未做**，仍在排期表里；DD-14（转换规则全局静态表 → 运行时装配持有）按用户决定**另记为独立待办**，本批不做。
- 仓库级门禁 `scripts/deploy_lint.ps1` 在本批开始前就是**红的**，两处失败均与本批改动无关、且在 HEAD 上即已存在：
  ① C23 `gateway-core/src/engine/metrics/scoped.rs` 532 行（≥500）——本批拆分修复；
  ② C24 `gateway-core/src/runtime/gateway.rs`（1004 行）与 `gateway-core/src/south/bus/serial_task.rs`（805 行）在基线建立后涨过 800 行却漏登记——本批按账本规则补登记并给硬上限。
  两个巨型文件**真正的拆分**未做（属重构，不在「护栏专项」范围），已记入剩余批次。
- 本批顺带修正了第 4 批遗留的一处不一致：`docs/api-surface.md` 未随新增端点 `/api/diagnostics/schedule-optimize` 重新生成，导致 `gen_api_surface.py --check` 报「清单与路由源码不一致」；已重新生成（176 个端点）并通过校验。

**第 6 批实际改动（ROI-46 历史查询口径 + ROI-47 北向 Modbus）**

| 文件 | 改动 |
|---|---|
| `gateway-api/src/web/types.rs` | DD-51：`QueueThresholds` 新增 `evict`（容量淘汰线，`#[serde(default)]` 兼容旧客户端）；注释写明它不是背压阈值 100000 |
| `gateway-api/src/web/api/system/handlers/status.rs` | DD-51：`evict = capacity_pressure_rows(max_queue_rows)`（与存储层删除口径同源，即 max_rows 的 90%） |
| `gateway-api/src/web/api/diagnostics/handlers/stats.rs` | DD-52：`DeviceStat.stored_points` + `StatsSummary.total_stored_points`；`generate_report` 接收按点位统计的落库样本数并汇总；handler 调队列 `stored_counts`，失败 warn 后降级为 0（不估算、不阻断报表） |
| `gateway-api/src/web/traits.rs` | DD-52：`DataQueueTrait::stored_counts`（默认空表）+ `StoredCountsResult` 别名，供 mock/内存后端编译与降级 |
| `gateway-core/src/queue/storage/mod.rs` | DD-52：`QueueStorage::stored_counts` 默认实现（空表），写明「过滤发生在入库前，计数即真实压缩结果」 |
| `gateway-core/src/queue/storage/sqlite/sqlite_storage.rs` | DD-52：SQLite 实现——原始表 `COUNT(*)` + 聚合表 `SUM(sample_count)` 按 point_id 合并，覆盖超 24h 被压成分钟桶的长窗口 |
| `gateway-core/src/queue/storage/sqlite/sqlite_storage/stored_counts_tests.rs`（新建） | DD-52：两例单测（原始+聚合相加、窗口闭区间过滤） |
| `gateway-core/src/queue/core/task.rs`、`queue/core/task/diag_handlers.rs`（新建） | DD-52：`QueueCommand::StoredCounts`；把 `OldestTs`/`StoredCounts` 的 spawn_blocking 细节外移到 `diag_handlers.rs`，本文件 944 → 939 行，守住 944 行上限 |
| `gateway-core/src/queue/stored_counts_api.rs`（新建）、`queue/mod.rs` | DD-52：`DataQueue::stored_counts` 外移（mod.rs 805 → 782 行，回到 800 行以内） |
| `gateway-web/src/api/generated/{QueueThresholds,DeviceStat,StatsSummary}.ts` | 由 ts-rs 重新导出（`evict` / `stored_points` / `total_stored_points`） |
| `gateway-web/src/api/diagnostics/types.ts`、`convert.ts`、`convert.spec.ts` | DD-52：视图模型补 `storedPoints` / `totalStoredPoints` 并做字段映射 |
| `gateway-web/src/views/data/pipeline/DataBuffering.vue`、`DataBuffering.spec.ts`（新建） | DD-51：删除写死的 100000；占比/文案/超线提示全部用后端 `queue_thresholds.evict`，未下发显示「—」；3 条回归用例 |
| `gateway-web/src/views/data/pipeline/DataCompression.vue`、`DataCompression.spec.ts`（新建） | DD-52：删除魔数 0.6/0.5 与「按配置比例编造存储量」；采集成功点 = `dataPoints`，实际存储点 = 后端 `storedPoints`，压缩率 = 1 − 存储/采集成功；按设备图表同样用后端字段；2 条回归用例 |
| `gateway-web/src/views/data/pipeline/DataCompare.vue`、`DataCompare.spec.ts` | DD-53：消费 `/api/history` 的 `truncated` / `has_more` / `total`，取截断最严重的点位显示「数据已截断：窗口内共 N 条，仅显示最近 M 条」；补 2 条用例（截断/未截断） |
| `gateway-web/src/utils/aggregateHistory.ts`、`aggregateHistory.spec.ts`（新建） | DD-54：小时/天二次聚合改为按 `sample_count` 加权（均值/方差），极值优先用后端桶内 `min_value`/`max_value`；补 5 条加权用例 |
| `gateway-web/src/composables/data/useHistoryQuery.ts` | DD-54：历史点类型带上 `sample_count`/极值；统计摘要在小时间粒度以上改用加权口径（与图表同源） |
| `gateway-core/src/north/server/modbus_server/handlers.rs` | DD-43：FC16 白名单校验与写入由 `wrapping_add` 统一为 `saturating_add`，与 FC01~FC04 读路径边界语义一致（越界饱和到 0xFFFF，不回绕到 0） |
| `gateway-core/src/north/server/tests.rs` | DD-43：新增 `fc16_address_saturates_at_u16_max`——同一请求验证白名单不误拒、写入不落到地址 0、读路径结果与写一致 |
| `gateway-core/src/north/server/modbus_server.rs`、`north/server/registry.rs`、`config/modbus_server.rs` | DD-42：把「写白名单口径与 OPC UA 一致」等注释改为如实说明——北向 Modbus 写**只更新本地寄存器镜像、不写设备**，下一采集周期被 `apply_registers` 覆盖；OPC UA 的 write_enabled 才走写设备路径 |

**第 6 批实施说明（如实记录）**

- DD-52 的「真实后端统计」落地为**队列存储侧 SQL 聚合**：原始表按行计数 + 分钟聚合表按 `sample_count` 还原原始样本数，窗口与诊断统计报表一致。仓库此前**没有**管线过滤（死区/变化上传）计数器，本批未新增计数器（属新增观测面），而是直接统计「实际落库」这个结果本身；压缩率的分母用 `dataPoints`（采集成功次数）而非 `totalReads`（含失败尝试），因为只有成功采集的样本才可能入库。
- DD-51 的 `evict` 是新增响应字段，旧客户端忽略即可；后端未下发时前端显示「—」，**不再回落到 100000**。
- DD-42 按用户口径**不新增「写设备」能力**：只让注释与实现一致（北向 Modbus 写 = 本地镜像，OPC UA 写 = 落设备）。
- DD-43 统一为 `saturating_add`（越界地址都落在 0xFFFF）；若未来产品要求「越界直接回 `EX_ILLEGAL_DATA_ADDRESS`」，需另一条明确口径，本批未改协议行为。
- 为守住行数预算（C24），本批只做**代码外移**（`diag_handlers.rs`、`stored_counts_api.rs`），未改任何既有行为。
- 仍**未做**：ROI-35（写路径统一）、ROI-43（OTA 验签）、ROI-44（调试台边界）、ROI-45（锁序）；DD-14 仍按用户决定记为独立待办。

**第 7 批实际改动（ROI-45 锁序规范覆盖真实代码）**

| 文件 | 改动 |
|---|---|
| `gateway-core/src/south/scheduler/spawning.rs` | DD-13：每轮采集前先取 `device_states` 快照（各点 `current_interval`）并立即释放，再单独锁 `breakers`；删除「持 `device_states` 时嵌套取 `breakers`」的加锁路径 |
| `gateway-core/src/south/scheduler/mod.rs` | DD-13：LOCK ORDER 表补齐此前漏登的 `breakers` / `paused_buses` / `timeline` / `fail_reload_injector`，写明 `breakers` 不得在持有 `device_states` 时取，并补记 `spawn_collector_task` 的实际加锁顺序 |
| `gateway-core/src/pipeline/edge.rs` | DD-16：`PipelineNode::prune(&HashSet<String>)` 默认空实现；`EdgeProcessor::prune` 遍历节点并清理 `last_quality`，按重载后的活跃点位淘汰 |
| `gateway-core/src/pipeline/nodes.rs` | DD-16：`DeadbandNode` / `ChangeNode` 实现 `prune`，点位下线后淘汰死区 / 变化上传基线 |
| `gateway-core/src/pipeline/virtual_point.rs` | DD-16：新增 `VirtualPointEngine::prune_cache(now_ms)`，把过期点位实时值缓存真正移出内存；新增 1 条回归用例 |
| `gateway-core/src/pipeline/mod.rs` | DD-16：新增 `ProcessingPipeline::prune_state(&active)`（重载后清理管线状态）；`run()` 增加 60 秒一次的 `prune_tick`，按 TTL 淘汰虚拟点实时值缓存 |
| `gateway-core/src/south/scheduler/reload.rs` | DD-16：`commit_points_reload` 清理 `device_states` 后调用 `pipeline.prune_state(&new_ids)`，点位删除同步传导到管线内部状态 |
| `gateway-core/src/pipeline/edge/tests.rs` | DD-16：新增 2 条回归用例——死区基线、变化上传基线在点位下线后必须被淘汰 |

**第 7 批实施说明（如实记录）**

- DD-13 只做「消除嵌套取锁 + 补全锁序表」，**未改任何锁类型**：`device_states` / `breakers` 仍为 `parking_lot::Mutex`。`last_attempt` 是每个采集任务私有的锁，不参与跨任务锁序。
- DD-16 的淘汰分两处触发：①点位热重载提交后按**活跃点位集合**淘汰死区 / 变化上传基线与质量边沿；②`ProcessingPipeline::run` 每 60 秒按 **TTL** 清理虚拟点实时值缓存（覆盖「点位未删除但长期不再上报」）。聚合节点每次 flush 已清空窗口缓冲，无需额外 prune。
- 本批**未新增功能面、未改协议行为**；ROI-35（写路径统一）、ROI-43（OTA 验签）、ROI-44（调试台边界）仍未做，DD-14 仍按用户决定记为独立待办。

---

## 0. 一句话总览

- **本次新增 22 项**（ROI-28 ~ ROI-49），覆盖 61 条需排期的债务（另 2 条为 P3 记录项，不排期）。
- **最值得先做的 4 项**：ROI-28（修 chaos 桩，让发布门禁恢复绿）、ROI-31（配置导入别再静默丢节）、ROI-32（队列降级要看得见）、ROI-34（调试台三条会「报错成功」的 P0）。
- **投入最大的 3 项**：ROI-35（写路径统一）、ROI-43（OTA 验签）、ROI-44（调试台资源与安全边界）。
- **本清单与既有剩余 4 项（ROI-09/13/14/15）无重叠**，可并行推进；交叉点在 §4。
- **未进入本清单的 2 条**：DD-45、DD-61 属 P3 记录项（经复核语义正确 / 前端已显式处理），不排期。

---

## 1. 工作量与价值对照（沿用既有口径）

### 工作量

| 记号 | 人话 | 典型内容 |
|---|---|---|
| **小** | 半天到 1 天，改一个文件里的一个函数 + 补几条测试 | 改常量、改判定条件、补计数器 |
| **小-中** | 1~2 天，改一个文件 + 同步改前后端契约 | 加一个字段、加一条接口字段 |
| **中** | 3~5 天，跨 2~3 个模块，要动数据结构或配置格式 | 改调度算法、加配置项、加模型字段 |
| **中-大** | 1 周左右，要重新设计一块逻辑 + 补大量测试 | 改重载语义、改指标体系 |
| **大** | 1~2 周以上，触及调度/生命周期主链路 | 写路径统一、OTA 验签体系 |

### 价值

| 记号 | 人话 |
|---|---|
| **高** | 防的是「数据错 / 报错成功 / 静默丢数据 / 门禁红」，现场直接受益 |
| **中高** | 显著降低排查成本或防止误操作，但不直接产生错误数据 |
| **中** | 一致性、可维护性、「以后少踩坑」类收益 |

---

## 2. 推荐清单主体（按 ROI 从高到低排）

### 🟢 A 档：立刻能做，小投入、大回报

#### ROI-28 让发布门禁恢复成绿的（DD-04 · P0）
- **现在的毛病**：chaos 集成测试有 5 例失败 —— ①测试桩把「功能码」当成 unit_id 回填（请求的 unit_id 其实在第 7 个字节），②BACnet 断言还在等旧的错误类型，③串口写断言还在等旧的错误分类。后两条是语义升级后没跟上更新的断言。
- **现场会看到什么**：**这个不直接影响现场**，但发布流水线是红的 —— 打 tag、每周定时发布门禁全都过不去，等于**发不出版本**；同时 CI 长期挂红会掩盖后面真正的新失败。
- **工作量**：**小**。改 1 行桩代码（回显请求的从站地址）+ 改 2 处断言 + 补注释。约 1~2 小时。
- **价值**：**高**。成本最低、解锁最大 —— 一处小修换回整条发布链路。
- **依赖**：无。可第一个做。
- **对应债务**：DD-04

#### ROI-29 把「点位超时」这个字段的前后端口径统一（DD-06 · P1）
- **现在的毛病**：后端这个字段是「可以不填」，前端类型定义里却是「必须填」。同一份结构里，别的可空字段（写入上下限）又是「可以不填」—— **三种写法混着来**。
- **现场会看到什么**：**不直接影响现场**，但类型检查这一关是红的，前端构建流水线过不去；而且这类口径分叉以后每次改模型都会再咬一次。
- **工作量**：**小**。统一 ts-rs 注解口径（二选一）+ 同步生成文件 + 改前端 4 处。约半天。
- **价值**：**中高**。本身零现场收益，但把「改一次模型要改三个地方」的成本降下来，且解掉一道卡构建的门禁。
- **依赖**：无。
- **对应债务**：DD-06

#### ROI-30 修掉「锁跨 await」那个写法（DD-05 · P1）
- **现在的毛病**：有一处代码在持锁期间跨越了 await 点。**本仓现在能编译过去，靠的是第三方库顺带打开的一个编译开关** —— 哪天那个库升级换了依赖，这个地方直接编译不过。
- **现场会看到什么**：**不直接影响现场**，但这是一颗定时炸弹：编译能不能过，取决于别人家的依赖怎么写。而且现在静态检查这一关是红的。
- **工作量**：**小**。按仓库里已经写好的锁序规范改写（先取值再放锁，再 await）。约 2 小时。
- **价值**：**中高**。消掉隐性依赖、恢复静态检查门禁；改动本身很小。
- **依赖**：无。
- **对应债务**：DD-05

#### ROI-31 配置导入别再悄悄吞掉一整节（DD-20 · P1）
- **现在的毛病**：前端维护了一份「网关配置有哪些小节」的名单，**漏了「协议转换规则」这一节**。用「导出 → 改 → 导入」的方式迁移配置时，这一节会被当成不认识的字段**悄悄删掉**。
- **现场会看到什么**：现场迁移配置后，**协议转换规则整段没了**，界面还提示「有字段不认识」—— 运维会先怀疑是自己配置写错了，白排查半天。
- **工作量**：**小**。名单补一项 + 改成由后端字段自动生成（避免下次再漏）。约半天。
- **价值**：**高**。这是「配置看着在、实际不生效」的一类问题，现场极难自查，且会随迁移操作反复触发。
- **依赖**：建议和 ROI-39（默认值同源）一起做，两件事共用同一份字段来源。
- **对应债务**：DD-20

#### ROI-32 队列扛不住的时候，要看得见也要真停（DD-17 / DD-18 · P1）
- **现在的毛病**：①数据库写不进去时，系统降级成「只用内存」继续跑，然后**当作成功返回**，也没有任何指标；②磁盘满的标志只写不读，日志却一直打印「已暂停入队」—— **实际根本没暂停**。
- **现场会看到什么**：磁盘写满时，接口照样返回 200、监控一片正常，数据全堆在内存里，**一断电就没了**；而日志说「已暂停」，运维以为队列停了、放松了警惕，实际还在持续写失败。
- **工作量**：**小-中**。补 2~3 个计数器 + 让磁盘满标志真正生效 + 改日志措辞。约 1~2 天。
- **价值**：**高**。把「静默丢数据」变成「看得见的降级」，且消除一条会误导现场的日志。
- **依赖**：建议在 ROI-44（队列深度指标）之前做，先有数再报警。
- **对应债务**：DD-17、DD-18

#### ROI-33 上行发出去的东西，不能悄悄没了（DD-23 / DD-24 · P1）
- **现在的毛病**：①上报模板里变量名写错时，会**生成一条空内容**然后直接跳过，不报错；②Webhook 消息落库失败时只打一条日志，既不计数也不返回失败。
- **现场会看到什么**：平台侧看到「设备在采、数据有量」，但**内容是空的**；或某批消息压根没发出去，监控上看不出任何异常。
- **工作量**：**小-中**。缺变量时报错 + 加计数器 + 失败路径按现有惯例返回。约 1~2 天。
- **价值**：**高**。都属于「静默丢数据」，且现场极难从平台侧发现。
- **依赖**：无。
- **对应债务**：DD-23、DD-24

#### ROI-34 调试台别把「失败」显示成「成功」（DD-01 / DD-02 / DD-03 · P0）
- **现在的毛病**：三条都是「报成功」类问题：
  1. **帧解析**：同一个协议帧，后端按「地址占 7 字节」解，前端按「多 1 字节仪表类型」解 —— **同一帧解出两个完全不同的结果**；
  2. **寄存器扫描**：中途设备断线后，前端永远在转圈（收尾条件写错了）；
  3. **报文回放**：回放到第 3 帧就中止了，页面却显示「回放完成，全部一致」。
- **现场会看到什么**：**这是本次最该先修的一条**。现场拿调试台的结果去调设备：帧解析错位 → 照着错的参数去调设备；回放中止被显示成完成 → 现场据此判定「协议没问题」，**而实际链路已经断了**。第 3 条属于最危险的一类：把失败报成成功。
- **工作量**：**中**。三条各改前后端一处判定 + 补解析口径对齐用例 + 补「中止要如实显示」的用例。约 3~5 天。
- **价值**：**高**。调试台是现场排障的主要工具，它给出的结论必须可信；其中「中止显示为完成」直接导致误判。
- **依赖**：建议 ROI-34 与 ROI-44 一起规划（调试台整体收口），但 ROI-34 本身可独立先做。
- **对应债务**：DD-01、DD-02、DD-03

---

### 🟡 B 档：值得做，投入中等

#### ROI-35 所有写操作走同一条路（DD-33 / DD-34 / DD-35 / DD-36 / DD-56 · P1）
- **现在的毛病**：写点位本来有一条统一入口（校验写入范围、核实读回、记审计都在里面），但 **OPC UA 那条路绕开了它**——自己写、自己报错，把「写入成功/失败/结果未知」三种结局压成了一种。另外：
  - 写不存在的点、写了不能写的点、值越界的点 —— **既不记审计也不计数**（成功的有记）；
  - 写超时了，审计里记的格式和控制历史认的格式对不上，解析失败时又当成 0 处理 → **结果未知的写，在控制历史里显示成「成功、写入值 0」**；
  - 审计里的「操作人」永远是 `api` / `opcua`，「设备名称」列填的是设备 ID。
- **现场会看到什么**：**同一个写动作，走不同入口行为不一样**（一个能写越界、一个把可疑结果报成成功）；现场查「谁在什么时候写过这个点」，只能看到一片 `api`，**越权或误操作的排查没有依据**。
- **工作量**：**中-大**。把 OPC UA 写路径并回统一入口（含范围校验与读回核实），统一审计格式与操作人来源，补齐被拒写的审计。约 1 周。
- **价值**：**高**。修的是「结果未知的写被显示成成功」和「被拒的写不留痕」两类高危问题，也是写语义这条线的收口工作。
- **依赖**：需先与 ROI-15（写属性建模：设定值/增量/脉冲）确认边界 —— 增量与脉冲点位的读回核实口径会影响统一入口的设计。
- **对应债务**：DD-33、DD-34、DD-35、DD-36、DD-56

#### ROI-36 前端别自己算，改成读后端的（DD-10 / DD-11 / DD-12 · P1）
- **现在的毛病**：三个页面的核心数字是**前端自己算的**，算法在后端并不存在或口径不同：
  - 调度优化页：自己按「固定间隔 + 设备+地址」合并，后端实际按「从站+功能码+超时」分组、间隔还自适应；
  - 通信画像页：自己按「当前周期折半 / 不变 / 翻倍 / 三倍」算健康分和建议周期，后端没有这套算法；
  - 通信参数页：退避档位写死一套，后端总线默认是另一套（页面还拿自己的那套兜底）。
- **现场会看到什么**：页面给出的「合并组数 / 健康分 / 恢复时间」和网关实际执行的对不上；现场按页面预估的恢复时间安排值守，**误判故障恢复时长**；不同页面对同一设备给出不同「健康」结论。
- **工作量**：**中**。把三个算法收到后端，前端只渲染；后端已有等价信息的（如退避档位）直接下发。约 3~5 天。
- **价值**：**中高**。不产生错误数据，但消除「同一件事两个答案」，这类不一致现场极难自查。
- **依赖**：无。
- **对应债务**：DD-10、DD-11、DD-12

#### ROI-37 改了配置，得真的生效（DD-07 / DD-08 / DD-09 · P1）
- **现在的毛病**：①判断「点位配置有没有变」的时候，**漏比了十几个字段**（超时、比例、是否可写、写入上下限、告警阈值、设备归属…）；②重载时重建采集组的那段循环，**把组键丢掉了**；③重连退避的失败计数**只在连接失败时加**，退避档位不跟着涨。
- **现场会看到什么**：现场把某个点位的读超时从 2 秒改成 0.5 秒并保存 —— **改完什么都没发生**，实际还是按 2 秒超时，而且界面不提示任何东西；设备持续离线时重连频率恒定，对现场网络和设备造成固定频率冲击。
- **工作量**：**中**。补齐比较字段 + 恢复组键使用 + 退避按失败次数推进 + 补回归用例。约 3~5 天。
- **价值**：**高**。「改了不生效且无提示」是现场最恼人的一类问题，且这类问题往往在交付后很久才暴露。
- **依赖**：建议在 ROI-12 已完成的重载原子性基础上做（已有基础）。
- **对应债务**：DD-07、DD-08、DD-09

#### ROI-38 配置里的错，要当场报出来（DD-14 / DD-15 / DD-41 · P1）
- **现在的毛病**：①「质量规则」装在一个**进程级全局变量**里，而且是由网页那一层绕路装进去的（正式装配路径不走这里），装失败只留一条日志；②配置写接口**返回 200 就算成功**；③转换规则填错时**静默丢弃**，而同样填错的虚拟点公式却保留着、界面上还能看到 —— 两种错误两种待遇。
- **现场会看到什么**：配置写成功了，界面上看着没问题，**实际规则根本没生效**；或者一条填错的规则就这么消失，运维不知道曾经配过什么。
- **工作量**：**中**。规则表改成由运行时装配持有、网页层只调装配好的接口；写接口返回真实安装结果；非法规则统一「保留 + 可见 + 标注不生效」。约 3~5 天。
- **价值**：**中高**。属于「配置与实际行为脱节」，影响面取决于规则使用深度，但一旦踩中就是全线数据不对。
- **依赖**：无。
- **对应债务**：DD-14、DD-15、DD-41

#### ROI-39 配置的默认值和差异对比，别手抄（DD-21 / DD-22 / DD-50 · P1）
- **现在的毛病**：①「版本对比 / 变更预览」**只比了 15 个小节**，漏掉的那些小节永远显示「无变化」；②前端**手抄了一份后端的默认值**（代码注释自己承认「必须逐项一致」）；③配置文本编辑的自动回写只覆盖 6 个小节。
- **现场会看到什么**：现场靠「变更预览」确认「我改好了」，但那一节压根没比 → 实际没生效，界面说没变化；后端加了个默认字段，前端不知道 → 表单里冒出后端不认的键。
- **工作量**：**中**。默认值和已知键清单改为由后端字段生成（与 ROI-31 共用同一来源）；差异对比改为遍历全部小节。约 3~5 天。
- **价值**：**中高**。消灭两处「必须手工保持一致」的重复实现，下次加字段不会再漏。
- **依赖**：建议与 ROI-31 合并做。
- **对应债务**：DD-21、DD-22、DD-50

#### ROI-40 指标和计数，得对得上它声称的事（DD-19 / DD-37 / DD-38 / DD-39 · P1）
- **现在的毛病**：①「数据库和 WAL 体积」这两个指标**生产环境从来不写**，永远是 0；②看门狗页面那列叫「重启次数」，实际统计的是「崩溃次数 / 上行发布失败数 / 采集失败数 / 心跳卡死数」四种混在一起；③硬件看门狗写失败**没有调用方**，计数一直是 0；④同一个服务信息结构，前端有**两份类型定义**（一个数字、一个大整数）。
- **现场会看到什么**：运维看到「重启次数 3」，以为是服务重启过 3 次，实际可能是上行发布失败了 3 次 —— **处置方向完全错**；想判断队列库是不是膨胀了，指标恒为 0，看不出任何东西。
- **工作量**：**小-中**。补 2 处指标采样 + 把混在一起的计数拆成具名字段 + 删掉手写类型定义。约 1~2 天。
- **价值**：**中高**。指标说错话比没有指标更危险 —— 它会把排查引向错误方向。
- **依赖**：无。
- **对应债务**：DD-19、DD-37、DD-38、DD-39

#### ROI-41 日志推送通道，补上实时通道已经有的保护（DD-62 · P1）
- **现在的毛病**：实时数据推送有 4 层保护（连接数上限、单连接缓冲上限、单条发送超时、读写分离），**日志推送这三条全没有**：连接数没上限、缓冲没上限、发消息没有超时，而且慢客户端会**卡住整条日志广播**。
- **现场会看到**：开了几个「实时日志」页签，其中一个网络慢 → **其它人的日志跟着卡住**；日志本身走的是「慢了就丢掉」的通道，**丢日志没有任何提示**。
- **工作量**：**小-中**。把实时通道已验证的保护接到日志通道（连接数单独计数）+ 丢掉日志时计数提示。约 1~2 天。
- **价值**：**中高**。不丢数据，但「一个慢客户端拖垮所有人」是现场很恼人的体验问题。
- **依赖**：无。
- **对应债务**：DD-62

#### ROI-42 死信的重投与清理，要用管事的权限（DD-63 · P1）
- **现在的毛病**：死信相关操作（重投 = 重新对外发包；清理 = 销毁唯一副本）落在**配置写权限**这一档里，而这个权限是**可以被授予给普通密钥**的（系统域那一档才是不可授予的）。路由注释写的是「必须强鉴权」。
- **现场会看到什么**：一个只拿到「配置写」权限的接口密钥，就能**重投和销毁北向死信**（死信里含失败报文原文，可能带凭据）。读那一侧已经保护好了，**缺口在写这一侧**。
- **工作量**：**小**。把死信写端点从配置域里单独摘出来，归到不可授予的系统域。约 2~3 小时。
- **价值**：**高**。权限分层比注释承诺的弱一档，属于安全类；改动小、影响面可控。
- **依赖**：需确认现场是否有人在用「配置写」权限调死信接口（大概率没有）。
- **对应债务**：DD-63

#### ROI-43 固件升级要验签，验不过就别装（DD-40 / DD-46 · P1）
- **现在的毛病**：固件包**只校验 CRC32，不验签**，而且校验不通过也照样放行；上传接口一律回 200「上传中」，审计记录写在处理之前、没有结果。
- **现场会看到**：被改过的固件包能装进现场网关（CRC32 很容易伪造），**装不上也不会被拦住**；审计里只有「开始升级」，看不出成没成功。
- **工作量**：**中-大**。接入签名校验（仓库里授权模块已有同类机制可复用）+ 改成校验失败一律拒绝 + 审计补结果。约 1 周。
- **价值**：**高**。安全类收益，且是「一次投入、长期受益」的防线。
- **依赖**：需先定签名方案（复用现有签发体系 vs 新增），这是产品与技术共同决策项。
- **对应债务**：DD-40、DD-46

---

### 🟠 C 档：价值明确，但改动面大

#### ROI-44 调试台的资源边界与安全边界（DD-25 ~ DD-32、DD-47 ~ DD-49 · P1/P2）
- **现在的毛病**：调试台这一整块有 11 条债，归成 4 类：
  1. **没有背压**：事件流用「无限队列」，前端接到异常直接跳过；
  2. **没有隔离**：回放会话存在**全局共享**的一份里、**没有过期时间**，局域网内任一登录用户都能看到并删掉别人的会话，还长期堆积；
  3. **暴露面太大**：从站模拟默认监听**所有网卡**（不只是本机），全局只能起一个实例；
  4. **同一件事两个答案**：报文捕获只有 Modbus 有后端解析、其余协议前端自己解，两边口径可能不同；「异常帧算不算在线」在两处判定**正好相反**；编辑报文发写帧**没有二次确认**（同页的调试向导却有）。
- **现场会看到什么**：多人共用一台网关调试时互相干扰会话；开着从站模拟等于**对整个局域网暴露一个从站**；误填一帧写命令直接下发到生产总线。
- **工作量**：**中**（若含会话归属改造则 中-大）。背压与隔离是结构性改动，其余多为局部修正。约 3~5 天起。
- **价值**：**中高**。多数不直接影响数据正确性，但「局域网暴露」「写帧无确认」两条有安全含义。
- **依赖**：建议在 ROI-34 之后做（先让调试台结果可信，再补边界）。
- **对应债务**：DD-25、DD-26、DD-27、DD-28、DD-29、DD-30、DD-31、DD-32、DD-47、DD-48、DD-49

#### ROI-45 锁序规范要能覆盖真实代码（DD-13 / DD-16 · P1/P2）
- **现在的毛病**：①仓库里有一份「锁的获取顺序」表，但**漏了三把锁**，而其中一处代码恰好在持有 A 锁时去拿 B 锁；②点位值缓存的清理函数**只有测试在调**，死区基线也一直没有淘汰。
- **现场会看到什么**：多总线并发重载时存在卡死窗口，且按现有锁序表推不出环路；长期运行后内存缓慢上升，点位规模大的现场尤其明显。
- **工作量**：**中**。锁序表补齐并修正嵌套取锁 + 缓存清理接入正常生命周期。约 3~5 天。
- **价值**：**中**。当前没有已复现的卡死，属于预防性修复。
- **依赖**：建议与 ROI-37（配置生效）一起做，两者都动调度主链路。
- **对应债务**：DD-13、DD-16

#### ROI-46 历史数据的查询口径，前后端要对齐（DD-51 ~ DD-54 · P2）
- **现在的毛病**：四个页面各算各的：①断连续传页写死了容量上限，而真实淘汰线是另一个数；②压缩率页面用两个写死的系数**编造压缩率**；③区间对比**忽略了「数据被截断」这个标志**；④对后端已经降采样过的数据，前端又做了一次「平均的平均」（没按桶的样本数加权）。
- **现场会看到什么**：页面上的压缩率是**编的**；区间对比结果看着完整，实际数据被截断过；历史曲线在采样过密的区段**平均值会偏**。
- **工作量**：**中**。四个口径都要以后端为准，前端改为直接消费后端结果。约 3~5 天。
- **价值**：**中**。分析类页面的准确性收益，不影响采集与上行主链路。
- **依赖**：无。
- **对应债务**：DD-51、DD-52、DD-53、DD-54

#### ROI-47 北向 Modbus 服务端的写语义与地址算法（DD-42 / DD-43 · P2）
- **现在的毛病**：①北向 Modbus 服务端收到写帧，只是**改了自己的本地寄存器**，下个采集周期就被覆盖；注册表的注释却说「写白名单口径与 OPC UA 一致」—— 实际全仓只有 OPC UA 两处真在写设备；②算地址时，白名单校验和写用「允许绕回」，读用「到顶就停」—— **同一个模块两套算法**。
- **现场会看到什么**：上位机通过北向 Modbus 写的值**不生效**（下个周期就被采集覆盖）；边界地址的行为在不同功能码下不一致。
- **工作量**：**中**。明确北向服务端的写语义边界（是设计如此还是缺陷，需产品口径）+ 统一地址算术 + 补边界用例。约 3~5 天。
- **价值**：**中**。取决于现场是否真的用北向 Modbus 写入，用得少就不划算。
- **依赖**：需先确认北向 Modbus 服务端是否**应该**写设备 —— 这一条是产品口径问题。
- **对应债务**：DD-42、DD-43

---

### 🔵 D 档：不写代码，或小改动，但能防下次再漏

#### ROI-48 把注释和文档改成跟代码一致（DD-44 / DD-59 / DD-60 · P2）
- **现在的毛病**：三处文档/注释与实现对不上：①协议能力表里有一句「某某协议还没实现某功能」，但其实已经实现了；②数据管线的文档写「先入队再广播」，代码实际是「先广播再入队」；③OTA / NTP / 证书这几个接口的代码物理上放在「设备」模块里，却由「系统」路由注册 —— 是有意归档还是遗留，没写。
- **现场会看到什么**：**不直接影响现场**，但下一个接手的人（含未来的我们）会照着文档去「修已经修好的东西」，或把归档错了的代码当架构问题处理。
- **工作量**：**小**。改 2 处注释 + 补 1 处归档说明。约 1~2 小时。
- **价值**：**中**。零风险、零副作用，防重复劳动。
- **依赖**：无。可与其它项并行。
- **对应债务**：DD-44、DD-59、DD-60

#### ROI-49 门禁再补三块（DD-55 / DD-57 / DD-58 · P2）
- **现在的毛病**：①接口测试覆盖是 174/175，**有一个读接口没有用例**；②前端有 800 行的行数上限，但**后端有冻结账本、前端没有** —— 前端的大文件没人看守；③有一个写接口前端已经完全不用了，属于孤儿端点。
- **现场会看到什么**：**不直接影响现场**，但「覆盖缺一个」「前端文件能无限长」都属于「以后再漏」的口子。
- **工作量**：**小-中**。补 1 条用例 + 建前端行数冻结账本 + 给孤儿端点标注废弃。约 1~2 天。
- **价值**：**中高**。属于「一次投入、长期受益」的护栏，特别是前端行数账本 —— 后端已有先例，照搬即可。
- **依赖**：无。
- **对应债务**：DD-55、DD-57、DD-58

---

## 3. 排期建议（若按顺序推进）

| 批次 | 内容 | 理由 |
|---|---|---|
| **第 1 批（半天~1 天）** | ROI-28、ROI-29、ROI-30 | 三条都是「小」工作量且各自解锁一道门禁（发布、类型检查、静态检查），先把流水线恢复成全绿 |
| **第 2 批（1~2 天）** | ROI-31、ROI-32、ROI-33 | 三条都是「静默丢数据 / 静默丢配置」类，现场高危且改动集中 |
| **第 3 批（3~5 天）** | ROI-34、ROI-42、ROI-40、ROI-41 | 调试台可信度 + 一条安全权限 + 指标说对话 + 一条通道保护；可 2 人并行 |
| **第 4 批（3~5 天）** | ROI-36、ROI-37、ROI-38、ROI-39 | 配置生效与前后端口径收口，建议合并成「配置一条线」一起做 |
| **第 5 批（1 周）** | ROI-35、ROI-43 | 写路径统一与 OTA 验签，都是需要设计决策的改动 |
| **第 6 批（按需）** | ROI-44 ~ ROI-48 | 视现场规模与使用情况排期 |
| **随时** | ROI-49 | 护栏类，与其它项不冲突 |

**总量估算**：第 1~4 批约 **2 周**（2 人并行可压到 1 周左右）；含第 5 批约 **3 周**。

---

## 4. 与既有剩余 4 项（ROI-09 / 13 / 14 / 15）的关系

既有清单剩 4 项（见 [remaining-roi-backlog-2026-11.md](../remaining-roi-backlog-2026-11.md)），与本清单**无编号冲突、无内容重复**，但有两处交叉：

| 既有项 | 交叉点 | 建议 |
|---|---|---|
| **ROI-09** 批量写语义（能不能整批重试） | 与本清单 **ROI-35**（写路径统一）同一片代码区 | 建议**合并做**：批量写语义要在统一写入口上声明，否则改两遍 |
| **ROI-15** 写属性建模（设定值/增量/脉冲） | 与 **ROI-35** 的读回核实口径直接相关 | ROI-35 先做「统一入口 + 范围校验 + 审计口径」，增量/脉冲的读回消歧留给 ROI-15 |
| **ROI-13** 配置文件监听 | 与 **ROI-37**（改了配置要真生效）同属配置链 | ROI-37 先落地「改了生效」，再接 ROI-13 的「自动触发」 |
| **ROI-14** Modbus 组间并发 | 与 **ROI-45**（锁序）同属调度主链路 | 两者都动调度，建议 ROI-45（锁序修正）先行，避免并发改造踩到锁序问题 |

**结论**：本清单与既有 4 项可**并行**，但建议把 **ROI-35 排在 ROI-09 / ROI-15 之前**（先统一入口，再在其上声明语义），把 **ROI-45 排在 ROI-14 之前**（先修锁序，再做并发）。

---

## 5. 一页速查表

| ROI | 一句话 | 工作量 | 价值 | 对应 DD |
|---|---|---|---|---|
| ROI-28 | 修 chaos 桩与断言，恢复发布门禁 | 小 | 高 | DD-04 |
| ROI-29 | 统一「点位超时」的前后端类型口径 | 小 | 中高 | DD-06 |
| ROI-30 | 修掉「锁跨 await」写法 | 小 | 中高 | DD-05 |
| ROI-31 | 配置导入别再吞掉「协议转换规则」 | 小 | 高 | DD-20 |
| ROI-32 | 队列降级要看得见、真停 | 小-中 | 高 | DD-17、DD-18 |
| ROI-33 | 上行内容为空/发送失败不能静默 | 小-中 | 高 | DD-23、DD-24 |
| ROI-34 | 调试台别把失败显示成成功 | 中 | 高 | DD-01~03 |
| ROI-35 | 所有写操作走同一条路 | 中-大 | 高 | DD-33~36、DD-56 |
| ROI-36 | 前端别自己算，读后端的 | 中 | 中高 | DD-10~12 |
| ROI-37 | 改了配置得真的生效 | 中 | 高 | DD-07~09 |
| ROI-38 | 配置里的错要当场报出来 | 中 | 中高 | DD-14、DD-15、DD-41 |
| ROI-39 | 默认值与差异对比别手抄 | 中 | 中高 | DD-21、DD-22、DD-50 |
| ROI-40 | 指标和计数得对得上它声称的事 | 小-中 | 中高 | DD-19、DD-37~39 |
| ROI-41 | 日志通道补上实时通道的保护 | 小-中 | 中高 | DD-62 |
| ROI-42 | 死信操作要用管事的权限 | 小 | 高 | DD-63 |
| ROI-43 | 固件升级要验签 | 中-大 | 高 | DD-40、DD-46 |
| ROI-44 | 调试台的资源与安全边界 | 中 | 中高 | DD-25~32、DD-47~49 |
| ROI-45 | 锁序规范覆盖真实代码 | 中 | 中 | DD-13、DD-16 |
| ROI-46 | 历史查询口径前后端对齐 | 中 | 中 | DD-51~54 |
| ROI-47 | 北向 Modbus 写语义与地址算法 | 中 | 中 | DD-42、DD-43 |
| ROI-48 | 注释文档改成跟代码一致 | 小 | 中 | DD-44、DD-59、DD-60 |
| ROI-49 | 门禁再补三块 | 小-中 | 中高 | DD-55、DD-57、DD-58 |

> DD-45、DD-61 为 P3 记录项（经复核语义正确 / 前端已显式处理），不排期。

---

## 6. 关联文件

- [01-功能摸底台账.md](01-功能摸底台账.md) — 10 域 × 72 页签 × 175 端点
- [02-技术债务清单.md](02-技术债务清单.md) — DD-01 ~ DD-63，含 file:line 证据
- [03-门禁基线与补强建议.md](03-门禁基线与补强建议.md) — 门禁实测基线
- [05-剩余项ROI推荐清单.md](05-剩余项ROI推荐清单.md) — 未实施项合并排期清单
- [06-bug修复验证清单.md](06-bug修复验证清单.md) — 每批修复的证据链验收清单（含 `scripts/verify_bugfix.ps1`）
- [remaining-roi-backlog-2026-11.md](../remaining-roi-backlog-2026-11.md) — 既有 ROI-01~27
