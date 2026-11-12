#!/usr/bin/env python3
# 给 apt 追加多路 fallback 源：官方源保持原样不动，国内镜像排在后面兜底。
#
# 背景（2026-11-11 实测）：Canonical 侧 ubuntu.com 权威 DNS 整族 SERVFAIL，
# archive.ubuntu.com / azure.archive.ubuntu.com / security.ubuntu.com 全部
# "Temporary failure resolving"，apt-get update 拿不到任何索引，于是装字体、
# 装 musl-tools 等步骤全部失败；同一批次不依赖 apt 的作业全绿，说明与本仓
# 改动无关。
#
# 关键取舍：**只追加，不改写**。
#   - 不删除、不覆盖 runner 预装的 ubuntu.sources / apt-mirrors.txt ——
#     ARM64 原生 runner 的 pocket 与官方源布局各不相同，凭空改写容易把它
#     需要的东西删掉（这类事故在真机上才会暴露）。
#   - apt 对同一 target（suite+component）会按 URIs 顺序逐个尝试，前一个
#     失败才试后一个。所以只要把国内镜像作为**同一 target 的额外 URI** 追加
#     进去，官方源整体不可用时 apt 会自动转过去，不需要自己写重试循环。
#   - 文件名带 zz- 前缀：apt 按文件名排序读 sources.list.d，保证官方源
#     排在前面（官方优先），国内镜像只在官方源失败时才会被用到。
#
# 用法：
#   sudo python3 apt_sources.py             # 改真实系统
#   python3 apt_sources.py --root /tmp/fake # 本地干跑，不动真实系统
import argparse
import pathlib
import re
import sys

# 兜底镜像：均在 ubuntu.com 域外，官方源整体挂掉时接管。
# resolute（26.04）/ noble（24.04）的 main / universe / restricted /
# multiverse 四个 pocket 已逐一实测可取。
FALLBACK_URIS = (
    "https://mirrors.tuna.tsinghua.edu.cn/ubuntu",
    "https://mirrors.aliyun.com/ubuntu",
    "https://mirrors.huaweicloud.com/ubuntu",
)

COMPONENTS = "main restricted universe multiverse"
SUITE_SUFFIXES = ("", "-updates", "-backports", "-security")

# 追加文件放最后（zz- 前缀）→ 官方源优先
FALLBACK_LIST = "sources.list.d/zz-ci-ubuntu-fallback.list"

# 签名密钥：one-line 格式不写 signed-by 时依赖 apt 的默认信任库，这里显式
# 指定，避免不同镜像里 keyring 路径不同导致签名校验失败。
KEYRING_CANDIDATES = (
    "usr/share/keyrings/ubuntu-archive-keyring.gpg",
    "etc/apt/trusted.gpg.d/ubuntu-keyring-20250801.gpg",
)


def detect_codename(root: pathlib.Path) -> str:
    """从 os-release 解析发行版代号（26.04 是 resolute，24.04 是 noble）。"""
    text = (root / "etc/os-release").read_text(encoding="utf-8")
    m = re.search(r'^VERSION_CODENAME="?([^"\n]+)"?', text, re.M)
    return m.group(1) if m else ""


def find_keyring(root: pathlib.Path) -> str:
    """返回真实系统里 keyring 的绝对路径；找不到就返回空串（交给 apt 默认信任库）。"""
    for rel in KEYRING_CANDIDATES:
        if (root / rel).is_file():
            return "/" + rel
    trusted = root / "etc/apt/trusted.gpg.d"
    if trusted.is_dir():
        for p in sorted(trusted.glob("ubuntu-keyring-*.gpg")):
            return "/" + str(p.relative_to(root)).replace("\\", "/")
    return ""


def main() -> int:
    ap = argparse.ArgumentParser(description="追加 apt 多路 fallback 源")
    ap.add_argument("--root", default="/", help="根目录前缀（本地干跑用）")
    args = ap.parse_args()
    root = pathlib.Path(args.root)

    codename = detect_codename(root)
    if not codename:
        print("[apt] 无法从 os-release 解析 VERSION_CODENAME，放弃追加兜底源", file=sys.stderr)
        return 1
    suites = [codename + s for s in SUITE_SUFFIXES]

    keyring = find_keyring(root)
    signed_by = f"[signed-by={keyring}]" if keyring else ""

    # 1) 写兜底源列表。幂等：先删掉上一次写的同名文件，避免重复累积。
    out = root / "etc/apt" / FALLBACK_LIST
    out.parent.mkdir(parents=True, exist_ok=True)
    if out.exists():
        out.unlink()
    lines = []
    for uri in FALLBACK_URIS:
        for suite in suites:
            parts = ["deb"]
            if signed_by:
                parts.append(signed_by)
            parts += [uri, suite, COMPONENTS]
            lines.append(" ".join(parts))
    out.write_text("\n".join(lines) + "\n", encoding="utf-8")

    # 2) 重试与超时：官方源整体不可达时不至于一次就放弃，也不至于久等。
    conf = root / "etc/apt/apt.conf.d/99ci-retries"
    conf.parent.mkdir(parents=True, exist_ok=True)
    conf.write_text(
        'Acquire::Retries "2";\n'
        'Acquire::http::Timeout "15";\n'
        'Acquire::https::Timeout "15";\n'
        'Acquire::http::Dl-Limit "0";\n',
        encoding="utf-8",
    )

    print(f"[apt] 已追加兜底源 {out}（{codename}，签名：{signed_by or '默认信任库'}）；")
    print("[apt] runner 预装的官方源保持原样，官方源可用时不会走到兜底源。")
    for line in lines:
        print("   ", line)
    return 0


if __name__ == "__main__":
    sys.exit(main())
