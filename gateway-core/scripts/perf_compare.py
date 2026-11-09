#!/usr/bin/env python3
"""CI 性能基线对比与装配（仅标准库）。

配套 perf_baseline.py 使用：
  · perf_baseline.py 负责「跑出一份 JSON」；
  · 本脚本负责「把这次 JSON 和基线 JSON 比一遍，给出 PASS / FAIL」。

为什么需要它：perf_baseline.py 只输出数据与软告警（warnings 只打印、不改变
退出码），单次运行无法回答「这次比上次慢了多少」——性能回退只有对比基线才
看得见。CI 的夜间任务因此固定跑四档场景，逐档与本文件里的阈值比对。

判定口径（可用命令行参数覆盖）：
  1. 稳定性（绝对值）：采集失败 = 0 且协议解码错误 = 0；
  2. 吞吐回退：本次吞吐 ≥ 基线 × --throughput-ratio（默认 0.8）；
  3. 延迟回退：本次 p99 ≤ max(基线 p99 × --latency-ratio（默认 1.5）,
     --latency-floor-ms（默认 50）)——基线很小时按绝对下限判，避免自然抖动假红；
  4. 长稳泄漏：duration_min ≥ 30 的场景，RSS **尾段**拟合斜率（最后 20 min）
     ≤ --max-rss-mb-per-hour（默认 50 MB/h），且峰值 RSS ≤ 512 MB。

两种「不判失败」的情况（避免门禁在数据不具备可比性时假红）：
  · 基线文件不存在或缺少某场景 → 输出 BASELINE-MISSING，退出码 0，
    提示先跑一次 --assemble 建立基线；
  · 机型（cpu_count）与基线不同 → 该场景 SKIP，必须重建基线，
    因为 4 vCPU 与 24 核的数字本来就不在一个尺度上。

用法：
  # 对比
  python3 gateway-core/scripts/perf_compare.py \\
      --baseline docs/perf-baseline-ci.json \\
      --current s800=/tmp/s800.json --current s2000=/tmp/s2000.json
  # 装配基线（CI 里用于产出候选基线文件，由人工提交）
  python3 gateway-core/scripts/perf_compare.py \\
      --current s800=/tmp/s800.json ... --assemble /tmp/perf-baseline-ci.json
"""

import argparse
import json
import os
import sys

# 长稳判定门槛：短于该时长的场景不判 RSS 斜率——几分钟的采样里，
# 连接池 / 索引页的一次性分配还没走完，斜率必然为正，判了只会假红。
SOAK_MIN_DURATION_MIN = 30

# 长稳场景的 RSS 峰值硬上限（MB）。斜率只看趋势，峰值兜住绝对占用：
# 队列库 / 页缓存 / 缓冲池被一次性撑大时，斜率可能已经走平但内存已经过高。
RSS_PEAK_LIMIT_MB = 512.0

# p99 延迟门禁的绝对下限（ms）。基线 p99 很小时（如 s800 的 10ms），1.5× 的阈值
# 只有 15ms，而同一场景历史抖动区间是 10~50ms——纯比例阈值必然假红。实际阈值
# 取「基线 × 比例」与下限中的较大者，下限之内只记录不拦截。
LATENCY_FLOOR_MS = 50.0

PASS, FAIL, SKIP, NOTE = "PASS", "FAIL", "SKIP", "NOTE"


def load_json(path: str) -> dict:
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def fmt(v, nd: int = 2, dash: str = "-") -> str:
    if v is None:
        return dash
    if isinstance(v, float):
        return f"{v:.{nd}f}"
    return str(v)


def ratio(cur, base) -> str:
    """本次 / 基线的百分比；任一缺失时返回 '-'。"""
    if cur is None or not base:
        return "-"
    return f"{cur / base * 100.0:.0f}%"


