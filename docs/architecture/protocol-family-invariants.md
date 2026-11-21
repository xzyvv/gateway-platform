# 次要协议族不变量清单（MC / FINS / S7Comm / OPC UA / MSTP / IEC101）

> 用途：对应 ROI-16。给 6 个次要协议族各定一张「什么必须成立」的验收表，改代码、接现场、排故障时逐条对照。
> 与 [`read-write-invariants.md`](read-write-invariants.md) 的分工：那份文档管**总线层**与**跨协议写语义**
> （B-01 ~ B-14、W-01 ~ W-16），本文只记**各协议族自己的帧、地址、超时、错误映射**，不重复其结论。
>
> 核对方式：2026-11-21 对 `gateway-core/src/south/protocol/**` 做静态阅读，**未连真机**。
> 文中所有「代码依据」都是当前源码的行号；标「待确认」的条目表示代码里读不到证据，不得据此推断设备行为。

**「测试守护」取值**

| 取值 | 含义 |
|---|---|
| 有 | `src/**/tests*` 中存在直接覆盖该不变量的用例（给出相对路径:行号） |
| 无 | 没有用例；或只有 fuzz target、构帧布局测试等**不覆盖该断言**的邻近测试（在备注中说明） |

「已有测试条数 / 缺口条数」按表中行计数（`有` 计 1，`无` 计 0），见文末汇总表。

---

## 0. 协议族与构建门控

| 协议 | 代码目录 | Cargo feature | 是否在默认发行集 `all-protocols` | 写能力 | 许可证配额 |
|---|---|---|---|---|---|
| MC（三菱 QnA 3E/4E/1E） | `gateway-core/src/south/protocol/mc` | `mc` | 是 | 读 + 写 | 仅受全局 `max_protocols` 数量上限约束，无按协议单独门控 |
| FINS（欧姆龙） | `gateway-core/src/south/protocol/fins` | `fins` | 否（退役观察，仅 `--features fins`） | 读 + 写（含 CPU RUN/STOP） | 同上 |
| S7Comm（西门子） | `gateway-core/src/south/protocol/s7comm` | `s7comm` | 是 | 读 + 写 | 同上 |
| OPC UA Client | `gateway-core/src/south/protocol/opcua` | `opcua-client`（`opcua` 为 client+server 别名） | 是 | **仅读**（未覆写 `write`） | 同上 |
| BACnet MS/TP | `gateway-core/src/south/protocol/bacnet_mstp` | `bacnet-mstp`（依赖 `bacnet`） | 否（退役观察） | **仅读**（未覆写 `write`） | 同上 |
| IEC 60870-5-101 | `gateway-core/src/south/protocol/iec101` | `iec101` | 是 | **仅读**（未覆写 `write`） | 同上 |

依据：`gateway-core/Cargo.toml:87`（default）、`:118`（all-protocols 不含 fins / bacnet-mstp）、
`:130`（opcua-client）、`:134`（s7comm）、`:136`（bacnet-mstp）、`:138`（mc）、`:140`（fins）、`:142`（iec101）；
模块门控见 `gateway-core/src/south/protocol/mod.rs:14 / :21 / :46 / :50 / :54 / :57`。

许可证只校验「已启用协议实例数」这一全局计数（`gateway-core/src/domain/license.rs:69`、`:132`），
**6 个协议族都没有各自的授权开关**；因此本列只说明「会被计入配额」，不构成优先级依据。

读能力说明：MC / S7 / FINS 用自建 socket（无总线层重试）；
MSTP / IEC101 走 `BusManager::transaction`，读事务可按全局 `retry_count` 重发；
OPC UA 由 async-opcua 会话栈负责重连，协议层无原始帧重试。

---

## 1. MC（三菱 MC / 目录 `mc`）

拓扑：自建 TCP 连接，单连接由流锁串行化；帧格式由变体选择，缺省 3E，可选 4E、1E
（`mc/frames.rs:39-56`）。读写都直接复用同一 `transact`（`mc/mod.rs:292`）。

