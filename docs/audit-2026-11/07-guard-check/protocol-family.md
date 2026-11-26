# 协议族不变量 —— 测试守护核对

> **2026-11-26 复核**：本分册列的 10 处「清单该列已过时」（MC-R4、MC-R7、MC-W1、MC-W2、MC-W4、MC-W5、MSTP-R5、UA-R7、UA-E2、IEC-R4）已在 `docs/architecture/protocol-family-invariants.md` 原文修正。本文件保留为 2026-11-22 基线快照，汇总计数不再代表现状（NB-04 已关闭）。

核对基线：当前工作区代码（2026-11-22）

## 汇总

| 指标 | 数值 |
|---|---|
| 条目总数 | 83 |
| 【有守护】 | 46 |
| 【部分守护】 | 15 |
| 【无守护】 | 22 |
| 清单「现有测试」列已过时 | 10 |

## 明细

| 编号 | 不变量要点 | 判定 | 守护证据（path:line + 测试名） | 判定依据 | 建议动作 |
|---|---|---|---|---|---|
| MC-R1 | 地址前缀映射到软元件码，编号可解析且 ≤ 0x00FF_FFFF，非法在建链前返回 `InvalidConfig` | 【部分守护】 | `gateway-core/src/south/protocol/mc/tests/address_parse.rs:39` `test_parse_address_errors` | 只对 4 个非法串断言 `is_err`，未断言 0x00FF_FFFF 上限与 `InvalidConfig` 变体 | 补断言 |
| MC-R2 | 3E/4E 响应校验子头、固定头长度及网络号/PC 号/模块 I/O 号/站号回显 | 【有守护】 | `gateway-core/src/south/protocol/mc/tests/codec.rs:68` `test_3e_response_header_echo_mismatch_rejected`；`:91` `test_4e_response_header_echo_mismatch_rejected` | 两个用例分别断言 3E/4E 回显不匹配被拒 | 保持 |
| MC-R3 | 4E 响应回显请求序列号，不匹配即判帧错 | 【有守护】 | `gateway-core/src/south/protocol/mc/tests/serial_4e.rs:87` `test_4e_serial_mismatch_rejected` | 断言 4E 序列号不匹配被拒 | 保持 |
| MC-R4 | 「结束码 + 数据」长度落在 2..=960 字节，禁止按对端长度直接分配 | 【无守护】 | — | 清单该列已过时（引用的 `gateway-core/fuzz/fuzz_targets/mc_decode.rs` 文件不存在）；无单测覆盖该长度边界 | 新增用例 |
| MC-R5 | 读结束码非 0 → `DeviceRejected`，不得当有效数据 | 【有守护】 | `gateway-core/src/south/protocol/mc/tests/e2e.rs:66` `test_error_end_code_is_rejected`；`gateway-core/src/south/protocol/mc/tests/codec.rs:57` `test_check_end_code_is_device_rejected` | 断言读结束码非 0 映射为 `DeviceRejected` | 保持 |
| MC-R6 | 1E 响应功能码等于请求功能码按位或 0x80；数据长度由点数推导且 ≤ 512 字节 | 【部分守护】 | `gateway-core/src/south/protocol/mc/tests/frame_1e.rs:11` `test_e2e_1e_read_word_over_tcp`；`:25` `test_e2e_1e_read_batch_and_write`；`:55` `test_e2e_1e_bit_read` | 只走桩正常路径，无功能码不匹配与 ≤ 512 上限断言 | 补断言 |
| MC-R7 | 单次 IO 超时取 `bus.timeout_ms`；发送/接收超时 → `NoReply` 并清空连接、下次惰性重建 | 【部分守护】 | `gateway-core/src/south/protocol/mc/tests/e2e.rs:150` `test_no_response_times_out`；`:81` `test_reconnect_after_connection_drop` | 用例断言 `IoError(_) \| NoReply` 未锁 `NoReply`；清单该列已过时（`:147`/`:78` 落在用例收尾 `}`） | 补断言 |
| MC-R8 | 批量读按软元件分组、地址升序合并，超限自动分片（479 字 / 1916 位 / 1E 256 单元） | 【有守护】 | `gateway-core/src/south/protocol/mc/tests/batch.rs:81` `test_plan_batches_splits_over_frame_limit`；`:172` `batch_read_splits_when_over_frame_limit`；`:196` `batch_read_1e_splits_at_256_units` | 断言超帧上限分片与 1E 256 单元分片 | 保持 |
| MC-R9 | 批量片段失败只污染该片段点位；建链失败整批同因 `IoError`，不逐点重复等建链 | 【有守护】 | `gateway-core/src/south/protocol/mc/tests/batch.rs:33` `batch_read_same_error_when_device_unreachable`；`:12` `batch_read_connects_once` | 断言设备不可达时整批同因、只建链一次 | 保持 |
| MC-R10 | 批量解码越界只让该点位 `InvalidFrame`，不得整批丢弃 | 【无守护】 | — | 无用例覆盖批量解码数据区不足时的逐点降级 | 新增用例 |
| MC-R11 | TCP 读用定长 `read_exact` 无半包/粘包；同一连接流锁串行化，分片帧不交叉 | 【无守护】 | — | 无用例断言批量与诊断并发时不读对方应答（`batch.rs:12` 只断言只建链一次） | 新增用例 |
| MC-W1 | 写结束码非 0 → `DeviceRejected` | 【有守护】 | `gateway-core/src/south/protocol/mc/tests/e2e.rs:117` `test_write_end_code_nonzero_is_device_rejected` | 断言写结束码非 0 → `DeviceRejected`；清单该列已过时（`:114` 是 doc comment，真实 fn 在 `:117`） | 保持 |
| MC-W2 | 写传输失败/无应答 → `WriteUncertain`，且不自动重发 | 【有守护】 | `gateway-core/src/south/protocol/mc/tests/e2e.rs:134` `test_write_no_reply_is_uncertain` | 断言无应答映射为 `WriteUncertain`；清单该列已过时（`:131` 为空行，真实 fn 在 `:134`） | 保持 |
| MC-W3 | 写值必须做类型范围校验，越界应在上线前拒绝 | 【无守护】 | — | 不成立：`gateway-core/src/south/protocol/mc/frames.rs:608-624` 用 `as` 强转，越界静默截断，无用例 | 需产品口径 |
| MC-W4 | 写后协议层不读回，读回仅由 API 层在 uncertain 时触发 | 【无守护】 | — | 无用例锁定「协议层不读回」；清单该列已过时（`:101` 落在 `}`） | 新增用例 |
| MC-W5 | 写事务不得自动重发 | 【无守护】 | — | 无用例断言重发次数；清单该列已过时（`:131` 为空行） | 新增用例 |
| MC-E1 | 读结束码可区分「地址不存在/设备拒绝」与「帧损坏」 | 【有守护】 | `gateway-core/src/south/protocol/mc/tests/e2e.rs:66` `test_error_end_code_is_rejected`；`gateway-core/src/south/protocol/mc/tests/codec.rs:57` `test_check_end_code_is_device_rejected` | 断言结束码非 0 → `DeviceRejected`，与 `InvalidFrame` 区分 | 保持 |
| MC-E2 | 1E 状态码与 3E/4E 结束码同口径映射 | 【无守护】 | — | 无用例断言 1E 状态码的映射口径 | 新增用例 |
| FINS-R1 | 地址前缀映射字/位存储区码，编号为 `u16`；非法配置返回 `InvalidConfig` | 【有守护】 | `gateway-core/src/south/protocol/fins/tests.rs:21` `test_parse_address_areas`；`:37` `test_parse_address_errors` | 断言存储区解析与非法地址报错 | 保持 |
| FINS-R2 | FINS/TCP 报头以 `FINS` 开头，长度在 8..=2048 内，再按长度读满 | 【无守护】 | — | 无 `8..=2048` 长度边界断言（`tests.rs:731` 只测载荷截断） | 新增用例 |
| FINS-R3 | 读响应长度 ≥ 14，且 MRC/SRC 与请求一致 | 【有守护】 | `gateway-core/src/south/protocol/fins/tests.rs:78` `test_parse_read_response_ok_and_error` | 断言 MRC/SRC 不匹配被拒 | 保持 |
| FINS-R4 | 读响应 MRES/SRES 非 0 → `InvalidFrame`（读路径） | 【有守护】 | `gateway-core/src/south/protocol/fins/tests.rs:78` `test_parse_read_response_ok_and_error`；`:301` `test_error_end_code_is_rejected` | 断言读结束码非 0 报帧错 | 保持 |
| FINS-R5 | 节点地址握手校验 `command == 0x0000_0002` 且 `tcp_error == 0`，取 4 字节载荷低字节为服务端节点 | 【无守护】 | — | 无用例覆盖握手失败分支（`tests.rs:44` 只测请求布局） | 新增用例 |
| FINS-R6 | 单次发送/接收超时取 `bus.timeout_ms`；超时 → `NoReply` 并丢弃连接 | 【部分守护】 | `gateway-core/src/south/protocol/fins/tests.rs:331` `test_write_no_reply_is_uncertain` | 只覆盖写路径 `NoReply`，读路径超时无断言 | 补断言 |
| FINS-R7 | 批量读整批共享一次建链；建链失败整批同因返回，不逐点重复建链 | 【有守护】 | `gateway-core/src/south/protocol/fins/tests.rs:364` `batch_read_connects_once`；`:384` `batch_read_same_error_when_device_unreachable` | 断言整批共享建链且同因失败 | 保持 |
| FINS-R8 | UDP 应答必须能区分「本次请求的应答」与「上一次的迟到应答」 | 【无守护】 | — | 不成立：`gateway-core/src/south/protocol/fins/mod.rs:262-272` 直接取首个数据报，不校验源地址、SID 或回显；`tests.rs:552`、`:570` 只覆盖正常收发 | 需产品口径 |
| FINS-W1 | CPU 控制地址 `RUN`/`STOP`/`CPU:RUN`/`CPU:STOP` 决定动作，走 0401/0402 | 【有守护】 | `gateway-core/src/south/protocol/fins/tests.rs:414` `test_parse_cpu_control_address`；`:444` `test_e2e_cpu_run_over_tcp`；`:453` `test_e2e_cpu_stop_over_tcp` | 断言 RUN/STOP 地址解析与端到端下发 | 保持 |
| FINS-W2 | 写响应 MRES/SRES 非 0 → `DeviceRejected` | 【有守护】 | `gateway-core/src/south/protocol/fins/tests.rs:315` `test_write_end_code_nonzero_is_device_rejected` | 断言写结束码非 0 → `DeviceRejected` | 保持 |
| FINS-W3 | 写传输失败 → `WriteUncertain`，不自动重发 | 【部分守护】 | `gateway-core/src/south/protocol/fins/tests.rs:331` `test_write_no_reply_is_uncertain` | 只验证定级为 `WriteUncertain`，未断言不重发次数 | 补断言 |
| FINS-W4 | 写数值必须做类型范围校验 | 【无守护】 | — | 不成立：`gateway-core/src/south/protocol/fins/frames.rs:405-417` 用 `as` 强转，无用例 | 需产品口径 |
| FINS-W5 | 写后协议层不读回，仅 API 层 uncertain 读回 | 【无守护】 | — | 无用例锁定协议层不读回 | 新增用例 |
| FINS-E1 | FINS/TCP 报头错误码（`tcp_error != 0`）应映射为设备拒绝而非帧损坏 | 【无守护】 | — | 不成立：`gateway-core/src/south/protocol/fins/mod.rs:244-249` 返回 `InvalidFrame`，无用例 | 需产品口径 |
| FINS-E2 | 读结束码应区分地址类错误与命令类错误 | 【无守护】 | — | 不成立：`gateway-core/src/south/protocol/fins/frames.rs:339-343` 统一 `InvalidFrame`，无用例 | 需产品口径 |
| S7-R1 | 地址解析支持 DB/M/I/Q 与位/字节/字/双字；DB 必须含 `.`，越界返回 `InvalidConfig` | 【有守护】 | `gateway-core/src/south/protocol/s7comm/tests/address_parse.rs:5-124`（`test_parse_addr_plain` 等共 14 例） | 14 个用例覆盖各区域合法形式与非法地址 | 保持 |
| S7-R2 | TPKT 必须校验版本 `0x03 0x00` 且总长 ≥4；读取前必须有上限 | 【无守护】 | — | `gateway-core/src/south/protocol/s7comm/frames/link.rs:54-80` `read_tpkt_frame` 无总长上限，无用例 | 新增用例 |
| S7-R3 | 响应校验 Protocol ID `0x32`、PDU type、`param_len/data_len` 切片边界，不得越界读 | 【有守护】 | `gateway-core/src/south/protocol/s7comm/tests/codec.rs:145` `test_parse_read_response_real`；`:176` `test_parse_read_response_error`；`tests/multi_read.rs:83` `test_parse_read_response_multi_all_ok`；`:129` `test_parse_read_response_multi_partial_failure`；`:171` `test_parse_read_response_multi_count_mismatch` | 断言读响应解析与切片边界 | 保持 |
| S7-R4 | 单点读 `return_code`：`RET_NOT_FOUND` → `InvalidConfig`，其它非 0 → `DeviceRejected` | 【有守护】 | `gateway-core/src/south/protocol/s7comm/tests/codec.rs:209` `test_parse_read_response_not_found`；`:235` `test_parse_read_response_rejected_return_code` | 断言两种返回码各自映射 | 保持 |
| S7-R5 | 值数据长度必须 ≥ 数据类型期望字节数，否则 `InvalidFrame` | 【部分守护】 | `gateway-core/src/south/protocol/s7comm/tests/codec.rs:138` `test_decode_too_short` | 只断言 `is_err`，未锁 `InvalidFrame` 变体 | 补断言 |
| S7-R6 | 多 item 读校验 `item_count` 与请求一致；单 item 失败只污染该点；按协商 PDU 分片 | 【有守护】 | `gateway-core/src/south/protocol/s7comm/tests/multi_read.rs:129` `test_parse_read_response_multi_partial_failure`；`:171` `test_parse_read_response_multi_count_mismatch`；`tests/regression.rs:18` `read_batch_chunks_by_negotiated_pdu_and_maps_by_index` | 断言逐项失败隔离、数量不匹配与按 PDU 分片后按下标映射 | 保持 |
| S7-R7 | 惰性建链；IO 失败清空连接，下次调用自动重连 | 【有守护】 | `gateway-core/src/south/protocol/s7comm/tests/e2e.rs:17` `c13_read_lazily_connects_over_tcp`；`:64` `c13_reconnect_after_connection_drop` | 断言首次读惰性建链与断线后重连 | 保持 |
| S7-R8 | 应答 PDU reference 必须与请求一致（防串包），读写路径同口径 | 【有守护】 | `gateway-core/src/south/protocol/s7comm/tests/codec.rs:371` `test_check_pdu_ref_echo`；`:387` `test_check_pdu_ref_rejects_short_frame` | 断言 PDU reference 回显校验 | 保持 |
| S7-R9 | 协商得到的 PDU 大小必须落在合理区间 | 【无守护】 | — | 不成立：`gateway-core/src/south/protocol/s7comm/collector/mod.rs:109-115` 仅 `if pdu > 0`，无区间上下界，无用例 | 需产品口径 |
| S7-W1 | 写值范围校验在上线前完成，越界 → `InvalidConfig` | 【有守护】 | `gateway-core/src/south/protocol/s7comm/tests/write_codec.rs:62` `test_encode_value_ranges`；`:94` `test_build_write_var_rejects_out_of_range` | 断言各类型取值范围与越界拒绝 | 保持 |
| S7-W2 | 写响应设备错误 class/code 与多 item 逐项 return code 非 0 → `DeviceRejected` | 【有守护】 | `gateway-core/src/south/protocol/s7comm/tests/write_codec.rs:114` `test_parse_write_response_error_code`；`:129` `test_parse_write_response_pdu_error`；`tests/e2e.rs:188` `write_error_response_is_device_rejected` | 断言写错误映射为设备拒绝 | 保持 |
| S7-W3 | 发送/读响应失败 → `WriteUncertain`，不自动重发 | 【有守护】 | `gateway-core/src/south/protocol/s7comm/tests/e2e.rs:206` `write_no_reply_is_uncertain` | 断言无应答 → `WriteUncertain` | 保持 |
| S7-W4 | 多写必须与入参等长且逐位对应，按 item 值长度分片 | 【有守护】 | `gateway-core/src/south/protocol/s7comm/tests/multi_write.rs:99` `test_parse_write_response_multi_all_ok`；`:121` `test_parse_write_response_multi_partial_failure`；`:148` `test_parse_write_response_multi_count_mismatch`；`:171` `test_parse_write_response_multi_empty_data_is_ok`；`tests/e2e.rs:162` `write_multi_merges_into_single_transaction` | 断言多写逐项对应、数量不匹配与合并下发 | 保持 |
| S7-W5 | 写后协议层不读回，仅 API 层 uncertain 读回 | 【无守护】 | — | 协议层不读回（`gateway-core/src/south/protocol/s7comm/collector/write.rs:90-93`），无用例锁定 | 新增用例 |
| S7-E1 | PDU 级错误（`pdu_type & 0x80`、error class/code）读写路径同口径 | 【有守护】 | `gateway-core/src/south/protocol/s7comm/tests/codec.rs:190` `test_parse_read_response_error_is_device_rejected`；`:235` `test_parse_read_response_rejected_return_code` | 断言读 PDU 错误 → `DeviceRejected`（写路径同口径由 S7-W2 覆盖） | 保持 |
| UA-R1 | NodeId 只接受 `ns=N;s=`、`ns=N;i=`、`ns=N;g=` 与 `ns,X` 简写，其它返回 `InvalidConfig` | 【有守护】 | `gateway-core/src/south/protocol/opcua/tests.rs:4` `test_parse_node_id_string`；`:10` `test_parse_node_id_numeric`；`:16` `test_parse_node_id_short_form`；`:22` `test_parse_node_id_invalid` | 四例分别覆盖字符串/数字/简写与非法输入 | 保持 |
| UA-R2 | 建链必须先 `wait_for_connection()` 成功，否则 `IoError`；重复 connect 不重建会话 | 【部分守护】 | `gateway-core/src/south/protocol/opcua/tests.rs:521` `ua_r2_read_connects_lazily_before_first_read` | 只测首次惰性建链，「重复 connect 不重建」无断言（实现 `opcua/mod.rs:619-620`） | 补断言 |
| UA-R3 | 批量读结果数必须 ≥ 请求数，不足按原下标逐点 `InvalidFrame` | 【有守护】 | `gateway-core/src/south/protocol/opcua/tests.rs:467` `ua_r3_short_batch_marks_only_missing_index` | 断言缺项只标记对应下标的点位 | 保持 |
| UA-R4 | 单节点 `StatusCode` 影响质量：Good/Uncertain/Bad，非 Good 必须带可辨识原因 | 【有守护】 | `gateway-core/src/south/protocol/opcua/tests.rs:312` `ua_e2_bad_and_uncertain_carry_identifiable_reason`；`:335` `ua_e2_good_has_no_reason_and_missing_status_is_bad` | 断言三种质量取值与 `quality_reason` | 保持 |
| UA-R5 | 读失败阈值必须小于会话超时 | 【部分守护】 | `gateway-core/src/south/protocol/opcua/tests.rs:35` `c11_read_failure_threshold_in_reasonable_range`；`:47` `c11_read_failure_threshold_not_too_aggressive` | 只锁常量取值范围，未断言阈值 < 会话超时 | 补断言 |
| UA-R6 | keepalive 失败达阈值必须拆会话；`try_lock` 抢不到必须跳过不得阻塞采集 | 【部分守护】 | `gateway-core/src/south/protocol/opcua/tests.rs:65` `c14_keepalive_timing_within_session_timeout`；`:87` `c14_keepalive_probes_standard_server_status_node` | 未断言达阈值拆会话与 `try_lock` 抢不到则跳过（实现 `opcua/mod.rs:273`、`:345-347`） | 补断言 |
| UA-R7 | 读失败清零/累加计数，成功即清零；单点问题不得累加链路失败 | 【有守护】 | `gateway-core/src/south/protocol/opcua/tests.rs:396` `ua_e2_bad_point_does_not_trip_session_teardown`（另 `:312`、`:335`） | 断言连续读不到值的点不计链路失败、不拆会话；清单该列已过时（`:371` 落在属性行，实际守护在 `:396`） | 保持 |
| UA-R8 | 批量读一次请求多节点并按原下标回填，单点解析失败不得改变其它点位下标 | 【有守护】 | `gateway-core/src/south/protocol/opcua/tests.rs:495` `ua_r8_batch_values_keep_request_order` | 断言批量结果保持请求顺序 | 保持 |
| UA-W1 | 不支持写，写请求必须是明确失败（`Unsupported`） | 【有守护】 | `gateway-core/src/south/protocol/registry/tests.rs:387` `test_supports_write_matches_implementation` | 断言能力位与实现一致，OPC UA 不可写 | 保持 |
| UA-E1 | 值提取错误原因可区分：不支持的 Variant → `Unsupported`，无值/非数字 → `InvalidFrame`；Bad/Uncertain 无值 → `DeviceRejected` | 【部分守护】 | `gateway-core/src/south/protocol/opcua/tests.rs:347` `ua_roi25_bad_status_without_value_is_device_rejected` | 仅覆盖 `DeviceRejected` 分支，`Unsupported`/`InvalidFrame` 分支无用例 | 补断言 |
| UA-E2 | 单节点 Bad/取不到值不得计为链路读失败；Bad/Uncertain 必须带可辨识 `quality_reason` | 【有守护】 | `gateway-core/src/south/protocol/opcua/tests.rs:396` `ua_e2_bad_point_does_not_trip_session_teardown`（另 `:312`、`:335`） | 断言单点 Bad 不拆会话且带可辨识 reason；清单该列已过时（`:371` 应 `:396`） | 保持 |
| MSTP-R1 | 帧以 `0x55 0xFF` 开头，`data_len ≤ 512`，头 CRC8 与数据 CRC16 必须正确 | 【部分守护】 | `gateway-core/src/south/protocol/bacnet_mstp/tests.rs:148` `test_parse_mstp_frame_bad_header_crc`；`:155` `test_parse_mstp_frame_bad_data_crc`；`:164` `test_parse_mstp_frame_too_short` | 只断言 `is_err` 未锁错误变体，且无 `data_len ≤ 512` 拒绝断言 | 补断言 |
| MSTP-R2 | 帧提取器在前导噪声、伪前导下逐字节重同步且不得 panic | 【有守护】 | `gateway-core/src/south/protocol/bacnet_mstp/tests.rs:224` `test_extract_mstp_frame_partial`；`:234` `test_extract_mstp_frame_with_garbage`；`:249` `test_extract_mstp_frame_bad_crc`；`:269` `test_extract_mstp_frame_multiple`；`:293` `extract_mstp_frame_hostile_input_never_panics`；`:440` `parse_mstp_frame_hostile_input_never_panics` | 断言重同步、多帧与敌意输入不 panic | 保持 |
| MSTP-R3 | 只接受 DataExpectingReply / DataNotExpectingReply / ReplyPostponed，其它按无应答处理 | 【无守护】 | — | 帧类型白名单（`gateway-core/src/south/protocol/bacnet_mstp/mod.rs:240-250`）无用例 | 新增用例 |
| MSTP-R4 | 目标站号与主站号必须 ≤127，否则 `InvalidConfig` | 【无守护】 | — | 站号上限校验（`bacnet_mstp/mod.rs:179-191`）无用例（`tests.rs:195`、`:201` 只测 `parse_master_addr`） | 新增用例 |
| MSTP-R5 | 应答 APDU 的 `invoke_id` 必须与请求一致 | 【有守护】 | `gateway-core/src/south/protocol/bacnet/tests/golden.rs:141` `test_parse_value_from_apdu_rejects_invoke_id_mismatch`（另 `:170` `read_ignores_stale_response_with_mismatched_invoke_id`） | 断言 invoke_id 不匹配被拒；清单该列已过时（清单写「无」，实际守护在 `bacnet/tests/golden.rs:141`） | 保持 |
| MSTP-R6 | 应答源地址必须与目标站一致（防他站应答） | 【无守护】 | — | 不成立：`gateway-core/src/south/protocol/bacnet_mstp/mod.rs:234` 解出 `_resp_src` 后丢弃，不比较，无用例 | 需产品口径 |
| MSTP-R7 | 单点一次事务，超时取 `bus.io_timeout()`；超时 → `NoReply` | 【无守护】 | — | 读路径超时 `NoReply` 无用例（仅 `tests.rs:425` `diag_reports_no_reply_instead_of_fake_success` 覆盖诊断路径） | 新增用例 |
| MSTP-R8 | BACnet Error / Reject / Abort PDU 必须映射为「设备明确拒绝」 | 【无守护】 | — | 不成立：`gateway-core/src/south/protocol/bacnet/frames/decode.rs:163-167` 只认 ComplexACK，其它一律 `InvalidFrame`，无用例 | 需产品口径 |
| MSTP-W1 | 不支持写，写必须返回 `Unsupported` | 【有守护】 | `gateway-core/src/south/protocol/registry/tests.rs:387` `test_supports_write_matches_implementation` | 断言能力位与实现一致，BACnet MS/TP 不可写 | 保持 |
| MSTP-E1 | CRC 错 → `ChecksumError`；结构/长度错 → `InvalidFrame`；无数据帧 → `NoReply` | 【部分守护】 | `gateway-core/src/south/protocol/bacnet_mstp/tests.rs:148` `test_parse_mstp_frame_bad_header_crc`；`:155` `test_parse_mstp_frame_bad_data_crc` | 只覆盖 CRC 分支，结构/长度与 `NoReply` 分支无用例 | 补断言 |
| MSTP-E2 | 诊断必须如实反映「Who-Is 无应答」，不得伪造成功 | 【有守护】 | `gateway-core/src/south/protocol/bacnet_mstp/tests.rs:407` `diag_reports_i_am_device_instance`；`:425` `diag_reports_no_reply_instead_of_fake_success` | 断言诊断如实上报设备实例与无应答 | 保持 |
| IEC-R1 | 三种帧型必须校验长度与结束符：单字节 `0xE5`、固定帧 5 字节、变长帧 `6+L`，均以 `0x16` 结尾 | 【有守护】 | `gateway-core/src/south/protocol/iec101/tests.rs:61` `test_parse_frame_fixed`；`:73` `test_parse_frame_variable`；`:101` `test_parse_frame_ack_and_unknown`；`:614` `golden_iec101_interrogation_addr_len_2`；`:660` `malformed_iec101_variable_frame_is_rejected_without_panic` | 断言各帧型解析与截断帧被拒 | 保持 |
| IEC-R2 | 校验和（字节累加 mod 256）必须校验，错误 → `ChecksumError` | 【有守护】 | `gateway-core/src/south/protocol/iec101/tests.rs:91` `test_parse_frame_checksum_error`；`:660` `malformed_iec101_variable_frame_is_rejected_without_panic` | 断言校验和错误被拒 | 保持 |
| IEC-R3 | 帧提取器必须丢弃前导噪声并重同步；不完整帧返回 `None` 等待更多字节 | 【有守护】 | `gateway-core/src/south/protocol/iec101/tests.rs:108` `test_extract_single_ack`；`:116` `test_extract_fixed_with_leading_noise`；`:126` `test_extract_variable_truncated_returns_none`；`:134` `test_extract_two_frames_back_to_back` | 断言重同步、截断等待与连续帧 | 保持 |
| IEC-R4 | 变长帧两个 L 字节必须一致且 `L≥2`；ASDU 长度 ≤253 | 【部分守护】 | `gateway-core/src/south/protocol/iec101/tests.rs:646-648` `malformed_iec101_variable_frame_is_rejected_without_panic` | 只测两个 L 字节矛盾；`L≥2` 与 ASDU ≤253 无用例（`iec101/frames.rs:168-185` 无 `l>=2` 校验）；清单该列已过时（清单写「无」，实际 `:646-648` 已覆盖） | 补断言 |
| IEC-R5 | 总召唤后必须轮询二级数据直到激活终止，最多 `MAX_POLLS=8` 轮 | 【部分守护】 | `gateway-core/src/south/protocol/iec101/tests.rs:389` `test_e2e_read_float_via_interrogation`；`:407` `test_e2e_read_normalized` | 未断言 `MAX_POLLS=8` 上限（`iec101/mod.rs:60`、`:145`） | 补断言 |
| IEC-R6 | 应答的链路地址与 ASDU 公共地址（CA）必须与本站配置一致 | 【有守护】 | `gateway-core/src/south/protocol/iec101/tests.rs:720` `foreign_link_addr_frame_is_never_cached`；`:743` `foreign_common_addr_frame_is_never_cached`；`:766` `foreign_frame_does_not_overwrite_own_value`；`:790` `foreign_frame_in_interrogation_reply_is_ignored` | 断言异站帧不入缓存、不覆盖本站值 | 保持 |
| IEC-R7 | ASDU 解码失败必须能上报为可辨识错误，不得静默丢弃 | 【有守护】 | `gateway-core/src/south/protocol/iec101/tests.rs:808` `unsupported_measurement_asdu_reports_unsupported`；`:855` `truncated_measurement_asdu_reports_invalid_frame` | 断言不支持类型与截断帧均有可辨识错误 | 保持 |
| IEC-R8 | IOA 必须为十进制 `u32`，非法配置返回 `InvalidConfig` | 【有守护】 | `gateway-core/src/south/protocol/iec101/tests.rs:510` `test_invalid_ioa_config_error` | 断言非法 IOA 报配置错误 | 保持 |
| IEC-R9 | 缓存缺该 IOA 时必须返回 `NoReply`，不得伪造 0 值 | 【有守护】 | `gateway-core/src/south/protocol/iec101/tests.rs:442` `test_unknown_ioa_returns_no_reply` | 断言未知 IOA 返回 `NoReply` | 保持 |
| IEC-R10 | 防重复帧：主站请求的 FCB 必须逐次翻转；非平衡模式应答不回显 FCB 故不校验应答位 | 【部分守护】 | `gateway-core/src/south/protocol/iec101/tests.rs:558` `golden_iec101_link_control_frames`（`:561-573`） | 只做静态两帧 FCB 位对比，运行时逐次翻转无用例 | 补断言 |
| IEC-W1 | 不支持写，写必须返回 `Unsupported` | 【有守护】 | `gateway-core/src/south/protocol/registry/tests.rs:387` `test_supports_write_matches_implementation` | 断言能力位与实现一致，IEC101 不可写 | 保持 |
| IEC-E1 | 链路复位失败可「尽力而为」忽略；总召唤/二级数据失败必须上抛，诊断如实记录复位结果 | 【有守护】 | `gateway-core/src/south/protocol/iec101/tests.rs:327` `test_diag_reports_reset_and_interrogation_count`；`:343` `test_diag_reports_no_data_honestly`；`:373` `test_diag_reports_unreachable_endpoint_instead_of_fake_success` | 断言诊断如实反映复位/总召唤/不可达结果 | 保持 |
| IEC-E2 | ASDU 层错误（COT 否定确认、类型不支持）映射为设备拒绝/不支持，而非笼统超时 | 【有守护】 | `gateway-core/src/south/protocol/iec101/tests.rs:832` `negative_confirmation_reports_device_rejected`；`:808` `unsupported_measurement_asdu_reports_unsupported` | 断言否定确认 → `DeviceRejected`、类型不支持 → `Unsupported` | 保持 |

