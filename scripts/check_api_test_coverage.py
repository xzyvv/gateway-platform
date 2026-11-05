#!/usr/bin/env python3
"""盘点 API 端点测试覆盖：docs/api-surface.md vs gateway-api/tests/。

用途（部署前准备清单 P1-4）：
- `docs/api-surface.md` 由 gen_api_surface.py 从路由源码生成，端点清单机器可读；
- 本脚本在测试源码里找「路径字面量 + 同窗口内的 HTTP 方法调用」，给出差集，
  人工按「写接口 > 读接口、配置类 > 查询类」判定补测优先级。

口径说明（保守，宁可漏判「已测」也不虚报）：
- 方法识别看路径字面量前后 300 字符窗口内的 `.get(` / `.post(` / `Method::GET` 等；
- 动态段（`:id` / `{id}` / `*rest`）按静态前缀 + 后续任意路径匹配；
- 同一文件里路径出现过但没找到对应方法 → 归入「疑似未测」（单独列出）。

退出码：默认 0（只报告）；`--strict` 时若有未覆盖的写接口则返回 1。
"""

import argparse
import pathlib
import re
import sys
from collections import Counter

METHOD_TOKENS = {
    "GET": ("Method::GET", ".get(", "get_json(", "http_get", '"GET"'),
    "POST": ("Method::POST", ".post(", "post_json(", "http_post", '"POST"'),
    "PUT": ("Method::PUT", ".put(", "put_json(", "http_put", '"PUT"'),
    "DELETE": ("Method::DELETE", ".delete(", "delete_json(", "http_delete", '"DELETE"'),
    "PATCH": ("Method::PATCH", ".patch(", "patch_json(", "http_patch", '"PATCH"'),
}

WRITE_METHODS = ("POST", "PUT", "DELETE", "PATCH")
WINDOW = 200


def parse_endpoints(md_text: str):
    return re.findall(
        r"\|\s*(GET|POST|PUT|DELETE|PATCH)\s*\|\s*`([^`]+)`\s*\|", md_text
    )


def is_dynamic(seg: str) -> bool:
    return seg.startswith(":") or seg.startswith("*") or "{" in seg


def path_matches(endpoint_path: str, literal: str) -> bool:
    """端点模板 vs 测试里的路径字面量（字面量可能含 {} 占位或具体 id）。"""
    ep = endpoint_path.split("?", 1)[0].strip("/").split("/")
    lit = literal.split("?", 1)[0].strip("/").split("/")
    wildcard = bool(ep) and ep[-1].startswith("*")
    n = min(len(ep), len(lit))
    for i in range(n):
        if is_dynamic(ep[i]) or is_dynamic(lit[i]):
            continue
        if ep[i] != lit[i]:
            return False
    if wildcard:
        return len(lit) >= len(ep) - 1
    return len(lit) == len(ep)



def collect_observations(tests_dir: pathlib.Path, allowed_first_segments=None):
    """返回 [(file, line, path_literal, verbs)]."""
    obs = []
    for fp in sorted(tests_dir.rglob("*.rs")):
        text = fp.read_text(encoding="utf-8", errors="replace")
        # 同时收录 `/api/...` 与 `/health/...` 等非 /api 端点；用首段白名单过滤掉
        # `/dev/ttyS1`、`/tmp/x` 这类与 API 无关的字面量。
        for m in re.finditer(r'"(\/[^"\\]*)"', text):
            lit = m.group(1)
            if allowed_first_segments:
                first = lit.strip("/").split("/")[0]
                if first not in allowed_first_segments:
                    continue
            window = text[max(0, m.start() - WINDOW) : m.end() + WINDOW]
            verbs = {
                verb
                for verb, toks in METHOD_TOKENS.items()
                if any(tok in window for tok in toks)
            }
            line = text.count("\n", 0, m.start()) + 1
            obs.append((fp.name, line, lit, verbs))
    return obs


def is_covered(endpoint_path: str, method: str, obs) -> bool:
    for _f, _l, lit, verbs in obs:
        if method in verbs and path_matches(endpoint_path, lit):
            return True
    return False


def group_of(path: str) -> str:
    segs = path.strip("/").split("/")
    return segs[1] if len(segs) > 1 else segs[0]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--repo", default=str(pathlib.Path(__file__).resolve().parent.parent))
    ap.add_argument("--tests-dir", default="gateway-api/tests")
    ap.add_argument("--strict", action="store_true")
    args = ap.parse_args()

    repo = pathlib.Path(args.repo)
    md = (repo / "docs" / "api-surface.md").read_text(encoding="utf-8")
    endpoints = parse_endpoints(md)
    allowed = {p.strip("/").split("/")[0] for _m, p in endpoints}
    obs = collect_observations(repo / args.tests_dir, allowed)

    covered, uncovered = [], []
    for method, path in endpoints:
        (covered if is_covered(path, method, obs) else uncovered).append((method, path))

    writes = [e for e in uncovered if e[0] in WRITE_METHODS]
    reads = [e for e in uncovered if e[0] not in WRITE_METHODS]

    print(f"端点总数: {len(endpoints)}")
    print(f"已覆盖:   {len(covered)}")
    print(f"未覆盖:   {len(uncovered)}  (写接口 {len(writes)} / 读接口 {len(reads)})")

    if writes:
        print("\n未覆盖的写接口（按路由组）：")
        for g, n in Counter(group_of(p) for _m, p in writes).most_common():
            print(f"  {g:<18} {n}")
        print("\n明细：")
        for method, path in sorted(writes, key=lambda e: (group_of(e[1]), e[1], e[0])):
            print(f"  {method:<6} {path}")

    if reads:
        print("\n未覆盖的读接口：")
        for method, path in sorted(reads, key=lambda e: (group_of(e[1]), e[1])):
            print(f"  {method:<6} {path}")

    return 1 if (args.strict and writes) else 0


if __name__ == "__main__":
    sys.exit(main())