| 编号 | 不变量 | 代码依据（文件:行） | 测试守护（有/无 + 位置） | 违反后的现场表现 |
|---|---|---|---|---|
| MC-R1 | 地址前缀必须能映射到软元件码，编号必须可解析且 ≤ `0x00FF_FFFF`；不合法在**建链前**返回 `InvalidConfig` | `mc/frames.rs:108-131`、`:155-181` | 有（`mc/tests/address_parse.rs:39`） | 配错点位整批不采集，日志报「unsupported device prefix」；不会发无效帧 |
| MC-R2 | 3E/4E 响应必须校验子头、固定头长度，以及网络号 / PC 号 / 模块 I/O 号 / 站号回显 | `mc/frames.rs:407-467` | 有（`mc/tests/codec.rs:68`、`:91`） | 串到上一帧、别站应答或非 MC 报文时被读成长度巨大的垃圾包，点位值串台 |
| MC-R3 | 4E 响应必须回显请求序列号，不匹配即判帧错 | `mc/frames.rs:435-440` | 有（`mc/tests/serial_4e.rs:87`） | 迟到应答被当成当前请求结果，点位值错位（典型「多台设备值相同」） |
| MC-R4 | 「结束码 + 数据」长度必须落在 2..=960 字节；禁止按对端给的长度直接分配 | `mc/frames.rs:387-398`、`:492` | 无（仅 fuzz `fuzz/fuzz_targets/mc_decode.rs:37` 调用，无单测） | 异常设备/扫描噪声返回超大长度时内存被一次性占满，进程可能 OOM |
| MC-R5 | 读结束码非 0 → `DeviceRejected`（PLC 明确拒绝），不得当成有效数据 | `mc/frames.rs:556-569`、`mc/mod.rs:464` | 有（`mc/tests/e2e.rs:66`、`mc/tests/codec.rs:57`） | 地址不存在时上送 0 或垃圾值，质量仍为 Good |
| MC-R6 | 1E 响应功能码必须等于请求功能码 \| 0x80；数据长度由请求点数推导且 ≤ 512 字节 | `mc/frames.rs:448-485` | 有（`mc/tests/frame_1e.rs:11`、`:25`、`:55`） | 1E 老设备上错帧被当成有效值；或读数据与点位错位 |
| MC-R7 | 单次 IO 超时取 `bus.timeout_ms`；发送/接收超时 → `NoReply` 并清空连接，下次惰性重建 | `mc/mod.rs:299-348` | 有（`mc/tests/e2e.rs:147`、`:78`） | 半开 TCP 不清理时整条通道永久卡死，后续所有点位一起超时 |
| MC-R8 | 批量读按软元件分组、地址升序合并；单帧上限 479 字 / 1916 位 / 1E 256 单元，超限自动分片 | `mc/frames.rs:90-94`、`:646`；`mc/batch.rs:97-112` | 有（`mc/tests/batch.rs:81`、`:172`、`:196`） | 点位跨区或超限时整帧被设备拒绝，表现为「一批点全错、单点读正常」 |
| MC-R9 | 批量片段失败只污染该片段的点位：建帧失败逐点回填同因错误，建链失败整批同因 `IoError`，不得逐点重复等建链超时 | `mc/batch.rs:112-150` | 有（`mc/tests/batch.rs:33`、`:12`） | 设备离线时批量刷新耗时 = 点位数 × 建链超时，轮询周期被拖垮 |
| MC-R10 | 批量解码越界（数据区不足）只能让该点位 `InvalidFrame`，不得整批丢弃 | `mc/batch.rs:170-194` | 无 | 一个点位数据异常导致同帧其它好点也被判错，故障面被放大 |
| MC-R11 | TCP 读用定长 `read_exact`，不存在半包/粘包；同一连接由流锁串行化，分片帧不交叉 | `mc/frames.rs:490-500`、`mc/mod.rs:294`、`:492` | 无（`batch.rs:12` 只断言「只建链一次」） | 批量与诊断并发下发时读到对方的应答，值串台 |
| MC-W1 | 写结束码非 0 → `DeviceRejected`（明确失败，可安全重试） | `mc/frames.rs:574-582`、`mc/mod.rs:539` | 有（`mc/tests/e2e.rs:114`） | 设备拒绝被报成「结果未知」，操作员被迫读回核实或人工确认 |
| MC-W2 | 写传输失败/无应答 → `WriteUncertain`，且不自动重发 | `mc/mod.rs:535-538` | 有（`mc/tests/e2e.rs:131`） | 上层若自动重试，累加/脉冲类点位翻倍执行 |
| MC-W3 | 写值必须做类型范围校验；越界应在上线前拒绝 | **不成立**：`mc/frames.rs:608-624` 用 `as` 强转，越界静默截断 | 无 | 下发 70000 到 U16 点位写成 4464，界面显示「写成功」但设备值是错的 |
| MC-W4 | 写后是否读回：协议层**不读回**，读回只由 API 层在 `outcome == uncertain` 时触发 | `mc/mod.rs:520-540`；API 层 `gateway-api/src/web/api/points/handlers/mod.rs:297`、`:462` | 无（`e2e.rs:101` 只验证写出帧正确） | 设备收到写但应答丢失时，只能靠上层读回消除不确定；纯协议层无法自证 |
| MC-W5 | 写事务不得自动重发（继承 W-01/W-02 口径） | `mc/mod.rs:535-538`（无重试分支） | 无（`e2e.rs:131` 只验定级，未验重发次数） | 一次超时被重发两次，脉冲输出触发两次 |
| MC-E1 | 读结束码应可区分「地址不存在/设备拒绝」与「帧损坏」 | **成立**：`mc/frames.rs:556-569` 结束码非 0 → `DeviceRejected`；`mc/batch.rs:160-169` 批量逐点保留该变体 | 有（`mc/tests/e2e.rs:66`、`mc/tests/codec.rs:57`） | —— |
| MC-E2 | 1E 状态码应与 3E/4E 结束码同口径映射 | `mc/frames.rs:468-484`、`mc/mod.rs:464` | 无 | 1E 站点地址错误显示为帧错误，排障方向跑偏 |

