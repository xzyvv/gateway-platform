# 热重载与在途操作不变量 —— 测试守护核对

> **2026-11-26 复核**：P2-1 已为 U-2/U-3/U-5/U-6/U-7 补直接用例、U-4 按决议 A 落地，本分册原文（2026-11-22 基线）的「无守护 / 需产品口径」结论已不再代表现状。最新状态见文末「2026-11-26 复核」；清单原文 `docs/architecture/reload-inflight-semantics.md` §5.2 已同步修正，NB-04 关闭。

核对基线：当前工作区代码（2026-11-22）

核对对象：`docs/architecture/reload-inflight-semantics.md` §5.2「尚未证明 / 存在窗口」表（U-1 ~ U-7，行 171-179）。
方法：逐条打开清单「依据/现状」列指向的实现文件确认语义，再用全仓检索（`gateway-core` 内 `#[test]`/`#[tokio::test]`、`tests/` 集成测试、`mod tests`/`mod r36_tests` 等子模块、`gateway-api/tests/`、`gateway-web/src/**/*.spec.ts`）定位候选测试，逐个 Read 打开确认其**断言**确实覆盖该条不变量点名的可观察行为。全程只做静态核对，未执行 `cargo test` / `npm test` / `clippy`，未修改仓库任何文件（除本输出文件）。

判定口径：
- **【有守护】**：存在测试，其断言直接校验了该不变量点名的可观察行为（错误枚举、错误码、计数、顺序、时序、是否为空等）。
- **【部分守护】**：存在相关测试但断言不完整——只覆盖该不变量点名的部分分支，或只测了读侧/写侧之一。
- **【无守护】**：全仓（含前端 spec）找不到任何断言覆盖它。

「清单该列已过时」口径：清单 §5.2「依据/现状」列所引 file:line 在当前代码中已不包含它所声称的内容（行号漂移到无关代码），或其结论已被后续修复推翻。按函数名仍能定位到同一守护用例的（U-1），只注漂移、不计入过时。

---

## 汇总

| 指标 | 数值 |
|---|---|
| 条目总数 | 7 |
| 【有守护】 | 1 |
| 【部分守护】 | 2 |
| 【无守护】 | 4 |
| 清单「现有测试」列已过时 | 2 |

已过时明细（2 条）：U-3、U-6。

补充说明：
- 前端 `gateway-web/src/**/*.spec.ts` 全量检索 `reload` / `inflight` / `abort`，命中的只有 `api/config.spec.ts`（仅断言 `POST /api/config/reload` 路径）、`composables/layout/useGatewaySwitcher.spec.ts`（前端切网关时 abort 在飞请求 + reload 的时序，属前端 UI 层）、`composables/config/useConfigActions.spec.ts`（mock `reloadConfig`）。**无一条断言覆盖后端 U-1~U-7 的语义**，故前端不构成守护证据。
- 清单「代码依据」列的行号普遍已漂移（如 U-3 引 `tcp_task.rs:300-304`、U-6 引 `gateway.rs:493-509`），本核对一律以**实现语义 + 测试断言**为准，行号以本次 Read 输出为准。

---

## 明细

