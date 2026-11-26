# 读写不变量 —— 测试守护核对

> **2026-11-26 复核**：本分册列的 11 处「清单该列已过时」（B-02、B-03、RTU-10、RTU-11、RTU-12、W-09、W-14、W-15、TCP-09、G-04、G-05）已在 `docs/architecture/read-write-invariants.md` 原文修正。本文件保留为 2026-11-22 基线快照，汇总计数不再代表现状（NB-04 已关闭）。

核对基线：当前工作区代码（2026-11-22，修正版）

## 汇总

| 指标 | 数值 |
|---|---|
| 条目总数 | 68 |
| 【有守护】 | 46 |
| 【部分守护】 | 12 |
| 【无守护】 | 10 |
| 清单「现有测试」列已过时 | 11 |

清单「现有测试」列已过时的 11 条：B-02、B-03、RTU-10、RTU-11、RTU-12、W-09、W-14、W-15、TCP-09、G-04、G-05。

- B-02/B-03/RTU-12：清单指向的用例不足以覆盖所述行为，现证据只剩 `bus_retry_budget` 纯函数算术；
- RTU-10/RTU-11：清单符号名 `effective_attempt_timeout` 已过时，实现为 `BusManager::attempt_timeout_for`（`gateway-core/src/south/bus/mod.rs:587`）；
- W-14：清单仍按 `InvalidFrame` 描述，ROI-25 统一后读写路径均返回 `DeviceRejected`；
- W-15：清单「现有测试」列仅写「仅文档说明」，与实际（有实现、无 IoError 用例）不符；
- W-09：清单指向 `points_write.rs` 的旧行为，现无「写属性」模型；
- TCP-09、G-04、G-05：清单标注 ❌ 未实现，实际口径与清单描述不同（详见对应条目依据）。

## 明细