**测试覆盖**：18 条中 11 条有守护，7 条是缺口。已覆盖集中在「构帧/地址/结束码/超时/批量分片/响应归属」；
缺的是**长度边界、写值范围**。

**风险与建议优先级（按现场最容易踩排序）**

1. **MC-W3（写值静默截断）** —— 现场只要一次 4~20mA 换算错或量程填错就会发生，且「写成功」但设备值不对，最容易被误判为设备故障。应有范围校验并返回 `InvalidConfig`。

---

## 2. FINS（欧姆龙 / 目录 `fins`）

拓扑：TCP 模式先做节点地址握手（`FINS` 报头 + `0x0000_0000`），拿到 `server_node` 后再发 FINS 帧
（`fins/mod.rs:133-172`）；UDP 模式由地址查询串 `?node=NN` 或 IPv4 末位推导目标节点
（`fins/frames.rs:75-100`）。读为逐点 `read`，批量读只共享一次建链（`fins/collector.rs:157-179`）。

| 编号 | 不变量 | 代码依据（文件:行） | 测试守护（有/无 + 位置） | 违反后的现场表现 |
|---|---|---|---|---|
| FINS-R1 | 地址前缀映射字/位存储区码，编号为 `u16`；非法配置返回 `InvalidConfig` | `fins/frames.rs:56-67`、`:103-130` | 有（`fins/tests.rs:21`、`:37`） | 配错区名整点位不采集，不发无效帧 |
| FINS-R2 | FINS/TCP 报头必须以 `FINS` 开头，长度必须在 8..=2048 内，再按长度读满 | `fins/frames.rs:27`、`:275-305` | 无（`tests.rs:731` 只测载荷截断） | 对端长度字段异常时读阻塞到超时，或按异常长度分配 |
| FINS-R3 | 读响应长度 ≥14，且 MRC/SRC 必须与请求一致 | `fins/frames.rs:320-338` | 有（`fins/tests.rs:78`） | 迟到/串包响应被当成本次数据，值错位 |
| FINS-R4 | 读响应 MRES/SRES 非 0 → `InvalidFrame`（读路径，落 `DECODE`） | `fins/frames.rs:339-343` | 有（`fins/tests.rs:78`、`:301`） | 「地址不存在」显示为帧错误，排障被误导到链路 |
| FINS-R5 | 节点地址握手必须校验 `command == 0x0000_0002` 且 `tcp_error == 0`，并取 4 字节载荷低字节作为服务端节点 | `fins/mod.rs:151-161`、`fins/frames.rs:308-315` | 无（`tests.rs:44` 只测请求布局） | 握手被拒仍按 `server_node=0` 发帧，全部点位 `NoReply` |
| FINS-R6 | 单次发送/接收超时取 `bus.timeout_ms`；超时 → `NoReply` 且丢弃连接 | `fins/mod.rs:217-243`、`:262-282` | 有（`fins/tests.rs:331`，仅写路径） | 半开连接不复位时，整批点位依次超时 |
| FINS-R7 | 批量读整批共享一次建链；建链失败整批同因返回，不逐点重复建链 | `fins/collector.rs:157-179`、`fins/mod.rs:316` | 有（`fins/tests.rs:364`、`:384`） | 设备离线时刷新耗时被点位数放大 N 倍 |
| FINS-R8 | UDP 应答必须能区分「本次请求的应答」与「上一次的迟到应答」 | **不成立**：`fins/mod.rs:262-272` 直接取首个数据报，不校验源地址、SID 或回显 | 无（`tests.rs:552`、`:570` 只覆盖正常收发） | 网络抖动下取到上一帧应答，值滞后一拍；排查极难 |
| FINS-W1 | CPU 单元控制：地址 `RUN` / `STOP` / `CPU:RUN` / `CPU:STOP` 决定动作（忽略 value），走 `0401/0402` | `fins/collector.rs:199-213`、`fins/frames.rs:206-223` | 有（`fins/tests.rs:414`、`:444`、`:453`） | 误把 STOP 当普通写值点位下发，设备停机 |
| FINS-W2 | 写响应 MRES/SRES 非 0 → `DeviceRejected`（明确失败） | `fins/frames.rs:369-396` | 有（`fins/tests.rs:315`） | 设备拒绝被报成未知，操作员被迫人工确认 |
| FINS-W3 | 写传输失败 → `WriteUncertain`，不自动重发 | `fins/collector.rs:242-247` | 有（`fins/tests.rs:331`） | 自动重试会让脉冲/累加点位重复执行 |
| FINS-W4 | 写数值必须做类型范围校验 | **不成立**：`fins/frames.rs:405-417` 用 `as` 强转 | 无 | 与 MC-W3 同：界面写成功、设备值是截断后的错值 |
| FINS-W5 | 写后是否读回：协议层不读回；仅 API 层 uncertain 读回 | `fins/collector.rs:242-248` | 无 | 同 MC-W4 |
| FINS-E1 | FINS/TCP 报头错误码（`tcp_error != 0`）应映射为「设备拒绝」而不是帧损坏 | **不成立**：`fins/mod.rs:244-249` 返回 `InvalidFrame` | 无 | 命令不支持/节点错等被显示为 `DECODE`，现场误判为噪声 |
| FINS-E2 | 读结束码应能区分地址类错误与命令类错误 | **不成立**：`fins/frames.rs:339-343` 只有一句错误文本，统一 `InvalidFrame` | 无 | 地址越界与未定义命令在页面上无法区分 |

