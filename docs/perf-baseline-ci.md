# CI 性能基线（GitHub Actions / Linux）

> 本文件描述**CI 基线**的产生方式与判定口径。
> 与 [perf-baseline.md](perf-baseline.md) 的关系：后者是开发机（24 核 Windows）
> 的历史数据，**两者不可互相比较**——即使点数与帧形态相同，两者也不在同一尺度上，
> 4 vCPU runner 上的绝对吞吐必然低于 24 核开发机。

## 1. 为什么有这份基线

真机尚未到位，交付形态又是 Linux（Buildroot / Debian）。开发机采样有三个问题：
数字依赖当天的机器负载、依赖采样人、依赖操作系统。把压测放进 CI 后，
**规格固定（ubuntu-latest，4 vCPU / 16 GB）、时间固定（每日 UTC 19:00）、参数固定**，
同一份代码在同一套参数下跑出来的差异，才能归因到代码而不是环境。

## 2. 场景与参数

工作流：[`perf-baseline.yml`](../.github/workflows/perf-baseline.yml)
压测脚本：[`gateway-core/scripts/perf_baseline.py`](../gateway-core/scripts/perf_baseline.py)
对比脚本：[`gateway-core/scripts/perf_compare.py`](../gateway-core/scripts/perf_compare.py)

| 场景名 | 点位 | 从站 | 总线 | 周期 | 时长 | 用途 |
|---|---|---|---|---|---|---|
| `s800` | 800 | 20 | 4 | 1s | 3 min | 小规模多点位：看尾延迟与稳定性 |
| `s2000` | 2 000 | 50 | 8 | 1s | 5 min | 中等规模：吞吐基准 |
| `s8000` | 8 000 | 1 | 8 | 1s | 3 min | 大规模：地址连续合并成 125 寄存器帧，压解码 + 入队 |
| `soak2000` | 2 000 | 50 | 8 | 1s | 60 min | 长稳：判 RSS 斜率 |

从站由脚本内置的 Modbus TCP 模拟器提供（每档 2 个 mock 进程，绕开 Python GIL），
不依赖任何外部设备；`mqtt` 在生成配置里关闭，北向不参与本次测量。

`soak2000` 额外加 `--drain`：启用内置模拟消费者做「出队 → ack」。MQTT 关闭时队列只进不出，
RSS 会随积压单调上涨（实测 60 min 从 73 MB 涨到 226 MB，全程拟合 144 MB/h），
量到的是历史库容量而不是网关稳态内存。开启 drain 后队列保持有界，RSS 斜率才有回归意义；
短场景不加 drain，保持「只压解码 + 入队」的口径。

每档都从干净的 `perf-run` 目录起跑（脚本先删后建）：四档共用一个 target 目录时，
上一档残留的队列库会让下一档继承积压（实测 soak 起跑即带 46.9 万行），
RSS 曲线变成「填充 + 锯齿」，斜率判据随之失效。

`s8000` 用单从站而非 200 从站的取舍：4 vCPU 上模拟从站自身撑不住 ~8000 事务/秒，
不合并时量到的是测试端上限而非网关上限；合并后事务数降到 ~64/轮，
测的是网关「帧解码 + 点位入队」这条路径——这正是大规模点位最先退化的地方。

## 3. 门禁口径

对比脚本对每档独立判定，任一项不满足即整次 FAIL：

