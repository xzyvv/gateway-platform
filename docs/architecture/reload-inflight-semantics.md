# 热重载与在途写事务的并发语义（ROI-18 审计）

本文回答什么问题：**改配置触发热重载时，正在执行中的写事务会不会被作废、会不会写一半、调用方会看到什么状态？**
具体拆成六个问题：reload 期间的锁粒度、写事务在总线层的生命周期、reload 重建总线时在途写的处理、
点位增量重载是否会读到半新半旧的配置、哪些边界已被代码证明安全、以及这些结论对 ROI-12（热重载原子化）意味着什么。

本文只做代码审计与结论归档，不修改任何实现。所有结论均给出本仓库内的代码位置（相对路径 + 行号）。
凡是代码里找不到证据的判断，统一标为「未验证」，不按经验补全。

---

## 1. 锁的粒度：reload 与写事务并不互斥

### 1.1 相关锁

| 锁 | 位置 | 保护范围 | 是否与写事务互斥 |
|---|---|---|---|
| `GatewayRuntime.reload_lock`（`AsyncMutex<()>`） | `gateway-core/src/runtime/gateway.rs:40`、`:289`、`:390` | 串行化 `reload_config` 与 `reset_serial_buses`；覆盖整个 reload 流程（含总线重建、MQTT、NTP） | **否**。写事务路径从不获取它 |
| `AppState.config.config_reload_lock`（`tokio::sync::Mutex<()>`） | `gateway-api/src/web/types.rs:51`；使用约定见 `gateway-api/src/web/config_apply.rs:21` | 串行化 API 侧的「保存 → 热重载 → 存快照」整段流程 | **否**。点位写端点入口未持有（`gateway-api/src/web/api/points/handlers/mod.rs:202-211`） |
| `Scheduler.reload_lock`（`Semaphore(1)`） | `gateway-core/src/south/scheduler/mod.rs:115`；获取点 `gateway-core/src/south/scheduler/reload.rs:27` | 串行化 `reload_points_incremental`（含暂停/恢复总线） | **否** |
| `bus_managers`（`parking_lot::Mutex`） | `gateway-core/src/runtime/gateway.rs:30`、`:493-496` | 只保护 map 的读取与整体替换，临界区极短 | **否** |
| `collectors`（`parking_lot::Mutex`） | `gateway-core/src/south/scheduler/mod.rs:80`；替换点 `gateway-core/src/runtime/gateway.rs:504` | 保护总线名 → 采集器映射 | **否** |

结论：`reload_lock` 只保证「同一时刻只有一个 reload」，**不提供与写事务的互斥**。
reload 在持锁期间会打开新串口、构造新 collector、替换 map、并等待受影响采集组退出（最长 3 秒，
`gateway-core/src/south/scheduler/reload.rs:147-157`）；与此同时，Web 写接口和北向写回都可以照常进入
`Collector::write`，不受任何 reload 锁约束。

### 1.2 写事务在总线层的生命周期

| 阶段 | 代码位置 | 失败/取消时的结果 |
|---|---|---|
| 入队 | `gateway-core/src/south/bus/mod.rs:324-361` | 入队超时 → `BusBusy`（`:378`） |
| 出队前检查 | `gateway-core/src/south/bus/mod.rs:611-624` | 调用方已放弃（`response_tx` 关闭）→ 直接丢弃、不下发 |
| 下发（串口） | `gateway-core/src/south/bus/serial_task.rs:228-287` 调用 `do_transaction`，写帧在 `:517`、`:633-680` | 写失败 → `IoError`；写事务随后不再重发（`:249-256`） |
| 下发（Modbus TCP） | `gateway-core/src/south/bus/tcp_task.rs:225-300` → `TransactionPool::submit_with_priority`（`gateway-core/src/south/protocol/modbus/transaction.rs:222-316`） | 池满 → `Busy`；写 socket 超时 → `abort_all` 并返回 `Disconnected`（`transaction.rs:284-298`） |
| 等响应 | 串口：`serial_task.rs:529-599`；TCP：`transaction.rs:302-315`；调用方总预算：`bus/mod.rs:351-374` | 总线 task 消失 → `RecvError` → `BusClosed`（`bus/mod.rs:371-372`）；预算耗尽 → `Timeout`（`:373`） |
| 结果上抛 | `gateway-core/src/south/protocol/mod.rs:240-259` | 按「能否证明写未生效」分为 `Failed` / `Uncertain`；API 侧映射为 5101 / 5102（`gateway-api/src/web/api/points/mod.rs:130-142`） |

