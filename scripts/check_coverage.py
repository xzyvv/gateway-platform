#!/usr/bin/env python3
"""后端覆盖率"防回落地板"：按目录聚合 lcov，低于阈值即失败（CI 门禁）。

对标前端 `gateway-web/vitest.config.ts` 的分目录 thresholds：
- 同样按目录分桶，而不是只看总数（总数被个别高覆盖目录拉平，掩盖回退）；
- 同样"下取整、留约 3pp 波动余量"定档，避免为追绿而反复改阈值；
- 同样"补测试时只许上调、不许下调"——下调阈值会吃掉已补的测试价值。

与前端的差异：后端 lcov **没有分支数据**（`cargo llvm-cov` 产出的 lcov 里
`BRF` 恒为 0，clang 的分支映射未启用），因此后端只对 lines / functions 设阈值，
不设 branches 阈值，避免写出永远为 0 的假门槛。

用法::
    python3 scripts/check_coverage.py lcov.info          # 校验，不达标退出码 1
    python3 scripts/check_coverage.py lcov.info --print   # 只打印实测值，不判定

退出码：0 达标；1 有目录低于阈值；2 输入缺失或格式异常。
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

# 阈值来源：CI run 41778224255（sha 398317f）的 backend-coverage-report artifact，
# 按"下取整、留约 3pp 余量"定档。口径 = 目录内 covered/total 聚合。
#
# 修改规则：补测试后允许上调；**不许下调**（下调等于把已补的测试价值退回去）。
THRESHOLDS: dict[str, dict[str, int]] = {
    "total": {"lines": 66, "functions": 66},
    "core": {"lines": 69, "functions": 70},
    "core/config": {"lines": 80, "functions": 75},
    "core/crypto": {"lines": 81, "functions": 72},
    "core/engine": {"lines": 80, "functions": 81},
    "core/license": {"lines": 88, "functions": 85},
    "core/north": {"lines": 62, "functions": 63},
    "core/pipeline": {"lines": 82, "functions": 89},
    "core/platform": {"lines": 53, "functions": 61},
    "core/queue": {"lines": 73, "functions": 74},
    "core/south": {"lines": 60, "functions": 63},
    "core/support": {"lines": 63, "functions": 63},
    "api/web": {"lines": 59, "functions": 56},
}


class Counter:
    """一个桶的累计计数。"""

    __slots__ = ("lf", "lh", "fnf", "fnh")

    def __init__(self) -> None:
        self.lf = 0
        self.lh = 0
        self.fnf = 0
        self.fnh = 0

    def add(self, lf: int, lh: int, fnf: int, fnh: int) -> None:
        self.lf += lf
        self.lh += lh
        self.fnf += fnf
        self.fnh += fnh

    @property
    def lines(self) -> float:
        return 100.0 * self.lh / self.lf if self.lf else 0.0

    @property
    def functions(self) -> float:
        return 100.0 * self.fnh / self.fnf if self.fnf else 0.0


def bucket_of(path: str) -> str | None:
    """把 lcov 的绝对路径映射到桶名；不属于本仓的路径返回 None。

    Windows 上 cargo-llvm-cov 写出的 SF 是反斜杠路径（`C:\\...\\gateway-core\\src\\...`），
    必须先归一成 `/` 才能命中下面的目录标记，否则所有文件都会被判成「不属于本仓」，
    结果就是全 0%——这不是覆盖率真的掉了，而是分桶失效。
    """
    path = path.replace("\\", "/")
    for marker, prefix in (("/gateway-core/src/", "core/"), ("/gateway-api/src/", "api/")):
        idx = path.find(marker)
        if idx == -1:
            continue
        rest = path[idx + len(marker) :]
        if prefix == "core/":
            # core 顶层文件（lib.rs 等）归 core/root，其余按首层模块分桶
            head = rest.split("/", 1)[0]
            return "core/root" if head.endswith(".rs") else f"core/{head}"
        # api：src/web/** 归 api/web，其余归 api/root
        head = rest.split("/", 1)[0]
        return "api/web" if head == "web" else "api/root"
    return None


def parse_lcov(path: Path) -> dict[str, Counter]:
    """解析 lcov.info，返回 {桶名: Counter}。"""
    buckets: dict[str, Counter] = {}
    current: str | None = None
    lf = lh = fnf = fnh = 0

    def flush() -> None:
        nonlocal lf, lh, fnf, fnh
        if current is None:
            return
        buckets.setdefault(current, Counter()).add(lf, lh, fnf, fnh)
        lf = lh = fnf = fnh = 0

    with path.open("r", encoding="utf-8", errors="replace") as fh:
        for raw in fh:
            line = raw.strip()
            if line.startswith("SF:"):
                flush()
                current = bucket_of(line[3:].strip())
            elif line.startswith("LF:"):
                lf = int(line[3:].strip() or 0)
            elif line.startswith("LH:"):
                lh = int(line[3:].strip() or 0)
            elif line.startswith("FNF:"):
                fnf = int(line[4:].strip() or 0)
            elif line.startswith("FNH:"):
                fnh = int(line[4:].strip() or 0)
    flush()
    return buckets


def collect(buckets: dict[str, Counter]) -> dict[str, Counter]:
    """在目录桶之外补出 core / api / total 三个聚合桶。"""
    out = dict(buckets)
    out["core"] = Counter()
    out["api"] = Counter()
    for name, ctr in buckets.items():
        if name.startswith("core/"):
            out["core"].add(ctr.lf, ctr.lh, ctr.fnf, ctr.fnh)
        elif name.startswith("api/"):
            out["api"].add(ctr.lf, ctr.lh, ctr.fnf, ctr.fnh)
    total = Counter()
    for name, ctr in out.items():
        if name in ("core", "api", "total"):
            continue
        total.add(ctr.lf, ctr.lh, ctr.fnf, ctr.fnh)
    out["total"] = total
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description="后端覆盖率防回落地板")
    ap.add_argument("lcov", help="lcov.info 路径")
    ap.add_argument("--print", dest="print_only", action="store_true", help="只打印实测值，不做判定")
    args = ap.parse_args()

    lcov_path = Path(args.lcov)
    if not lcov_path.is_file():
        print(f"[coverage] ✗ 找不到 lcov 文件：{lcov_path}", file=sys.stderr)
        return 2

    try:
        buckets = collect(parse_lcov(lcov_path))
    except (ValueError, OSError) as exc:
        print(f"[coverage] ✗ lcov 解析失败：{exc}", file=sys.stderr)
        return 2

    if not buckets:
        print("[coverage] ✗ lcov 中没有任何属于本仓的源文件记录", file=sys.stderr)
        return 2

    order = ["total", "core", "api"] + sorted(
        name for name in buckets if name not in ("total", "core", "api")
    )
    print("[coverage] 目录覆盖率（threshold = 防回落地板，actual = 本次实测）")
    failures: list[str] = []
    for name in order:
        ctr = buckets.get(name)
        if ctr is None:
            continue
        thr = THRESHOLDS.get(name)
        line_parts = [f"{name:<16}"]
        for metric, value in (("lines", ctr.lines), ("functions", ctr.functions)):
            if thr is None:
                line_parts.append(f"{metric}={value:6.2f}% (无阈值)")
                continue
            limit = thr[metric]
            mark = "OK " if value >= limit else "FAIL"
            line_parts.append(f"{metric}={value:6.2f}% (阈值 {limit}%) [{mark}]")
            if value < limit:
                failures.append(f"{name}.{metric}: {value:.2f}% < 阈值 {limit}%")
        print("  " + "  ".join(line_parts))

    if args.print_only:
        return 0

    if failures:
        print("[coverage] ✗ 覆盖率低于防回落地板：", file=sys.stderr)
        for item in failures:
            print(f"  - {item}", file=sys.stderr)
        print(
            "[coverage] 补测试可上调阈值；下调阈值等于退回已补的测试价值，不允许。",
            file=sys.stderr,
        )
        return 1

    print("[coverage] ✓ 全部目录达标")
    return 0


if __name__ == "__main__":
    sys.exit(main())
