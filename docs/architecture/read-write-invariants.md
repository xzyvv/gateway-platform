# 读写语义不变量清单

> 用途：每次改动 `gateway-core/src/south/**`（总线 / 协议层）前后，对照本表逐条检查。
> 表中每条都是**必须成立的语义**，不是建议。破坏任何一条都会在现场表现为「偶发」问题——
> 这类问题没有编译期信号、没有自动告警，只靠人发现，因此必须固化成清单与断言。

**状态标记**

| 标记 | 含义 |
|---|---|
| ✅ | 成立，且有测试守着 |
| 🟡 | 成立，但**无测试守着**（回归风险） |
| ❌ | 不成立 / 未实现 |

**用法**：改动涉及总线或协议层时，先在改动说明里勾选受影响的条目；新增语义必须补一行，
并尽量补上对应测试。

---

## 1. 总线层（`south/bus/**`，与协议无关）

| # | 不变量 | 状态 | 守点 |
|---|---|---|---|
| B-01 | 写事务失败后**不得自动重发**（写帧可能已被从站执行） | ✅ | `BusTransaction.retry_allowed` + `transaction_no_retry`；测试 `write_timeout_is_not_resent_and_reports_uncertain` |
| B-02 | 读/采集事务允许按 `retry_count` 重发 | 🟡 | `bus/mod.rs` 重试循环；测试 `bus_retry_budget_covers_all_attempts_and_retry_delays` 只锁预算算术、`write_transaction_never_retries` 只锁读可重试的谓词，均未断言读路径实际重发次数 |
| B-03 | 写事务的应答预算按「不重试」推导，不白等重试轮次 | ✅ | `transaction_inner` 中 `effective_retries`；测试 `write_reply_deadline_ignores_retry_budget` 断言写事务预算恒为 `1T + 余量`，与配置的重试轮次无关 |
| B-04 | 调用方已放弃（超时/断开）的事务**不得下发** | ✅ | `prepare_dequeued` |
| B-05 | 写（控制）事务的出队优先级高于读 | ✅ | 双通道 + `select!{biased}` |
| B-06 | 写事务不得因采集占满配额而拿不到槽位 | ✅ | `reserved_control_slots`（唯一公式来源）；测试 `pool_control_transaction_reserves_slots_under_pressure` |
| B-07 | 采集并发上限必须与池对普通事务的准入上限一致 | ✅ | `BusManager::read_concurrency()`；测试 `read_concurrency_reserves_slots_for_control` |
| B-08 | 池满不得直接失败，应先短等待退避 | ✅ | `TransactionPool::acquire_slot`；测试 `pool_waits_for_slot_instead_of_failing_immediately` |
| B-09 | 池满/本地拥塞必须报 `BusBusy`，不得伪装成「设备不应答」 | ✅ | `map_txn_err`：`Busy → BusBusy` |
| B-10 | 事务前的陈旧字节必须排空 | ✅ | 串口 `clear_input()` |
| B-11 | 排空的陈旧字节必须可观测（非 0 = 上一帧没收完整） | ✅ | `gateway_bus_stale_bytes_dropped_total` + warn 日志 |
| B-12 | 串口在途事务不可被抢占（物理约束，非缺陷） | ✅ | 串口 task 单事务串行，控制通道只在当前事务结束后消费；测试 `in_flight_serial_transaction_is_not_preempted` 断言读事务等应答期间控制写不落线、两帧不交错 |
| B-13 | 串口故障分级：设备级失败不得重开端口 | ✅ | `serial_task` 自检帧 + `MIN_REOPEN_INTERVAL` |
| B-14 | 广播地址（slave=0）不得记为「成功通信过的从站」 | ✅ | `last_good_slave` 过滤 `slave == 0` |

## 2. 写语义（跨协议统一口径）

**定级判据只有一个：能否证明写没有生效。**

- 能证明 → **明确失败**（可安全重试）：`DeviceRejected`（设备回了错误码）/ `ModbusException` /
  `BusBusy` / `Unsupported` / `InvalidConfig`
- 证明不了 → **结果未知**（`WriteUncertain`，**禁止**自动重试）：`NoReply` / `Timeout` /
  `IoError` / `ChecksumError` / `InvalidFrame`

刻意**不**按「错误出现在哪个阶段」定级（发送阶段 / 应答阶段 / 解析阶段），
只按「能否证明」——阶段信息在协议层拿不到（Modbus 总线层两者混合），
按阶段定级会得出「发送失败 = 安全可重试」这种错误结论。