**测试覆盖**：15 条中 8 条有守护，7 条是缺口。缺的是**TCP 长度边界、握手失败分支、UDP 串包、写值范围、错误码归类**。

**风险与建议优先级**

1. **FINS-R8（UDP 无应答匹配）** —— UDP 模式下一次迟到应答就会被当成本次结果，且没有任何日志能看出来。现场表现为「偶发某个点值落后一拍/跳变」，几乎无法复现。优先补「源地址 + SID/帧头回显」校验，或在文档里明确 UDP 仅适用于低速轮询（**设备是否回显 SID 待确认**）。
2. **FINS-W4（写值截断）** —— 与 MC 同源问题，PLC 类项目写设定值频率高，错值直接进设备。

---

## 3. S7Comm（西门子 / 目录 `s7comm`）

拓扑：自建 TCP，先 COTP 连接，再 Setup Communication 协商 PDU，读/写各走 S7 PDU
（`s7comm/collector/mod.rs:20-127`）。单点与多 item 两条路径都在协议层做 index 对齐。

| 编号 | 不变量 | 代码依据（文件:行） | 测试守护（有/无 + 位置） | 违反后的现场表现 |
|---|---|---|---|---|
| S7-R1 | 地址解析必须支持 DB/M/I/Q 与位/字节/字/双字；DB 必须含 `.`，位号与偏移越界返回 `InvalidConfig` | `s7comm/address.rs:72-200` | 有（`s7comm/tests/address_parse.rs:5-124`，共 14 例） | 点位配错时整点不采集，且不发请求 |
| S7-R2 | TPKT 必须校验版本 `0x03 0x00` 且总长 ≥4；**读取前必须有上限** | 版本/下限：`s7comm/frames/link.rs:59-73`；**上限缺失** | 无 | 异常设备回报 65535 长度时按 64KB 分配并阻塞等待，表现为整条通道卡死 |
| S7-R3 | 响应必须校验 Protocol ID `0x32`、PDU type、`param_len/data_len` 切片边界，不得越界读 | `s7comm/frames/read.rs:60-120`、`:198-250` | 有（`s7comm/tests/codec.rs:145`、`:176`；`multi_read.rs:83`、`:129`、`:171`） | 错帧被解析成有效值，或 panic 后任务退出（历史 Bug 已修） |
| S7-R4 | 单点读 `return_code`：`RET_NOT_FOUND` → `InvalidConfig`，其它非 0 → `DeviceRejected` | `s7comm/frames/read.rs:121-135` | 有（`s7comm/tests/codec.rs:209`、`:235`） | 点位地址不存在显示为 `CONFIG`，而设备内部拒绝显示为设备拒绝 |
| S7-R5 | 值数据长度必须 ≥ 数据类型期望字节数，否则 `InvalidFrame` | `s7comm/frames/read.rs:142-148`、`s7comm/frames/decode.rs:5-40` | 有（`s7comm/tests/codec.rs:138`） | 短帧被解码成错误数值，质量仍为 Good |
| S7-R6 | 多 item 读必须校验 `item_count` 与请求一致；单个 item 失败只污染该点；按协商 PDU 分片（`(pdu-14)/12`，1..=255） | `s7comm/frames/read.rs:245-275`、`s7comm/collector/read.rs:195-225` | 有（`multi_read.rs:129`、`:171`；`regression.rs:18`） | 批量点位错位映射，出现「整屏值相同/错位」 |
| S7-R7 | 惰性建链；IO 失败清空连接，下次调用自动重连 | `s7comm/collector/read.rs:18-20`、`:47-76` | 有（`s7comm/tests/e2e.rs:17`、`:64`） | 断链后不重连，整站永久离线 |
| S7-R8 | 应答的 PDU reference 必须与请求一致（防串包），读写路径同口径 | `s7comm/frames/read.rs:58-69`；`collector/read.rs:79`、`collector/mod.rs:264`、`collector/write.rs:92`、`:211` | 有（`s7comm/tests/codec.rs:371`、`:387`） | 高并发或设备缓冲异常时取到上一请求的应答，值串台 |
| S7-R9 | 协商得到的 PDU 大小必须落在合理区间 | **不成立**：`s7comm/collector/mod.rs:109-114` 只挡 `pdu == 0`，超大值直接采用 | 无 | 设备回报异常大 PDU 时按超大值切分，实际超帧被设备拒绝，整批读失败 |
| S7-W1 | 写值范围校验在上线前完成：越界 → `InvalidConfig`（明确失败） | `s7comm/frames/decode.rs:5-40`、`collector/write.rs:39-49` | 有（`s7comm/tests/write_codec.rs:62`、`:94`） | 越界写被静默截断（与 MC/FINS 相反，S7 已守住） |
| S7-W2 | 写响应设备错误 class/code 与多 item 逐项 return code 非 0 → `DeviceRejected` | `s7comm/frames/write.rs:80-130`、`:200-260` | 有（`s7comm/tests/write_codec.rs:114`、`:129`；`e2e.rs:188`） | 设备拒绝被报成未知，无法安全重试 |
| S7-W3 | 发送/读响应失败 → `WriteUncertain`，不自动重发 | `s7comm/collector/write.rs:51-92` | 有（`s7comm/tests/e2e.rs:206`） | 自动重试会让写操作重复执行 |
| S7-W4 | 多写必须与入参等长且逐位对应，按 item 值长度分片（不套用读的 12B/item 预算） | `s7comm/collector/write.rs:109-210` | 有（`multi_write.rs:99`、`:121`、`:148`、`:171`；`e2e.rs:162`） | 批写结果与点位错位，写错设备 |
| S7-W5 | 写后是否读回：协议层不读回，仅 API 层 uncertain 读回 | `s7comm/collector/write.rs:90-93` | 无 | 应答丢失时协议层无法自证，需上层读回 |
| S7-E1 | PDU 级错误（`pdu_type & 0x80`，error class/code）在读写路径应同口径 | **成立**：读 → `DeviceRejected`（`frames/read.rs:73-88`、`:210-225`），写 → `DeviceRejected`（`frames/write.rs:81-92`）；多 item 逐项失败见 `collector/read.rs:222-234` | 有（`s7comm/tests/codec.rs:190`、`:235`） | —— |

