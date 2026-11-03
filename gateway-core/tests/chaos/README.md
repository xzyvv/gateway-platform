# 故障注入测试（T1.3）

工业级稳定性 = 防患于未然。本目录提供可重复运行的故障注入测试集。

## 规则

- 所有场景必须触达生产代码（DataQueue / Scheduler / BusManager 等），
  杜绝"测 mock 自己"的剧场测试。
- Mock 仅作为桩支撑（harness），不替代真实路径。
- 每个场景必须给出对应的工业事故复盘背景。

## 用法

```bash
cargo test --test chaos -- --test-threads=1
```

`--test-threads=1` 强制串行，避免共享资源（SQLite 文件、Mock 状态）互踩。

## 场景清单（47 个，全部触达生产代码）

### Queue 真逻辑（4）

| 场景 | 验证点 | 触达代码 |
|------|--------|---------|
| `scenario_disk_full_during_continuous_traffic` | 磁盘满期间持续入队 → 全部进 fallback，恢复后 flush 不丢 | DataQueue、MemoryStorage |
| `scenario_fallback_capacity_evicts_low_priority` | 容量未超时 Bad+Good 共存 | DataQueue |
| `scenario_fallback_overflow_evicts_bad_first` | 超容量时 Bad/Stale 优先被踢，Good 全保留 | DataQueue、MemoryStorage |
| `scenario_restart_does_not_lose_persisted_data` | 进程重启不丢已持久化数据（in_flight 必恢复） | DataQueue、SqliteStorage |

### Modbus TCP 真总线（3）

| 场景 | 验证点 | 触达代码 |
|------|--------|---------|
| `scenario_modbus_tcp_roundtrip` | 真 TCP 从站桩 FC03 读写往返 | TransactionPool |
| `scenario_modbus_tcp_disconnect_during_request` | 请求中断线 → 错误返回 + active_count 归零 | TransactionPool |
| `scenario_modbus_tcp_concurrent_responses_routed_by_tx_id` | 8 从站并发，响应按 tx_id 正确路由 | TransactionPool |

### MQTT OutboxStore 真路径（4）

| 场景 | 验证点 | 触达代码 |
|------|--------|---------|
| `scenario_outbox_lifecycle_persist_ack_cleanup` | persist → ack → cleanup 全生命周期 | OutboxStore |
| `scenario_outbox_data_id_idempotent` | 同 data_id 重复 persist 幂等 | OutboxStore |
| `scenario_outbox_enforce_pending_limit` | pending 上限强制淘汰 | OutboxStore |
| `scenario_outbox_survives_process_restart` | 进程重启后 pending 可重放 | OutboxStore |

### MQTT 端到端（3，真 broker 桩）

| 场景 | 验证点 | 触达代码 |
|------|--------|---------|
| `scenario_mqtt_publish_end_to_end` | queue → publish → broker 收 PUBLISH → ack | MqttClient |
| `scenario_mqtt_publish_with_outbox` | publish 与 outbox 持久化联动 | MqttClient、OutboxStore |
| `scenario_mqtt_publish_broker_unreachable_then_recover` | broker 不可达 → 恢复后自动重连上送 | MqttClient 事件循环 |

### 断网重传专项（2）

| 场景 | 验证点 | 触达代码 |
|------|--------|---------|
| `scenario_mqtt_outbox_replay_after_disconnect` | 断连 → 重连 → outbox replay → queue+outbox drain | MqttClient、OutboxStore |
| `scenario_mqtt_stop_during_reconnect_backoff` | 退避期间 stop() ≤6s 优雅退出 | MqttClient 事件循环 |

### Scheduler 热重载（2）

| 场景 | 验证点 | 触达代码 |
|------|--------|---------|
| `scenario_scheduler_concurrent_reload_no_deadlock` | 8 路并发 reload 无死锁无 panic，采集不中断 | Scheduler::reload |
| `scenario_scheduler_reload_retains_active_device_states` | reload 删点：活跃点 device_state 保留、被删点清退 | Scheduler、device_states |

### Lua 死循环防护（1）

| 场景 | 验证点 | 触达代码 |
|------|--------|---------|
| `scenario_lua_stuck_script_does_not_block_pipeline` | 死循环脚本被 hook 中断并回收线程、数据标 Bad 续传、active/stuck 归零 | LuaEngine、ProcessingPipeline |

### BusManager RS485 串行化（1）