| # | 不变量 | 状态 | 守点 |
|---|---|---|---|
| W-01 | 写结果「可能已生效」必须用 `WriteUncertain` 上报，不得报成普通失败 | ✅ | `mark_write_uncertain` / `mark_write_uncertain_counted`（协议无关） |
| W-02 | `WriteUncertain` **不可重试** | ✅ | `is_retryable` 刻意排除 |
| W-03 | 能证明写未生效的错误不得转成 `WriteUncertain`（`DeviceRejected` / `ModbusException` / `BusBusy` / `Unsupported` / `InvalidConfig`） | ✅ | `mark_write_uncertain` 白名单；测试 `definite_failures_keep_their_variant` |
| W-03b | `InvalidFrame`（应答无法解析）**属于结果未知**，不是明确失败 | ✅ | `mark_write_uncertain` 黑名单；测试 `uncertain_errors_become_write_uncertain` |
| W-03c | 设备明确回了错误码（拒绝 / 异常码 / 否定确认）必须用 `DeviceRejected`，不得混进 `InvalidFrame` | ✅ | `DeviceRejected` 变体 + 各协议错误点改造；测试 `device_rejected_is_distinct_from_invalid_frame` |
| W-04 | 写结果未知必须计数 | ✅ | 指标 `gateway_bus_write_uncertain_total`（Prom 名带 `bus_` 前缀）；自研协议经 `mark_write_uncertain_with(metrics, e)` 计入 —— **缺口 G-08 已关闭** |
| W-05 | 写路径必须走控制通道（优先级） | ✅ | Modbus / DLT645 / CJT188 全部 `BusPriority::Control` |
| W-06 | 写不确定的错误必须传到 API 层可见 | ✅ | `CollectorError::write_outcome()` → `engine::write::WriteOutcome`（中立层）；API 层 `results[].outcome` + 业务码 5102 |
| W-07 | 上游（前端 / MES）不得对 `WriteUncertain` 自动重试 | ✅ | 业务码 **5102**（结果未知）+ `outcome: "uncertain"` + 统一提示「设备可能已执行，请先读回核实，不要自动重试」；前端仅提示不阻断重试 |
| W-08 | 写超时后应能读回核实（消除「未知」） | ✅ | API 写入口在 `outcome == uncertain` 时自动读回：单写 `read`（5s 预算），批写按总线 `read_batch`（`batch_timeout_budget`）；**只做正向收敛**——读回值与写入值一致（容差 = 半量化单位 `|scale|/2`）才判定写已生效，不一致或读不到都保持 5102 并把读回值附给操作员（`write_readback_matches`） |
| W-09 | 点位**不建模**「写值 / 增量 / 脉冲」写属性（决议 A）：写通道由 `function_code` 决定，寄存器宽度由 `data_type` 决定，写入值为绝对值 | ✅ | 测试 `write_function_code_is_configured_and_value_is_absolute`：fc=6→FC06、fc 缺省→FC06、f32→自动升 FC16、fc=1→FC05，且写入值为绝对值（无累加语义） |
| W-10 | Modbus 写异常响应必须保留 `ModbusException` 语义 | ✅ | `trait_impl.rs` 先判 5 字节异常帧；测试 `write_exception_frame_maps_to_modbus_exception` |
| W-11 | 写响应按**语义字段**校验（slave/fc/addr/值），不得整帧比对 | ✅ | `verify_write_echo`（CRC 由定帧层负责） |
| W-11b | 写回显**不匹配** → 应答无法解析 → 结果未知（写很可能已执行），不得报成普通失败 | ✅ | Modbus `verify_write_echo` 失败 → `mark_write_uncertain`；测试 `write_echo_mismatch_is_write_uncertain` |
| W-12 | 自研客户端协议（s7comm / fins / mc / iec104 / bacnet）的写路径必须套用同一套定级 | ✅ | 各协议写路径 `mark_write_uncertain`；测试 `s7comm/mc/fins/iec104/bacnet` 写语义用例 |
| W-13 | **未下发**的失败（地址解析失败、参数非法、建链失败、`BusBusy`、不可写功能码）不得算「结果未知」 | ✅ | 这些路径不套 `mark_write_uncertain`；不可写 FC → `Unsupported`；测试 `write_unsupported_function_code_is_not_uncertain` |
| W-14 | 同一协议的**读**路径不得因写语义改造而改变错误口径 | ✅ | mc 读路径保留 `check_end_code`、写路径另用 `check_write_end_code`；ROI-25 后两者均返回 `DeviceRejected`，由测试 `test_check_end_code_is_device_rejected` 锁定读写同口径 |
| W-15 | 本地 IO 写（GPIO）不参与本分级（写生效与否确定） | 🟡 | `gpio/mod.rs` 把硬件失败 `map_err` 为 `CollectorError::IoError`；现有用例只有 `test_gpio_write_rejects_di_pin`、`test_gpio_write_rejects_invalid_address`（均为 `InvalidConfig`），无 `IoError` 断言 |
| W-16 | 总线协议（modbus / dlt645 / cjt188）的写响应状态字非 0 → `DeviceRejected` | ✅ | dlt645 / cjt188 `parse_write_*_response`；测试 `write_status_nonzero_is_device_rejected` |