**测试覆盖**：15 条中 12 条有守护，3 条是缺口。缺的是**TPKT 上限、PDU 大小边界、写后读回**。

**风险与建议优先级**

1. **S7-R2（TPKT 无上限）** —— 一个畸形/错位的长度字段就能让采集任务阻塞到超时并反复重连，且只在现场网络异常时出现。应加与 MC `body_len_checked`、FINS `8..=2048` 同等口径的上限。

---

## 4. OPC UA Client（目录 `opcua`）

拓扑：async-opcua 客户端，共享 `Arc<Mutex<Option<Session>>>`；读入口惰性建链，
连续读失败达阈值后由后台任务拆会话（`opcua/mod.rs:189-208`），并有 keepalive 探测
（`opcua/mod.rs:241-338`）。**未覆写 `write`，默认实现返回 `Unsupported`**（`protocol/mod.rs:502-511`）。

| 编号 | 不变量 | 代码依据（文件:行） | 测试守护（有/无 + 位置） | 违反后的现场表现 |
|---|---|---|---|---|
| UA-R1 | NodeId 只接受 `ns=N;s=`、`ns=N;i=`、`ns=N;g=` 与 `ns,X` 简写；其它形式返回 `InvalidConfig` | `opcua/frames.rs:16-66` | 有（`opcua/tests.rs:4`、`:10`、`:16`、`:22`） | 点位地址写错时不发请求，日志明确指出格式 |
| UA-R2 | 建链必须先 `wait_for_connection()` 成功，否则 `IoError`；重复 connect 不重建会话 | `opcua/mod.rs:556-630` | 无（只有 `tests.rs:108` 验证 diag 在连接失败时如实报错） | 会话未就绪就发起读，全部点位 `IoError`，且可能反复重建 |
| UA-R3 | 批量读返回的结果数必须 ≥ 请求数；不足的按原下标逐点 `InvalidFrame` | `opcua/mod.rs:455-462` | 无 | 少返回时部分点位拿到别人的值或空结果 |
| UA-R4 | 单节点 `StatusCode` 必须影响质量：Good/Uncertain/Bad，且非 Good 必须带可辨识原因 | `opcua/mod.rs:120-146`（映射）、`:475`（批量）、`:814`（单点） | 有（`opcua/tests.rs:312`、`:335`；映射函数单测，两条读路径共用） | 见 UA-E2：Bad 以 `Ok(DataPoint)` 上送且无原因，现场无法区分「配错/权限/设备没值」 |
| UA-R5 | 读失败阈值必须小于会话超时，避免「会话先掉、计数还没到」 | `opcua/mod.rs:89`（`READ_FAILURE_THRESHOLD`）、`:104`（`KEEPALIVE_MAX_CONSECUTIVE_FAILURES`）、`:189-207` | 有（`opcua/tests.rs:35`、`:47`，只锁阈值取值范围） | 阈值配错时会话永不重建，整站离线但页面显示「连接中」 |
| UA-R6 | keepalive 探测失败达阈值必须拆会话并标记断连；`try_lock` 抢不到时必须跳过，不得阻塞采集 | `opcua/mod.rs:241-338` | 有（`opcua/tests.rs:65`、`:87`） | 半开会话不重建；或 keepalive 与采集争锁导致轮询周期抖动 |
| UA-R7 | 读失败必须清零/累加计数，成功即清零；单点问题不得累加链路失败 | `opcua/mod.rs:210-218`（计数）、`:440`/`:804`（成功清零）、`:384`/`:431`/`:795`（仅链路级失败累加） | 有（`opcua/tests.rs:371`：连续读不到值的点不计链路失败、不拆会话） | 偶发失败累计到阈值后误拆健康会话，出现周期性「闪断」 |
| UA-R8 | 批量读必须一次请求多节点，并按原下标回填；单点解析失败不得改变其它点位下标 | `opcua/mod.rs:355-506`（`:444-499` 回填） | 无 | 一个坏地址让后续点位整体错位 |
| UA-W1 | 该协议不支持写，写请求必须是「明确失败」（`Unsupported`），不得伪装成未知 | `protocol/mod.rs:502-511`；`opcua/mod.rs` 无 `fn write` | 有（`registry/tests.rs:387`，跨协议能力位一致性） | 前端显示可写而实际必失败（已被能力位测试挡住） |
| UA-E1 | 值提取错误必须给出可区分的原因：不支持的 Variant → `Unsupported`，无值/字符串非数字 → `InvalidFrame`；Bad/Uncertain 且无值 → `DeviceRejected` | `opcua/frames.rs:69-92`；`opcua/mod.rs:142-152`（`rejected_status`）、`:477-485`/`:819-825`（两读路径） | 有（`opcua/tests.rs:347`） | 点位类型不匹配时只看到 `DECODE`，无法区分「配错类型」「设备没值」与「设备拒绝」 |
| UA-E2 | 单节点 Bad / 取不到值不得被计为「链路读失败」；Bad/Uncertain 必须带可辨识 `quality_reason` | **成立**：`opcua/mod.rs:439-440`（server 已响应即 `record_read_ok`，先于取值）、`:795-809`（取不到值只降级该点，不再 `record_read_failure`）；`:120-146` 生成 `opcua_status_<StatusCode>:OTHER` | 有（`opcua/tests.rs:312`、`:335`、`:347`、`:371`） | 一个节点被设备拒绝（权限/不存在）时，连续刷新不再拆会话；页面可按 `quality_reason` 区分具体 StatusCode |