| 场景 | 验证点 | 触达代码 |
|------|--------|---------|
| `scenario_serial_bus_dead_port_serializes_transactions` | 半双工严格串行执行、通道满背压、连续失败阈值重连后总线存活 | BusManager、run_serial_bus_task、S-3 reopen |

另：`BusManager::transaction` 在总线 task 死亡后快速失败 `BusClosed`（Bug 148）
由 [src/south/bus/mod.rs](../../src/south/bus/mod.rs) 内联单测覆盖——集成测试
无法构造「task 死了 manager 还在」的状态（task 只在全部 sender drop 后退出，
唯一 sender 由 BusManager 持有）。

### 串口协议真总线（7，`SerialHandle` seam 注入）

通过 `BusDeps`（`SerialOpener` + `GpioOutput`）注入可编程串口流桩，
让 DLT645/CJT188/Modbus RTU 与 DE 时序走真实 `run_serial_bus_task` + `do_transaction`
+ 协议编解码，而非绕过总线直接调解码函数。

| 场景 | 验证点 | 触达代码 |
|------|--------|---------|
| `scenario_serial_dlt645_end_to_end_roundtrip` | DLT645 真总线往返；请求字节独立校验（0x33 偏移、CS 覆盖首字节起） | BusManager、serial_task、DLT645 编解码 |
| `scenario_serial_cjt188_end_to_end_roundtrip` | CJT188 7 字节地址、无第二 0x68 头的帧边界 | BusManager、serial_task、CJT188 编解码 |
| `scenario_serial_anomaly_family_retry_bad_frame_bad_value` | 静默重试成功 / 损坏帧被拒为 NoReply（不误判成功）/ 非法 BCD 标 Bad 且链路续传 | serial_task 重试、帧提取、值解码 |
| `scenario_serial_reopen_recovers_after_startup_open_failure` | 启动时 serial=None → 连续两笔重开失败（各 2s 退避）→ 第三笔重开成功恢复采集 | serial_task S-3 reopen、SerialOpener |
| `scenario_serial_modbus_rtu_end_to_end_roundtrip` | Modbus RTU 真总线往返（FC03）；请求字节独立校验（01 03 0000 0001 + CRC 84 0A） | BusManager、serial_task、Modbus RTU 编解码 |
| `scenario_serial_modbus_rtu_anomaly_family` | 静默重试成功 / 坏 CRC 帧被拒为 NoReply（不误判成功）且链路续传 | serial_task 重试、`extract_modbus_frame` CRC 校验 |
| `scenario_serial_modbus_rtu_write_family` | 单点写：负值编码 / 多寄存器完整写入 / 写后回显校验 | Modbus RTU 写编码、serial_task 事务 |

其中「写/读超时分离」「DE 早于写、晚于读的时序」「分片响应重组」「ASCII 模式
请求转换」由 [src/south/bus/serial_task/tests.rs](../../src/south/bus/serial_task/tests.rs)
内联单测覆盖（需要精确控制 poll 时序，集成层难以稳定构造）。

### S7comm 真从站（3，TCP ISO-on-TCP：TPKT + COTP + S7Comm）

harness 内起真 TCP 从站桩，讲真 TPKT/COTP/S7Comm 三层协议，帧构造独立于生产
`frames.rs`（手写字节，可作交叉验证 oracle），完整走通 `S7CommCollector` 的
连接握手与 Read Var 解码。

| 场景 | 验证点 | 触达代码 |
|------|--------|---------|
| `scenario_s7comm_end_to_end_roundtrip` | COTP 连接 + Setup 协商 + Read Var 往返，DBD0 按 REAL 解码 42.5 | S7CommCollector::connect/read |
| `scenario_s7comm_handshake_rejected` | COTP 被拒 / Setup 协商被拒均显式返回 IoError，不静默成功 | connect 握手校验 |
| `scenario_s7comm_disconnect_during_request` | 请求中断线 → 显式 IoError 且连接状态被清理（触发上层重连） | read 断线清理、is_connected |

### IEC 60870-5-104 真从站（3，TCP：U 帧 STARTDT + I 帧总召唤）

| 场景 | 验证点 | 触达代码 |
|------|--------|---------|
| `scenario_iec104_end_to_end_roundtrip` | STARTDT act/con 握手 + 总召唤 ASDU 解码短浮点量测 230.5 | Iec104Collector::connect/read |
| `scenario_iec104_anomaly_no_reply_bad_quality` | 总召唤无响应 → NoReply（不误判 0）；QDS 坏位 → 数据点标 Bad（值仍可用） | read 超时、品质位解析 |
| `scenario_iec104_control_command_write` | 单点命令（C_SC_NA_1）下发 + 镜像回送确认 | Iec104Collector::write |

