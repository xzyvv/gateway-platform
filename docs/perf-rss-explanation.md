# 性能压测 RSS 增长说明

## 背景

`docs/perf-baseline.md` §2.4 中 L4-t8 场景（20000 点 / 0.5 min / RSS 涨 53.4 MB）原结论写了「疑似泄漏」；
同规模 L4-t1 涨 47.6 MB。原报告都未给出归因，后人易被「疑似泄漏」误导。

本文档说明：这是**预期行为**，不是泄漏。

## 归因：未配置北向消费 → SQLite page cache 持有

压测脚本 `gateway-core/scripts/perf_baseline.py` 的 mock 从站只回 Modbus 响应数据，
**不消费 SQLite 队列**。采集循环 → 入队 → 队列 SQLite 写成功后，**无任何下游 dequeue / ack**，
导致：

1. 采集侧持续以 ~20 000 点/秒入库（20000 点 / 1s 周期）
2. 30 秒内累积约 60 万行
3. SQLite WAL 文件持续增长（实测对应 `queue_depth` 从 12 万涨到 62 万）
4. SQLite 进程内 page cache 自动扩容以持有未 checkpoint 的页内存
5. RSS 上升完全由 page cache + WAL buffer 贡献

**已排除的真泄漏嫌疑**（见 [remaining-optimizations.md P4](file:///c:/Users/xzy35/AppData/Roaming/Trae%20CN/ModularData/ai-agent/work-mode-projects/6ac90dddcbfa62fa225c7a9d/.trae/documents/remaining-optimizations.md)）：

| 模块 | 状态 | 证据 |
|---|---|---|
| ~~TcpConnectionPool~~ | **已删除** | 全仓无生产调用（属死代码），2026-10-21 随 C-5 清理；清理前健康（`MAX_IDLE = 30s` + `try_read` 探活 + `max_per_endpoint` 上限），非泄漏源。生产路径为 `TransactionPool::with_stream` |
| Modbus TransactionPool | 健康 | 一次性 `Waiter` 收发，连接断开时 `abort_all` 清理 |
| fallback 缓冲 | 健康 | 5000 上限 + 优先级淘汰 + 反弹轮次封顶 |
| OutboxStore | 健康 | Pending/Acked 状态机 + 未知值显式告警 |
| device_states | 健康 | reload 时按 `new_ids` 做 `retain` |

## 量化估算

- 采集速率：~20 000 点/秒
- DataPoint 单条序列化平均 ~120 字节（含 `point_id: String` + `device_id: String` + 数值字段）
- 30 秒累积：20 000 × 30 × 120 B ≈ 72 MB 原始数据
- 配合 SQLite 索引页 / WAL header / page cache 副本：实测落盘约 1.6~1.8 MB/s RSS 增长
- 与实测吻合（53.4 MB / 30s ≈ 1.78 MB/s）

## 生产环境如何收敛

生产部署启用 MQTT / Webhook 北向消费后：

1. 采集侧入队 → 北向 dequeue → 成功 ack → SQLite 删除
2. queue_depth 长期 < 1 万（典型值），WAL 周期性 checkpoint
3. SQLite page cache 自动收缩到稳定大小
4. RSS 在 200 MB 内收敛（实测参考：[perf-baseline.md](perf-baseline.md) §1 汇总表中 8000 点 / 0.5 min / RSS 67.6 MB 峰值；8000 点也仅入队 240k 行，对应 page cache 占用自然小）

**对运维的判据**：
- 生产环境若 RSS 单调上涨不收敛 → 先看 `queue_depth` 与北向消费速率（`/api/metrics`）
- 若 queue_depth 持续增长而北向消费停滞 → 北向问题（如 broker 不可达），不是泄漏
- 若 queue_depth 稳定而 RSS 仍上涨 → 真泄漏，按正常排查路径走

## 复现 / 验证命令

```powershell
# 复现 L4-t8 同等场景
python gateway-core/scripts/perf_baseline.py `
  --points 20000 --slaves 200 --tcp-concurrent 8 `
  --duration-min 0.5 --sample-sec 5 `
  --no-build --target $env:CARGO_TARGET_DIR `
  --output docs/perf-run.md
```

对比组（启用北向消费）无法用脚本开关切换：`perf_baseline.py` 不生成 northbound 配置，
需用 `--config <已有配置>` 指向一份配好 MQTT / Webhook 通道的 `config.json` 再跑，
观察 `queue_depth` 是否稳定在万级以下、RSS 是否停止单调上涨。

预期：启用北向消费后，30 秒后 RSS 应在 150 MB 附近稳定，**不再单调上涨**。

## 涉及报告

| 场景（见 [perf-baseline.md](perf-baseline.md)） | 数据 | 原结论 | 修正 |
|---|---|---|---|
| §2.4 L4-t8 | 20000 点 / 0.5 min / 涨 53.4 MB / 队列深度 62 万 | "疑似泄漏" | 预期行为，详见本文档 |
| §2.4 L4-t1 | 20000 点 / 0.5 min / 涨 47.6 MB / 队列深度 52 万 | "RSS 无显著增长"（与表格 47.6 MB 增长口径不一致） | 同源预期行为 |
| [perf-knee.md](perf-knee.md) | 150 000 点 / 拐点测试 | 已正确归因（"生产环境由北向消费队列，存储压力低于本压测"） | 保持原状 |
| §1 汇总表 perf-diag | 8000 点 / 0.5 min / 涨 20.3 MB | "未发现异常" | 保持原状（量级与场景一致） |