**测试覆盖**：11 条中 8 条有守护（UA-R4、UA-R7 由 ROI-21 补齐，UA-E2 已成立，UA-E1 由 ROI-25 补齐），3 条是缺口。
读服务剩余缺口集中在**建链前置、批量结果少返回、下标回填**（UA-R2/R3/R8），仍无端到端用例。

**风险与建议优先级**

1. **UA-E2（单点 Bad 触发整会话拆除）** —— 已修复（ROI-21）：server 响应即清零链路计数，取不到值只降级该点；Bad/Uncertain 现在带 `opcua_status_<StatusCode>:OTHER`。
2. **UA-R3/UA-R8（批量结果数与下标对齐）** —— 属于「不报错但值不对」的一类，一旦遇到只能靠比对上游数据发现。

---

## 5. BACnet MS/TP（目录 `bacnet_mstp`）

拓扑：串行 MS/TP，经 `BusManager::transaction` 发送**单点**请求（无批量读覆写），
接收后校验 CRC、帧类型，再交给 BACnet APDU 解析（`bacnet_mstp/mod.rs:168-275`）。
诊断走 Who-Is 广播探针（`bacnet_mstp/mod.rs:118-166`）。

| 编号 | 不变量 | 代码依据（文件:行） | 测试守护（有/无 + 位置） | 违反后的现场表现 |
|---|---|---|---|---|
| MSTP-R1 | 帧必须以 `0x55 0xFF` 开头，`data_len ≤ 512`，头 CRC8 与数据 CRC16 必须正确 | `bacnet_mstp/frames.rs:6-7`、`:75-145` | 有（`tests.rs:142`、`:148`、`:155`、`:164`） | 噪声帧被当成数据，解析出随机值 |
| MSTP-R2 | 帧提取器必须在前导噪声、伪前导（数据中出现 `0x55 0xFF`）下逐字节重同步，且不得 panic | `bacnet_mstp/frames.rs:149-207` | 有（`tests.rs:224`、`:234`、`:249`、`:269`、`:293`、`:440`） | 一次串口噪声后整条总线永久错帧，只能重启进程 |
| MSTP-R3 | 只接受 DataExpectingReply / DataNotExpectingReply / ReplyPostponed；其它帧类型按无应答处理 | `bacnet_mstp/mod.rs:237-245` | 无 | 令牌帧被当数据解析 → 点位随机错值 |
| MSTP-R4 | 目标站号与主站号必须 ≤127，否则 `InvalidConfig` | `bacnet_mstp/mod.rs:178-191` | 无（`tests.rs:195`、`:201` 只测 `parse_master_addr`） | 站号配错时发不出合法帧，全部点位超时 |
| MSTP-R5 | 应答 APDU 的 `invoke_id` 必须与请求一致 | `bacnet/frames/decode.rs:169-174`（`bacnet_mstp/mod.rs:257` 调用） | 无 | 串包时把别的请求结果当本次值 |
| MSTP-R6 | 应答源地址必须与目标站一致（防他站应答） | **不成立**：`bacnet_mstp/mod.rs:234` 解出 `_resp_src` 后丢弃，不比较 | 无 | 多主站/重复地址现场：值来自别的设备，且无任何告警 |
| MSTP-R7 | 单点一次事务，超时取 `bus.io_timeout()`；超时 → `NoReply` | `bacnet_mstp/mod.rs:228-231` | 无（无读路径 e2e） | 从站掉线时每个点都要等满超时，轮询周期被点位数放大 |
| MSTP-R8 | BACnet Error / Reject / Abort PDU 必须映射为「设备明确拒绝」 | **不成立**：`bacnet/frames/decode.rs:163-167` 只认 ComplexACK，其它一律 `InvalidFrame` | 无 | 设备拒绝读写属性时显示为 `DECODE`，现场去查 CRC/接线 |
| MSTP-W1 | 该协议不支持写，写必须返回 `Unsupported` | `bacnet_mstp/mod.rs` 无 `fn write`；`protocol/mod.rs:502-511` | 有（`registry/tests.rs:387`） | 前端误开放写入口（已被能力位测试挡住） |
| MSTP-E1 | CRC 错 → `ChecksumError`；结构/长度错 → `InvalidFrame`；无数据帧 → `NoReply` | `bacnet_mstp/frames.rs:106-113`、`:129-138`；`mod.rs:240-250` | 有（`tests.rs:148`、`:155`，仅 CRC 分支） | 统计里 CRC 与「设备不回」混在一起，无法判断是接线还是从站故障 |
| MSTP-E2 | 诊断必须如实反映「Who-Is 无应答」，不得伪造成功 | `bacnet_mstp/mod.rs:118-166` | 有（`tests.rs:407`、`:425`） | 诊断显示正常但实际离线，运维被误导 |