### BACnet/IP 真从站（2，UDP：BVLC + NPDU + APDU ComplexACK）

| 场景 | 验证点 | 触达代码 |
|------|--------|---------|
| `scenario_bacnet_ip_end_to_end_roundtrip` | ReadProperty → ComplexACK REAL 解码 21.5 | BacnetCollector::connect/read |
| `scenario_bacnet_ip_anomaly_family` | BVLC-Result 错误 / 非法 BVLC 类型 → InvalidFrame；不响应 → 3s 超时 NoReply | BVLC 校验、超时处理 |

### BACnet MS/TP 真串口总线（2，复用 `SerialHandle` seam）

经 `BusDeps` seam 注入可编程串口流，MS/TP 帧（前导码 + 头 CRC8 + 数据 CRC16）
由测试独立构造，走真实 `run_serial_bus_task` + `extract_mstp_frame` + 应用层解码。

| 场景 | 验证点 | 触达代码 |
|------|--------|---------|
| `scenario_bacnet_mstp_end_to_end_roundtrip` | DataExpectingReply 帧 → 真总线 → 帧提取 + CRC 校验 → ComplexACK 解码 33.5；请求字节与 DE 时序独立校验 | BusManager、extract_mstp_frame、BacnetMstpCollector |
| `scenario_bacnet_mstp_anomaly_family` | 数据 CRC 被破坏 → 帧被拒为 NoReply（不误判成功）且链路续传；从站静默 → NoReply | 帧提取 CRC 校验、超时 |

### OPC UA Client 真服务端（2，async-opcua：None 安全策略 + 匿名）

harness 内用 async-opcua `ServerBuilder` 起真 OPC UA 服务端（与北向
`opcua_server` 同源 API），生产 `OpcUaCollector` 作为真客户端，节点命名空间
index 由服务端动态分配并经 `address()` 暴露，避免硬编码 ns。

| 场景 | 验证点 | 触达代码 |
|------|--------|---------|
| `scenario_opcua_client_end_to_end_roundtrip` | 匿名会话建立 + 两次 Read（会话复用、值随服务端更新） | OpcUaCollector::connect/read |
| `scenario_opcua_client_fault_family` | 未连接即读 → IoError；服务端不可达 → connect 返回 IoError 且不标记已连接 | read/connect 前置校验 |

### 批量读 / 写补充（3）

| 场景 | 验证点 | 触达代码 |
|------|--------|---------|
| `scenario_bacnet_ip_rpm_and_write_property` | ReadPropertyMultiple 批量读 + WriteProperty 写回（决策表 #20） | BacnetCollector::read/write |
| `scenario_dlt645_batch_read_single_di_per_point` | read_batch 逐点单 DI 事务（Bug 223 回归） | Dlt645Collector::read_batch |
| `scenario_cjt188_batch_read_single_di_per_point` | read_batch 逐点单 DI 事务（Bug 224 回归） | Cjt188Collector::read_batch |

### 现场六类故障（6，C1.3）

每个场景按「故障 → 现象 → 自愈 → 恢复后数据完整性」组织，处置步骤见
[docs/故障处置卡.md](../../../docs/故障处置卡.md)。

| 场景 | 验证点 | 触达代码 |
|------|--------|---------|
| `scenario_certificate_expired_is_flagged_and_renewal_restores` | 过期证书判 `Expired`（不冒充正常），换发后回 `Ok`，降级期间数据照常入队 | CertManager::check_expiry_days、CertExpiryScan |
| `scenario_license_expiry_degrades_then_renewal_recovers` | 过期 license `load` 明确返回 Err 且状态降级（present=false），续期后恢复 Ok | LicenseManager::load/current、签名校验 |
| `scenario_ntp_drift_rollback_then_resync_recovers` | 时钟回拨超 48h 判 `Tampered`，NTP resync 后回 `Ok`，失步窗口数据不丢 | ClockGuard::check_and_update/resync |
| `scenario_watchdog_stall_detected_then_restart_recovers` | 循环卡死超宽限期 → `stale_loops` 报出 + 超时计数边沿触发 1 次；恢复 kick 后清零 | Watchdog::register/run/stale_loops |
| `scenario_network_switch_reconnects_and_resumes` | 承载网络中断 → 读取如实失败（不返回旧值）；恢复后自动重连继续采集 | TransactionPool、Modbus TCP 重连 |
| `scenario_southbound_long_silence_recovers_without_data_loss` | 从站连续静默 → 多笔超时如实报错；恢复应答后下一笔成功，静默窗口数据完整 | BusManager 串口事务、重试/超时 |