## 3. 读语义（跨协议）

| # | 不变量 | 状态 | 守点 |
|---|---|---|---|
| R-01 | 读响应必须校验从站地址与 `byte_count` | ✅ | `parse_read_response` / `parse_coils_response` |
| R-02 | 读异常码必须区分「配置类」与「可恢复类」 | ✅ | `classify_exception` |
| R-03 | 批量读必须按 `(slave_id, 功能码)` 分组，不得跨从站合并 | ✅ | `read_batch_points_with_slots` |
| R-04 | 批量写合并必须地址**严格连续**，不得跨空洞 | ✅ | `merge_write_batches` |
| R-05 | 批量读合并上限：线圈 2000 点 / 寄存器 125 点 | ✅ | `merge_batches` |

## 4. Modbus RTU 特有（串口）

| # | 不变量 | 状态 | 守点 |
|---|---|---|---|
| RTU-01 | RTU 帧结构 `[slave][fc][data][crc_lo][crc_hi]`，CRC 必校 | ✅ | `frames.rs` |
| RTU-02 | 异常帧定长 5 字节；读响应 `3 + byte_count + 2`；写响应 8 字节 | ✅ | `extract_modbus_frame` |
| RTU-03 | FC21（写文件记录）变长回显必须支持 | ✅ | `extract_modbus_frame` 新增 `0x15` 分支 |
| RTU-04 | 未知功能码不得被误判为完整帧（必须逐字节重同步） | ✅ | `extract_modbus_frame` 默认分支 |
| RTU-05 | 寄存器读 `expected_count` 校验公式 = `n * 2` | ✅ | `extract_modbus_frame` |
| RTU-06 | 线圈读 `expected_count` 校验公式 = `ceil(n / 8)` | ✅ | 同上；测试 `coil_read_with_expected_count_parses_valid_response` |
| RTU-07 | 帧间静默 ≥ 3.5 字符（写后、读前） | ✅ | `write_frame` 尾部 `gap_ms` |
| RTU-08 | DE 拉高后必须有建立时间再发字节 | ✅ | `DE_SETUP_TIME`（1ms，仅外部 DE 控制时生效） |
| RTU-09 | DE 拉低后不得立即进入读（避免吃首字节） | ✅ | 同 RTU-07 |
| RTU-10 | 读超时必须 ≥ 帧长 × 10bit / 波特率 + 余量 | ✅ | `BusManager::attempt_timeout_for`（`south/bus/mod.rs`）；测试 `adaptive_timeout_covers_frame_time_at_low_baud`（`south/bus/tests.rs`） |
| RTU-11 | 帧长估算只在 Modbus 总线启用（其它协议第 2 字节不是功能码） | ✅ | `BusManager::attempt_timeout_for` 的协议隔离（`south/bus/mod.rs`）；测试 `adaptive_timeout_is_disabled_for_non_modbus_protocol`（`south/bus/tests.rs`） |
| RTU-12 | 写超时预算按不重试推导（不白等 `retry_count` 轮） | ✅ | `transaction_inner`；测试 `write_reply_deadline_ignores_retry_budget`（写事务预算恒为 `1T + 余量`，不叠加重试轮次） |
| RTU-13 | ASCII 缓冲溢出不得整段丢弃（应保留末段） | ✅ | `serial_task.rs` 超过 `ASCII_BUF_LIMIT` 时保留末段并 `inc_ascii_frame_dropped()` —— **缺口 G-06 已关闭** |

## 5. Modbus TCP 特有