| 编号 | 不变量要点 | 判定 | 守护证据（path:line + 测试名） | 判定依据 | 建议动作 |
|---|---|---|---|---|---|
| B-01 | 写事务超时不重发，返回结果不确定 | 【有守护】 | `gateway-core/src/south/protocol/modbus/tests.rs:607` `fn write_timeout_is_not_resent_and_reports_uncertain` | 断言假服务端只收到 1 次请求，且事务结果为 `WriteUncertain` | 保持 |
| B-02 | 读/采集事务允许按 retry_count 重发 | 【部分守护】 | `gateway-core/src/south/bus/tests.rs:1126` `fn bus_retry_budget_covers_all_attempts_and_retry_delays` | 清单该列已过时：该用例只断言 `bus_retry_budget` 纯函数的时间预算算术（`2/3/0` 次重试的 T 与 delay 组合、饱和运算），未断言「读路径确实重发 retry_count 次」；`gateway-core/src/south/protocol/modbus/tests.rs:1047` `fn batch_read_retries_recoverable_exception_once` 覆盖的是协议层可恢复异常重试一次，不是总线层 retry_count 重发 | 补断言 |
| B-03 | 写事务的应答预算按「不重试」推导 | 【部分守护】 | `gateway-core/src/south/bus/tests.rs:1126` `fn bus_retry_budget_covers_all_attempts_and_retry_delays` | 清单该列已过时：断言了 `bus_retry_budget(t,0,0)==t` 与多轮预算算术，但未断言「写事务实际传入 effective_retries=0」这一决策点 | 补断言 |
| B-04 | 出队时事务已被取消则不再下发 | 【有守护】 | `gateway-core/src/south/bus/tests.rs:169` `fn stale_transaction_is_skipped_and_not_written` | 断言 `prepare_dequeued` 之后事务未发出（帧计数为 0） | 保持 |
| B-05 | 控制/写事务优先于普通读事务出队 | 【有守护】 | `gateway-core/src/south/bus/tests.rs:317` `fn control_transaction_is_served_before_queued_normal_transaction` | 断言控制事务先于排队中的普通事务被服务 | 保持 |
| B-06 | 控制事务保留专属槽位不被读占满 | 【有守护】 | `gateway-core/src/south/protocol/modbus/tests.rs:363` `fn pool_control_transaction_reserves_slots_under_pressure` | 断言读事务占满普通槽位时控制事务仍能拿到保留槽位 | 保持 |
| B-07 | 读并发上限为「并发上限减控制预留」 | 【有守护】 | `gateway-core/src/south/bus/tests.rs:535` `fn read_concurrency_reserves_slots_for_control` | 断言读事务拿到 `max_concurrent - reserved_control_slots` 个槽位 | 保持 |
| B-08 | 无空闲槽位时排队等待而非立即失败 | 【有守护】 | `gateway-core/src/south/protocol/modbus/tests.rs:325` `fn pool_waits_for_slot_instead_of_failing_immediately` | 断言槽位耗尽时事务被挂起等待而非返回 Busy | 保持 |
| B-09 | 总线 Busy 映射为可重试的 Busy 错误 | 【有守护】 | `gateway-core/src/south/bus/tcp_task/tests.rs:84` `fn map_txn_err_covers_all_variants` | 逐变体断言 `map_txn_err` 的映射结果，Busy 映射到 `BusBusy` | 保持 |
| B-10 | 串口事务前排空输入缓冲，避免上一次残留 | 【有守护】 | `gateway-core/src/south/bus/serial_task/tests.rs:564` `fn stale_input_is_drained_before_transaction` | 断言残留字节被丢弃且当前事务应答正确解析 | 保持 |
| B-11 | 排空的陈旧字节必须可观测 | 【部分守护】 | `gateway-core/src/engine/metrics/bus.rs:154` 断言 `gateway_bus_stale_bytes_dropped_total` 出现在 Prometheus 输出 | 只断言指标名存在（渲染测试里手工 `fetch_add(7)`），未断言「串口排空确实递增该计数」这条因果链 | 补断言 |
| B-12 | 串口在途事务不可被抢占（RS485 半双工物理约束，非缺陷） | 【有守护】 | `in_flight_serial_transaction_is_not_preempted` | **已落地（决议 A，P2-2）**：串口 task 单事务串行，控制通道只在当前事务结束后消费；用例用门控串口把「读等应答」窗口拉长，断言控制写不落线、两帧不交错 | 已落地 |
| B-13 | 串口故障分级：自检失败才重开串口 | 【有守护】 | `gateway-core/src/south/bus/tests.rs:922` `fn self_check_passes_so_silent_device_does_not_reopen_port`；`gateway-core/src/south/bus/tests.rs:1006` `fn self_check_fails_when_all_slaves_silent_and_reopens_port` | 两个用例分别断言自检通过不重开、自检失败才重开 | 保持 |
| B-14 | 广播地址（slave=0）不参与自检从站集合 | 【无守护】 | — | 实现存在：`gateway-core/src/south/bus/serial_task.rs:354` 用 `filter(\|&s\| s != 0)` 把 0 从 `last_good_slave` 中滤掉；缺的是用例——`gateway-core/src/south/bus/tests.rs:1062` `fn self_check_skipped_without_known_good_slave_keeps_handle` 只覆盖「无已知良好从站」，没有任何用例构造 slave=0 的成功响应验证其被排除 | 新增用例 |
| W-01 | 写路径错误须经写语义定级，不得原样透传 | 【有守护】 | `gateway-core/src/south/protocol/write_semantics_tests.rs:24` `fn uncertain_errors_become_write_uncertain`；`gateway-core/src/south/protocol/write_semantics_tests.rs:51` `fn definite_failures_keep_their_variant` | 逐错误变体断言「不确定→WriteUncertain」与「确定失败→保留原变体」 | 保持 |
| W-02 | WriteUncertain 不可重试 | 【有守护】 | `gateway-api/src/web/error_codes.rs:349` `fn retryable_and_category_cover_all_codes`（逐码断言 `retryable(5102)==false`） | 断言了业务码表里 5102 的 retryable 为 false | 保持 |
| W-03 | 写错误按确定/不确定二分 | 【有守护】 | `gateway-core/src/south/protocol/write_semantics_tests.rs:51` `fn definite_failures_keep_their_variant` | 逐变体断言二分归属 | 保持 |
| W-03b | 传输层超时/断连→WriteUncertain | 【有守护】 | `gateway-core/src/south/protocol/write_semantics_tests.rs:24` `fn uncertain_errors_become_write_uncertain` | 断言 NoReply/Disconnected 映射为 WriteUncertain | 保持 |
| W-03c | 设备明确拒绝→DeviceRejected（与 InvalidFrame 区分） | 【有守护】 | `gateway-core/src/south/protocol/write_semantics_tests.rs:188` `fn device_rejected_is_distinct_from_invalid_frame`；`gateway-core/src/south/protocol/modbus/tests.rs:531` `fn write_exception_frame_maps_to_modbus_exception` | 前者断言两者不等价，后者断言 Modbus 异常帧映射为设备拒绝 | 保持 |
| W-04 | 不确定写须计数（可观测） | 【有守护】 | `gateway-core/src/south/protocol/write_semantics_tests.rs:151` `fn injected_metrics_counts_write_uncertain`；`gateway-core/src/south/protocol/write_semantics_tests.rs:98` `fn counted_variant_increments_metric_only_on_uncertain` | 断言注入计数器后只有不确定变体才递增 | 保持 |
| W-05 | 写路径走控制通道（高优先级） | 【无守护】 | — | 协议侧确实传 `BusPriority::Control`（`gateway-core/src/south/protocol/modbus/trait_impl.rs`、dlt645、cjt188），但没有任何用例断言「写事务以 Control 优先级入队」；`gateway-core/src/south/bus/tests.rs:317` 只测了手工构造的控制事务优先级，未覆盖协议写路径的实际投递优先级 | 新增用例 |
| W-06 | 写结果须带明确业务码 5102/5101 | 【有守护】 | `gateway-api/tests/points_write_unified.rs:239` `fn unified_write_single_entry_uncertain_returns_5102`；`gateway-api/tests/points_write_unified.rs:268` `fn unified_write_single_entry_definite_failure_stays_5101`；`gateway-api/tests/points_write_unified.rs:292` `fn unified_write_multi_entry_reports_per_item_outcome` | 断言单条/多条接口分别返回 5102、5101 且逐条带 outcome | 保持 |
| W-07 | 上游不得对结果不确定自动重试 | 【部分守护】 | `gateway-web/src/composables/data/usePointWrite.spec.ts:143` `单写 5102 结果未知：标 uncertain 并保留后端提示文案`（断言 `uncertain==true`、`status=='uncertain'`、`error` 含「不要自动重试」）；`gateway-web/src/utils/errorCodes.spec.ts:119`（断言 5102 属 `UNCERTAIN` 分类） | 该用例只锁定了「标记 uncertain + 保留提示文案」，没有断言「只调用一次 `writePoints`、无自动重发」：既未校验 mock 调用次数，也未用假定时器断言无二次写。`gateway-web/src/composables/data/usePointWrite.ts` 全文无 `retry`/`setTimeout`/`setInterval`，隔离观察下行为成立，但没有测试锁定 | 补断言 |
| W-08 | 写后回读核实，读回不一致不得报成功 | 【有守护】 | `gateway-api/tests/points_write_unified.rs:483` `fn unified_write_single_entry_uncertain_readback_confirms_success`；`gateway-api/tests/points_write_unified.rs:508` `fn unified_write_single_entry_uncertain_readback_mismatch_stays_5102`；`gateway-api/tests/points_write_unified.rs:537` `fn unified_write_multi_entry_readback_confirms_only_matching_items` | 断言回读匹配才转成功、不匹配仍返回 5102、多条只确认匹配项 | 保持 |
| W-09 | 点位**不建模**写属性（设定值/增量/脉冲），写通道由配置决定 | 【有守护】 | `write_function_code_is_configured_and_value_is_absolute` | **已落地（决议 A，P2-2）**：`PointConfig` 不引入写属性字段；写通道由 `function_code` 决定，寄存器宽度由 `data_type` 决定（多寄存器自动升 FC16），写入值为绝对值。用例抓取线上帧锁定 fc=6→FC06、缺省→FC06、f32→FC16、fc=1→FC05 | 已落地 |
| W-10 | Modbus 异常应答帧→设备拒绝 | 【有守护】 | `gateway-core/src/south/protocol/modbus/tests.rs:531` `fn write_exception_frame_maps_to_modbus_exception` | 断言异常帧不映射为 WriteUncertain | 保持 |
| W-11 | 写响应按语义字段校验，不整帧比对 | 【部分守护】 | `gateway-core/src/south/protocol/modbus/tests.rs:651` `fn write_echo_mismatch_is_write_uncertain` | 只断言了「回显不匹配→不确定」这一负向路径，未断言「回显按语义字段（slave/fc/addr/值）比对、忽略 CRC 等非语义字节」这一正向规则 | 补断言 |
| W-11b | 回显不匹配按结果不确定处理 | 【有守护】 | `gateway-core/src/south/protocol/modbus/tests.rs:651` `fn write_echo_mismatch_is_write_uncertain` | 断言回显不匹配返回 WriteUncertain | 保持 |
| W-12 | 自研协议/各协议写路径套用同一套写语义定级 | 【有守护】 | `gateway-core/src/south/protocol/s7comm/tests/e2e.rs:188` `write_error_response_is_device_rejected`、`:206` `write_no_reply_is_uncertain`；`gateway-core/src/south/protocol/fins/tests.rs:315` `test_write_end_code_nonzero_is_device_rejected`、`:331` `test_write_no_reply_is_uncertain`；`gateway-core/src/south/protocol/mc/tests/e2e.rs:117` `test_write_end_code_nonzero_is_device_rejected`、`:134` `test_write_no_reply_is_uncertain`；`gateway-core/src/south/protocol/iec104/tests/write_semantics.rs:72` `negative_confirmation_is_device_rejected`、`:99` `unknown_type_id_is_unsupported`、`:126` `no_confirmation_is_write_uncertain`；`gateway-core/src/south/protocol/bacnet/tests/write_rpm.rs:247` `write_property_bvlc_result_error_is_device_rejected`、`:289` `write_property_no_reply_is_uncertain` | S7/FINS/MC/IEC104/BACnet 五个协议的写语义用例齐全（每协议覆盖「明确拒绝→DeviceRejected」与「无应答→WriteUncertain」，IEC104 另覆盖不支持类型）；叠加 W-16 的 DLT645/CJT188 状态字用例，写定级已跨协议覆盖 | 保持 |
| W-13 | 写错误不得被笼统计入「不确定」 | 【有守护】 | `gateway-core/src/south/protocol/modbus/tests.rs:671` `fn write_unsupported_function_code_is_not_uncertain`；`gateway-core/src/south/protocol/write_semantics_tests.rs:51` `fn definite_failures_keep_their_variant` | 前者断言不支持的 FC 不算不确定，后者逐变体断言 | 保持 |
| W-14 | mc 读路径保留 check_end_code 语义 | 【有守护】 | `gateway-core/src/south/protocol/mc/tests/codec.rs:57` `test_check_end_code_is_device_rejected` | 清单该列已过时：ROI-25 统一后读路径不再是 `InvalidFrame`，该用例直接断言 `check_end_code(0xC059)` 返回 `DeviceRejected(msg)` 且 msg 含码值；实现 `gateway-core/src/south/protocol/mc/frames.rs:606` `check_end_code` 与 `:620` `check_write_end_code` 现均返回 `DeviceRejected`，读写口径由同一用例锁定 | 保持 |
| W-15 | GPIO 等本地 IO 写失败返回明确 IO 错误 | 【无守护】 | — | 清单该列自述「仅文档说明」，属清单已知无测试；`gateway-core/src/south/protocol/gpio/tests.rs` 只有 `test_gpio_write_rejects_di_pin`（`:137`）与 `test_gpio_write_rejects_invalid_address`（`:153`）两个 InvalidConfig 用例，无任何 `IoError` 断言；实现 `gateway-core/src/south/protocol/gpio/mod.rs:186`（`:170` 为注释声明）用 `map_err(\|e\| CollectorError::IoError(e.to_string()))` 映射硬件失败 | 新增用例 |
| W-16 | 写状态字非零→设备拒绝 | 【有守护】 | `gateway-core/src/south/protocol/dlt645/tests.rs:422` `fn write_status_nonzero_is_device_rejected`；`gateway-core/src/south/protocol/cjt188/tests.rs:487` `fn write_status_nonzero_is_device_rejected` | 两种自研协议各有一个用例断言状态字非零 → DeviceRejected | 保持 |
| R-01 | 读响应须校验从站地址与字节数 | 【有守护】 | `gateway-core/src/south/protocol/modbus/tests.rs:1138` `fn parse_read_response_rejects_every_single_bit_flip`；`gateway-core/src/south/protocol/modbus/tests.rs:694` `fn malformed_modbus_read_response_is_rejected_without_panic`；`gateway-core/src/south/protocol/modbus/tests.rs:737` `fn malformed_modbus_coils_response_is_rejected_without_panic` | 逐位翻转 + 畸形帧断言均被拒绝 | 保持 |
| R-02 | 读异常按配置/可恢复/未知三级分类 | 【有守护】 | `gateway-core/src/south/protocol/modbus/tests.rs:38` `fn test_classify_exception_config_error`；`:51` `fn test_classify_exception_recoverable`；`:64` `fn test_classify_exception_gateway_and_unknown` | 三类各有用例断言分类标签与 retryable 布尔 | 保持 |
| R-03 | 批量读按从站号与功能码分组 | 【部分守护】 | `gateway-core/src/south/protocol/modbus/tests.rs:1211` `fn roi11_read_group_key_separates_different_timeouts` | 该用例已断言 `read_group_key` 返回 `(slave_id, fc, timeout)` 三元组结构，并断言从站号参与分组键；但未断言「跨从站不合并」的运行时行为（多从站点位进入批量读时是否真的各成一组、绝不拼进同一帧） | 补断言 |
| R-04 | 写批量合并须地址连续 | 【有守护】 | `gateway-core/src/south/protocol/modbus/tests.rs:785` `fn write_batches_split_on_address_gap`；`:804` `fn write_batches_split_when_exceeding_frame_limit`；`:823` `fn write_batches_merge_exactly_at_limit` | 断言地址不连续即拆分、超帧长拆分、临界长度合并 | 保持 |
| R-05 | 读批量合并上限（寄存器 125 / 线圈 2000） | 【有守护】 | `gateway-core/src/south/protocol/modbus/tests.rs:849` `fn read_batches_never_cross_max_count`；`:868` `fn read_batches_width_follows_data_type_over_register_count` | 断言合并不超上限且按数据类型定宽 | 保持 |
| RTU-01 | RTU 帧必须过 CRC 校验 | 【有守护】 | `gateway-core/src/south/protocol/modbus/tests.rs:1138` `fn parse_read_response_rejects_every_single_bit_flip`；`gateway-core/src/south/protocol/modbus/tests.rs:1158` `fn extract_modbus_frame_random_bytes_never_panics` | 逐位翻转与随机字节用例覆盖 CRC 拒绝路径 | 保持 |
| RTU-02 | 异常帧/成功帧长度按功能码固定 | 【部分守护】 | `gateway-core/src/south/protocol/modbus/tests.rs:157` `fn extract_modbus_frame_fc01_uses_bit_count_formula`；`:187` `fn extract_modbus_frame_fc03_still_uses_register_formula`；`:204` `fn extract_modbus_frame_fc21_uses_byte_count` | 覆盖了 FC01/FC03/FC21 的长度公式，但未覆盖「异常帧固定 5 字节」与「写响应固定 8 字节」这两条长度约束 | 补断言 |
| RTU-03 | FC21 变长响应按字节数解析 | 【有守护】 | `gateway-core/src/south/protocol/modbus/tests.rs:204` `fn extract_modbus_frame_fc21_uses_byte_count` | 断言 FC21 按 byte_count 计算总长 | 保持 |
| RTU-04 | 未知功能码不得误判，需重同步 | 【有守护】 | `gateway-core/src/south/protocol/modbus/tests.rs:1111` `fn extract_modbus_frame_resyncs_to_valid_frame_after_garbage` | 断言垃圾前缀后能重同步到合法帧 | 保持 |
| RTU-05 | 寄存器读 expected_count = 数量×2 | 【有守护】 | `gateway-core/src/south/protocol/modbus/tests.rs:1177` `fn read_roundtrip_covers_coil_and_register_length_formulas` | 断言线圈/寄存器两种长度公式的往返一致 | 保持 |
| RTU-06 | 线圈读字节数向上取整 | 【有守护】 | `gateway-core/src/south/protocol/modbus/tests.rs:551` `fn coil_read_with_expected_count_parses_valid_response`；`gateway-core/src/south/protocol/modbus/tests.rs:157` `fn extract_modbus_frame_fc01_uses_bit_count_formula` | 断言线圈按位向上取整并正确解析 | 保持 |
| RTU-07 | 帧间隔须满足 3.5 字符静默时间 | 【无守护】 | — | `gateway-core/src/south/bus/serial_task/tests.rs` 内无任何 gap/3.5 字符静默断言；实现见 `gateway-core/src/south/bus/serial_task.rs:736-737`（`gap_ms = 35_000.div_ceil(baud).max(1)` 后 `sleep`），该计算与等待时长无测试覆盖 | 新增用例 |
| RTU-08 | 写帧前 DE 拉高并保持建立时间 | 【部分守护】 | `gateway-core/src/south/bus/serial_task/tests.rs:412` `fn de_goes_high_before_write_and_low_before_read` | 断言了 DE 的电平时序（写前拉高、读前拉低），未断言「DE 建立时间达到规范值」这一量化约束 | 补断言 |
| RTU-09 | 读帧前 DE 拉低 | 【部分守护】 | `gateway-core/src/south/bus/serial_task/tests.rs:412` `fn de_goes_high_before_write_and_low_before_read` | 该用例只测 DE 电平（写前高、读前低），不测静默时长，因此对「读前 DE 拉低并保持到线路静默达标」只覆盖了电平一半，属部分守护 | 补断言 |
| RTU-10 | 读超时须覆盖整帧传输时间 | 【有守护】 | `gateway-core/src/south/bus/tests.rs:597` `fn adaptive_timeout_covers_frame_time_at_low_baud` | 断言低波特率下自适应超时覆盖整帧传输时间；注意清单该列已过时，清单写的符号名 `effective_attempt_timeout` 已不存在，实现为 `BusManager::attempt_timeout_for`（`gateway-core/src/south/bus/mod.rs:587`），用例位于 `bus/tests.rs` 而非 `serial_task/tests.rs` | 保持 |
| RTU-11 | 自适应超时仅用于 Modbus | 【有守护】 | `gateway-core/src/south/bus/tests.rs:677` `fn adaptive_timeout_is_disabled_for_non_modbus_protocol` | 断言非 Modbus 协议禁用自适应超时；同样清单该列符号名 `effective_attempt_timeout` 已过时（现为 `BusManager::attempt_timeout_for`，`bus/mod.rs:587`），且用例在 `bus/tests.rs` 不在 `serial_task/tests.rs` | 保持 |
| RTU-12 | 写帧超时预算按不重试推导 | 【部分守护】 | `gateway-core/src/south/bus/tests.rs:1126` `fn bus_retry_budget_covers_all_attempts_and_retry_delays` | 清单该列已过时：断言了预算纯函数（含 `retries=0` 时等于单次 timeout），未断言「写路径确实以 effective_retries=0 预算下发超时」 | 补断言 |
| RTU-13 | ASCII 缓冲溢出须保留最后一段起始位 | 【有守护】 | `gateway-core/src/south/bus/serial_task/tests.rs:627` `fn ascii_overflow_drops_partial_frame_and_accepts_next_valid_frame`；`:677` `fn ascii_overflow_keeps_started_frame_and_completes_after_next_read`；`:723` `fn shrink_ascii_buf_keeps_last_frame_start` | 三个用例覆盖丢弃残帧、续帧、收缩保留起始位 | 保持 |
| TCP-01 | MBAP 长度须 ≤ 253 且与实际一致 | 【有守护】 | `gateway-core/src/south/protocol/modbus/transaction/frame.rs:173` `fn oversized_mbap_length_is_rejected_before_reading_body`；`:202` `fn maximum_mbap_length_is_accepted` | 断言超限拒绝、临界长度接受 | 保持 |
| TCP-02 | 响应按 tx_id 路由到等待者 | 【有守护】 | `gateway-core/src/south/protocol/modbus/tests.rs:413` `fn pool_accepts_response_with_matching_unit_id`；`gateway-core/src/south/protocol/modbus/tests.rs:325` `fn pool_waits_for_slot_instead_of_failing_immediately` | 前者断言匹配响应原样交给对应调用方（tx_id 命中才投递），后者断言并发事务拿到不同 `tx_id`（`tx2 != tx_id`）而不复用；实现 `gateway-core/src/south/protocol/modbus/transaction.rs:396-414` 以 `active.remove(&tx_id)` 按 tx_id 精确路由 | 保持 |
| TCP-03 | 响应 unit_id 须与请求一致 | 【有守护】 | `gateway-core/src/south/protocol/modbus/tests.rs:392` `fn pool_rejects_response_with_mismatched_unit_id`；`:413` `fn pool_accepts_response_with_matching_unit_id` | 断言不匹配被拒、匹配被接受 | 保持 |
| TCP-04 | 同一连接的写事务必须串行 | 【无守护】 | — | 实现存在：`gateway-core/src/south/bus/tcp_task.rs:100` `Semaphore::new(1)`、`:243` `write_sem.acquire().await`，但全仓 tests 中 `write_sem` 零命中，无任何「写与写串行」断言；原版引用的槽位隔离用例与写串行无关 | 新增用例 |
| TCP-05 | 断线重连须使旧事务失效 | 【有守护】 | `gateway-core/src/south/bus/tcp_task/tests.rs:226` `fn stale_disconnect_does_not_clear_new_pool`；`:388` `fn disconnect_aborts_pool_and_triggers_reconnect` | 断言旧代次池不会被新代次清理、断线会中止池并触发重连 | 保持 |
| TCP-06 | 写超时须中止在途事务且不重发 | 【有守护】 | `gateway-core/src/south/protocol/modbus/transaction/tests.rs:60` `fn write_timeout_aborts_pool_and_returns_disconnected`；`gateway-core/src/south/protocol/modbus/tests.rs:607` `fn write_timeout_is_not_resent_and_reports_uncertain` | 前者断言超时清空在途事务，后者断言不重发 | 保持 |
| TCP-07 | 中止原因须可观测 | 【部分守护】 | `gateway-core/src/south/bus/tcp_task/tests.rs:388` `fn disconnect_aborts_pool_and_triggers_reconnect` | 实现侧已有可观测性：`gateway-core/src/south/protocol/modbus/transaction.rs:417-429` `abort_all` 返回作废事务数 `count`，`:400` 对 unit_id 不匹配有 warn 原因日志；但测试只断言 `conns==2` 与 `io_errors>0`，未断言计数与中止原因被记录 | 补断言 |
| TCP-08 | 写与读共享同一 socket 的并发上限 | 【部分守护】 | `gateway-core/src/south/bus/tests.rs:535` `fn read_concurrency_reserves_slots_for_control` | 只覆盖读并发与控制预留的槽位计算，未断言「写占用同一 socket 时与读共享同一上限」 | 补断言 |
| TCP-09 | 单台不应答设备不得占满整条总线槽位 | 【有守护（决议 A）】 | `single_slave_timeout_is_bounded_by_retry_and_backoff` | **已落地（2026-11-26）**：不做 per-slave 限流/熔断，沿用「总线每事务 `retry_count` 上限 + 设备退避 5×/15×/60×」；用例断言退避中的设备不按基础周期重试、同总线健康点仍按基础周期到期 | 已关闭 |
| G-01 | Modbus TCP 响应 unit_id 必须与请求一致 | 【有守护】 | `gateway-core/src/south/protocol/modbus/tests.rs:392` `fn pool_rejects_response_with_mismatched_unit_id` | 缺口关闭后行为由该用例守护：断言 unit_id 不匹配的响应被拒绝（同 TCP-03） | 保持 |
| G-02 | 上游必须能区分「写失败」与「结果不确定」 | 【有守护】 | `gateway-api/tests/points_write_unified.rs:239` `fn unified_write_single_entry_uncertain_returns_5102`；`:292` `fn unified_write_multi_entry_reports_per_item_outcome` | 缺口关闭后由接口用例守护：5102 与 5101 区分、逐条带 outcome（同 W-06） | 保持 |
| G-03 | 写路径需可回读核实 | 【有守护】 | `gateway-api/tests/points_write_unified.rs:483` `fn unified_write_single_entry_uncertain_readback_confirms_success`；`:508` `fn unified_write_single_entry_uncertain_readback_mismatch_stays_5102` | 缺口关闭后由回读用例守护：匹配才成功、不匹配仍 5102（同 W-08） | 保持 |
| G-04 | 点位写入属性（设定值/增量/脉冲）未建模 | 【已关闭（不做）】 | 同 W-09 | **决议 A（P2-2）：不建模**。写属性由 `function_code` + `data_type` 推导，写入值为绝对值；如现场出现增量/脉冲点位，再按 ROI-15 立项 | 已关闭 |
| G-05 | 单台不应答设备不得占满整条总线槽位 | 【已关闭（不做 per-slave 限流）】 | 同 TCP-09 | **决议 A（2026-11-26）**：不建模 per-slave 限流/熔断，沿用总线级退避；ROI-14 保留为现场实测后升级项 | 已关闭 |
| G-06 | RTU ASCII 溢出保最后段起始 | 【有守护】 | `gateway-core/src/south/bus/serial_task/tests.rs:723` `fn shrink_ascii_buf_keeps_last_frame_start`；`:677` `fn ascii_overflow_keeps_started_frame_and_completes_after_next_read` | 缺口关闭后由 ASCII 溢出用例守护最后段起始位保留（同 RTU-13） | 保持 |
| G-07 | 写路径统一定级 | 【有守护】 | `gateway-core/src/south/protocol/write_semantics_tests.rs:51` `fn definite_failures_keep_their_variant`；`:24` `fn uncertain_errors_become_write_uncertain` | 缺口关闭后由写语义定级用例逐变体守护（同 W-03/W-03b） | 保持 |
| G-08 | 不确定写必须计数 | 【有守护】 | `gateway-core/src/south/protocol/write_semantics_tests.rs:151` `fn injected_metrics_counts_write_uncertain` | 缺口关闭后由指标用例守护：仅不确定变体递增计数（同 W-04） | 保持 |

