# 微基准基线（criterion）

`gateway-core/benches/` 的 criterion 微基准实测结果存档。用于**同机回归对照**，
不是发布门禁（CI 只跑 `cargo bench --no-run` 保证可编译，数值不进阈值判定）。

与 [`perf-baseline.md`](perf-baseline.md)（端到端 13 场景压测）的分工：
本文件测的是**单点成本**（一次 CRC、一帧构建、一批队列读写），用于定位优化是否有效；
端到端吞吐 / RSS / 队列深度请看 `perf-baseline.md`。

## 1. 本次基线环境（2026-11-06）

| 项 | 值 |
|---|---|
| 机型 | 笔记本（Intel Core Ultra 9 275HX，24 核 24 线程） |
| 内存 | 63.4 GB |
| 操作系统 | Windows 11（10.0.26200） |
| Rust | rustc 1.98.1（2026-09-01） |
| criterion | 0.5（`html_reports`） |
| 采样 | criterion 默认：预热 3 s、测量 5 s、100 样本 |
| 构建口径 | `profile.bench` 继承 `[profile.release]`：`opt-level = "z"`、`lto = true`、`codegen-units = 1` |

**口径提醒**：数值来自**体积优先**的 release 配置（`opt-level="z"` + LTO），
不是吞吐优先的 `opt-level=3`。不要在其它机器 / 其它 profile / debug 构建下直接对比。

复现：`cargo bench -p gateway-core`（全量）或 `cargo bench -p gateway-core --bench protocol_decode` /
`--bench queue_push_pop`（单组）。HTML 报告在 `target/criterion/report/index.html`。

## 2. 基线数值

均值（mean）与中位数（median）取自 `target/criterion/**/new/estimates.json`。

### 2.1 协议帧（`benches/protocol_decode.rs`）

| 基准 | mean | median | 吞吐 |
|---|---|---|---|
| `crc16/8`（8 字节） | 70.5 ns | 70.5 ns | 14.19 M 次/s |
| `crc16/64` | 568.5 ns | 571.1 ns | 1.76 M 次/s |
| `crc16/256` | 2.28 µs | 2.29 µs | 438.7 k 次/s |
| `crc16/1024` | 8.98 µs | 8.98 µs | 111.3 k 次/s |
| `modbus_frame_build/1`（1 寄存器 + CRC） | 94.1 ns | 94.9 ns | 10.63 M 帧/s |
| `modbus_frame_build/8` | 288.1 ns | 289.8 ns | 3.47 M 帧/s |
| `modbus_frame_build/32` | 819.8 ns | 828.4 ns | 1.22 M 帧/s |
| `modbus_frame_build/128` | 2.77 µs | 2.80 µs | 361 k 帧/s |
| `modbus_verify_256_frames`（整组 256 帧 CRC 校验） | 13.12 µs | 13.26 µs | 19.5 M 帧/s（≈51.3 ns/帧） |

### 2.2 队列（`benches/queue_push_pop.rs`）

| 基准 | mean | median | 折算吞吐 |
|---|---|---|---|
| `memory_enqueue/1` | 159.6 ns | 159.6 ns | 6.27 M 点/s |
| `memory_enqueue/64` | 9.36 µs | 9.39 µs | 6.84 M 点/s |
| `memory_enqueue/1024` | 135.7 µs | 135.7 µs | 7.55 M 点/s |
| `memory_dequeue_ack/1` | 298.1 ns | 299.3 ns | 3.35 M 点/s |
| `memory_dequeue_ack/64` | 12.67 µs | 12.56 µs | 5.05 M 点/s |
| `memory_dequeue_ack/1024` | 199.4 µs | 198.7 µs | 5.14 M 点/s |
| `sqlite_cycle/1`（入队→出队→ACK，含 WAL） | 3.20 ms | 3.13 ms | 312 次/s |
| `sqlite_cycle/64` | 11.75 ms | 11.50 ms | 5.4 k 点/s |
| `sqlite_cycle/256` | 33.79 ms | 33.39 ms | 7.6 k 点/s |
| `dataqueue_roundtrip/1`（生产路径 mpsc + actor） | 48.7 µs | 48.5 µs | 20.5 k 次/s |
| `dataqueue_roundtrip/64` | 96.25 µs | 96.35 µs | 665 k 点/s |
| `dataqueue_roundtrip/256` | 193.6 µs | 192.8 µs | 1.32 M 点/s |

## 3. 读数口径与解读

1. **`sqlite_cycle` 的绝对值包含磁盘 fsync**，标准差很大（`/1` 达 386 µs，`/256` 达 6.1 ms）。
   它测的是持久化真实成本，不是 CPU 成本；单次采样的波动属正常，别据此判定「性能退化」。
   需要稳态判据时，看 `perf-baseline.md` 的端到端队列数据，或多次重跑取中位数。
2. **`dataqueue_roundtrip` 含 Tokio 调度**，是生产路径的编排开销；纯数据结构成本看
   `memory_*` 组。两者不要混比。
3. **`modbus_verify_256_frames` 只做长度检查 + CRC**，不等于完整解帧（未含功能码 / 异常码 /
   点位映射）。协议解析真实成本请结合 soak 的解码错误率与端到端压测。
4. 内存队列吞吐随批量增大（`1 → 1024` 点由 6.3 M 提升到 7.6 M 点/s），说明批量入队摊薄了
   单次调用开销；生产采集已是批量路径，`/1` 的数据只代表单点路径。

## 4. 回归判定建议（非门禁）

- 同一台机器、同一 rustc、同一 profile 重跑；先跑两次取中位数，排除冷启动 / 后台扫描干扰。
- 单项 mean 相比本表**劣化 > 20%** 时再排查（先看是否有新增锁 / 克隆 / 分配，
  再对照 rustc 版本与 Windows 电源模式）。
- 跨机器数值无可比性（CPU 代际、硬盘、电源策略差异远大于 20%）。
- 优化前后对比时，把 commit 与日期写进本节下方，便于回溯。

| 日期 | commit | 说明 |
|---|---|---|
| 2026-11-06 | 本轮（`fix:` soak + 代码审查批次之后） | 首次入档：7 组基准 / 21 个采样点 |