要点：**总线层只回传一个终态**（`Result<Vec<u8>, CollectorError>`）。它没有「已入队 / 已下发 / 已收应答」
的分阶段状态位，因此一旦总线 task 被取消，调用方只能从 oneshot 的断开推断，无法知道帧是否已经写到线上。

> **ROI-20 修复**：`BusTransaction` 增加 `frame_sent` 标记（`bus/mod.rs:120-125`）。总线任务在写帧前置 true
> （`serial_task.rs:571-577`），任务被 abort 时由 `TxnResponder` 的 `Drop` 向调用方回送 `WriteUncertain`
> （`serial_task.rs:92-125`）；未下发或读事务仍按 `BusClosed` 处理。

---

## 2. reload 重建总线时，在途写事务被如何处理

### 2.1 现有作废机制不归 reload 管

`abort_all()` 只存在于 Modbus TCP 的事务池，调用点只有三处：

- reader 读到 EOF / 非法帧：`gateway-core/src/south/protocol/modbus/transaction/frame.rs:128`、`:143`
- 写 socket 超时：`gateway-core/src/south/protocol/modbus/transaction.rs:290`
- 实现本体：`transaction.rs:417-429`，把 `Disconnected` 广播给全部 waiter 并返回作废数量

**reload 路径从不调用 `abort_all()`**，也没有任何「reload 作废 N 笔在途事务」的计数或日志。
所以现状不是「reload 统一作废在途写」，而是「reload 按总线实例的析构时机被动打断」。
`docs/architecture/read-write-invariants.md` 中 TCP-06 / TCP-07 描述的是断链与写超时场景，不含 reload。

### 2.2 三类在途状态的实际结局

| 在途状态 | 机制 | 调用方看到 | 判定是否正确 |
|---|---|---|---|
| 仍在队列里，未被总线 task 取出 | 旧 `BusManager` 析构时 `bus_task` 被 abort（`gateway-core/src/south/bus/mod.rs:585-590`），`rx`/`ctrl_rx` 关闭，`BusTransaction` 连同 `response_tx` 一起 drop | `RecvError` → `BusClosed`（`bus/mod.rs:371-372`） | **正确**：帧未下发，回报「明确失败」成立 |
| 串口总线上已出队，正在写帧或等应答 | 同上，`bus_task` future 在任意 `.await` 点被 drop；`TxnResponder` 的 `Drop` 兜底 | 已下发 → `WriteUncertain`；未下发 → `BusClosed` | **已修复（ROI-20）**：`frame_sent=true` 时回送 `WriteUncertain` → 5102，不再按 `BusClosed → Failed → 5101 可安全重试` 上报（`serial_task.rs:113-125`、`bus/tests.rs` 的 `aborted_in_flight_write_reports_write_uncertain`） |
| Modbus TCP 上已出队 | 每个事务由独立 task 提交（`tcp_task.rs:225-300`），该 task 持有 `Arc<TransactionPool>` 与 `response_tx`；abort 的只是外层总线 task，`in_flight` 的 `JoinHandle` 被直接 drop（detach） | 事务可继续完成并把真实响应回传；调用方仍在预算内等待 | **不作废**。风险不在结果，而在 reload 后旧连接与新连接并存（见第 4 节） |

补充一个关键时序：旧 manager 并不是在 `bus_managers` 替换的那一刻立即析构。`reload_buses` 在
`gateway-core/src/runtime/gateway.rs:416` 克隆了一份旧 collector map，且未变更的采集组仍持有同一个
collector，而 collector 持有 `Arc<BusManager>`（工厂签名见
`gateway-core/src/south/protocol/registry/factories.rs:36-46`）。因此旧总线 task 可能在 reload 的
建新口、停组阶段继续存活，这段时间内新的写请求仍可能落到旧实例上。

### 2.3 「会不会写一半」

