#!/usr/bin/env python3
"""生成 / 校验 docs/api-surface.md（W2-#19：API 端点清单，防路由与文档漂移）。

唯一事实来源是 Rust 路由源码，本脚本只做文本解析，不依赖编译器：

- ``gateway-api/src/web/mod.rs``            —— health 探针路由
- ``gateway-api/src/web/api/<子域>/routes.rs`` —— 各子域路由组

用法::

    python3 scripts/gen_api_surface.py           # 重新生成 docs/api-surface.md
    python3 scripts/gen_api_surface.py --check   # 只校验；不一致退出码 1（CI 用）

解析策略：以 ``Router::new()`` 为切分点把文件分为若干「路由段」，每段内提取
``.route("<path>", <method router>)`` 与紧随其后的
``.route_layer(from_fn_with_state(..., <鉴权中间件>))``。
新增/删除路由而未重新生成本文件时，CI 会因输出差异而失败。

退出码：0 成功/一致；1 校验不一致（或解析异常）；2 文件缺失。
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
WEB_SRC = REPO_ROOT / "gateway-api" / "src" / "web"
OUT_FILE = REPO_ROOT / "docs" / "api-surface.md"

ROUTE_RE = re.compile(r"\.route\s*\(")
ROUTER_NEW_RE = re.compile(r"Router::new\s*\(\s*\)")
# `get(handler)` / `put(a).delete(b)` 等；handler 允许 `super::handler` 形式
METHOD_RE = re.compile(
    r"\b(get|post|put|patch|delete|head|options|any)\s*\(\s*([A-Za-z0-9_:]+)\s*\)"
)
AUTH_MW_RE = re.compile(
    r"from_fn_with_state\s*\(\s*state(?:\.\s*clone\s*\(\s*\))?\s*,\s*([A-Za-z0-9_:]+)"
)

AUTH_LABEL = {
    "require_auth": "token",
    "require_auth_if_anon_disabled": "token（`allow_anon_read=true` 时可匿名）",
    "require_admin_or_exec_scope": "admin / exec scope",
}

GROUP_LABELS = {
    "health": "健康检查（探针，内联于 `web/mod.rs`）",
    "auth": "认证与会话",
    "system": "系统与配置",
    "audit": "审计日志",
    "history": "历史数据",
    "points": "点位值读写",
    "realtime": "实时数据 / WebSocket",
    "license": "授权",
    "devices": "IO 控制与设备模板",
    "config": "设备 / 点位 CRUD 与配置导入导出",
    "uplink": "上行配置与上送测试",
    "users": "用户管理",
    "device_ops": "设备运维",
    "points_ops": "点位运维",
    "diagnostics": "诊断",
    "access": "访问控制（密钥 / 限流 / 计划任务）",
    "protocol_tools": "协议工具",
    "quality": "数据质量规则",
    "sim_consumer": "内置模拟消费者",
}


def _balanced_end(text: str, open_idx: int) -> int:
    """返回与 ``text[open_idx] == '('`` 配对的右括号下标（不匹配则抛 ValueError）。

    跳过字符串字面量与转义，足以应对路由宏/闭包里的括号。
    """
    depth = 0
    i = open_idx
    in_str = False
    while i < len(text):
        ch = text[i]
        if in_str:
            if ch == "\\":
                i += 2
                continue
            if ch == '"':
                in_str = False
        elif ch == '"':
            in_str = True
        elif ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
            if depth == 0:
                return i
        i += 1
    raise ValueError("括号不配平")


def _strip_test_modules(text: str) -> str:
    """去掉文件末尾的 ``#[cfg(test)]`` 测试模块（测试里的 Router 不是线上路由）。"""
    idx = text.find("#[cfg(test)]")
    return text[:idx] if idx >= 0 else text


def _routes_in_segment(segment: str, source: Path) -> list[tuple[str, str, str]]:
    """返回段内 [(方法, 路径, handler), ...]。"""
    out: list[tuple[str, str, str]] = []
    for match in ROUTE_RE.finditer(segment):
        open_idx = match.end() - 1
        try:
            end_idx = _balanced_end(segment, open_idx)
        except ValueError:
            print(f"[warn] {source}: 无法解析 .route( 括号（偏移 {open_idx}）", file=sys.stderr)
            continue
        inner = segment[match.end() : end_idx]
        head = re.match(r'\s*"([^"]+)"\s*,?', inner)
        if not head:
            print(f"[warn] {source}: .route() 首个参数不是字符串字面量，已跳过", file=sys.stderr)
            continue
        path = head.group(1)
        rest = inner[head.end() :]
        methods = METHOD_RE.findall(rest)
        if not methods:
            print(f"[warn] {source}: {path} 未识别到方法路由", file=sys.stderr)
            continue
        for verb, handler in methods:
            out.append((verb.upper(), path, handler))
    return out


def _auth_in_segment(segment: str) -> str:
    found = AUTH_MW_RE.findall(segment)
    if not found:
        return "无"
    mw = found[-1].split("::")[-1]
    return AUTH_LABEL.get(mw, mw)


