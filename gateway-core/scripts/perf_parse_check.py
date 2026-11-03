"""验证 /api/metrics 指标解析在「按 by_reason 标签化」之后仍然正确。

背景：`gateway_collect_failures` / `gateway_bus_transaction_failures_total` 已从
「1 条无标签序列」改为「N 条带标签序列」，`perf_baseline.py:parse_metrics` 的解析
逻辑随之改为「按指标名求和」。本脚本用于**独立验证**该解析：

1. 用生产规模（默认 8000 点）生成合成 payload（可离线、有精确真值）；
2. 或读取一次真实压测保存的抓取文本（`--file`）；
3. 或直接抓运行中 gatewayd 的指标（`--live`，压测进行中使用）。

校验维度：
  A 结构  by_reason 10 个标签值全部存在（序列集合恒定，无抖动）、
          scoped 序列数 ≤ POINT_EXPORT_LIMIT、points_failing ≥ 导出序列数、
          直方图桶单调不减且 +Inf == _count
  B 一致  parse_metrics 与「逐字符独立扫描」实现结果完全一致（防正则回归）
  C 真值  仅合成数据：失败数 / scoped 失败数 / 直方图计数与注入值精确相等
  D 性能  --repeat 次重复解析，输出中位与最慢耗时、行/秒吞吐

用法：
  python gateway-core/scripts/perf_parse_check.py
  python gateway-core/scripts/perf_parse_check.py --points 8000 --repeat 10
  python gateway-core/scripts/perf_parse_check.py --file scrape.txt
  python gateway-core/scripts/perf_parse_check.py --live http://127.0.0.1:18080/api/metrics
"""

from __future__ import annotations  # 兼容 Python 3.9

import argparse
import os
import random
import statistics
import sys
import time
import urllib.error
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from perf_baseline import histogram_quantile, parse_metrics  # noqa: E402

# ---- 与被测端保持一致的常量（漂移时本脚本应立即失败）-------------------------
# 见 gateway-core/src/engine/metrics/scoped.rs: REASON_LABELS / POINT_EXPORT_LIMIT
REASON_LABELS = [
    "timeout",
    "no_reply",
    "io",
    "crc",
    "decode",
    "bus_closed",
    "busy",
    "protocol_exception",
    "config",
    "other",
]
POINT_EXPORT_LIMIT = 500
# 见 gateway-core/src/engine/metrics/mod.rs: COLLECT_DURATION_BUCKETS（微秒）
COLLECT_BUCKETS_US = [100, 500, 1_000, 5_000, 10_000, 50_000, 100_000, 500_000, 1_000_000]


# ============================================================================
# 数据来源
# ============================================================================
def fetch_live(url: str, token: str) -> str:
    req = urllib.request.Request(url, headers={"Authorization": f"Bearer {token}"})
    with urllib.request.urlopen(req, timeout=10) as resp:
        return resp.read().decode("utf-8", "replace")