- **串口**：`write_frame` 使用 `tokio::time::timeout(write_timeout, serial.write(data))`
  （`gateway-core/src/south/bus/serial_task.rs:655-666`），底层是 `write_all + flush`
  （`gateway-core/src/south/bus/serial.rs:64-68`）。future 在写中途被取消时，代码无法证明已写字节数，
  物理上**存在只写出帧前缀的可能**。半帧通常会被从站按长度 / CRC 丢弃而不执行，但本仓库没有证据能证明
  「一定不会执行」，只能归入「结果未知」。**结论：窗口存在，实际触发概率未验证。**
  ROI-20 之后，写帧一旦开始（含 `write_all` 中途）即标记 `frame_sent`，取消一律按 `WriteUncertain` 上报，
  不再声称「明确失败、可安全重试」。
- **Modbus TCP**：整帧 MBAP 在 writer 互斥锁内一次 `write_all`（`transaction.rs:267-299`），且 reload
  不会取消已出队的事务 task，因此 **reload 本身不会造成 TCP 半帧**。TCP 的问题是旧 socket 继续存活，
  不是帧被截断。

### 2.4 调用方（API）看到的状态

写入核心在 `gateway-api/src/web/api/points/handlers/mod.rs:254-326`：

1. 取配置快照与 collector 快照（`:254-255`）；
2. 调 `collector.write`（`:278`）；
3. 失败时取 `e.write_outcome()`（`:284`），**只有 `Uncertain` 才读回核实**（`:292-307`）；
4. 单写失败按三态映射业务码（`:216-227`；`gateway-api/src/web/api/points/mod.rs:130-142`）。

因此当 reload 打断串口在途写并产生 `BusClosed` 时，API 会返回 **5101「写入失败」**，前端据此认为
「可安全重试」——而设备可能已经执行。该语义错位已由 **ROI-20** 消除：串口在途写被 abort 时回送
`WriteUncertain` → 5102；只有「未出队 / 未下发」才保持 `BusClosed` → 5101。

---

## 3. 点位增量重载期间会不会读到半新半旧的配置

### 3.1 采集任务：不会

- 采集组在创建时把点位列表封成 `Arc<Vec<PointConfig>>` 并由任务闭包持有
  （`gateway-core/src/south/scheduler/spawning.rs:124`、`:133-146`），单个 tick 内只读这一份快照。
- reload 的顺序是「先停变更组并等待退出 → 再替换 points → 再重建组」
  （`gateway-core/src/south/scheduler/reload.rs:127-157`、`:160`、`:202-206`）。
  停止信号在当前 tick 结束后才生效（`gateway-core/src/south/scheduler/task.rs:96-103`），超时才 abort。
- 因此正在执行的采集任务看到的始终是自己启动时的完整旧快照，**不会出现组内一半新一半旧**。

### 3.2 写路径：存在跨代组合窗口

写请求不是从采集组里发起的，而是每次请求现场取两份快照：

- `gateway-api/src/web/api/points/handlers/mod.rs:254` 取配置快照（`state.config.load_full()`）
- `gateway-api/src/web/api/points/handlers/mod.rs:255` 取 collector 快照（`scheduler.collectors()`）

这两份快照没有共同的版本号。reload 过程中 collector map 在
`gateway-core/src/runtime/gateway.rs:504` 就被替换，而配置快照直到
`gateway-api/src/web/config_apply.rs:588` 才 `store`。两者之间的窗口里，写请求会拿到
**新 collector + 旧点位地址 / 从站号 / 功能码**。北向 `write_point` 同理：点位从
`scheduler.point_by_id`（`gateway-core/src/south/scheduler/mod.rs:212-214`，points 已在
`reload.rs:160` 替换）取，collector 从 `scheduler.collectors()` 取，也不是同一原子快照。

**结论：单笔已经开始的写是自洽的（沿用入口时拿到的旧点位与旧 collector），但跨 reload 的写请求可能把
旧地址发到新总线上。该窗口由代码结构证明存在，现场后果未验证。**

---

## 4. 场景汇总