## 覆盖度

✅ 已覆盖：DataQueue（enqueue/dequeue/ack/release + 磁盘满 fallback + SQLite 持久化）、
TransactionPool（真 TCP 从站 + tx_id 路由 + 断线清理）、OutboxStore（持久化 + 幂等 + 上限 + 重启）、
MqttClient（端到端 publish + outbox replay + 指数退避优雅停止）、
Scheduler（并发热重载 + device_states 保留语义）、LuaEngine（死循环防护 + 线程上限）、
BusManager（RS485 串行化 + 背压 + 阈值重连存活 + BusClosed 快速失败）、
串口协议真总线（DLT645/CJT188 端到端 + 异常族 + 热插拔重开；写/读超时分离与 DE 方向时序）
（串口桩经 `BusDeps` seam 注入，Windows CI 可覆盖）、
五协议端到端 + 故障注入（S7comm / IEC104 / BACnet/IP / BACnet MS/TP / OPC UA：
真协议从站桩经真 TCP/UDP/串口总线，覆盖往返、握手拒绝、断线、超时无响应、坏帧/坏质量、
CRC 破坏等，全部触达各 `Collector::connect/read` 生产路径）

注：以上覆盖的是**网关侧驱动逻辑**（总线串行化、帧收发、时序、重连、解码）。
真串口芯片 / RS485 收发器 / 现场接线等物理层，以及真实 PLC/RTU/BACnet 控制器/
第三方 OPC UA Server 的固件行为，仍无法在 CI 中验证——真从站桩只保证「网关按协议
正确编解码并对异常做出正确反应」，不等价于对真实设备互操作性的背书。

## 何时新增场景

每次线上事故复盘后，对应添加一个 `scenario_*` 测试函数。事故 → 复现 → 测试 → 修复 → 防回归。

## 历史

- 2026-10-12 审计后重写：删除 13 个"测 mock 自己"的伪场景，分批补齐 18 个触达生产代码的真场景
  （Queue 4 + Modbus TCP 3 + Outbox 4 + MQTT 端到端 3 + 断网重传 2 + Scheduler 热重载 2）。
- 2026-10-12 Lua 死循环防护（场景 31）+ BusManager RS485 串行化（场景 32）入库，
  补齐 README 漏登的 Lua 场景；Bug 148（BusClosed 快速失败）由 bus/mod.rs 内联单测钉住。
- 2026-10-12 串口 seam 落地：新增 `BusDeps`（`SerialOpener` + `GpioOutput`）注入点，
  生产 `BusManager::spawn` 行为零变化；补齐场景 33-36（DLT645/CJT188 真总线端到端、
  异常族、热插拔重开），并在 `serial_task/tests.rs` 新增 5 个时序单测
  （写超时/DE 时序/静默走满超时/ASCII 转换/分片重组）。场景总数 20 → 24。
- 2026-10-12 五协议 e2e + 故障注入：新增 harness 真从站桩（S7comm TPKT/COTP/S7、
  IEC104 U/I 帧、BACnet/IP UDP/BVLC、BACnet MS/TP 帧构造器、OPC UA async-opcua 服务端），
  补齐场景 37-47（S7comm 3 + IEC104 2 + BACnet/IP 2 + BACnet MS/TP 2 + OPC UA 2），
  全部触达各协议 `Collector::connect/read` 生产路径；生产代码零行为变化。
  场景总数 24 → 35。
- 2026-10-31 审计整改补登：Modbus RTU 写族、IEC104 单点命令、BACnet/IP 批量读/写、
  DLT645/CJT188 批量读 5 个场景此前漏登，场景总数 37 → 42（以 `scenarios.rs` 函数数为准）。
- 2026-11-03 C1.3 现场故障补齐：新增证书过期、许可证到期、NTP 失步、看门狗触发、
  网络切换、南向长时无响应 6 个场景（场景 42-47），同步产出 `docs/故障处置卡.md`；
  场景总数 41 → 47。`ModbusTestServer` 新增 `set_network_ok` 以模拟承载网络中断（不重启监听器）。
