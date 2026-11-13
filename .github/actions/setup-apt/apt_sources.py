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
# ── 架构适配（2026-11-13 实测）────────────────────────────────────────
# 国内镜像按架构分开同步，实测矩阵（noble / resolute 同构）：
#     <host>/ubuntu          （main 归档）  amd64 ✅  arm64 仅 resolute ✅、noble ❌
#     <host>/ubuntu-ports    （ports 归档） arm64 ✅  amd64 ❌
#
# apt 的 `deb` 行默认对**所有**架构生效，所以一行不带架构限定的 amd64 源，
# 在 ARM64 runner 上照样会去取 binary-arm64 的索引；镜像返回 404 →
# apt 报 "Some index files failed to download" 并以 exit 100 结束 →
# 装包步骤必挂。实测后果：ARM64 原生装机验证 job 100% 失败，
# 而 ARM64 runner 自带的官方源其实是好的——是我们追加的兜底源把它弄坏的。
#
# 因此这里做两件事：
#   1) 每一行都加 [arch=...] 限定，只对本 runner 真正需要的架构生效；
#   2) 逐个镜像实测「这个根路径 + 这个架构的索引」确实取得到，取不到就不写
#      这一行。宁可少一个兜底源，也不要写一个必然 404 的源——后者会把
#      「官方源整体不可用」从偶发变成必挂。
#
# 用法：
#   sudo python3 apt_sources.py             # 改真实系统
#   python3 apt_sources.py --root /tmp/fake # 本地干跑，不动真实系统
import argparse
import pathlib
import re
import subprocess
import sys
import urllib.error
import urllib.request

# 兜底镜像：均在 ubuntu.com 域外，官方源整体挂掉时接管。
FALLBACK_HOSTS = (
    "mirrors.tuna.tsinghua.edu.cn",
    "mirrors.aliyun.com",
    "mirrors.huaweicloud.com",
)

# 同一台镜像的两个根路径：main 归档与 ports 归档。按顺序探测，
# 取第一个真正提供目标架构索引的那个。
ARCH_ROOTS = ("/ubuntu", "/ubuntu-ports")

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

PROBE_TIMEOUT = 10
PROBE_UA = "gateway-platform-ci/apt-probe"


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


UNAME_TO_ARCH = {
    "x86_64": "amd64",
    "aarch64": "arm64",
    "armv7l": "armhf",
    "armv6l": "armhf",
    "i386": "i386",
    "i686": "i386",
    "ppc64le": "ppc64el",
    "s390x": "s390x",
    "riscv64": "riscv64",
}


def host_architectures(override: list[str]) -> list[str]:
    """本机 apt 会取索引的架构：主架构 + 已启用的外来架构。

    用 dpkg 而不是 uname：dpkg 的架构名与 apt 的 binary-<arch> 目录名一致，
    且外来架构（i386 等）也一并算上，不会漏取索引。
    dpkg 不可用时退回 uname -m（映射表见 UNAME_TO_ARCH）。
    """
    if override:
        return list(dict.fromkeys(override))
    archs: list[str] = []
    for args in (["--print-architecture"], ["--print-foreign-architectures"]):
        try:
            out = subprocess.run(
                ["dpkg", *args],
                capture_output=True,
                text=True,
                timeout=10,
                check=False,
            ).stdout
        except (OSError, subprocess.SubprocessError):
            continue
        for a in out.split():
            if a and a not in archs:
                archs.append(a)
    if archs:
        return archs
    try:
        machine = subprocess.run(
            ["uname", "-m"], capture_output=True, text=True, timeout=10, check=False
        ).stdout.strip()
    except (OSError, subprocess.SubprocessError):
        machine = ""
    mapped = UNAME_TO_ARCH.get(machine)
    return [mapped] if mapped else []