**测试覆盖**：11 条中 5 条有守护，6 条是缺口。帧层（CRC/重同步）覆盖很好；
**采集路径（帧类型过滤、源地址、invoke_id、超时、Error PDU）没有任何 e2e 用例**。

**风险与建议优先级**

1. **MSTP-R6（不校验应答源地址）** —— MS/TP 是多点共享总线，重复站号或第三方主站应答时会把别的设备数据写进本设备点位，且只能靠上游数据异常发现。这是本协议族最该补的一条。
2. **MSTP-R3 + MSTP-R8** —— 令牌帧/错误 PDU 被当数据或帧错误，现场表现为「偶发单点跳变」，很难复现。

---

## 6. IEC 60870-5-101（目录 `iec101`）

拓扑：串行 FT1.2，经 `BusManager::transaction`；`refresh()` 做一次总召唤 + 最多 8 轮二级数据轮询，
把 IOA 快照写入缓存，点位读取只从缓存取值（`iec101/mod.rs:89-185`）。**未覆写 `write`**。

| 编号 | 不变量 | 代码依据（文件:行） | 测试守护（有/无 + 位置） | 违反后的现场表现 |
|---|---|---|---|---|
| IEC-R1 | 三种帧型必须校验长度与结束符：单字节 `0xE5`、固定帧 5 字节且 `0x16` 结尾、变长帧 `6+L` 且 `0x16` 结尾 | `iec101/frames.rs:131-206` | 有（`tests.rs:61`、`:73`、`:101`、`:614`、`:660`） | 截断帧被当完整帧，后续字节全部错位 |
| IEC-R2 | 校验和（字节累加 mod 256）必须校验，错误 → `ChecksumError` | `iec101/frames.rs:63-65`、`:154-156`、`:191-194` | 有（`tests.rs:91`、`:660`） | 噪声帧被采纳为有效数据 |
| IEC-R3 | 帧提取器必须丢弃前导噪声并重同步；不完整帧返回 `None` 等待更多字节 | `iec101/frames.rs:212-267` | 有（`tests.rs:108`、`:116`、`:126`、`:134`） | 长帧被切碎后永久错位，整站数据不刷新 |
| IEC-R4 | 变长帧两个 L 字节必须一致且 `L≥2`；ASDU 长度 ≤253 | `iec101/frames.rs:168-185`、`:46`、`:78-84` | 无（`tests.rs:45` 只测构帧布局） | 长度字段错误时整帧被丢弃并重同步，表现为偶发丢一轮数据 |
| IEC-R5 | 总召唤后必须轮询二级数据直到激活终止，最多 `MAX_POLLS=8` 轮；收到数据后遇空确认即结束 | `iec101/mod.rs:104-221` | 有（`tests.rs:378`、`:412`） | 轮询不结束 → 每轮采集都超时；或数据未收全就返回，点位缺失 |
| IEC-R6 | 应答的链路地址与 ASDU 公共地址（CA）必须与本站配置一致 | `iec101/mod.rs:116-186`（收发两处比对）、`common/asdu.rs:528-538` | 有（`tests.rs:720`、`:743`、`:766`、`:790`） | 同总线多站/错站应答按 IOA 覆盖本设备缓存，出现「值来自别的站」 |
| IEC-R7 | ASDU 解码失败必须能上报为可辨识错误，不得静默丢弃 | `common/asdu.rs:326-514`（`parse_asdu_checked`）、`iec101/mod.rs:195-206`、`:254-281` | 有（`tests.rs:808`、`:855`） | 数据格式不符时点位一直 `NoReply`，现场以为设备没回，实际是解码失败 |
| IEC-R8 | IOA 必须为十进制 `u32`，非法配置返回 `InvalidConfig` | `iec101/mod.rs:284-288` | 有（`tests.rs:510`） | 点位配错时不刷新该点，不发请求 |
| IEC-R9 | 缓存缺该 IOA 时必须返回 `NoReply`，不得伪造 0 值 | `iec101/mod.rs:223-245` | 有（`tests.rs:442`） | 点值恒为 0 且质量 Good，上游看不出异常 |
| IEC-R10 | 应答控制域的 FCB/ACD 位必须与请求匹配（防重复帧） | **不成立**：`iec101/mod.rs:145-148`、`:168-171` 只递增请求 FCB，不校验应答 | 无 | 重复帧被当新数据，总召唤数据重复入库 |
| IEC-W1 | 该协议不支持写，写必须返回 `Unsupported` | `iec101/mod.rs` 无 `fn write`；`protocol/mod.rs:502-511` | 有（`registry/tests.rs:387`） | 前端误开放写入口（已被能力位测试挡住） |
| IEC-E1 | 链路复位失败可「尽力而为」忽略；总召唤/二级数据失败必须上抛，且诊断文本如实记录复位结果 | `iec101/mod.rs:91-102`、`:307-340` | 有（`tests.rs:327`、`:343`、`:373`） | 复位被拒仍显示「诊断正常」，或第一次采集前需要人工复位 |
| IEC-E2 | ASDU 层错误（COT 否定确认、类型不支持）应映射为设备拒绝/不支持，而非笼统超时 | `iec101/mod.rs:254-281`、`common/asdu.rs:516-522` | 有（`tests.rs:832`、`:808`） | 设备回否定确认时点位显示 `NoReply`，现场去查串口接线 |