def parse_file(source: Path) -> tuple[str, list[tuple[str, str, str, str]]]:
    """解析单个路由文件，返回 (入口函数名, [(方法, 路径, 鉴权, handler), ...])。"""
    text = _strip_test_modules(source.read_text(encoding="utf-8"))
    fn = re.search(r"pub fn ([A-Za-z0-9_]+)[^)]*->\s*Router", text)
    fn_name = fn.group(1) if fn else source.parent.name + "_routes"

    rows: list[tuple[str, str, str, str]] = []
    starts = [m.start() for m in ROUTER_NEW_RE.finditer(text)]
    starts.append(len(text))
    for i in range(len(starts) - 1):
        segment = text[starts[i] : starts[i + 1]]
        auth = _auth_in_segment(segment)
        for verb, path, handler in _routes_in_segment(segment, source):
            rows.append((verb, path, auth, handler))
    return fn_name, rows


def collect() -> tuple[list[tuple[str, str, list[tuple[str, str, str, str]]]], list[str]]:
    """返回 ([(key, 入口函数名, rows)], warnings)。"""
    groups: list[tuple[str, str, list[tuple[str, str, str, str]]]] = []
    warnings: list[str] = []

    mod_rs = WEB_SRC / "mod.rs"
    if mod_rs.exists():
        _, rows = parse_file(mod_rs)
        if rows:
            # health 路由直接内联在 build_router 里，没有独立入口函数
            groups.append(("health", "build_router", rows))

    api_dir = WEB_SRC / "api"
    for source in sorted(api_dir.glob("*/routes.rs")):
        key = source.parent.name
        if key not in GROUP_LABELS:
            warnings.append(f"{source}: 路由组 {key} 未在 GROUP_LABELS 登记中文名")
        fn_name, rows = parse_file(source)
        groups.append((key, fn_name, rows))

    return groups, warnings


def render(groups: list[tuple[str, str, list[tuple[str, str, str, str]]]]) -> str:
    total_endpoints = sum(len(rows) for _, _, rows in groups)
    total_paths = len({row[1] for _, _, rows in groups for row in rows})
    ws_count = sum(1 for _, _, rows in groups for row in rows if row[3].split("::")[-1].endswith("_ws"))

    lines: list[str] = [
        "# API 端点清单（自动生成）",
        "",
        "> **禁止手工编辑**。本文件由 `scripts/gen_api_surface.py` 从路由源码生成，",
        "> CI 以 `python3 scripts/gen_api_surface.py --check` 校验；修改路由后请在本地",
        "> 执行 `python3 scripts/gen_api_surface.py` 并提交本文件。",
        "",
        "**数据来源**：`gateway-api/src/web/mod.rs`（health 探针）+ "
        "`gateway-api/src/web/api/*/routes.rs`（各业务子域）。",
        "",
        f"**统计**：{len(groups)} 个路由组 / {total_paths} 条唯一路径 / "
        f"{total_endpoints} 个「方法 + 路径」端点（其中 WebSocket 端点 {ws_count} 个）。",
        "",
        "**鉴权说明**：`token` = 始终需要 `Authorization: Bearer <token>`；"
        "`token（allow_anon_read=true 时可匿名）` = 匿名只读开关打开时可不带 token；",
        "`admin / exec scope` = 需要管理员或 `exec` scope 的 API 密钥；`无` = 公开端点。",
        "",
    ]

    for key, fn_name, rows in groups:
        label = GROUP_LABELS.get(key, key)
        lines.append(f"## `{key}` — {label}")
        lines.append("")
        lines.append(f"入口函数：`{fn_name}`；端点数：{len(rows)}")
        lines.append("")
        lines.append("| 方法 | 路径 | 鉴权 | 处理器 |")
        lines.append("|---|---|---|---|")
        for verb, path, auth, handler in rows:
            lines.append(f"| {verb} | `{path}` | {auth} | `{handler}` |")
        lines.append("")

    lines.append("---")
    lines.append("")
    lines.append(
        "> 端点分组与鉴权层级见 "
        "[`architecture/protocols.md`](architecture/protocols.md#51-前后端通信)。"
    )
    lines.append("")
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description="生成 / 校验 docs/api-surface.md")
    parser.add_argument(
        "--check",
        action="store_true",
        help="只校验：与已提交文件不一致时退出码 1（CI 用）",
    )
    args = parser.parse_args()

    if not WEB_SRC.exists():
        print(f"[error] 找不到路由源码目录：{WEB_SRC}", file=sys.stderr)
        return 2

    groups, warnings = collect()
    for warning in warnings:
        print(f"[warn] {warning}", file=sys.stderr)

    content = render(groups)

    if args.check:
        if not OUT_FILE.exists():
            print(f"[error] {OUT_FILE} 不存在，请先运行 python3 scripts/gen_api_surface.py", file=sys.stderr)
            return 2
        current = OUT_FILE.read_text(encoding="utf-8")
        if current == content:
            print(f"[ok] {OUT_FILE.relative_to(REPO_ROOT)} 与路由源码一致")
            return 0
        print(
            f"[error] {OUT_FILE.relative_to(REPO_ROOT)} 与路由源码不一致——"
            "请运行 python3 scripts/gen_api_surface.py 后提交",
            file=sys.stderr,
        )
        return 1

    OUT_FILE.parent.mkdir(parents=True, exist_ok=True)
    # 统一 LF：Windows 本地生成与 CI(Linux) 校验必须字节一致
    with OUT_FILE.open("w", encoding="utf-8", newline="\n") as handle:
        handle.write(content)
    total = sum(len(rows) for _, _, rows in groups)
    print(
        f"[ok] 已生成 {OUT_FILE.relative_to(REPO_ROOT)}："
        f"{len(groups)} 个路由组 / {total} 个端点"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