| 编号 | 不变量要点 | 判定 | 守护证据（path:line + 测试名） | 判定依据 | 建议动作 |
|---|---|---|---|---|---|
| U-1 | 串口 abort 落在「帧已写出」之后时，不得回送 `BusClosed`（明确失败），须回送 `WriteUncertain`（结果未知、禁止重试） | 【有守护】 | `gateway-core/src/south/bus/tests.rs:1238-1267` `fn aborted_in_flight_write_reports_write_uncertain`；对照组 `gateway-core/src/south/bus/tests.rs:1269-1291` `fn aborted_in_flight_read_still_reports_bus_closed`；API 三态映射 `gateway-api/tests/points_write_unified.rs:238-264` `fn unified_write_single_entry_uncertain_returns_5102` | 打开用例确认：用例先 `wait_until(|| !port.requests().is_empty(), 2000, "写帧未下发")` 确认写帧真正落线，随后 `bm.handle…abort()` 模拟 reload abort 旧总线任务，最后断言 `matches!(result, Err(CollectorError::WriteUncertain(_)))`——正是 U-1 点名的「帧已写出后取消」行为；对照用例锁住读事务取消仍为 `BusClosed`；API 用例锁住 `WriteUncertain → 5102` 且提示含「设备可能已执行」「不要自动重试」。清单该列写的 `south::bus::tests::aborted_in_flight_write_reports_write_uncertain` 函数名与真实用例完全一致，仅模块路径写法略有出入，不计过时 | 保持 |
| U-2 | 串口 `write_all` 被取消时实际写出多少字节未知，对外口径须保守统一为 `WriteUncertain` | 【部分守护】 | `gateway-core/src/south/bus/tests.rs:1238-1267` `fn aborted_in_flight_write_reports_write_uncertain`（间接覆盖「帧已完整写出后被取消」分支） | 该用例只覆盖「帧已完整写出后才取消」这一分支；**缺**「`write_all` 尚未写完（正在写入）时被取消」的独立断言——`gateway-core/src/south/bus/serial_task/tests.rs:256-302` `fn write_hang_hits_short_write_timeout_and_releases_de` 走的是写超时 → `CollectorError::IoError("serial write timeout")` 路径且 `frame_sent: None`（`:273`），根本不是取消路径 | 补断言 |
| U-3 | reload 后旧 TCP 连接还会存活多久、与新连接是否会向同一设备交叉写 | 【无守护】 | — | `gateway-core/src/south/bus/tcp_task/tests.rs` 的全部用例（`:226` `stale_disconnect_does_not_clear_new_pool`、`:388` `disconnect_aborts_pool_and_triggers_reconnect`、`:486` `reconnect_backoff_resets_only_after_real_communication`、`:495` `connect_success_does_not_clear_backoff`）都是断链/重连场景；chaos 侧 `gateway-core/tests/chaos/scenarios.rs:1119` `scenario_scheduler_concurrent_reload_no_deadlock`、`:1174` `scenario_scheduler_reload_retains_active_device_states` 只覆盖 `Scheduler::reload`；全仓无任何用例断言 reload 与 TCP 在途写的存活期或新旧连接交叉写。清单该列已过时：所引 `tcp_task.rs:300-304` 现落在 Disconnected 分支（清 `pool_slot` + 回送 `IoError("disconnected")`），detach 语义实际在 `tcp_task.rs:239`（独立事务 task）、`:314`（`in_flight.push`）、`:320-323`（shutdown abort） | 新增用例 |
| U-4 | 「新 collector + 旧点位」窗口内下发会造成什么现场后果 | 【无守护】 | — | `gateway-api/src/web/api/points/handlers/mod.rs:254-255` 分别取配置快照与 collector 快照，两者无共同版本号；Rust 侧 grep `epoch`/`generation` 只命中时间戳与证书相关代码，无代次绑定机制；无任何用例断言「写请求跨 reload 时点位与 collector 属同一配置代次」。清单该列结论（无测试、无版本号约束）仍成立，不计过时 | 需产品口径 |
| U-5 | 同一串口被新旧两个 `SerialHandle` 同时打开的后果 | 【无守护】 | — | `gateway-core/src/south/bus/serial.rs:110-119` `open_bus` 仍无排他；`gateway-core/src/runtime/gateway.rs:636-643` 已改为先 `old.close_task(300ms)` 再 `open_bus`，但**无任何用例断言该端口交接顺序**——`gateway-core/src/runtime/gateway/r36_tests.rs:11-41` `fn reload_buses_keeps_old_manager_when_open_fails` 走的是 `reset_serial_buses` 自愈路径（`:30`）而非 `apply_bus_plan`，且断言的是「打开失败保留旧 manager」。清单该列所引 `serial.rs:116-118` 行号与结论仍准确，不计过时 | 新增用例 |
| U-6 | reload 失败后运行态（managers/collectors）能否恢复 | 【部分守护】 | `gateway-api/tests/config_reload_atomicity.rs:79-140` `fn reload_failure_leaves_memory_and_disk_untouched`；`gateway-core/src/runtime/gateway/r36_tests.rs:47-83` `fn prepare_reload_rejects_unknown_protocol_without_side_effects`；`:89-114` `fn prepare_reload_rejects_point_on_missing_bus`；`gateway-core/src/south/scheduler/tests.rs:290-351` `fn r5_reload_failure_rolls_back_collectors_and_points` | 打开用例确认：API 侧用例断言 `Arc::ptr_eq(&mem_before, &mem_after)` 且 `disk_bytes(&state) == disk_before`（但用的是 mock gateway，只覆盖 API 侧内存/磁盘）；core 侧两例断言 prepare 失败零副作用；scheduler 例断言 prepare 注入失败后 collectors/points 数量回滚。**缺** commit 阶段「串口重开失败 → `restore_serial_buses`（`gateway-core/src/runtime/gateway.rs:694-730`）把运行态恢复到旧配置」的直接用例。清单该列已过时：所引 `gateway.rs:493-509` 现为 `commit_reload` 文档注释与函数签名，且「managers 无回滚路径」已被 ROI-12 推翻（现有 `restore_serial_buses` 自愈路径） | 补断言 |
| U-7 | reload 期间被取消的写事务数量 | 【无守护】 | — | 指标侧只有 `write_uncertain_total`（`gateway-core/src/engine/metrics/counter.rs:252` `fn inc_write_uncertain`）与 `config_reload_*`（`gateway-core/src/engine/metrics/lifecycle.rs:23-37`），既无「reload 期间被取消写事务数」计数也无日志；`abort_all` 只存在于 Modbus TCP 事务池（`gateway-core/src/south/protocol/modbus/transaction.rs:417`），reload 路径 grep 无任何调用。清单该列结论仍成立，不计过时 | 需产品口径 |

---

## 待补守护清单

> 2026-11-26：以下为 11-22 基线快照；所列 U-2~U-7 均已落地（U-4 端到端已补），现状见上方明细表与文末复盘。