def build_synthetic(points: int, failing_points: int, rounds: int, seed: int = 20261101):
    """生成与 gatewayd 导出格式一致的 payload，并返回注入的真值。

    真值 = 生成时写入的各项计数，用于与解析结果做精确比对（不是自证）。
    """
    rng = random.Random(seed)
    attempts = points * rounds
    # 10 条原因序列：注入已知分布（含 0 值序列，验证「未发生也导出」）
    reason_counts = {label: 0 for label in REASON_LABELS}
    reason_counts["timeout"] = rounds * 3
    reason_counts["no_reply"] = rounds * 2
    reason_counts["io"] = rounds
    reason_counts["protocol_exception"] = rounds * 4
    failures_total = sum(reason_counts.values())

    # scoped：真实失败点数可能超过导出口径上限，故 points_failing 是独立计数
    exported = min(failing_points, POINT_EXPORT_LIMIT)
    scoped_counts = {}
    for i in range(exported):
        # 点位名故意包含空格 / 逗号 / 等号 / 引号，验证标签值转义后仍可被还原计数
        if i == 0:
            name = 'slave1/pt "A",addr=40001'
        elif i == 1:
            name = "slave2/pt 7"
        else:
            name = f"slave{(i // 97) + 1}/pt{i}"
        scoped_counts[name] = 1 + (i % 7)
    scoped_sum = sum(scoped_counts.values())

    # 直方图：往各桶灌数，保证累积单调
    per_bucket = [0] * (len(COLLECT_BUCKETS_US) + 1)  # 末位为 +Inf
    remaining = attempts
    for idx in range(len(per_bucket)):
        take = min(remaining, rng.randint(0, max(1, attempts // 4)))
        per_bucket[idx] = take
        remaining -= take
    per_bucket[-1] += remaining

    def esc(v: str) -> str:
        return v.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n")

    lines = [
        "# HELP gateway_collect_total Collection attempts",
        "# TYPE gateway_collect_total counter",
        f"gateway_collect_total {attempts}",
        "# HELP gateway_collect_failures Collection failures by reason",
        "# TYPE gateway_collect_failures counter",
    ]
    for label in REASON_LABELS:
        lines.append(f'gateway_collect_failures{{by_reason="{label}"}} {reason_counts[label]}')
    lines += [
        "# HELP gateway_bus_transaction_failures_total Total failed bus transactions by reason",
        "# TYPE gateway_bus_transaction_failures_total counter",
    ]
    for label in REASON_LABELS:
        lines.append(f'gateway_bus_transaction_failures_total{{by_reason="{label}"}} {reason_counts[label]}')
    lines += [
        "# HELP gateway_protocol_decode_errors_total Protocol decode errors",
        "# TYPE gateway_protocol_decode_errors_total counter",
        f"gateway_protocol_decode_errors_total {rounds}",
        "# HELP gateway_queue_depth Current data queue depth",
        "# TYPE gateway_queue_depth gauge",
        "gateway_queue_depth 17",
        "# HELP gateway_collect_duration_us Collection duration histogram in microseconds",
        "# TYPE gateway_collect_duration_us histogram",
    ]
    cum = 0
    for bound, count in zip(COLLECT_BUCKETS_US + [None], per_bucket):
        cum += count
        le = "+Inf" if bound is None else str(bound)
        lines.append(f'gateway_collect_duration_us_bucket{{le="{le}"}} {cum}')
    lines.append(f"gateway_collect_duration_us_sum {cum * 700}")
    lines.append(f"gateway_collect_duration_us_count {cum}")
    lines += [
        "# HELP gateway_collect_failures_scoped Collection failures per point",
        "# TYPE gateway_collect_failures_scoped gauge",
    ]
    for name, count in scoped_counts.items():
        lines.append(f'gateway_collect_failures_scoped{{point="{esc(name)}"}} {count}')
    lines += [
        "# HELP gateway_collect_scoped_points_failing Number of points with collection failures",
        "# TYPE gateway_collect_scoped_points_failing gauge",
        f"gateway_collect_scoped_points_failing {failing_points}",
        "",
    ]

    truth = {
        "attempts": attempts,
        "failures_total": failures_total,
        "scoped_sum": scoped_sum,
        "scoped_exported": exported,
        "points_failing": failing_points,
        "hist_count": cum,
    }
    return "\n".join(lines), truth


# ============================================================================
# 独立解析：逐字符扫描（不复用正则，用于与 parse_metrics 交叉校验）
# ============================================================================
def strict_scan(text: str) -> tuple[dict, dict]:
    totals: dict[str, float] = {}
    buckets: dict[str, float] = {}
    for line in text.splitlines():
        if not line or line.startswith("#"):
            continue
        i, n = 0, len(line)
        while i < n and line[i] not in "{ \t":
            i += 1
        name = line[:i]
        labels = ""
        if i < n and line[i] == "{":
            start, in_quote, escaped = i, False, False
            while i < n:
                ch = line[i]
                if escaped:
                    escaped = False
                elif ch == "\\":
                    escaped = True
                elif ch == '"':
                    in_quote = not in_quote
                elif ch == "}" and not in_quote:
                    i += 1
                    break
                i += 1
            labels = line[start:i]
        rest = line[i:].strip()
        if not rest:
            continue
        try:
            value = float(rest.split()[0])
        except ValueError:
            continue
        if name == "gateway_collect_duration_us_bucket":
            marker = 'le="'
            if marker in labels:
                buckets[labels.split(marker, 1)[1].split('"', 1)[0]] = value
            continue
        totals[name] = totals.get(name, 0.0) + value
    return totals, buckets


# ============================================================================
# 校验
# ============================================================================
def check(text: str, truth: dict | None) -> list[tuple[str, bool, str]]:
    rows: list[tuple[str, bool, str]] = []

    def add(name: str, ok: bool, detail: str) -> None:
        rows.append((name, ok, detail))

    gauges, buckets = parse_metrics(text)
    scanned, scanned_buckets = strict_scan(text)

    # B 一致性：两条独立实现必须得出同一结果
    diff = {k for k in set(gauges) | set(scanned) if gauges.get(k) != scanned.get(k)}
    add("B 解析实现一致", not diff, "无差异" if not diff else f"差异键 {sorted(diff)[:5]}")
    diff_b = {k for k in set(buckets) | set(scanned_buckets) if buckets.get(k) != scanned_buckets.get(k)}
    add("B 直方图一致", not diff_b, "无差异" if not diff_b else f"差异桶 {sorted(diff_b)}")

    # A 结构：by_reason 序列集合恒定（10 个标签值都有样本行）
    present = set()
    for line in text.splitlines():
        if line.startswith('gateway_collect_failures{by_reason="'):
            present.add(line.split('by_reason="', 1)[1].split('"', 1)[0])
    missing = [label for label in REASON_LABELS if label not in present]
    unknown = sorted(present - set(REASON_LABELS))
    add(
        "A by_reason 标签集合",
        not missing and not unknown,
        f"{len(present)}/10 条" + (f"，缺 {missing}" if missing else "") + (f"，多 {unknown}" if unknown else ""),
    )

    # A 结构：scoped 导出序列数不得超过上限
    scoped_rows = [ln for ln in text.splitlines() if ln.startswith("gateway_collect_failures_scoped{")]
    failing = gauges.get("gateway_collect_scoped_points_failing")
    ok_limit = len(scoped_rows) <= POINT_EXPORT_LIMIT
    add("A scoped 序列上限", ok_limit, f"{len(scoped_rows)} 条（上限 {POINT_EXPORT_LIMIT}）")
    ok_failing = failing is not None and failing >= len(scoped_rows)
    add(
        "A 失败点计数自洽",
        ok_failing,
        f"points_failing={failing}, 导出序列 {len(scoped_rows)}",
    )

    # A 结构：直方图桶累积单调、+Inf == _count
    ordered = []
    for le, cum in buckets.items():
        ordered.append((float("inf") if le == "+Inf" else float(le), cum))
    ordered.sort()
    monotonic = all(ordered[i][1] <= ordered[i + 1][1] for i in range(len(ordered) - 1))
    count = gauges.get("gateway_collect_duration_us_count")
    ok_inf = bool(ordered) and count is not None and ordered[-1][1] == count
    add("A 桶单调不减", monotonic, f"{len(ordered)} 个桶")
    add("A +Inf == _count", ok_inf, f"+Inf={ordered[-1][1] if ordered else None}, _count={count}")

    # 分位可算且落在桶范围内（口径冒烟，不比对具体值）
    p50 = histogram_quantile(0.50, buckets)
    p99 = histogram_quantile(0.99, buckets)
    upper = ordered[-1][0] if ordered else None
    ok_q = (
        p50 is not None
        and p99 is not None
        and p50 <= p99
        and (upper is None or p99 <= upper)
    )
    add("A 分位口径", ok_q, f"p50={p50}, p99={p99}")

    # C 真值（仅合成数据）
    if truth:
        total = gauges.get("gateway_collect_failures")
        add("C 失败数求和", total == truth["failures_total"], f"解析 {total} vs 注入 {truth['failures_total']}")
        scoped_sum = gauges.get("gateway_collect_failures_scoped")
        add("C scoped 求和", scoped_sum == truth["scoped_sum"], f"解析 {scoped_sum} vs 注入 {truth['scoped_sum']}")
        add(
            "C 失败点总数",
            failing == truth["points_failing"],
            f"解析 {failing} vs 注入 {truth['points_failing']}",
        )
        add(
            "C 采集总次数",
            gauges.get("gateway_collect_total") == truth["attempts"],
            f"解析 {gauges.get('gateway_collect_total')} vs 注入 {truth['attempts']}",
        )
        add("C 直方图计数", count == truth["hist_count"], f"解析 {count} vs 注入 {truth['hist_count']}")

    # 未标签指标必须仍然可读（回归：不能被标签化改动波及）
    for name in (
        "gateway_collect_total",
        "gateway_protocol_decode_errors_total",
        "gateway_queue_depth",
        "gateway_collect_duration_us_count",
    ):
        short = name.replace("gateway_", "")
        add(f"C 未标签可读 {short}", gauges.get(name) is not None, f"{gauges.get(name)}")

    return rows


def bench(text: str, repeat: int) -> tuple[float, float, int]:
    costs = []
    for _ in range(repeat):
        t0 = time.perf_counter()
        parse_metrics(text)
        costs.append(time.perf_counter() - t0)
    return statistics.median(costs), max(costs), len(text.splitlines())


# ============================================================================
# 入口
# ============================================================================
def main() -> int:
    p = argparse.ArgumentParser(description="验证 gateway 指标解析（标签化后）")
    p.add_argument("--points", type=int, default=8000, help="合成数据的点位总数（默认生产规模 8000）")
    p.add_argument("--failing-points", type=int, default=1200, help="合成数据中失败的点位数（默认 1200）")
    p.add_argument("--rounds", type=int, default=6, help="合成数据中的采集轮数")
    p.add_argument("--repeat", type=int, default=5, help="解析重复次数（性能取样）")
    p.add_argument("--file", default=None, help="改为校验已保存的抓取文本")
    p.add_argument("--dump", default=None, help="把合成 payload 另存为文件（供 --file 复现 / 共享现场）")
    p.add_argument("--live", default=None, help="改为抓取运行中的 /api/metrics URL")
    p.add_argument("--token", default="perf-baseline-token-00000001", help="--live 时的 Bearer Token")
    args = p.parse_args()

    truth = None
    if args.live:
        try:
            text = fetch_live(args.live, args.token)
        except (urllib.error.URLError, OSError) as exc:
            print(f"[parse-check] 抓取失败：{exc}", file=sys.stderr)
            return 2
        source = f"live {args.live}"
    elif args.file:
        with open(args.file, encoding="utf-8", errors="replace") as fh:
            text = fh.read()
        source = f"file {args.file}"
    else:
        text, truth = build_synthetic(args.points, args.failing_points, args.rounds)
        source = f"synthetic points={args.points} failing={args.failing_points} rounds={args.rounds}"
        if args.dump:
            with open(args.dump, "w", encoding="utf-8") as fh:
                fh.write(text)
            print(f"[parse-check] 合成 payload 已写入 {args.dump}")

    rows = check(text, truth)
    failed = [r for r in rows if not r[1]]

    median, worst, lines = bench(text, max(1, args.repeat))

    print(f"[parse-check] 数据来源：{source}（{lines} 行，{len(text)} 字节）")
    print(f"[parse-check] 解析耗时：中位 {median * 1000:.2f} ms / 最慢 {worst * 1000:.2f} ms"
          f"（{args.repeat} 次，{lines / max(median, 1e-9):.0f} 行/秒）")
    print()
    for name, ok, detail in rows:
        print(f"  {'PASS' if ok else 'FAIL'}  {name:<28} {detail}")
    print()
    if failed:
        print(f"[parse-check] {len(failed)}/{len(rows)} 项失败")
        return 1
    print(f"[parse-check] 全部 {len(rows)} 项通过")
    return 0


if __name__ == "__main__":
    sys.exit(main())