def judge_scenario(name: str, base: dict, cur: dict, args) -> dict:
    """判定单个场景，返回 {name, rows, verdict, notes}。

    rows 是 (指标, 基线, 本次, 判据, 结论) 五元组，直接渲染成 Markdown 表行。
    """
    rows = []
    verdict = PASS

    def add(metric, b, c, rule, ok):
        nonlocal verdict
        if ok is False:
            verdict = FAIL
        rows.append((metric, b, c, rule, PASS if ok else (FAIL if ok is False else NOTE)))

    # 0) 机型可比性：cpu_count 不同 → 整档跳过（数字不在同一尺度上）。
    if base.get("cpu_count") and cur.get("cpu_count") \
            and base["cpu_count"] != cur["cpu_count"]:
        rows.append(("机型", f"{base['cpu_count']} 核", f"{cur['cpu_count']} 核",
                     "必须一致", SKIP))
        return {"name": name, "rows": rows, "verdict": SKIP}

    # 1) 稳定性：绝对值门禁，与基线无关。任何一次采集失败 / 解码错误都是缺陷。
    ok_stable = (cur.get("failures") or 0) == 0 and (cur.get("decode_errors") or 0) == 0
    add("失败 / 解码错误",
        f"{base.get('failures', 0)} / {base.get('decode_errors', 0)}",
        f"{cur.get('failures', 0)} / {cur.get('decode_errors', 0)}",
        "均为 0", ok_stable)

    # 2) 吞吐：周期封顶时吞吐 ≈ 点数 / 采集周期，反映不了总线上限，
    #    但仍按同一阈值判——真回退到封顶以下时它会先红。
    tp_base = base.get("throughput_pts_per_sec")
    tp_cur = cur.get("throughput_pts_per_sec")
    add("吞吐（点/秒）", fmt(tp_base, 1), f"{fmt(tp_cur, 1)}（{ratio(tp_cur, tp_base)}）",
        f"≥ 基线 {args.throughput_ratio * 100:.0f}%",
        None if (tp_base is None or tp_cur is None)
        else tp_cur >= tp_base * args.throughput_ratio)

    # 3) 延迟：只比 p99——p50 会被采集周期掩盖，p99 才反映尾部长事务与排队。
    #    阈值 = max(基线 × 比例, 绝对下限)：基线很小时避免被自然抖动打成假红。
    p99_base = (base.get("latency_ms") or {}).get("p99")
    p99_cur = (cur.get("latency_ms") or {}).get("p99")
    p99_limit = (None if p99_base is None
                 else max(p99_base * args.latency_ratio, args.latency_floor_ms))
    add("p99 延迟（ms）", fmt(p99_base), f"{fmt(p99_cur)}（{ratio(p99_cur, p99_base)}）",
        f"≤ {fmt(p99_limit, 1)}（基线×{args.latency_ratio:.1f}，下限 {args.latency_floor_ms:.0f}）",
        None if (p99_limit is None or p99_cur is None)
        else p99_cur <= p99_limit)

    # 4) 长稳：RSS 尾段拟合斜率 + 峰值。短场景只记录数值，不判。
    slope = cur.get("rss_mb_per_hour")
    peak = (cur.get("rss_mb") or {}).get("peak")
    dur = cur.get("duration_min") or 0
    if dur >= SOAK_MIN_DURATION_MIN:
        add("RSS 尾段斜率（MB/h）", fmt(base.get("rss_mb_per_hour"), 1), fmt(slope, 1),
            f"≤ {args.max_rss_mb_per_hour:.1f}",
            None if slope is None else slope <= args.max_rss_mb_per_hour)
        add("RSS 峰值（MB）", fmt((base.get("rss_mb") or {}).get("peak"), 1), fmt(peak, 1),
            f"≤ {RSS_PEAK_LIMIT_MB:.0f}",
            None if peak is None else peak <= RSS_PEAK_LIMIT_MB)
    else:
        add("RSS 斜率（MB/h）", fmt(base.get("rss_mb_per_hour"), 1), fmt(slope, 1),
            f"仅长稳 ≥{SOAK_MIN_DURATION_MIN}min 判定", None)

    return {"name": name, "rows": rows, "verdict": verdict}


def render(results: list[dict]) -> str:
    """渲染 Markdown 摘要：一场景一行 + 明细表。"""
    lines = ["### 性能基线对比", ""]
    if not results:
        lines.append("> 没有可对比的场景。")
        return "\n".join(lines) + "\n"

    lines.append("| 场景 | 结论 |")
    lines.append("| --- | --- |")
    icon = {PASS: "✅ PASS", FAIL: "❌ FAIL", SKIP: "⏭ SKIP", NOTE: "⚠️ NOTE"}
    for r in results:
        lines.append(f"| `{r['name']}` | {icon.get(r['verdict'], r['verdict'])} |")

    for r in results:
        lines.append("")
        lines.append(f"#### `{r['name']}`")
        lines.append("")
        lines.append("| 指标 | 基线 | 本次 | 判据 | 结论 |")
        lines.append("| --- | --- | --- | --- | --- |")
        for metric, b, c, rule, ok in r["rows"]:
            lines.append(f"| {metric} | {b} | {c} | {rule} | {icon.get(ok, ok)} |")
    return "\n".join(lines) + "\n"


