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
| B-02 | 读/采集事务允许按 `retry_count` 重发 | ✅ | `bus/mod.rs` 重试循环 |
| B-03 | 写事务的应答预算按「不重试」推导，不白等重试轮次 | ✅ | `transaction_inner` 中 `effective_retries` |
| B-04 | 调用方已放弃（超时/断开）的事务**不得下发** | ✅ | `prepare_dequeued` |
| B-05 | 写（控制）事务的出队优先级高于读 | ✅ | 双通道 + `select!{biased}` |
| B-06 | 写事务不得因采集占满配额而拿不到槽位 | ✅ | `reserved_control_slots`（唯一公式来源）；测试 `pool_control_transaction_reserves_slots_under_pressure` |
| B-07 | 采集并发上限必须与池对普通事务的准入上限一致 | ✅ | `BusManager::read_concurrency()`；测试 `read_concurrency_reserves_slots_for_control` |
| B-08 | 池满不得直接失败，应先短等待退避 | ✅ | `TransactionPool::acquire_slot`；测试 `pool_waits_for_slot_instead_of_failing_immediately` |
| B-09 | 池满/本地拥塞必须报 `BusBusy`，不得伪装成「设备不应答」 | ✅ | `map_txn_err`：`Busy → BusBusy` |
| B-10 | 事务前的陈旧字节必须排空 | ✅ | 串口 `clear_input()` |
| B-11 | 排空的陈旧字节必须可观测（非 0 = 上一帧没收完整） | ✅ | `gateway_bus_stale_bytes_dropped_total` + warn 日志 |
| B-12 | 串口在途事务不可被抢占（物理约束，非缺陷） | ⚠️ 约束 | RS485 半双工；需秒级写响应只能配独立串口 |
| B-13 | 串口故障分级：设备级失败不得重开端口 | ✅ | `serial_task` 自检帧 + `MIN_REOPEN_INTERVAL` |
| B-14 | 广播地址（slave=0）不得记为「成功通信过的从站」 | ✅ | `last_good_slave` 过滤 `slave == 0` |

## 2. 写语义（跨协议：Modbus / DLT645 / CJT188）

| # | 不变量 | 状态 | 守点 |
|---|---|---|---|
| W-01 | 写结果「可能已生效」必须用 `WriteUncertain` 上报，不得报成普通失败 | ✅ | `mark_write_uncertain`（协议无关实现） |
| W-02 | `WriteUncertain` **不可重试** | ✅ | `is_retryable` 刻意排除 |
| W-03 | 能证明写未生效的错误不得转成 `WriteUncertain`（`BusBusy` / `ModbusException` / `InvalidFrame`） | ✅ | `mark_write_uncertain` 白名单 |
| W-04 | 写结果未知必须计数 | ✅ | `gateway_write_uncertain_total` |
| W-05 | 写路径必须走控制通道（优先级） | ✅ | Modbus / DLT645 / CJT188 全部 `BusPriority::Control` |
| W-06 | 写不确定的错误必须传到 API 层可见 | 🟡 | 目前靠 `Display` 文案冒泡，无独立错误码 |
| W-07 | 上游（前端 / MES）不得对 `WriteUncertain` 自动重试 | ❌ | **缺口 G-02**：API 契约未区分 |
| W-08 | 写超时后应能读回核实（消除「未知」） | ❌ | **缺口 G-03**：读回消歧未实现 |
| W-09 | 点位应标记「写值 / 增量 / 脉冲」，以决定能否读回消歧 | ❌ | **缺口 G-04**：点位模型无写入属性 |
| W-10 | Modbus 写异常响应必须保留 `ModbusException` 语义 | ✅ | `trait_impl.rs` 先判 5 字节异常帧；测试 `write_exception_frame_maps_to_modbus_exception` |
| W-11 | 写响应按**语义字段**校验（slave/fc/addr/值），不得整帧比对 | ✅ | `verify_write_echo`（CRC 由定帧层负责） |

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
| RTU-10 | 读超时必须 ≥ 帧长 × 10bit / 波特率 + 余量 | ✅ | `effective_attempt_timeout`；测试 `adaptive_timeout_covers_frame_time_at_low_baud` |
| RTU-11 | 帧长估算只在 Modbus 总线启用（其它协议第 2 字节不是功能码） | ✅ | `BusManager.is_modbus`；测试 `adaptive_timeout_is_disabled_for_non_modbus_protocol` |
| RTU-12 | 写超时预算按不重试推导（不白等 `retry_count` 轮） | ✅ | `transaction_inner` |
| RTU-13 | ASCII 缓冲溢出不得整段丢弃（应保留末段） | ❌ | **缺口 G-06** |

## 5. Modbus TCP 特有

| # | 不变量 | 状态 | 守点 |
|---|---|---|---|
| TCP-01 | MBAP `protocol_id` 必须为 0、`length` 必须 ≤ 255 | ✅ | `read_mbap_response` |
| TCP-02 | 响应必须按 `tx_id` 路由到等待者 | ✅ | `TransactionPool.deliver` |
| TCP-03 | 响应 `unit_id` 必须与请求的从站地址一致 | ❌ | **缺口 G-01**：`let _unit_id = header[6]` 被丢弃 |
| TCP-04 | 写与写必须串行（同一 socket 上不可交叉） | ✅ | `write_sem = Semaphore(1)` |
| TCP-05 | 连接断开必须触发重连，且不得重发在途请求 | ✅ | `tcp_task` 代次校验（`Arc::ptr_eq`） |
| TCP-06 | 写超时必须作废在途事务并强制重连，且**不得重发**该写 | ✅ | `abort_all()` + `retry_allowed=false` |
| TCP-07 | 断链/写超时的原因与作废数量必须可观测 | ✅ | `abort_all` 返回计数 + 原因日志 |
| TCP-08 | 写与读共享同一 socket 时的并发上限必须一致 | ✅ | 池槽位 = `tcp_concurrent`，写保留槽位 |
| TCP-09 | 单台不应答设备不得占满整条总线的槽位 | ❌ | **缺口 G-05**：无 per-slave 限流/熔断/观测 |

## 6. 缺口清单（按 ROI）

| # | 缺口 | 工作量 | 价值 | 备注 |
|---|---|---|---|---|
| G-01 | TCP 响应 `unit_id` 不校验 | S | 低 | 异常网关下会误认应答 |
| G-02 | API 契约未区分 `WriteUncertain`（错误码 / `outcome` 字段 / 前端文案） | M | 高 | 上游无法判断该不该重试，重复写风险仍在 |
| G-03 | 写超时读回消歧 | M~L | 高 | 需先有 G-02 的语义定义 |
| G-04 | 点位写入属性（写值 / 增量 / 脉冲）未建模 | M | 中高 | 决定 G-03 能否落地，也是增量点位的正确性前提 |
| G-05 | TCP 单设备拖垮总线（无 per-slave 观测/熔断） | M~L | 中高 | 建议先加 per-slave NoReply 观测再定方案 |
| G-06 | RTU ASCII 缓冲溢出整段丢弃 | S | 低 | 仅 ASCII 模式且跨帧边界时触发 |

## 7. 变更检查清单

改动 `south/bus/**` 或 `south/protocol/**` 时：

1. 是否影响 **B-01 / W-01**（写不重发、写不确定语义）？
2. 是否改变了 **B-06 / B-07**（保留槽位与采集并发的一致性）？
3. 新增的错误变体是否已从 **`is_retryable`** 中排除？
4. 新增的失败路径是否可观测（计数 / 日志）？
5. 是否新增了需要守住的语义？→ 补进本表 + 补测试
