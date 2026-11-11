#!/usr/bin/env python3
"""变异测试门禁：判断一个 cargo-mutants 分片的结果能不能放行。

背景
----
cargo-mutants 的退出码语义（见其 src/outcome.rs 的 exit_code()）是：
    baseline 失败  -> BaselineFailed
    有超时变异体  -> Timeout
    有存活变异体  -> FoundProblems
    其余          -> Success
也就是说，只要还存在一个「测试抓不到的变异体」，cargo mutants 就以非零码退出。
而在把断言补齐之前，这类变异体是客观存在的存量缺口，若直接拿退出码当门禁，
这条工作流永远是红的，红色也就失去了意义。

本脚本把「存量」和「新增」分开：
  * 存量漏网（已登记在基线里的 missed）-> 只报告，不拦；
  * 新增漏网（基线里没有的 missed）    -> 拦下，这是新引入的覆盖退化；
  * 超时变异体                        -> 一律拦下。超时意味着被测代码会挂死，
    这是线上隐患而不是覆盖缺口；
  * 分片没跑完（结果数对不上 mutants.json）-> 拦下，避免「半截跑完」被当成通过。

这与仓库既有的「存量只减不增」口径一致（见 scripts/line_budget_frozen.txt）：
基线只允许缩小，不允许变大。

用法
----
校验当前分片：
    python3 scripts/mutation_gate.py --out mutants.out --baseline scripts/mutation_baseline.txt

用若干份 missed.txt 重建基线（把各分片的漏网清单合并去重）：
    python3 scripts/mutation_gate.py --update shards/missed-*.txt --baseline scripts/mutation_baseline.txt

退出码：0 通过，1 拦下。
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

DEFAULT_OUT = "mutants.out"
DEFAULT_BASELINE = "scripts/mutation_baseline.txt"


def read_names(path: Path) -> list[str]:
    """读取一行一个名称的清单文件，忽略空行。"""
    if not path.is_file():
        return []
    names = []
    # utf-8-sig：容忍带 BOM 的清单文件（Windows 上重定向写出的文件会带 BOM）
    for line in path.read_text(encoding="utf-8-sig").splitlines():
        line = line.strip()
        if line and not line.startswith("#"):
            names.append(line)
    return names


def load_expected(out_dir: Path) -> set[str] | None:
    """从 mutants.json 读出本分片应有的变异体清单。

    cargo-mutants 每跑完一个变异体就把结果写进 caught/missed/timeout/unviable，
    mutants.json 则是本分片的完整清单。两者对不上就说明分片被中途打断。
    """
    manifest = out_dir / "mutants.json"
    if not manifest.is_file():
        return None
    try:
        data = json.loads(manifest.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, OSError) as exc:
        print(f"[FAIL] mutants.json 解析失败：{exc}", file=sys.stderr)
        return None
    return {item["name"] for item in data if "name" in item}


def announce(level: str, message: str) -> None:
    """在 GitHub Actions 里打注解，本地就打印普通文本。"""
    if os.environ.get("GITHUB_ACTIONS") == "true":
        print(f"::{level}::{message}")
    else:
        prefix = {"warning": "[WARN]", "error": "[FAIL]", "notice": "[INFO]"}.get(level, "[INFO]")
        print(f"{prefix} {message}")


def update_baseline(baseline_path: Path, inputs: list[str]) -> int:
    """把若干份 missed.txt 合并去重后写成基线。"""
    names: set[str] = set()
    for item in inputs:
        path = Path(item)
        if not path.is_file():
            print(f"[FAIL] 找不到漏网清单：{item}", file=sys.stderr)
            return 1
        names.update(read_names(path))
    baseline_path.parent.mkdir(parents=True, exist_ok=True)
    header = (
        "# 变异测试存量漏网基线（由 scripts/mutation_gate.py --update 生成）\n"
        "#\n"
        "# 每一行是一个「测试抓不到」的变异体（cargo-mutants 判定为 missed）。\n"
        "# 门禁只拦「不在本文件里的新增漏网」，所以本文件只应缩小、不应变大：\n"
        "# 补了断言的变异体会从 missed 变成 caught，届时应从本文件里删掉对应行。\n"
        "#\n"
        "# 不要手工编辑本文件，用下面的命令重建：\n"
        "#   python3 scripts/mutation_gate.py --update <各分片 missed.txt> \\\n"
        "#       --baseline scripts/mutation_baseline.txt\n"
    )
    baseline_path.write_text(header + "\n".join(sorted(names)) + "\n", encoding="utf-8")
    print(f"[OK] 基线已写入 {baseline_path}，共 {len(names)} 条存量漏网")
    return 0


def check(out_dir: Path, baseline_path: Path) -> int:
    """校验单个分片的结果。"""
    if not out_dir.is_dir():
        print(f"[FAIL] 没有找到产物目录 {out_dir}", file=sys.stderr)
        return 1

    caught = read_names(out_dir / "caught.txt")
    missed = read_names(out_dir / "missed.txt")
    timeouts = read_names(out_dir / "timeout.txt")
    unviable = read_names(out_dir / "unviable.txt")
    done = set(caught) | set(missed) | set(timeouts) | set(unviable)

    expected = load_expected(out_dir)
    if expected is None:
        print(f"[FAIL] {out_dir} 下没有可读的 mutants.json，无法核对分片完整性", file=sys.stderr)
        return 1
    if not expected:
        print(f"[INFO] 本分片没有变异体，跳过")
        return 0
    if len(done) < len(expected):
        missing = len(expected) - len(done)
        announce(
            "error",
            f"分片未跑完：已出结果 {len(done)}/{len(expected)}，还差 {missing} 个。"
            f"多半是 runner 被取消或超时，别把半截结果当成通过。",
        )
        return 1

    print(
        f"[INFO] 分片结果：caught={len(caught)} missed={len(missed)} "
        f"timeout={len(timeouts)} unviable={len(unviable)} 合计={len(done)}/{len(expected)}"
    )

    if timeouts:
        announce(
            "error",
            f"发现 {len(timeouts)} 个超时变异体，被测代码在这些变异下会挂死，一律拦下：",
        )
        for name in timeouts:
            print(f"  timeout {name}")
        return 1

    baseline = set(read_names(baseline_path))
    if not baseline:
        announce(
            "warning",
            f"基线 {baseline_path} 为空，处于初始化模式：本次只报告 {len(missed)} 条存量漏网，"
            f"不拦。跑完后请用 --update 重建基线并提交。",
        )
        for name in missed:
            print(f"  missed {name}")
        return 0

    new_missed = sorted(set(missed) - baseline)
    fixed = len(baseline - set(missed))
    if fixed:
        print(f"[INFO] 有 {fixed} 条存量漏网已被修掉（不再出现在 missed 里），建议同步删掉基线对应行")
    if new_missed:
        announce(
            "error",
            f"新增 {len(new_missed)} 条漏网变异体（测试抓不到），这是新引入的覆盖退化：",
        )
        for name in new_missed:
            print(f"  new-missed {name}")
        return 1

    print(f"[OK] 本分片没有新增漏网，存量漏网 {len(set(missed) & baseline)} 条已登记在基线里")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="变异测试门禁")
    parser.add_argument("--out", default=DEFAULT_OUT, help="cargo-mutants 产物目录")
    parser.add_argument("--baseline", default=DEFAULT_BASELINE, help="存量漏网基线文件")
    parser.add_argument(
        "--update",
        nargs="+",
        metavar="MISSED_TXT",
        help="重建基线模式：传入若干份 missed.txt",
    )
    args = parser.parse_args()

    if args.update:
        return update_baseline(Path(args.baseline), args.update)
    return check(Path(args.out), Path(args.baseline))


if __name__ == "__main__":
    sys.exit(main())