## 待补守护清单

（只列【部分守护】和【无守护】的条目，共 22 条，按编号排序）

| 编号 | 缺什么 | 建议测试名 | 建议放置位置 |
|---|---|---|---|
| B-02 | 缺「读路径确实按 retry_count 重发且次数正确」的断言（现有只测预算算术与协议层异常重试） | `fn read_transaction_is_retried_up_to_retry_count` | `gateway-core/src/south/bus/tests.rs` |
| B-03 | 缺「写事务以 effective_retries=0 计算应答预算」的断言 | `fn write_transaction_uses_zero_retry_budget` | `gateway-core/src/south/bus/tests.rs` |
| B-11 | 缺「串口排空陈旧字节后计数确实递增」的因果断言（现有仅断言指标名出现在渲染输出） | `fn draining_stale_input_increments_stale_bytes_metric` | `gateway-core/src/south/bus/serial_task/tests.rs` |
| B-12 | 已落地（P2-2）：串口在途事务不被抢占/交错 | `in_flight_serial_transaction_is_not_preempted` | `gateway-core/src/south/bus/serial_task/tests.rs` |
| B-14 | 缺 slave=0 广播成功响应不计入良好从站集合的断言（实现已滤除，`:354`） | `fn broadcast_slave_zero_is_excluded_from_known_good_slaves` | `gateway-core/src/south/bus/tests.rs` |
| W-05 | 缺「协议写路径以 Control 优先级入队」的断言 | `fn protocol_write_submits_with_control_priority` | `gateway-core/src/south/protocol/modbus/tests.rs` |
| W-07 | 缺「只调用一次 writePoints、无自动重发」的断言（现有只锁定 uncertain 标记与提示文案） | `fn uncertain_write_is_not_automatically_retried` | `gateway-web/src/composables/data/usePointWrite.spec.ts` |
| W-09 | 已落地（P2-2）：写通道由配置决定、写入值为绝对值；不建模写属性 | `write_function_code_is_configured_and_value_is_absolute` | `gateway-core/src/south/protocol/modbus/tests.rs` |
| W-11 | 缺「写回显按语义字段比对、忽略 CRC 等非语义字节」的正向断言 | `fn write_echo_compares_semantic_fields_only` | `gateway-core/src/south/protocol/modbus/tests.rs` |
| W-15 | 缺「GPIO 写硬件失败→CollectorError::IoError」的断言（现有只有 InvalidConfig 用例） | `fn gpio_write_hardware_failure_maps_to_io_error` | `gateway-core/src/south/protocol/gpio/tests.rs` |
| R-03 | 缺「跨从站不合并」的运行时断言（现有只锁定分组键三元组结构） | `fn read_batches_never_merge_across_slaves` | `gateway-core/src/south/protocol/modbus/tests.rs` |
| RTU-02 | 缺「异常帧固定 5 字节」「写响应固定 8 字节」两条长度约束的断言 | `fn extract_modbus_frame_enforces_exception_and_write_lengths` | `gateway-core/src/south/protocol/modbus/tests.rs` |
| RTU-07 | 缺帧间隔（3.5 字符静默时间）计算的断言（实现 `serial_task.rs:736-737`） | `fn inter_frame_gap_meets_three_and_half_char_silence` | `gateway-core/src/south/bus/serial_task/tests.rs` |
| RTU-08 | 缺 DE 建立时间达到规范值的量化断言（现有只测电平时序） | `fn de_setup_time_is_enforced_before_write` | `gateway-core/src/south/bus/serial_task/tests.rs` |
| RTU-09 | 缺「读前 DE 拉低并保持到线路静默达标」的时长断言（现有只测电平） | `fn de_stays_low_through_inter_frame_silence` | `gateway-core/src/south/bus/serial_task/tests.rs` |
| RTU-12 | 缺「写路径以 0 重试预算下发超时」的断言 | `fn write_frame_timeout_uses_no_retry_budget` | `gateway-core/src/south/bus/serial_task/tests.rs` |
| TCP-04 | 缺「两个写事务不得并发下发到同一 TCP socket」的断言（实现有 `Semaphore::new(1)`，无测试） | `fn write_transactions_are_serialized_per_connection` | `gateway-core/src/south/bus/tcp_task/tests.rs` |
| TCP-07 | 缺「中止数量与中止原因被记录」的可观测性断言 | `fn abort_reason_and_count_are_recorded` | `gateway-core/src/south/bus/tcp_task/tests.rs` |
| TCP-08 | 缺「写与读共享同一 socket 并发上限」的断言 | `fn write_and_read_share_the_same_concurrency_limit` | `gateway-core/src/south/bus/tcp_task/tests.rs` |
| TCP-09 | 已落地（决议 A）：不做 per-slave 限流，设备退避 + 总线重试上限保证不被单台占满 | `single_slave_timeout_is_bounded_by_retry_and_backoff` | `gateway-core/src/south/scheduler/spawning.rs` |
| G-04 | 已关闭（决议 A：不建模，与 W-09 合并） | `write_function_code_is_configured_and_value_is_absolute` | `gateway-core/src/south/protocol/modbus/tests.rs` |
| G-05 | 已关闭（决议 A：不建模 per-slave 限流，与 TCP-09 合并） | `single_slave_timeout_is_bounded_by_retry_and_backoff` | `gateway-core/src/south/scheduler/spawning.rs` |