**测试覆盖**：13 条中 11 条有守护，2 条是缺口（R4 变长帧边界、R10 FCB 校验）。
地址归属（链路地址/CA）、ASDU 错误码、解码失败上报已由 ROI-22 补齐（`d8d85bc`）；仍无 fuzz target。

**风险与建议优先级**

1. **IEC-R10（不校验应答 FCB）** —— 重复帧被当新数据，总线重发/干扰时同一帧会重复入库，且不会有任何告警。
2. **IEC-R4（变长帧 L 字节边界）** —— 错误长度会整帧丢弃并重同步，表现为偶发丢一轮；建议补边界用例 + fuzz。

---

## 7. 汇总与优先补测建议

| 协议 | 表格行数 | 有测试守护 | 缺口 |
|---|---|---|---|
| MC | 18 | 11 | 7 |
| FINS | 15 | 8 | 7 |
| S7Comm | 15 | 12 | 3 |
| OPC UA | 11 | 8 | 3 |
| BACnet MS/TP | 11 | 5 | 6 |
| IEC101 | 13 | 11 | 2 |
| 合计 | 83 | 55 | 28 |

**先补哪两个**

1. **OPC UA**：已补 UA-R4/R7、UA-E2、UA-E1，剩余缺口集中在建链前置与批量对齐（UA-R2/R3/R8）——仍属「不报错但值不对」的高风险区。
2. **IEC101**：地址归属、ASDU 错误码、解码失败上报已补齐（ROI-22，`d8d85bc`）；剩余 IEC-R10（FCB 校验）与 IEC-R4（变长帧边界），且没有 fuzz。

**跨协议共性缺口（值得一次性处理）**

- **写值范围校验**：MC-W3、FINS-W4 都缺，S7-W1 已有——同一类问题在不同协议实现不一致。
- **应答归属**：MC 3E/4E 固定头回显（网络号/PC 号/模块 I/O/站号）与 S7 PDU reference 已由 ROI-26 补齐并在读写路径统一校验；IEC101 已在 ROI-22 校验链路地址/公共地址。OPC UA 由协议栈按 RequestId 配对，应用层无额外可比标识；批量结果按原下标回填且不足逐点报错（UA-R3）。仅 FINS（UDP 无匹配）与 MSTP（不校验源地址）因退役观察未覆盖。
- **错误码粒度**：设备明确拒绝被归入 `InvalidFrame`/`DECODE` 的现象在 FINS TCP（`tcp_error`）与 MSTP（Error/Reject/Abort PDU）仍存在（两族列入退役观察，暂不修改）。MC 读结束码、S7 读 PDU/逐项返回码、OPC UA Bad/Uncertain 无值已在 ROI-25 统一为 `DeviceRejected`（`608418f`）；IEC101 已在 ROI-22 把 COT 否定确认映射为 `DeviceRejected`、类型不支持映射为 `Unsupported`。`read-write-invariants.md` 的 W-03c（明确错误码必须用 `DeviceRejected`）现在读写路径口径一致。