| 判定项 | 阈值 | 说明 |
|---|---|---|
| 采集失败 / 协议解码错误 | = 0（绝对值） | 与基线无关，出现即为缺陷 |
| 吞吐 | ≥ 基线 × 0.80 | 周期封顶场景同样按此判 |
| p99 采集延迟 | ≤ max(基线 × 1.50, 50 ms)，**或** p99/p50 ≤ 基线形状 × 2.00 | 双判据，任一成立即通过。GitHub 的 4 vCPU runner 是共享的，整机被抢占时所有延迟同比例放大——实测同一份代码 s8000 的 p50 从 75 ms 抖到 300 ms、p99 从 100 ms 抖到 496 ms，而 p99/p50 只从 1.33 变到 1.66，只看绝对值必然误报。形状判据把整机缩放约掉，真回归（p50 平稳而 p99 拉长）两条都拦。50 ms 绝对下限用于基线 p99 只有 10 ms 的场景，纯比例阈值会被 10~50 ms 的自然抖动打穿 |
| CPU 漂移 | 基线 ±40%（仅提示） | 超出说明本次机器状态与基线不同（被抢占 / 降频），延迟数字可比性下降。只记 NOTE 并在摘要里点明，不判失败——判红等于让环境噪声决定门禁 |
| RSS 尾段拟合斜率 | ≤ 50 MB/h（仅 `soak2000`） | 只取最后 20 min 做最小二乘拟合：启动期分配与队列库首次填充属于一次性占用，全程拟合会把填库摊成上百 MB/h |
| RSS 峰值 | ≤ 512 MB（仅 `soak2000`） | 绝对占用上限，兜住「斜率已走平但内存已被撑大」 |

RSS 为什么不用 24h 长稳的口径（1 MB/h）：CI 的 soak 只有 60 min，且 `RSS` 量化到 0.1 MB、
采样 5s，斜率本身的噪声就有几个 MB/h；1 MB/h 在这条时间尺度上没有区分力，
只会把正常的内存走平判成泄漏。真实的线性泄漏在尾段 20 min 里仍然会体现出来，
长时间尺度的 1 MB/h 门禁由 `gateway-core/tests/soak` 的 24h 判定负责。

两种**不判失败**的情况（避免误报）：

- **BASELINE-MISSING**：`docs/perf-baseline-ci.json` 不存在，或缺少某个场景。
  输出提示并以 0 退出，等首次数据被采纳为基线。
- **SKIP**：本次 `cpu_count` 与基线不同（runner 规格换了）。数字不在同一尺度上，
  必须重建基线后再启用门禁。

## 4. 建立 / 更新基线

1. 打开 Actions → **perf-baseline** → Run workflow，勾选 `update_baseline`；
2. 运行成功后下载产物 `perf-baseline-candidate`（内含 `perf-baseline-ci.json`）；
3. 人工核对：四档的吞吐 / p99 / RSS 斜率与本文件 §2 的预期量级是否一致，
   `failures` / `decode_errors` 是否为 0，`cpu_count` 是否为 4；
4. 覆盖 `docs/perf-baseline-ci.json` 并提交。

**不要由 CI 直接改写基线**：基线下调会把回退阈值一起放宽，
一次异常的夜间运行就能让后续真实退化不再报警。更新基线属于评审动作。

## 5. 本地复现

```bash
# Windows / Linux 通用；Linux 上可执行名自动取 gatewayd，Windows 取 gatewayd.exe
cargo build --release -p gatewayd
python3 gateway-core/scripts/perf_baseline.py --no-build \
  --points 2000 --slaves 50 --buses 8 --mock-procs 2 --duration-min 5 \
  --json /tmp/run.json --output /tmp/run.md

# 与 CI 基线对比（本机 4 核以上的绝对数字不可比，仅用于流程验证）
python3 gateway-core/scripts/perf_compare.py \
  --baseline docs/perf-baseline-ci.json --current local=/tmp/run.json
```

资源采样在 Linux 上不依赖 psutil：直接读 `/proc/<pid>/stat`、`/proc/<pid>/status`
与 `/proc/stat`（CI runner 默认没有 psutil，缺了这条 CPU / RSS 会全是 null）。

## 6. 已知局限

- runner 是共享虚拟机，单次日更数据有 ±5~10% 抖动；判定阈值（0.80 / 1.50）
  按这个抖动留了余量。要更精细的趋势，看连续几天的产物而不是单次。
- 模拟从站与网关抢同一台 4 vCPU 机器，`sys_cpu_pct` 接近 100% 时说明瓶颈
  可能在测试端——此时吞吐数字只能当**下限**看，不能当网关容量。
- 串口（RTU）路径不覆盖：runner 无串口设备，本基线只覆盖 Modbus TCP。