（只列【部分守护】和【无守护】的条目，按编号排序）

| 编号 | 缺什么 | 建议测试名 | 建议放置位置 |
|---|---|---|---|
| U-2 | 缺「`write_all` 正在写入（尚未写完）时总线 task 被取消」的独立断言——现只有「帧已完整写出后取消」一例；需验证写入中途取消同样回送 `WriteUncertain`（而非 `BusClosed`），且 `frame_sent` 已置位 | `fn aborted_during_write_all_reports_write_uncertain` | `gateway-core/src/south/bus/tests.rs`（可复用 `SwitchablePort` 桩，新增一个「写到一半即 Pending」的写桩） |
| U-3 | 缺 reload 与 TCP 在途写并发的断言：旧连接上事务的终态、旧连接存活时长、新旧连接是否对同一设备重复下发 | `fn tcp_inflight_write_during_reload_has_single_terminal_outcome`、`fn reload_does_not_duplicate_write_to_same_device` | `gateway-core/src/south/bus/tcp_task/tests.rs`（复用 `fake_modbus_server` 桩）+ `gateway-core/src/runtime/gateway/r36_tests.rs` |
| U-4 | 缺「写请求跨 reload 时点位与 collector 属同一配置代次」的断言；需先裁定期望语义（跨代写应拒绝 / 允许但带代次标记 / 允许并落审计），再据此写断言 | `fn write_uses_point_and_collector_from_same_config_generation` | `gateway-api/src/web/api/points/handlers/mod.rs` 对应的 `gateway-api/tests/points_write_unified.rs`（口径裁定后按最终语义编写） |
| U-5 | 缺「reload 开新串口前旧端口已释放（`close_task` 先于 `open_bus`）」的顺序断言——现无任何用例守护该交接顺序 | `fn commit_reload_closes_old_serial_task_before_opening_new_port` | `gateway-core/src/runtime/gateway/r36_tests.rs`（覆盖 `apply_bus_plan` 而非 `reset_serial_buses`） |
| U-6 | 缺 commit 阶段串口重开失败后运行态自愈的直接断言：`restore_serial_buses` 须按旧配置重开总线、重建 collector、重建受影响采集组，且 commit 返回 `Err` | `fn commit_reload_restores_runtime_when_serial_reopen_fails` | `gateway-core/src/runtime/gateway/r36_tests.rs`（需能注入 `open_bus` 失败） |
| U-7 | 缺「reload 期间被取消写事务数 / drain 等待数 / 超时数」的指标与断言；指标本身不存在，需先定名与口径（含是否与 `write_uncertain_total` 区分） | `fn reload_cancelled_write_transactions_are_counted` | `gateway-core/src/engine/metrics/lifecycle.rs` + `gateway-core/src/runtime/gateway/r36_tests.rs`（口径确定后新增） |

---

## 2026-11-26 复核（P2-1 后）

| 指标 | 11-22 基线 | 11-26 现状 |
|---|---|---|
| 条目总数 | 7 | 7 |
| 【有守护】 | 1 | 7（U-1~U-7） |
| 【部分守护】 | 2 | 0 |
| 【无守护】 | 4 | 0 |
| 清单「现有测试」列已过时 | 2 | 0 |

新增证据：

| 编号 | 落地 | 用例 / 指标 |
|---|---|---|
| U-2 | `frame_sent` 进入写帧前置位，`TxnResponder::Drop` 保守回送 `WriteUncertain` | `gateway-core/src/south/bus/tests.rs` `aborted_during_write_all_reports_write_uncertain` |
| U-3 | reload abort 主循环不打断在途事务；旧连接续命至事务结束、不重复下发 | `gateway-core/src/south/bus/tcp_task/tests.rs` `reload_does_not_duplicate_inflight_tcp_write` |
| U-4 | `ConfigRefs.generation` + 写审计 ` [cfg:<n>]`，解析侧兼容；端到端断言已补 | 核心：`gateway-core/src/runtime/write_service.rs` `audit_detail_appends_config_generation`、`gateway-core/src/engine/write.rs` `parse_detail_ignores_config_generation_suffix`；端到端：`gateway-api/tests/points_write_unified.rs` `write_audit_records_current_config_generation`、`gateway-api/tests/config_reload_atomicity.rs` `reload_failure_leaves_memory_and_disk_untouched`（失败不推进代次、成功 +1） |
| U-5 | `close_task` 先于 `open_bus` | `gateway-core/src/runtime/gateway/r36_tests.rs` `reload_closes_old_serial_port_before_opening_new_one` |
| U-6 | 串口重开失败按旧配置自愈并返回 `Err` | `gateway-core/src/runtime/gateway/r36_tests.rs` `reload_serial_open_failure_restores_previous_config_and_errors` |
| U-7 | 新增 `reload_cancelled_writes_total` / `reload_drain_timeout_total` | `gateway-core/src/engine/metrics/lifecycle.rs`；取消写计数由 U-2 用例断言 |