| 场景 | 当前行为 | 证据 | 风险等级 |
|---|---|---|---|
| reload 时写事务还在队列，未取出 | 通道关闭 → `BusClosed`，帧未下发，报「明确失败」 | `gateway-core/src/south/bus/mod.rs:371-372`、`:611-624` | 低（语义正确） |
| 串口在途写已完整写出帧、尚未收到应答时 reload | 总线 task 被 abort → 已下发 → `WriteUncertain` → 5102，调用方不会把它当「明确失败」重试 | `serial_task.rs:113-125`、`bus/tests.rs` | 低（ROI-20 已修复） |
| 串口在途写正处于 `write_all` 中途被 abort | 进入写帧即标记 `frame_sent`，中途取消同样报 `WriteUncertain` | `serial_task.rs:571-577`、`serial.rs:64-68` | 低（ROI-20 已修复，保守按结果未知） |
| Modbus TCP 在途写在 reload 期间 | 独立事务 task 被 detach，继续完成；旧 pool / 旧 socket 存活到任务结束 | `tcp_task.rs:225-300`、`:300-304`、`:306-314` | 中（旧新连接并存、写序不可控） |
| reload 打开新串口时旧串口句柄仍被旧总线持有 | 两个句柄可能同时指向同一端口；无排他锁 | `gateway.rs:434-461`、`serial.rs:110-118` | 中（平台相关，未验证） |
| 变更总线打开失败 | 保留旧 manager/collector，跳过重建计数，但 `reload_config` 仍返回成功并提示 "buses reloaded online" | `gateway.rs:440-459`、`:299-304` | 中（提示与实际不符） |
| 点位增量重载时采集组正在跑 | 本 tick 用旧快照跑完，随后停止/重建；无半新半旧 | `spawning.rs:124`、`reload.rs:127-160` | 低（语义正确） |
| 写请求跨 reload 取配置与 collector | 可能「新 collector + 旧点位」组合下发 | `points/handlers/mod.rs:254-255`、`gateway.rs:504`、`config_apply.rs:588` | 中高 |
| reload 中 `reload_points_incremental` 失败 | managers/collectors 已替换，API 却回滚磁盘并不更新内存快照 → 运行态新、内存与磁盘旧 | `gateway.rs:493-509`、`config_apply.rs:573-585` | **高**（ROI-12 核心） |
| 断链 / 写超时（非 reload） | `abort_all` 广播 `Disconnected` 并计数 | `transaction.rs:284-298`、`:417-429`；用例 `gateway-core/src/south/bus/tcp_task/tests.rs:322-345`、`:385-435` | 低（已有语义） |
| 写结果 `NoReply` / `Timeout`（非 reload） | 走 `WriteUncertain` + 读回核实，与常态一致 | `protocol/mod.rs:240-259`、`points/handlers/mod.rs:288-307` | 低（与 reload 无关） |

---

## 5. 已证明安全 / 尚未证明

### 5.1 已证明安全（有代码与用例支撑）

1. **队列中的写不会在 reload 时被下发**：出队前 `prepare_dequeued` 检查 `response_tx` 是否关闭
   （`bus/mod.rs:611-624`），reload 关闭通道后事务被丢弃。
2. **写事务失败后不会自动重发**：`retry_allowed=false` 贯穿总线层
   （`bus/mod.rs:110-118`、`serial_task.rs:249-256`、`tcp_task.rs:261-269`），
   与 `read-write-invariants.md` 的 B-01 一致。
3. **采集组不会跨代读点位**：组内点位是启动时快照，且 reload 先停组再换 points
   （`spawning.rs:124`、`reload.rs:127-160`）。
4. **断链/写超时的作废语义明确**：`abort_all` 清空在途表、广播 `Disconnected`、返回计数
   （`transaction.rs:417-429`），并由 `tcp_task/tests.rs` 与 `transaction/tests.rs` 守住。
5. **写结果三态上抛是闭合的**：`BusClosed` 被明确归类为「可证明未生效」
   （`protocol/mod.rs:250-255`），API 按 5101/5102 分流（`points/mod.rs:130-142`）。

### 5.2 尚未证明 / 存在窗口