def main() -> int:
    p = argparse.ArgumentParser(description="CI 性能基线对比 / 装配")
    p.add_argument("--baseline", default=None,
                   help="基线 JSON（perf_compare.py --assemble 产出的格式）")
    p.add_argument("--current", action="append", default=[], metavar="NAME=PATH",
                   help="本次结果，可重复：场景名=perf_baseline.py --json 的路径")
    p.add_argument("--assemble", default=None,
                   help="不对比，把 --current 组合成基线 JSON 写到该路径")
    p.add_argument("--throughput-ratio", type=float, default=0.80,
                   help="吞吐门禁：本次 / 基线 的下限（默认 0.80）")
    p.add_argument("--latency-ratio", type=float, default=1.50,
                   help="p99 门禁：本次 / 基线 的上限（默认 1.50）")
    p.add_argument("--latency-floor-ms", type=float, default=LATENCY_FLOOR_MS,
                   help=f"p99 门禁的绝对下限 ms，阈值取「基线×比例」与该值较大者"
                        f"（默认 {LATENCY_FLOOR_MS:.0f}）")
    p.add_argument("--max-rss-mb-per-hour", type=float, default=50.0,
                   help="长稳 RSS 尾段（最后 20 min）斜率上限 MB/h（默认 50）")
    p.add_argument("--out", default=None, help="Markdown 摘要输出路径（同时打印到 stdout）")
    args = p.parse_args()

    currents: dict[str, dict] = {}
    for item in args.current:
        if "=" not in item:
            print(f"[compare] --current 需要 NAME=PATH 形式：{item}", file=sys.stderr)
            return 2
        name, path = item.split("=", 1)
        if not os.path.exists(path):
            print(f"[compare] 缺少本次结果 {path}（场景 {name}）", file=sys.stderr)
            return 2
        currents[name] = load_json(path)

    if not currents:
        print("[compare] 没有 --current 输入", file=sys.stderr)
        return 2

    # ---- 装配模式：产出候选基线文件 ----
    if args.assemble:
        doc = {
            "_comment": "CI 性能基线：与 docs/perf-baseline.md（24 核本机数据）不可混用。"
                        "由 gateway-core/scripts/perf_compare.py --assemble 生成。",
            "scenarios": currents,
        }
        os.makedirs(os.path.dirname(args.assemble) or ".", exist_ok=True)
        with open(args.assemble, "w", encoding="utf-8") as f:
            json.dump(doc, f, indent=2, ensure_ascii=False)
        print(f"[compare] 候选基线写入 {args.assemble}（需人工提交到仓库后才生效）")
        return 0

    # ---- 对比模式 ----
    if not args.baseline or not os.path.exists(args.baseline):
        print(f"[compare] BASELINE-MISSING：找不到基线 {args.baseline}")
        print("[compare] 首次建立基线：确认本次数据合理后，用 --assemble 产出 JSON 并提交；"
              "本次不判失败。")
        return 0

    base_doc = load_json(args.baseline)
    base_scenarios = base_doc.get("scenarios", {})

    results = []
    for name, cur in currents.items():
        base = base_scenarios.get(name)
        if base is None:
            # 新增场景还没有基线：只提示，不判失败，等首次数据被采纳为基线。
            results.append({"name": name, "verdict": NOTE,
                            "rows": [("基线", "-", "-", "BASELINE-MISSING（首次运行）", NOTE)]})
            continue
        results.append(judge_scenario(name, base, cur, args))

    md = render(results)
    print(md, end="")
    if args.out:
        os.makedirs(os.path.dirname(args.out) or ".", exist_ok=True)
        with open(args.out, "w", encoding="utf-8") as f:
            f.write(md)

    failed = [r["name"] for r in results if r["verdict"] == FAIL]
    if failed:
        print(f"[compare] 回退场景：{', '.join(failed)}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