| # | 不变量 | 状态 | 守点 |
|---|---|---|---|
| TCP-01 | MBAP `protocol_id` 必须为 0、`length` 必须 ≤ 255 | ✅ | `read_mbap_response` |
| TCP-02 | 响应必须按 `tx_id` 路由到等待者 | ✅ | `TransactionPool.deliver` |
| TCP-03 | 响应 `unit_id` 必须与请求的从站地址一致 | ✅ | `transaction.rs::deliver(tx_id, unit_id, payload)` 校验 `pending.slave_id == unit_id`，不一致则丢弃并记原因 —— **缺口 G-01 已关闭** |
| TCP-04 | 写与写必须串行（同一 socket 上不可交叉） | ✅ | `write_sem = Semaphore(1)` |
| TCP-05 | 连接断开必须触发重连，且不得重发在途请求 | ✅ | `tcp_task` 代次校验（`Arc::ptr_eq`） |
| TCP-06 | 写超时必须作废在途事务并强制重连，且**不得重发**该写 | ✅ | `abort_all()` + `retry_allowed=false` |
| TCP-07 | 断链/写超时的原因与作废数量必须可观测 | ✅ | `abort_all` 返回计数 + 原因日志 |
| TCP-08 | 写与读共享同一 socket 时的并发上限必须一致 | ✅ | 池槽位 = `tcp_concurrent`，写保留槽位 |
| TCP-09 | 单台不应答设备不得占满整条总线的槽位 | ❌ | **缺口 G-05**：无 per-slave 限流/熔断/观测 |

## 6. 缺口清单（按 ROI）

| # | 缺口 | 工作量 | 价值 | 备注 |
|---|---|---|---|---|
| G-01 | TCP 响应 `unit_id` 不校验 | — | — | **已关闭**：`TransactionPool::deliver` 校验 `pending.slave_id == unit_id`，不一致丢弃并记原因（`ab44c3d`） |
| G-02 | API 契约未区分 `WriteUncertain`（错误码 / `outcome` 字段 / 前端文案） | — | — | **已关闭**：双轨契约（`results[].outcome` 三态 + 业务码 5102）+ 前端仅提示不阻断 |
| G-03 | 写超时读回消歧 | — | — | **已关闭**：单写 `read` / 批写 `read_batch` 读回，仅「读回值 == 写入值」正向收敛为成功；不一致保持 5102 并附读回值（不做反向判定，避免设备限幅/联锁导致误判可重试） |
| G-04 | 点位写入属性（写值 / 增量 / 脉冲）未建模 | ~~M~~ | 已关闭 | 决议 A（2026-11-26）：**不建模**。写属性由配置的 `function_code` 与 `data_type` 推导，写入值为绝对值；如现场出现增量/脉冲点位，再按 ROI-15 立项 |
| G-05 | TCP 单设备拖垮总线（无 per-slave 观测/熔断） | M~L | 中高 | 建议先加 per-slave NoReply 观测再定方案 |
| G-06 | RTU ASCII 缓冲溢出整段丢弃 | — | — | **已关闭**：`ASCII_BUF_LIMIT` + 收缩保留末段 + `inc_ascii_frame_dropped()`（`ab44c3d`） |
| G-07 | 写路径错误分级分散在各协议（无统一入口） | — | — | **已关闭**：统一走 `mark_write_uncertain` / `mark_write_uncertain_counted` |
| G-08 | 自研协议（s7comm / fins / mc / iec104 / bacnet）写结果未知**不计数** | — | — | **已关闭**：`mark_write_uncertain_with(metrics, e)`，5 个 collector 均注入 `Option<Arc<Metrics>>`（`9748c60`）；Prom 名 `gateway_bus_write_uncertain_total` |

## 7. 变更检查清单

改动 `south/bus/**` 或 `south/protocol/**` 时：

1. 是否影响 **B-01 / W-01**（写不重发、写不确定语义）？
2. 是否改变了 **B-06 / B-07**（保留槽位与采集并发的一致性）？
3. 新增的错误变体是否已从 **`is_retryable`** 中排除？
4. 新增的失败路径是否可观测（计数 / 日志）？
5. 新增 / 修改的写失败路径是否已按 **W-03 / W-03b / W-03c** 定级？
   （能证明未生效 → `DeviceRejected` 等；证明不了 → `mark_write_uncertain`）
6. 是否只改了**写**路径却顺手改动了**读**路径的错误口径？→ 违反 **W-14** 必须回退
7. 是否新增了需要守住的语义？→ 补进本表 + 补测试

改动 `gateway-api/**` 或 `gateway-web/**` 的写路径时：

8. 新增的写失败分支是否已按三态区分业务码（5101 明确失败 / 5102 结果未知）？
   → 违反 **W-07** 必须回退
9. 新增的响应字段是否带了 `outcome` 三态与 `uncertain` 计数？
   → 违反 **W-06** 必须回退
10. 前端遇到 `uncertain` 是否走了**提示**而不是**阻断**？（重试按钮保留，读回核实交操作员）
11. 是否新增了「读回核实」路径？→ 必须守住 **W-08**：只允许「读回值 == 写入值」正向收敛为成功；读回不等 / 读不到一律保持「结果未知」，容差只取半量化单位而**不得**用 `deadband`（deadband 过大会把没生效的写误判成成功）