| # | 未证明项 | 依据/现状 |
|---|---|---|
| U-1 | 串口 abort 落在「帧已写出」之后时，`BusClosed → Failed` 是否会造成现场重复写 | **已修复（ROI-20）**：`frame_sent` + `TxnResponder` Drop → `WriteUncertain`；用例 `south::bus::tests::aborted_in_flight_write_reports_write_uncertain` |
| U-2 | 串口 `write_all` 被取消时实际写出多少字节 | 驱动层仍未知，但对外口径已保守统一为 `WriteUncertain`（ROI-20），不再声称明确失败 |
| U-3 | reload 后旧 TCP 连接还会存活多久、与新连接是否会向同一设备交叉写 | 事务 task 被 detach（`tcp_task.rs:300-304`），旧 pool 由 `Arc` 续命；无观测指标 |
| U-4 | 「新 collector + 旧点位」窗口内下发会造成什么现场后果 | 代次不一致由代码结构证明，无测试、无版本号约束 |
| U-5 | 同一串口被新旧两个 `SerialHandle` 同时打开的后果 | `serial.rs:116-118` 无排他；Windows/Linux 行为不同，未验证 |
| U-6 | reload 失败后运行态（managers/collectors）能否恢复 | `gateway.rs:493-509` 先替换再重建组，失败时只回滚 API 侧磁盘与内存；managers 无回滚路径 |
| U-7 | reload 期间被取消的写事务数量 | 无指标、无日志；`abort_all` 不被 reload 调用 |

---

## 6. 与既有文档的关系

- `docs/config-reload-matrix.md` 回答「哪些配置项立即生效、哪些要重启」，不涉及在途事务；本文只补齐并发语义，不改变其分节归类结论。
- `docs/architecture/read-write-invariants.md` 的 B-01 / B-04 / TCP-06 / TCP-07 给出「写不重发」「调用方已放弃不得下发」「断链/写超时作废」的基准；本文的增量结论是：**reload 不是 `abort_all` 的触发源，也不遵守这些边界**。
- 注意一处文档与代码的表述冲突：`docs/config-reload-matrix.md:50` 写「落盘失败返回 500，但内存已是新配置」，而现行代码是**先落盘再热重载**，落盘失败时运行态未动（`gateway-api/src/web/config_apply.rs:565-570`）。该文档不在本次改动范围内，已列入待确认项。

---

## 7. 待确认项

1. `config-reload-matrix.md:50` 的「落盘失败但内存已新」与 `config_apply.rs:565-570` 的现行顺序不一致，以哪边为准？需要主流程统一修正。
2. ~~串口在途写被 reload 取消时，API 应报 5102（结果未知）还是允许 5101（明确失败）？~~ **已确定（ROI-20）**：
   已下发的在途写报 5102（结果未知），仅未下发的排队事务报 5101。
3. 变更总线打开失败时，整次保存是否应该失败并回滚？当前是「保留旧实例 + 仍返回成功」（`gateway.rs:440-459`）。
4. 同一串口在 reload 窗口内被新旧句柄同时打开，在目标部署平台（Linux / Windows）是否可接受？未做真机验证。
5. 旧 TCP 连接在 reload 后被 detach 的事务是否需要在切换前 drain？当前没有任何等待。
6. 是否存在「reload 与写并发」的现场故障样本？现有测试只覆盖 reload 失败回滚（`gateway-api/tests/config_reload_atomicity.rs`），没有覆盖 reload 与在途写并发。

---

## 8. 对 ROI-12 的输入

### 8.1 最小原子化边界

原子化应落在**单条总线（`BusManager`）**，而不是 API handler：

- API 层无法知道帧是否已经写到线上，在 handler 加锁只能覆盖「入队前」，覆盖不到已在途的事务；
- 采集组已经按总线分组（`group_key`，`gateway-core/src/south/scheduler/mod.rs:476-487`），
  reload 也已按 `changed_buses` 做定向重建（`gateway.rs:421-482`），具备按总线收敛的基础。

建议的边界：**「停止接受新写 → 等待在途写 drain（有界超时）→ 替换实例 → 恢复」**，
未变更的总线不受影响。

### 8.2 具体建议（不写实现）

1. **在 `BusManager` 增加写事务门控（epoch / 读写锁）**：写事务从入队到结果返回全程持读侧；
   reload 对受影响总线持写侧。这样 abort 只可能发生在可控点（未下发），不再有「已写帧被静默取消」的窗口。