def http_ok(url: str) -> bool:
    """HEAD 探测一个 URL 是否可取（2xx/3xx 算可取）。取不到就返回 False。"""
    req = urllib.request.Request(url, method="HEAD", headers={"User-Agent": PROBE_UA})
    try:
        with urllib.request.urlopen(req, timeout=PROBE_TIMEOUT) as resp:
            return 200 <= resp.status < 400
    except urllib.error.HTTPError:
        return False
    except Exception:  # 网络不可达 / 超时 / TLS 失败
        return False


def pick_root(host: str, codename: str, arch: str) -> str:
    """返回该镜像下真正提供 <codename>/<arch> 索引的根路径；都没有则空串。"""
    for root in ARCH_ROOTS:
        url = f"https://{host}{root}/dists/{codename}/main/binary-{arch}/Packages.gz"
        if http_ok(url):
            return root
    return ""


def existing_suites(host: str, root: str, codename: str) -> list[str]:
    """只保留该镜像上真实存在的套件（镜像缺某个 pocket 时 apt 会直接报错）。"""
    suites = []
    for suffix in SUITE_SUFFIXES:
        suite = codename + suffix
        if http_ok(f"https://{host}{root}/dists/{suite}/Release"):
            suites.append(suite)
    return suites


def main() -> int:
    ap = argparse.ArgumentParser(description="追加 apt 多路 fallback 源")
    ap.add_argument("--root", default="/", help="根目录前缀（本地干跑用）")
    ap.add_argument(
        "--arch",
        action="append",
        default=[],
        help="覆盖自动探测的架构（可重复；本地干跑时用来验证不同架构的源选择）",
    )
    args = ap.parse_args()
    root = pathlib.Path(args.root)

    codename = detect_codename(root)
    if not codename:
        print("[apt] 无法从 os-release 解析 VERSION_CODENAME，放弃追加兜底源", file=sys.stderr)
        return 1
    archs = host_architectures(args.arch)
    if not archs:
        print("[apt] 无法确定本机架构，放弃追加兜底源（宁可无兜底，也不用必然 404 的源）", file=sys.stderr)
        return 1

    keyring = find_keyring(root)
    signed_by = f"signed-by={keyring}" if keyring else ""

    # 逐镜像逐架构实测：只有「根路径确实提供该架构索引」的组合才写进配置。
    lines: list[str] = []
    for host in FALLBACK_HOSTS:
        for arch in archs:
            arch_root = pick_root(host, codename, arch)
            if not arch_root:
                print(f"[apt] {host} 未提供 {codename}/{arch} 索引，跳过该组合", file=sys.stderr)
                continue
            suites = existing_suites(host, arch_root, codename)
            if not suites:
                print(f"[apt] {host}{arch_root} 上没有 {codename} 的可用套件，跳过", file=sys.stderr)
                continue
            opts = f"arch={arch}"
            if signed_by:
                opts += f" {signed_by}"
            for suite in suites:
                lines.append(f"deb [{opts}] https://{host}{arch_root} {suite} {COMPONENTS}")

    # 1) 写兜底源列表。幂等：先删掉上一次写的同名文件，避免重复累积。
    out = root / "etc/apt" / FALLBACK_LIST
    out.parent.mkdir(parents=True, exist_ok=True)
    if out.exists():
        out.unlink()
    if not lines:
        # 探测全灭：宁可什么都不写，也不要留下必然 404 的源——
        # 一个 404 的源会让 apt-get update 以 exit 100 收场，
        # 把「官方源可用」也一起弄挂。
        print("[apt] 没有镜像同时满足本机架构与套件要求，本轮不追加兜底源", file=sys.stderr)
    else:
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

    print(f"[apt] 目标：{codename} / 架构 {', '.join(archs)}；签名：{signed_by or '默认信任库'}")
    print("[apt] runner 预装的官方源保持原样，官方源可用时不会走到兜底源。")
    if lines:
        print(f"[apt] 已追加兜底源 {out}，共 {len(lines)} 行：")
        for line in lines:
            print("   ", line)
    return 0


if __name__ == "__main__":
    sys.exit(main())