> 前端 `gateway-web/src/**/*.spec.ts` 已全量检索：仅覆盖 UI 层能力位/错误码映射/Modbus-DLT645-CJT188 帧解析，**无任何 MC/FINS/S7/UA/MSTP/IEC 帧级断言**，故各条判定均以 Rust 侧用例为准。

## 待补守护清单

| 编号 | 缺什么 | 建议测试名 | 建议放置位置 |
|---|---|---|---|
| MC-R1 | 缺 0x00FF_FFFF 上限与 `InvalidConfig` 变体断言 | `fn parse_address_rejects_index_over_max_as_invalid_config` | `gateway-core/src/south/protocol/mc/tests/address_parse.rs` |
| MC-R4 | 缺「结束码+数据」长度 2..=960 边界拒绝断言 | `fn decode_rejects_body_len_out_of_2_to_960` | `gateway-core/src/south/protocol/mc/tests/codec.rs` |
| MC-R6 | 缺 1E 功能码不匹配与数据长度 ≤512 上限断言 | `fn decode_1e_rejects_function_code_mismatch_and_over_512` | `gateway-core/src/south/protocol/mc/tests/codec.rs` |
| MC-R7 | 超时只看 `IoError(_) \| NoReply` 过宽，未锁 `NoReply` 与连接清空 | `fn read_timeout_is_no_reply_and_clears_connection` | `gateway-core/src/south/protocol/mc/tests/e2e.rs` |
| MC-R10 | 缺批量解码数据区不足只标该点 `InvalidFrame` 的断言 | `fn batch_decode_overflow_marks_only_that_point_invalid_frame` | `gateway-core/src/south/protocol/mc/tests/batch.rs` |
| MC-R11 | 缺批量与诊断并发下分片帧不交叉的断言 | `fn concurrent_batch_and_diag_do_not_cross_frames` | `gateway-core/src/south/protocol/mc/tests/e2e.rs` |
| MC-W3 | 实现用 `as` 强转静默截断，缺越界拒绝断言（先要产品口径） | `fn write_value_out_of_range_is_rejected_before_send` | `gateway-core/src/south/protocol/mc/tests/e2e.rs` |
| MC-W4 | 缺「协议层写后不读回」的锁定断言 | `fn write_does_not_read_back_at_protocol_layer` | `gateway-core/src/south/protocol/mc/tests/e2e.rs` |
| MC-W5 | 缺「写事务不自动重发」的重发次数断言 | `fn write_transaction_is_not_resent` | `gateway-core/src/south/protocol/mc/tests/e2e.rs` |
| MC-E2 | 缺 1E 状态码与 3E/4E 结束码同口径映射断言 | `fn decode_1e_status_code_maps_like_3e_end_code` | `gateway-core/src/south/protocol/mc/tests/codec.rs` |
| FINS-R2 | 缺 FINS/TCP 报头长度 8..=2048 边界断言 | `fn tcp_header_length_out_of_range_is_rejected` | `gateway-core/src/south/protocol/fins/tests.rs` |
| FINS-R5 | 缺握手 command/tcp_error 校验失败分支断言 | `fn node_addr_handshake_rejects_bad_command_and_error` | `gateway-core/src/south/protocol/fins/tests.rs` |
| FINS-R6 | 缺读路径超时 → `NoReply` 并丢弃连接的断言 | `fn read_timeout_is_no_reply_and_drops_connection` | `gateway-core/src/south/protocol/fins/tests.rs` |
| FINS-R8 | UDP 应答无源地址/SID/回显匹配（实现不成立，先要产品口径） | `fn udp_reply_must_match_request_source_and_echo` | `gateway-core/src/south/protocol/fins/tests.rs` |
| FINS-W3 | 缺「写失败不自动重发」的次数字断言 | `fn write_timeout_is_not_resent` | `gateway-core/src/south/protocol/fins/tests.rs` |
| FINS-W4 | 实现用 `as` 强转，缺写值范围校验（先要产品口径） | `fn write_value_out_of_range_is_rejected` | `gateway-core/src/south/protocol/fins/tests.rs` |
| FINS-W5 | 缺「协议层写后不读回」的锁定断言 | `fn write_does_not_read_back_at_protocol_layer` | `gateway-core/src/south/protocol/fins/tests.rs` |
| FINS-E1 | `tcp_error != 0` 现映射为 `InvalidFrame`，缺设备拒绝口径断言（先要产品口径） | `fn tcp_header_error_maps_to_device_rejected` | `gateway-core/src/south/protocol/fins/tests.rs` |
| FINS-E2 | 读结束码未区分地址类与命令类错误（先要产品口径） | `fn read_end_code_distinguishes_address_and_command_errors` | `gateway-core/src/south/protocol/fins/tests.rs` |
| S7-R2 | 缺 TPKT 总长上限拒绝断言 | `fn read_tpkt_frame_rejects_oversized_length` | `gateway-core/src/south/protocol/s7comm/tests/codec.rs` |
| S7-R5 | 只 `is_err`，缺 `InvalidFrame` 变体断言 | `fn decode_too_short_is_invalid_frame` | `gateway-core/src/south/protocol/s7comm/tests/codec.rs` |
| S7-R9 | 协商 PDU 只挡 0，缺区间上下界（先要产品口径） | `fn negotiated_pdu_size_is_range_checked` | `gateway-core/src/south/protocol/s7comm/tests/e2e.rs` |
| S7-W5 | 缺「协议层写后不读回」的锁定断言 | `fn write_does_not_read_back_at_protocol_layer` | `gateway-core/src/south/protocol/s7comm/tests/e2e.rs` |
| UA-R2 | 缺「重复 connect 不重建会话」断言 | `fn ua_r2_repeated_connect_does_not_rebuild_session` | `gateway-core/src/south/protocol/opcua/tests.rs` |
| UA-R5 | 缺「读失败阈值 < 会话超时」的关系断言 | `fn ua_r5_threshold_is_below_session_timeout` | `gateway-core/src/south/protocol/opcua/tests.rs` |
| UA-R6 | 缺「达阈值拆会话」与「`try_lock` 抢不到则跳过」断言 | `fn ua_r6_keepalive_threshold_tears_down_and_skips_on_lock_contention` | `gateway-core/src/south/protocol/opcua/tests.rs` |
| UA-E1 | 缺 `Unsupported` / `InvalidFrame` 提取错误分支断言 | `fn ua_e1_value_extraction_maps_unsupported_and_invalid_frame` | `gateway-core/src/south/protocol/opcua/tests.rs` |
| MSTP-R1 | 只 `is_err` 未锁变体，且缺 `data_len ≤ 512` 拒绝断言 | `fn parse_mstp_frame_rejects_oversized_data_len_and_locks_variants` | `gateway-core/src/south/protocol/bacnet_mstp/tests.rs` |
| MSTP-R3 | 缺非数据帧类型（令牌帧等）按无应答处理的用例 | `fn mstp_ignores_non_data_frame_types` | `gateway-core/src/south/protocol/bacnet_mstp/tests.rs` |
| MSTP-R4 | 缺站号 >127 → `InvalidConfig` 的用例 | `fn mstp_rejects_station_address_over_127` | `gateway-core/src/south/protocol/bacnet_mstp/tests.rs` |
| MSTP-R6 | 应答源地址未校验（实现不成立，先要产品口径） | `fn mstp_reply_source_address_must_match_target` | `gateway-core/src/south/protocol/bacnet_mstp/tests.rs` |
| MSTP-R7 | 缺读路径超时 → `NoReply` 的用例（现有仅诊断路径） | `fn mstp_read_timeout_is_no_reply` | `gateway-core/src/south/protocol/bacnet_mstp/tests.rs` |
| MSTP-R8 | Error/Reject/Abort PDU 现一律 `InvalidFrame`（先要产品口径） | `fn bacnet_error_pdus_map_to_device_rejected` | `gateway-core/src/south/protocol/bacnet_mstp/tests.rs` |
| MSTP-E1 | 只覆盖 CRC 分支，缺结构/长度错与无数据帧分支断言 | `fn parse_mstp_frame_maps_structure_errors_and_empty_to_variants` | `gateway-core/src/south/protocol/bacnet_mstp/tests.rs` |
| IEC-R4 | 缺 `L≥2` 与 ASDU 长度 ≤253 的边界用例 | `fn variable_frame_rejects_l_lt_2_and_asdu_over_253` | `gateway-core/src/south/protocol/iec101/tests.rs` |
| IEC-R5 | 缺 `MAX_POLLS=8` 上限停止轮询的用例 | `fn interrogation_polling_stops_at_max_polls` | `gateway-core/src/south/protocol/iec101/tests.rs` |
| IEC-R10 | 只做静态两帧对比，缺运行时逐次 FCB 翻转用例 | `fn request_fcb_toggles_across_consecutive_polls` | `gateway-core/src/south/protocol/iec101/tests.rs` |