2. **如果保留 abort 作为兜底，必须区分「未下发 / 已下发」**：给 `BusTransaction` 或 `BusManager`
   增加发送标记，在 future 被 drop 时由清理守卫向调用方回送 `WriteUncertain`，而不是让 oneshot 静默断开
   被解释成 `BusClosed`。否则 U-1 无法修复：`BusClosed` 在 `write_outcome` 里被固定归类为明确失败
   （`gateway-core/src/south/protocol/mod.rs:250-255`）。**已实现（ROI-20，兜底路径）**：见 `frame_sent` 与
   `TxnResponder`（`serial_task.rs:92-125`）；ROI-12 的门控/drain 仍可作为进一步收敛。
3. **给配置与 collector 绑定同一代次**：reload 生成 generation，写入口一次性取「配置 + collector」同代快照；
   或在 API 写路径上获取 reload 的读侧门控。这解决 U-4，而不是靠调整 `store` 与 map 替换的先后顺序。
4. **reload 失败必须可回滚**：把 `reload_points_incremental` 的失败挡在「替换 managers/collectors 之前」，
   例如先构建新实例与组计划、全部成功后再一次性切换；当前顺序在 `gateway.rs:493-509`，失败后运行态已新、
   内存与磁盘被 API 退回旧值（U-6）。
5. **补可观测性**：reload 等待 drain 的写数量、drain 超时数、被强制取消的写数量。当前这些数字完全不存在，
   现场无法判断「命令是不是撞上了重载」。
6. **不要做的事**：不要在 `persist_then_apply` 外层再包一把锁就宣称原子——它只能阻止并发 reload，
   不能阻止写事务，也无法撤销已经写到线上的帧。

### 8.3 对 ROI-12 的验收建议

- 并发用例：串口在途写 + reload 同时发生，断言调用方拿到的 `outcome` 只能是 `Uncertain`（除非能证明帧未下发）。
- TCP 用例：reload 前后旧连接上的事务必须有明确终态，不得出现「旧连接写完、新连接又写一次」的重复下发。
- 点位用例：reload 窗口内发起写，断言点位地址与 collector 属同一配置代次。
- 失败用例：`reload_points_incremental` 注入失败后，运行态、内存快照、磁盘三者必须一致（当前会不一致）。

---

## 9. ROI-12 实施结果（2026-11）

本节记录 ROI-12 落地后与上文审计结论的差异；上文保留为当时的审计快照。

### 9.1 已收敛

| 上文问题 | 落地方式 |
|---|---|
| 串口新口打开时旧句柄仍被旧总线持有（§4 第 5 行） | commit 阶段先 `BusManager::close_task`（abort + 等待 task 退出）释放端口，再 `open_bus` 新配置 |
| 变更总线打开失败却返回成功（§4 第 6 行） | 串口打开失败 → commit 返回 `Err`，API 回滚磁盘并返回错误；运行态先用旧配置重开串口并重建受影响组 |
| `reload_points_incremental` 失败后三态不一致（§4 第 8 行） | 点位重载拆为 `prepare`（纯校验、零副作用）/ `commit`（不可失败）；prepare 失败发生在落盘之前 |
| MQTT 连不上仅记 notes、磁盘已新（ROI-12 核心之一） | 新客户端 ConnAck 成功后才允许落盘；失败整次保存被拒 |
| 落盘顺序 | `persist_then_apply` 改为 prepare → 落盘 → commit；落盘失败时计划自动释放，磁盘与运行态均不变 |

### 9.2 仍未收敛（保留在上文建议中）

- **写事务与 reload 的代次绑定（U-4 / §8.2 第 1、3 条）**：本次未引入 epoch/读写门控，
  跨 reload 的写请求仍可能「新 collector + 旧点位」；
- **TCP 在途写在 reload 后的终态**：仍由独立事务 task 完成，不保证新旧连接不重复下发；
- **drain 可观测性（§8.2 第 5 条）**：未新增等待/超时/取消计数。

上述窗口均不影响「配置本身要么全生效、要么全不生效」这一 ROI-12 目标，
但属于后续 ROI 的输入。
