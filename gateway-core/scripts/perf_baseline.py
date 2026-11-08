#!/usr/bin/env python3
"""gateway-platform 性能基线采集（T0.2 重写版）。

采集指标：
  - 吞吐：采集事务速率（点/秒）
  - 可靠性：采集失败数、协议解码错误数
  - 队列：数据队列深度峰值
  - 资源：进程 CPU%（相对核数）、RSS（MB、峰值与增长，用于识别泄漏）
  - 延迟：采集耗时 p50/p95/p99（毫秒，取自 /api/metrics 的 Prometheus 直方图）

工作机理：
  1) release 构建 gatewayd（可用 --no-build 跳过）
  2) 生成压测配置（Modbus TCP 总线 + N 点 + 打开 Web），经 GATEWAY_CONFIG_PATH 传给 gatewayd
     （gatewayd 的 main 不解析 CLI 参数，配置路径只能由该环境变量提供）
  3) 默认由脚本内置一个最小 Modbus TCP 从站（FC3/FC4），实现零外部依赖的自洽压测；
     若目标设备已有真实从站，用 --no-slave --slave-addr host:port 指向它
  4) 启动 gatewayd，按 --sample-sec 周期采样 /api/metrics 与进程 CPU/RSS
  5) 汇总写出 Markdown 报告（可选 JSON）

用法：
  python gateway-core/scripts/perf_baseline.py --duration-min 5 --points 500
  python gateway-core/scripts/perf_baseline.py --duration-min 30 --points 2000 --sample-sec 10
  python gateway-core/scripts/perf_baseline.py --no-slave --slave-addr 192.168.1.10:502 --points 1000
"""

from __future__ import annotations  # 兼容 Python 3.9（`X | None` 等注解延迟求值）

import argparse
import json
import os
import re
import socket
import struct
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request

# 脚本位于 <root>/gateway-core/scripts/perf_baseline.py
REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
CPU_COUNT = os.cpu_count() or 1
# 产物名随平台变：Windows 是 gatewayd.exe，Linux/macOS 是 gatewayd。
# 此前写死 "gatewayd.exe"，导致脚本在 GitHub Actions 的 ubuntu runner 上
# 构建完却报「gatewayd 不存在」——CI 里跑性能基线必须先修这一条。
EXE_NAME = "gatewayd.exe" if sys.platform == "win32" else "gatewayd"
IS_LINUX = sys.platform.startswith("linux")


# ============================================================================
# 最小 Modbus TCP 从站（FC3 读保持寄存器 / FC4 读输入寄存器）
# ============================================================================
class ModbusTcpSlave:
    """应答 FC3/FC4 读请求的最小从站，供无外部设备时自洽压测。"""

    def __init__(self, host: str, port: int):
        self.host = host
        self.port = port
        self._srv: socket.socket | None = None
        self._stop = threading.Event()

    def start(self) -> None:
        self._srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self._srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self._srv.bind((self.host, self.port))
        self._srv.listen(64)
        threading.Thread(target=self._accept_loop, daemon=True).start()

    def stop(self) -> None:
        self._stop.set()
        try:
            if self._srv is not None:
                self._srv.close()
        except OSError:
            pass

    def _accept_loop(self) -> None:
        while not self._stop.is_set():
            try:
                conn, _ = self._srv.accept()
            except OSError:
                break
            threading.Thread(target=self._serve, args=(conn,), daemon=True).start()

    @staticmethod
    def _recv_exact(conn: socket.socket, n: int) -> bytes | None:
        buf = b""
        while len(buf) < n:
            chunk = conn.recv(n - len(buf))
            if not chunk:
                return None
            buf += chunk
        return buf

    def _serve(self, conn: socket.socket) -> None:
        with conn:
            conn.settimeout(30)
            # 关键：关闭 Nagle。客户端（网关）以流水线方式同时投递多帧，
            # 若本端 Nagle 开启，背靠背的小响应会被 Nagle+延迟 ACK 交互扣住，
            # 造成每帧数十毫秒的假性延迟，压不出网关真实并发能力。
            conn.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            while not self._stop.is_set():
                try:
                    hdr = self._recv_exact(conn, 7)  # MBAP: tx_id, proto, len, unit
                    if hdr is None:
                        break
                    tx_id, _proto, length, unit = struct.unpack(">HHHB", hdr)
                    pdu = self._recv_exact(conn, length - 1)
                    if pdu is None:
                        break
                    func = pdu[0]
                    if func in (3, 4) and len(pdu) >= 5:
                        _start, qty = struct.unpack(">HH", pdu[1:5])
                        qty = max(1, min(qty, 125))
                        resp_pdu = bytes([func, qty * 2]) + bytes(qty * 2)
                    else:
                        resp_pdu = bytes([func | 0x80, 0x01])  # 非法功能码
                    resp = struct.pack(">HHHB", tx_id, 0, len(resp_pdu) + 1, unit) + resp_pdu
                    conn.sendall(resp)
                except (OSError, struct.error):
                    break


def wait_ports_ready(host: str, ports: list[int], timeout: float = 5.0) -> bool:
    """轮询直到 host:port 全部可连接（mock 子进程就绪），或超时。"""
    deadline = time.time() + timeout
    pending = list(ports)
    while pending and time.time() < deadline:
        still = []
        for port in pending:
            try:
                with socket.create_connection((host, port), timeout=0.5):
                    pass
            except OSError:
                still.append(port)
        pending = still
        if pending:
            time.sleep(0.05)
    return not pending


# ============================================================================
# 配置生成
# ============================================================================
def point_template(idx: int, bus: str, interval_sec: int, slave_id: int, addr: int) -> dict:
    return {
        "id": f"pt_{idx}",
        "name": f"point-{idx}",
        "device_id": None,
        "bus": bus,
        "protocol": "modbus",
        "address": str(slave_id),
        "slave_id": slave_id,
        "address_num": addr,
        "data_id": None,
        "function_code": 3,
        "data_type": "uint16",
        "byte_order": None,
        "register_count": None,
        "interval": interval_sec,
        "scale": 1.0,
        "offset": 0.0,
        "formula": None,
        "ct_ratio": None,
        "pt_ratio": None,
        "deadband": 0.0,
        "on_change": False,
        "unit": None,
        "alarm_high": None,
        "alarm_low": None,
        "aggregation": None,
        "modbus_register": None,
    }


def build_config(points: int, interval_sec: int, web_addr: str, token: str,
                 slaves: int = 1, tcp_concurrent: int = 8,
                 bus_addrs: list[str] | None = None, drain: bool = False) -> dict:
    # 关键：point.bus 必须等于 bus.port_name（采集器表按 port_name 索引，
    # 不匹配则调度器找不到 collector，采集量为 0）。
    #
    # bus_addrs：每条总线一个 `host:port`。K 条总线各连一个（可独立进程的）mock 从站，
    # 用于把测试端从「单连接单线程 Python + GIL」的天花板下解放出来：
    # 网关会为每条 tcp 总线开一条独立 TCP 连接 → mock 端每连接一个线程/进程并行。
    if not bus_addrs:
        bus_addrs = ["127.0.0.1:15020"]
    slaves = max(1, min(slaves, 247))  # Modbus 从站号合法范围 1..247
    buses = max(1, min(len(bus_addrs), 247))
    bus_names = [f"bus{i}" for i in range(buses)]
    bus_objs = []
    for i, bn in enumerate(bus_names):
        bus_objs.append({
            "port_name": bn,
            "baud_rate": 9600,
            "parity": "none",
            "data_bits": 8,
            "stop_bits": 1,
            "timeout_ms": 500,
            "protocol": "modbus",
            "retry_count": 2,
            "retry_delay_ms": 200,
            "de_pin": None,
            "modbus_mode": "rtu",
            "tcp_addr": bus_addrs[i % len(bus_addrs)],
            "tcp_concurrent": max(1, tcp_concurrent),
            "channel_capacity": 1024,
            "reconnect_backoff": {"seconds": [1, 2, 5, 10, 30], "jitter_ratio": 0.0},
        })
    cfg = {
        "schema_version": 1,
        "buses": bus_objs,
        # --slaves N：点位轮转分配到 N 个 Modbus 从站号（1..N）。
        # N=1：全部同从站、地址连续 → merge_batches 把同一 (slave,fc) 合并进 ≤125 寄存器的帧，
        #        事务数远小于点数（贴近"少量集中点位"的现场）。
        # N>1：地址被打散（步长 N，gap>5 不再合并）→ 每点各自成帧，总线事务数≈点数，
        #        用于把「总线/调度」的真实上限压出来。
        # --buses K：点位轮转分摊到 K 条总线（各总线并行采集，事务总量不变但连接数 ×K）。
        # address_num 是 u16（≤65535），故点位规模不能直接等于地址：
        # 同一从站内的地址按 stride=8 递增（>merge gap=5，保证每点独立成帧、事务数≈点数），
        # 从站号与总线号分别轮转，使 65535/8×247 的地址空间足以承载十万级点位。
        "points": [
            point_template(i, bus_names[i % buses], interval_sec, (i % slaves) + 1,
                           (i // slaves) * 8 if slaves > 1 else i)
            for i in range(points)
        ],
        "devices": None,
        "mqtt": {"enabled": False},
        "ntp": {"server": "ntp.aliyun.com", "sync_interval": 3600},
        "web": {
            "api_token": token,
            "username": "admin",
            "password_hash": "",
            "listen_addr": web_addr,
            "allow_anon_read": False,
        },
        "webhook": None,
        "ha": None,
        "cellular": None,
        "opcua": None,
        "modbus_server": None,
        "modbus_passthrough": None,
        "device_groups": None,
        "device_assets": None,
        "virtual_points": None,
        "telemetry": None,
        "license": None,
        "self_heal": None,
    }
    if drain:
        # 长稳场景必须让队列走完「出队 → ack」：MQTT 关闭时队列只进不出，
        # RSS 会随本地历史库单调上涨（实测 60 min 涨到 226 MB，拟合 144 MB/h），
        # 那时量到的是「未消费积压的存储占用」而不是网关稳态内存。
        # 模拟消费者不外发数据，只做 dequeue + ack，正好用来让队列稳态。
        cfg["sim_consumer"] = {
            "enabled": True,
            "batch_size": 500,
            "interval_ms": 100,
            "log_every": 0,
        }
    return cfg


# ============================================================================
# 进程资源采样（psutil 优先；缺失时 Linux 读 /proc、Windows 回退 PowerShell）
# ============================================================================
def _read_cpu_rss_proc(pid: int) -> tuple[float, int] | None:
    """Linux：直接读 /proc/<pid>，不依赖 psutil。

    GitHub Actions 的 ubuntu runner 没有预装 psutil，而 CPU / RSS 正是
    CI 性能基线最核心的两项——缺了它们，作业只能证明「跑完了」。
    """
    try:
        with open(f"/proc/{pid}/stat", encoding="utf-8", errors="replace") as f:
            stat = f.read()
        # 第 2 列 comm 允许含空格和括号（如 "(tokio-runtime-w)"），
        # 必须从**最后一个** ')' 之后切分，否则所有列号错位。
        rest = stat[stat.rindex(")") + 2:].split()
        utime, stime = int(rest[11]), int(rest[12])  # 第 14/15 列
        ticks = os.sysconf("SC_CLK_TCK")
        rss_kb = 0
        with open(f"/proc/{pid}/status", encoding="utf-8", errors="replace") as f:
            for line in f:
                if line.startswith("VmRSS:"):
                    rss_kb = int(line.split()[1])
                    break
        return ((utime + stime) / ticks, rss_kb * 1024)
    except Exception:
        return None


def read_cpu_rss(pid: int) -> tuple[float, int] | None:
    """返回 (累计 CPU 秒, RSS 字节)；失败返回 None。"""
    try:
        import psutil  # type: ignore

        p = psutil.Process(pid)
        t = p.cpu_times()
        return (t.user + t.system, p.memory_info().rss)
    except ImportError:
        pass
    except Exception:
        return None

    if IS_LINUX:
        return _read_cpu_rss_proc(pid)

    try:
        out = subprocess.run(
            ["powershell", "-NoProfile", "-Command",
             f"$p=Get-Process -Id {pid} -ErrorAction Stop; '{0}|{1}' -f $p.CPU,$p.WorkingSet64"],
            capture_output=True, text=True, timeout=10,
        )
        cpu_s, rss = out.stdout.strip().split("|")
        return (float(cpu_s), int(rss))
    except Exception:
        return None


# 上一次 /proc/stat 读数（total, idle），用于差分求整机 CPU 占用。
_prev_sys_stat: tuple[int, int] | None = None


def _system_cpu_proc() -> float | None:
    """Linux：/proc/stat 两次读数差分。首次只记基线，返回 None。"""
    global _prev_sys_stat
    try:
        with open("/proc/stat", encoding="utf-8") as f:
            # 只取前 8 列（user..steal）：guest / guest_nice 已计入 user / nice，
            # 一并求和会重复计数。
            vals = [int(x) for x in f.readline().split()[1:]][:8]
        idle = vals[3] + (vals[4] if len(vals) > 4 else 0)  # idle + iowait
        total = sum(vals)
    except Exception:
        return None
    prev, _prev_sys_stat = _prev_sys_stat, (total, idle)
    if prev is None:
        return None
    dt = total - prev[0]
    if dt <= 0:
        return None
    return (dt - (idle - prev[1])) / dt * 100.0


def system_cpu(interval: float | None) -> float | None:
    """整机 CPU 使用率（%）。interval=None 时返回距上次调用以来的均值。

    用于区分「网关自身 CPU 饱和」与「测试端 mock 与网关抢同一台机器」：
    网关 core 数低而整机接近 100% → 瓶颈在共享的测试机上，而非网关。
    """
    try:
        import psutil  # type: ignore

        return psutil.cpu_percent(interval=interval)
    except Exception:
        pass

    if IS_LINUX:
        # 采样循环本身按 sample-sec 周期调用，这里不需要再按 interval 阻塞。
        return _system_cpu_proc()
    return None


def rss_slope_mb_per_hour(snapshots: list[dict]) -> float | None:
    """RSS 小时斜率（MB/h），最小二乘拟合。

    与 soak 判定器（gateway-core/tests/soak/verdict.rs 的 rss_slope_per_hour）
    同口径：RSS 是锯齿曲线，首尾两点差会被噪声主导（同一份数据两点差值可能
    是拟合值的数倍），长稳泄漏判据必须用拟合值。
    丢掉开头 5% 的样本，避开启动期一次性分配（连接池 / 索引页 / 预分配缓冲）——
    那一段的增长不属于泄漏，摊进斜率会把正常进程判成泄漏。
    """
    pts = [(s["t"], s["rss_mb"]) for s in snapshots if s["rss_mb"] is not None]
    if len(pts) < 4:
        return None
    pts = pts[len(pts) // 20:]
    n = len(pts)
    mx = sum(t for t, _ in pts) / n
    my = sum(v for _, v in pts) / n
    den = sum((t - mx) ** 2 for t, _ in pts)
    if den <= 0:
        return None
    num = sum((t - mx) * (v - my) for t, v in pts)
    return num / den * 3600.0  # 秒 → 小时


# ============================================================================
# /api/metrics 抓取与 Prometheus 直方图分位计算
# ============================================================================
def scrape_metrics(url: str, token: str) -> str | None:
    req = urllib.request.Request(url, headers={"Authorization": f"Bearer {token}"})
    try:
        with urllib.request.urlopen(req, timeout=5) as resp:
            return resp.read().decode("utf-8", "replace")
    except (urllib.error.URLError, OSError):
        return None


# Prometheus 文本样本行：`name[{labels}] value [timestamp]`。
# labels 段允许「引号内的转义序列与任意字符（含空格、引号转义）」以及引号外的
# 键名 / 等号 / 逗号，因此用「带引号串 + 非引号非花括号字符」的交替匹配。
_SAMPLE_RE = re.compile(
    r'^([a-zA-Z_:][a-zA-Z0-9_:]*)(\{(?:"(?:[^"\\]|\\.)*"|[^"\\{])*\})?\s+(\S+)(?:\s+\S+)?\s*$'
)
_LE_RE = re.compile(r'le="((?:[^"\\]|\\.)*)"')


def parse_metrics(text: str) -> tuple[dict, dict]:
    """返回 ({指标名: 值}, {le 上界: 累积计数})。

    带标签的样本（`name{label="v"} value`）按**指标名求和**聚合，与原「1 条无标签
    序列」口径等价：例如 `gateway_collect_failures` 改为按 `by_reason` 多序列导出后，
    求和结果仍是失败总数，`gateway_bus_transaction_failures_total` 同理。
    """
    gauges: dict[str, float] = {}
    buckets: dict[str, float] = {}
    for line in text.splitlines():
        if not line or line.startswith("#"):
            continue
        m = _SAMPLE_RE.match(line)
        if not m:
            continue
        name, labels, val = m.group(1), m.group(2) or "", m.group(3)
        try:
            value = float(val)
        except ValueError:
            continue
        if name == "gateway_collect_duration_us_bucket":
            le = _LE_RE.search(labels)
            if le:
                buckets[le.group(1)] = value
            continue
        gauges[name] = gauges.get(name, 0.0) + value
    return gauges, buckets


def _bucket_bounds(buckets: dict) -> list[tuple[float | None, float]]:
    """把 le 上界排序为 [(上界(秒), 累积计数)]，+Inf 记为 None。"""
    out = []
    for le, cum in buckets.items():
        out.append((None if le == "+Inf" else float(le), cum))
    # 数值升序，+Inf 放最后
    out.sort(key=lambda x: (x[0] is None, x[0] if x[0] is not None else 0.0))
    return out


def histogram_quantile(q: float, buckets: dict) -> float | None:
    """标准 Prometheus histogram_quantile 线性插值；返回微秒。"""
    bounds = _bucket_bounds(buckets)
    if not bounds:
        return None
    total = bounds[-1][1]
    if total <= 0:
        return None
    rank = q * total
    prev_bound, prev_cum = 0.0, 0.0
    for bound, cum in bounds:
        if cum >= rank:
            if bound is None:  # 落在 +Inf 桶，退化为上一有限上界
                return prev_bound
            if cum > prev_cum:
                ratio = (rank - prev_cum) / (cum - prev_cum)
            else:
                ratio = 0.0
            return prev_bound + (bound - prev_bound) * ratio
        prev_bound = bound if bound is not None else prev_bound
        prev_cum = cum
    return None


# ============================================================================
# 主流程
# ============================================================================
def tail_logs(run_dir: str, lines: int = 20) -> str:
    """返回 run_dir/logs 下最新日志文件的尾部（诊断启动失败用）。"""
    log_dir = os.path.join(run_dir, "logs")
    if not os.path.isdir(log_dir):
        return "(无日志目录)"
    files = [os.path.join(log_dir, f) for f in os.listdir(log_dir)
             if os.path.isfile(os.path.join(log_dir, f))]
    if not files:
        return "(无日志文件)"
    newest = max(files, key=os.path.getmtime)
    try:
        with open(newest, encoding="utf-8", errors="replace") as fh:
            return f"({newest})\n" + "".join(fh.readlines()[-lines:])
    except OSError:
        return "(日志读取失败)"


def main() -> int:
    p = argparse.ArgumentParser(description="gateway-platform 性能基线采集")
    p.add_argument("--duration-min", type=float, default=5.0, help="运行时长（分钟）")
    p.add_argument("--points", type=int, default=500, help="点位数量")
    p.add_argument("--slaves", type=int, default=1,
                   help="Modbus 从站号数量（1..247）。>1 时打散地址、取消帧合并，每点各自成帧，用于压总线事务上限")
    p.add_argument("--buses", type=int, default=1,
                   help="总线数量（每条总线一条独立 TCP 连接，点位轮转分摊）。找拐点时用 >1 解除测试端单连接瓶颈")
    p.add_argument("--mock-procs", type=int, default=1,
                   help="内置 mock 从站进程数（第 i 条总线连到第 i%%N 个 mock 端口）。>1 时用多进程绕开 Python GIL 天花板")
    p.add_argument("--tcp-concurrent", type=int, default=8,
                   help="Modbus TCP 并发事务数（BusConfig.tcp_concurrent，默认 8）。高事务率下这是主要时延来源")
    p.add_argument("--interval-ms", type=int, default=1000, help="采集周期（毫秒，向下取整为秒，最小 1s）")
    p.add_argument("--sample-sec", type=float, default=5.0, help="采样周期（秒）")
    p.add_argument("--slave-addr", default="127.0.0.1:15020", help="Modbus TCP 从站基准地址 host:port（多 mock 时端口递增）")
    p.add_argument("--no-slave", action="store_true", help="不使用内置从站（指向真实设备）")
    p.add_argument("--mock-only", action="store_true",
                   help="仅启动内置 mock 从站并阻塞（供 --mock-procs 拉起子进程用，不对外暴露）")
    p.add_argument("--sweep-buses", default=None,
                   help="扫描模式：逗号分隔的总线数列表，如 \"1,4\"；与 --sweep-tcp 组成矩阵逐个跑并生成拐点报告")
    p.add_argument("--sweep-tcp", default="1,4,8,16,32",
                   help="扫描模式：逗号分隔的 tcp_concurrent 列表")
    p.add_argument("--web-addr", default="127.0.0.1:18080", help="gatewayd Web 监听地址 host:port")
    p.add_argument("--token", default="perf-baseline-token-00000001",
                   help="Web API Token（必须 >=16 字符，否则 gatewayd 拒绝启动 Web 服务）")
    p.add_argument("--config", default=None, help="使用已有配置（默认自动生成）")
    p.add_argument("--bin", default=None,
                   help=f"gatewayd 可执行文件路径（默认 <target>/release/{EXE_NAME}）")
    p.add_argument("--target", default=os.environ.get("CARGO_TARGET_DIR") or os.path.join(REPO_ROOT, "target"),
                   help="Cargo target 目录")
    p.add_argument("--features", default=None, help="附加 cargo feature（如 tracing-otlp）")
    p.add_argument("--no-build", action="store_true", help="跳过构建")
    p.add_argument("--drain", action="store_true",
                   help="启用内置模拟消费者（出队→ack）：长稳场景必须开，"
                        "否则 MQTT 关闭时队列只进不出、RSS 随积压单调上涨")
    p.add_argument("--output", default=os.path.join("docs", "perf-baseline.md"), help="Markdown 报告路径")
    p.add_argument("--json", dest="json_out", default=None, help="可选 JSON 结果路径")
    args = p.parse_args()

    # --mock-only：仅充当 mock 从站子进程，阻塞不退出（由 --mock-procs 拉起）。
    if args.mock_only:
        host, port_s = args.slave_addr.rsplit(":", 1)
        srv = ModbusTcpSlave(host, int(port_s))
        srv.start()
        print(f"[mock] listening on {args.slave_addr}", flush=True)
        try:
            while True:
                time.sleep(3600)
        except KeyboardInterrupt:
            pass
        finally:
            srv.stop()
        return 0

    # 扫描模式：跑 (buses × tcp) 矩阵并生成拐点报告。
    if args.sweep_buses:
        return run_sweep(args)

    run_dir = os.path.join(args.target, "perf-run")
    os.makedirs(run_dir, exist_ok=True)

    # ---- Step 1: 构建 ----
    exe = args.bin or os.path.join(args.target, "release", EXE_NAME)
    if not args.no_build:
        cmd = ["cargo", "build", "--release", "-p", "gatewayd"]
        if args.features:
            cmd += ["--features", args.features]
        print(f"[perf] building: {' '.join(cmd)}")
        r = subprocess.run(cmd, cwd=REPO_ROOT,
                           env={**os.environ, "CARGO_TARGET_DIR": args.target})
        if r.returncode != 0:
            print("[perf] build failed", file=sys.stderr)
            return 1
    if not os.path.exists(exe):
        print(f"[perf] gatewayd 不存在: {exe}", file=sys.stderr)
        return 1

    # ---- Step 2: 配置 ----
    cfg_path = args.config or os.path.join(run_dir, "perf-mock.json")
    base_host, base_port_s = args.slave_addr.rsplit(":", 1)
    base_port = int(base_port_s)
    n_mock = max(1, args.mock_procs)
    n_buses = max(1, args.buses)
    # 第 i 条总线连到第 i%N 个 mock 端口（多 mock 时端口 base_port..base_port+N-1）。
    bus_addrs = [f"{base_host}:{base_port + (i % n_mock)}" for i in range(n_buses)]
    if args.config is None:
        interval_sec = max(1, args.interval_ms // 1000)
        with open(cfg_path, "w", encoding="utf-8") as f:
            json.dump(build_config(args.points, interval_sec, args.web_addr, args.token,
                                   args.slaves, args.tcp_concurrent, bus_addrs,
                                   drain=args.drain), f, indent=2)

    # ---- Step 3: 内置从站（可选；每个 mock 一个独立子进程，绕开 GIL） ----
    mock_procs: list[subprocess.Popen] = []
    if not args.no_slave:
        mock_ports = [base_port + i for i in range(n_mock)]
        for port in mock_ports:
            mock_procs.append(subprocess.Popen(
                [sys.executable, os.path.abspath(__file__), "--mock-only",
                 "--slave-addr", f"{base_host}:{port}"],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
        if not wait_ports_ready(base_host, mock_ports, timeout=8.0):
            print("[perf] 内置从站启动失败（端口占用？）", file=sys.stderr)
            for mp in mock_procs:
                mp.terminate()
            return 1
        print(f"[perf] built-in modbus slave x{n_mock} on {base_host}:{mock_ports[0]}"
              f"..{mock_ports[-1]}  (buses={n_buses})")

    # ---- Step 4: 启动 gatewayd ----
    # 所有落盘路径指向 run_dir，避免污染仓库根目录
    env = {
        **os.environ,
        "GATEWAY_CONFIG_PATH": os.path.abspath(cfg_path),
        "GATEWAY_DB_PATH": os.path.join(run_dir, "data_queue.db"),
        "GATEWAY_OUTBOX_DB": os.path.join(run_dir, "mqtt_outbox.db"),
        "GATEWAY_VERSION_DB": os.path.join(run_dir, "config_versions.db"),
        "GATEWAY_LICENSE_CLOCK_DB": os.path.join(run_dir, "license_clock.db"),
        "GATEWAY_AUDIT_LOG": os.path.join(run_dir, "audit.log"),
        "GATEWAY_DIAG_LOG": os.path.join(run_dir, "gateway.log"),
        "GATEWAY_LOG_DIR": os.path.join(run_dir, "logs"),
        "GATEWAY_CERT_DIR": os.path.join(run_dir, "certs"),
        "GATEWAY_OTA_STAGING": os.path.join(run_dir, "ota"),
        "RUST_LOG": os.environ.get("RUST_LOG", "warn"),
    }
    out_path = os.path.join(run_dir, "gatewayd.out.log")
    err_path = os.path.join(run_dir, "gatewayd.err.log")
    with open(out_path, "w", encoding="utf-8") as fo, open(err_path, "w", encoding="utf-8") as fe:
        proc = subprocess.Popen([exe], cwd=REPO_ROOT, env=env, stdout=fo, stderr=fe)

    metrics_url = f"http://{args.web_addr}/api/metrics"
    duration_sec = args.duration_min * 60
    print(f"[perf] gatewayd pid={proc.pid}, 运行 {args.duration_min} 分钟, 采样 {args.sample_sec}s")

    snapshots: list[dict] = []
    t_start = time.time()
    try:
        # 等待就绪：轮询 /api/metrics 直到 Web 起来并能出数。
        # 数万点位的配置解析 + 建连接 + 首轮采集可能耗时十数秒，固定 sleep 会采到启动态，
        # 污染吞吐与延迟分位。就绪后再计时，使测量窗口只覆盖稳态。
        deadline = time.time() + max(30.0, duration_sec)
        while time.time() < deadline:
            if proc.poll() is not None:
                break
            if scrape_metrics(metrics_url, args.token) is not None:
                break
            time.sleep(0.5)
        if proc.poll() is not None:
            print(f"[perf] gatewayd 提前退出（code={proc.returncode}），日志：", file=sys.stderr)
            print(open(err_path, encoding="utf-8", errors="replace").read()[-2000:], file=sys.stderr)
            return 1
        t_start = time.time()

        first_cpu: float | None = None
        prev_cpu: float | None = None
        prev_t = t_start
        system_cpu(None)  # 预热 psutil 计数（首次调用返回 0）
        while time.time() - t_start < duration_sec:
            time.sleep(args.sample_sec)
            if proc.poll() is not None:
                print(f"[perf] gatewayd 中途退出（code={proc.returncode}）", file=sys.stderr)
                break
            now = time.time()
            cr = read_cpu_rss(proc.pid)
            sys_cpu_pct = system_cpu(None)
            text = scrape_metrics(metrics_url, args.token)
            gauges, buckets = parse_metrics(text) if text else ({}, {})

            cpu_pct = None
            rss_mb = None
            if cr is not None:
                cpu_s, rss = cr
                if first_cpu is None:
                    first_cpu = cpu_s
                rss_mb = rss / (1024 * 1024)
                if prev_cpu is not None and now > prev_t:
                    cpu_pct = (cpu_s - prev_cpu) / (now - prev_t) / CPU_COUNT * 100.0
                prev_cpu, prev_t = cpu_s, now

            snapshots.append({
                "t": now - t_start,
                "sys_cpu_pct": sys_cpu_pct,
                "collect_total": gauges.get("gateway_collect_total"),
                "failures": gauges.get("gateway_collect_failures"),
                "decode_errors": gauges.get("gateway_protocol_decode_errors_total"),
                "queue_depth": gauges.get("gateway_queue_depth"),
                "cpu_pct": cpu_pct,
                "rss_mb": rss_mb,
                "buckets": buckets,
            })
            print(f"[perf] t={now - t_start:6.1f}s cpu={cpu_pct if cpu_pct is None else round(cpu_pct,1)}% "
                  f"sys={sys_cpu_pct if sys_cpu_pct is None else round(sys_cpu_pct,1)}% "
                  f"rss={rss_mb if rss_mb is None else round(rss_mb,1)}MB "
                  f"collect={snapshots[-1]['collect_total']}")
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()
        for mp in mock_procs:
            mp.terminate()
        for mp in mock_procs:
            try:
                mp.wait(timeout=5)
            except subprocess.TimeoutExpired:
                mp.kill()

    if len(snapshots) < 2:
        print("[perf] 采样点不足，无法汇总", file=sys.stderr)
        return 1

    # 一次都没抓到 /api/metrics → 多半是 Web 未启动（如 api_token 过短、端口被占），
    # 打印网关日志尾部便于定位。
    if not any(s["collect_total"] is not None for s in snapshots):
        print("[perf] 未抓到 /api/metrics（Web 很可能未启动）。网关日志尾部：", file=sys.stderr)
        print(tail_logs(run_dir), file=sys.stderr)

    # ---- Step 5: 汇总 ----
    first, last = snapshots[0], snapshots[-1]
    span = last["t"] - first["t"]
    dc = (last["collect_total"] or 0) - (first["collect_total"] or 0)
    throughput = dc / span if span > 0 else 0.0

    cpu_vals = [s["cpu_pct"] for s in snapshots if s["cpu_pct"] is not None]
    sys_vals = [s["sys_cpu_pct"] for s in snapshots if s.get("sys_cpu_pct") is not None]
    rss_vals = [s["rss_mb"] for s in snapshots if s["rss_mb"] is not None]
    queue_peak = max([s["queue_depth"] or 0 for s in snapshots] or [0])
    rss_growth = (rss_vals[-1] - rss_vals[0]) if len(rss_vals) >= 2 else None
    rss_slope = rss_slope_mb_per_hour(snapshots)

    # 各采样点瞬时速率（相邻差分）
    rates = []
    for a, b in zip(snapshots, snapshots[1:]):
        if a["collect_total"] is not None and b["collect_total"] is not None:
            dt = b["t"] - a["t"]
            rates.append((b["t"], (b["collect_total"] - a["collect_total"]) / dt if dt > 0 else 0.0,
                          histogram_quantile(0.99, b["buckets"]) or 0.0))

    def ms(us):
        return None if us is None else round(us / 1000.0, 3)

    p50 = ms(histogram_quantile(0.50, last["buckets"]))
    p95 = ms(histogram_quantile(0.95, last["buckets"]))
    p99 = ms(histogram_quantile(0.99, last["buckets"]))

    # 告警判据（稳定性优先）
    warnings = []
    if (last["failures"] or 0) > 0:
        warnings.append(f"采集失败 {int(last['failures'])} 次（>0）")
    if (last["decode_errors"] or 0) > 0:
        warnings.append(f"协议解码错误 {int(last['decode_errors'])} 次（>0）")
    if throughput <= 0:
        warnings.append("吞吐为 0：从站不可达 / point.bus 与 bus.port_name 不匹配 / 配置无效")
    if rates and rates[0][1] > 0 and rates[-1][1] < rates[0][1] * 0.7:
        warnings.append(f"吞吐衰减：首样本 {rates[0][1]:.0f} → 末样本 {rates[-1][1]:.0f} 点/秒")
    if len(rss_vals) >= 2 and rss_vals[-1] > rss_vals[0] * 1.5 and rss_vals[-1] - rss_vals[0] > 50:
        warnings.append(f"RSS 增长 {rss_vals[0]:.0f}→{rss_vals[-1]:.0f} MB，疑似泄漏")

    def fmt(v, nd=2, dash="-"):
        return dash if v is None else (f"{v:.{nd}f}" if isinstance(v, float) else str(v))

    lines = []
    lines.append("# 性能基线报告\n")
    lines.append(f"生成时间：{time.strftime('%Y-%m-%d %H:%M:%S')}\n")
    lines.append("## 参数\n")
    lines.append(f"- 点位：{args.points}")
    lines.append(f"- Modbus 从站号数量：{max(1, min(args.slaves, 247))}")
    lines.append(f"- 总线数量：{n_buses}")
    lines.append(f"- TCP 并发事务数：{max(1, args.tcp_concurrent)}")
    lines.append(f"- 采集周期：{args.interval_ms} ms")
    lines.append(f"- 运行时长：{args.duration_min} min（采样 {args.sample_sec}s，共 {len(snapshots)} 点）")
    lines.append(f"- Modbus 从站：{args.slave_addr} +0..{n_mock - 1}"
                 f"{'（内置多进程）' if not args.no_slave else '（外部）'}")
    lines.append(f"- 二进制：{exe}")
    lines.append(f"- CPU 核数：{CPU_COUNT}\n")

    # 一轮耗时 = 点数 / 吞吐；若 < 采集周期则该轮被周期封顶，测不出总线真实上限。
    interval_sec = max(1, args.interval_ms // 1000)
    round_sec = (args.points / throughput) if throughput > 0 else None
    period_capped = round_sec is not None and round_sec <= interval_sec * 1.02

    lines.append("## 结果摘要\n")
    lines.append("| 指标 | 值 |")
    lines.append("| --- | --- |")
    lines.append(f"| 吞吐 | **{throughput:.1f} 点/秒** |")
    lines.append(f"| 一轮耗时 | {fmt(round_sec,2)} s（采集周期 {interval_sec}s）{'  ← 周期封顶' if period_capped else ''} |")
    lines.append(f"| 采集总次数 | {int(last['collect_total'] or 0)} |")
    lines.append(f"| 采集失败 | {int(last['failures'] or 0)} |")
    lines.append(f"| 协议解码错误 | {int(last['decode_errors'] or 0)} |")
    lines.append(f"| 队列深度峰值 | {queue_peak} |")
    lines.append(f"| 采集延迟 p50 / p95 / p99 | {fmt(p50)} / {fmt(p95)} / {fmt(p99)} ms |")
    lines.append(f"| CPU（相对核数）均值 / 峰值 | {fmt(sum(cpu_vals)/len(cpu_vals) if cpu_vals else None,1)} / {fmt(max(cpu_vals) if cpu_vals else None,1)} % |")
    lines.append(f"| 整机 CPU 均值 / 峰值 | {fmt(sum(sys_vals)/len(sys_vals) if sys_vals else None,1)} / {fmt(max(sys_vals) if sys_vals else None,1)} % |")
    lines.append(f"| RSS 均值 / 峰值 | {fmt(sum(rss_vals)/len(rss_vals) if rss_vals else None,1)} / {fmt(max(rss_vals) if rss_vals else None,1)} MB |")
    lines.append(f"| RSS 增长（首→末） | {fmt(rss_growth,1)} MB（拟合斜率 {fmt(rss_slope,1)} MB/h） |\n")

    lines.append("## 采样时间线\n")
    lines.append("| t(s) | 点/秒 | 队列深度 | CPU% | RSS(MB) | p99(ms) |")
    lines.append("| --- | --- | --- | --- | --- | --- |")
    rate_by_t = {round(t, 1): (r, q99) for t, r, q99 in rates}
    for s in snapshots:
        r, q99 = rate_by_t.get(round(s["t"], 1), (None, None))
        lines.append(f"| {s['t']:.1f} | {fmt(r,1)} | {s['queue_depth'] if s['queue_depth'] is not None else '-'} | "
                     f"{fmt(s['cpu_pct'],1)} | {fmt(s['rss_mb'],1)} | {fmt(ms(q99)) if q99 else '-'} |")

    lines.append("\n## 结论\n")
    if warnings:
        lines.append("> ⚠️ 发现以下问题：\n")
        for w in warnings:
            lines.append(f"- {w}")
    else:
        lines.append("未发现异常：零失败、零解码错误、吞吐稳定、RSS 无显著增长。")
    lines.append("\n> 火焰图：用外部工具（perf/cargo-flamegraph）对 release 构建的 gatewayd 采样，不依赖网关内建探针。")

    report = "\n".join(lines) + "\n"
    out_abs = os.path.join(REPO_ROOT, args.output) if not os.path.isabs(args.output) else args.output
    os.makedirs(os.path.dirname(out_abs) or ".", exist_ok=True)
    with open(out_abs, "w", encoding="utf-8") as f:
        f.write(report)
    print(f"[perf] 报告写入 {out_abs}")

    if args.json_out:
        with open(args.json_out, "w", encoding="utf-8") as f:
            json.dump({
                # 机型信息随结果落盘：CI runner（4 vCPU）与本机（24 核）的
                # 数字不可互相比较，没有这两个字段的基线 JSON 无法判定可比性。
                "cpu_count": CPU_COUNT,
                "platform": sys.platform,
                "samples": len(snapshots),
                "points": args.points,
                "slaves": max(1, min(args.slaves, 247)),
                "buses": n_buses,
                "tcp_concurrent": max(1, args.tcp_concurrent),
                "interval_ms": args.interval_ms,
                "duration_min": args.duration_min,
                "throughput_pts_per_sec": throughput,
                "round_sec": round_sec,
                "period_capped": period_capped,
                "collect_total": last["collect_total"],
                "failures": last["failures"],
                "decode_errors": last["decode_errors"],
                "queue_depth_peak": queue_peak,
                "latency_ms": {"p50": p50, "p95": p95, "p99": p99},
                "cpu_pct": {"avg": (sum(cpu_vals) / len(cpu_vals)) if cpu_vals else None,
                            "peak": max(cpu_vals) if cpu_vals else None},
                "sys_cpu_pct": {"avg": (sum(sys_vals) / len(sys_vals)) if sys_vals else None,
                                "peak": max(sys_vals) if sys_vals else None},
                "rss_mb": {"avg": (sum(rss_vals) / len(rss_vals)) if rss_vals else None,
                           "peak": max(rss_vals) if rss_vals else None},
                # 长稳判据用的两项：首末差给「涨了多少」，拟合斜率给「是否持续上涨」。
                "rss_growth_mb": rss_growth,
                "rss_mb_per_hour": rss_slope,
                "warnings": warnings,
            }, f, indent=2)

    if warnings:
        print("[perf] 警告：" + "；".join(warnings))
    return 0


# ============================================================================
# 扫描模式：跑 (buses × tcp_concurrent) 矩阵，定位吞吐拐点
# ============================================================================
def run_sweep(args) -> int:
    buses_list = [int(x) for x in args.sweep_buses.split(",") if x.strip()]
    tcp_list = [int(x) for x in args.sweep_tcp.split(",") if x.strip()]
    out_dir = os.path.join(args.target, "perf-sweep")
    os.makedirs(out_dir, exist_ok=True)
    script = os.path.abspath(__file__)
    exe = args.bin or os.path.join(args.target, "release", EXE_NAME)

    # 先统一构建一次，后续子档全部 --no-build，避免每档重复编译。
    if not args.no_build:
        print("[sweep] 构建 release gatewayd ...")
        r = subprocess.run(["cargo", "build", "--release", "-p", "gatewayd"],
                           cwd=REPO_ROOT, env={**os.environ, "CARGO_TARGET_DIR": args.target})
        if r.returncode != 0 or not os.path.exists(exe):
            print("[sweep] 构建失败", file=sys.stderr)
            return 1

    host, port_s = args.web_addr.rsplit(":", 1)
    base_web = int(port_s)
    total = len(buses_list) * len(tcp_list)
    rows: list[dict] = []
    idx = 0
    for buses in buses_list:
        for tcp in tcp_list:
            idx += 1
            jpath = os.path.join(out_dir, f"b{buses}_t{tcp}.json")
            mpath = os.path.join(out_dir, f"b{buses}_t{tcp}.md")
            cmd = [sys.executable, script,
                   "--points", str(args.points), "--slaves", str(args.slaves),
                   "--buses", str(buses), "--tcp-concurrent", str(tcp),
                   # 每条总线配一个独立 mock 进程：mock 线程数=buses，彻底绕开单进程 GIL 天花板。
                   "--mock-procs", str(buses),
                   "--duration-min", str(args.duration_min),
                   "--sample-sec", str(args.sample_sec),
                   "--interval-ms", str(args.interval_ms),
                   "--slave-addr", args.slave_addr,
                   "--web-addr", f"{host}:{base_web + idx}",
                   "--target", args.target, "--no-build",
                   "--output", mpath, "--json", jpath]
            print(f"\n[sweep] ({idx}/{total}) buses={buses} tcp={tcp}")
            r = subprocess.run(cmd, cwd=REPO_ROOT)
            if r.returncode != 0 or not os.path.exists(jpath):
                print(f"[sweep] buses={buses} tcp={tcp} 失败，跳过", file=sys.stderr)
                continue
            with open(jpath, encoding="utf-8") as f:
                rows.append(json.load(f))

    if not rows:
        print("[sweep] 无有效结果", file=sys.stderr)
        return 1

    report = build_sweep_report(args, rows, buses_list, tcp_list)
    out_abs = os.path.join(REPO_ROOT, args.output) if not os.path.isabs(args.output) else args.output
    os.makedirs(os.path.dirname(out_abs) or ".", exist_ok=True)
    with open(out_abs, "w", encoding="utf-8") as f:
        f.write(report)
    jall = os.path.join(out_dir, "sweep.json")
    with open(jall, "w", encoding="utf-8") as f:
        json.dump(rows, f, indent=2)
    print(f"\n[sweep] 报告写入 {out_abs}\n[sweep] 原始数据 {jall}")
    return 0


def build_sweep_report(args, rows: list[dict], buses_list: list[int],
                       tcp_list: list[int]) -> str:
    def f2(v):
        return "-" if v is None else f"{v:.2f}"

    def f1(v):
        return "-" if v is None else f"{v:.1f}"

    def rss(r):
        return ((r["rss_mb"]["avg"] or 0) if r.get("rss_mb") else 0)

    lines = []
    lines.append("# 网关性能拐点（knee）压测报告\n")
    lines.append(f"生成时间：{time.strftime('%Y-%m-%d %H:%M:%S')}\n")
    lines.append("## 方法\n")
    lines.append(f"- 点位：{args.points}（每点独立成帧，slaves={max(1, min(args.slaves, 247))} 打散地址、禁用帧合并）")
    lines.append("- 采集周期：1000 ms；每档运行 %.1f min、采样 %.0f s" % (args.duration_min, args.sample_sec))
    lines.append(f"- 维度：buses ∈ {buses_list}（每总线一条独立 TCP 连接，每总线一个独立 mock 进程）× "
                 f"tcp_concurrent ∈ {tcp_list}（单连接在途事务上限）")
    lines.append("- 判定口径：**一轮耗时 = 点数 / 吞吐**。一轮耗时 ≤ 采集周期时该档被周期封顶（测不出总线上限）；"
                 "吞吐随并发停涨 / 延迟分位陡增 / CPU 逼近饱和处即为拐点\n")

    lines.append("## 数据\n")
    lines.append("| 总线数 | tcp并发 | 吞吐(点/s) | 一轮(s) | 周期封顶 | p50(ms) | p95(ms) | p99(ms) | 网关CPU(核) | 整机CPU% | RSS(MB) | 失败 |")
    lines.append("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |")
    for buses in buses_list:
        group = sorted([r for r in rows if r["buses"] == buses], key=lambda r: r["tcp_concurrent"])
        for r in group:
            cpu_cores = (r["cpu_pct"]["avg"] or 0) / 100.0 * CPU_COUNT
            lat = r.get("latency_ms", {})
            lines.append(
                f"| {buses} | {r['tcp_concurrent']} | **{r['throughput_pts_per_sec']:.0f}** | "
                f"{f2(r.get('round_sec'))} | {'是' if r.get('period_capped') else '否'} | "
                f"{f2(lat.get('p50'))} | {f2(lat.get('p95'))} | {f2(lat.get('p99'))} | "
                f"{f1(cpu_cores)} | {f1((r.get('sys_cpu_pct') or {}).get('avg'))} | "
                f"{f1(rss(r))} | {int(r.get('failures') or 0)} |")

    # ---- 拐点判定 ----
    best = max(rows, key=lambda r: r["throughput_pts_per_sec"])
    lines.append("\n## 拐点判定\n")
    lines.append(f"- 全局最大吞吐：**{best['throughput_pts_per_sec']:.0f} 点/秒** "
                 f"@ buses={best['buses']}, tcp={best['tcp_concurrent']}"
                 f"（一轮 {f2(best.get('round_sec'))}s，CPU {(best['cpu_pct']['avg'] or 0)/100*CPU_COUNT:.1f} 核）")
    if not best.get("period_capped"):
        lines.append("  - 该档一轮耗时 > 采集周期，**未被周期封顶**，吞吐反映了真实采集能力。")
    else:
        lines.append("  - ⚠️ 该档一轮耗时 ≤ 采集周期，吞吐被周期封顶；提高点数才能继续压出上限。")

    for buses in buses_list:
        group = sorted([r for r in rows if r["buses"] == buses], key=lambda r: r["tcp_concurrent"])
        if len(group) < 2:
            continue
        # 第一个「相对前一档增益 < 5% 或周期封顶」的档位即该总线数下的拐点
        knee = None
        for prev, cur in zip(group, group[1:]):
            gain = cur["throughput_pts_per_sec"] / max(1.0, prev["throughput_pts_per_sec"])
            if cur.get("period_capped") or gain < 1.05:
                knee = cur
                break
        if knee is None:
            lines.append(f"- buses={buses}：吞吐随并发持续上升（未出现平台），拐点 tcp ≥ {group[-1]['tcp_concurrent']}。")
        else:
            lines.append(f"- buses={buses}：拐点约在 tcp={knee['tcp_concurrent']}"
                         f"（吞吐 {knee['throughput_pts_per_sec']:.0f} 点/秒"
                         f"{'，受周期封顶' if knee.get('period_capped') else ''}）。")

    # ---- 瓶颈归因 ----
    lines.append("\n## 瓶颈归因\n")
    peak_cpu_cores = max((r["cpu_pct"]["avg"] or 0) / 100.0 * CPU_COUNT for r in rows)
    sys_avg_peak = max(((r.get("sys_cpu_pct") or {}).get("avg") or 0) for r in rows)
    lines.append(f"- 全矩阵网关 CPU 峰值约 {peak_cpu_cores:.1f} / {CPU_COUNT} 核"
                 f"（占 {peak_cpu_cores/CPU_COUNT*100:.0f}%）；整机 CPU 峰值约 {sys_avg_peak:.0f}%。")
    cpu_saturated = peak_cpu_cores >= CPU_COUNT * 0.7
    machine_saturated = sys_avg_peak >= 85.0
    plateau_but_idle = any(
        max((r["cpu_pct"]["avg"] or 0) / 100.0 * CPU_COUNT for r in rows
            if r["buses"] == b) < CPU_COUNT * 0.5 for b in buses_list)
    if cpu_saturated:
        lines.append("- 判定：**网关自身 CPU 逼近饱和**，网关即瓶颈，拐点可信。")
    elif machine_saturated:
        lines.append(f"- 判定：整机 CPU 已近打满（约 {sys_avg_peak:.0f}%），**瓶颈在共享的测试机上**——"
                     "网关（及其内置多进程 mock）争抢同一台机器的 CPU。")
        lines.append(f"  - 该档网关自身仅用 ~{peak_cpu_cores:.1f}/{CPU_COUNT} 核，远未饱和；"
                     "结论是「网关吞吐 ≥ 本报告峰值」，真实上限需独立压测机才能测出。")
    elif plateau_but_idle:
        lines.append("- 判定：吞吐出现平台但 CPU 远未饱和 → **瓶颈在网关外部**（mock 从站 / 单机回环网络栈 / 调度周期），"
                     "网关本身仍未达上限；需增加 mock 进程或换独立压测机才能继续逼近。")
    else:
        lines.append("- 判定：未观察到明显平台，吞吐仍在上升，网关未达上限。")

    lines.append("\n## 结论\n")
    lines.append(f"- 在本机（{CPU_COUNT} 核）+ 内置多进程 mock 条件下，网关实测吞吐上限约 "
                 f"**{best['throughput_pts_per_sec']:.0f} 点/秒**，对应一轮 {f2(best.get('round_sec'))}s。")
    lines.append("- 各档零失败、零解码错误是拐点结论成立的前提；若某档出现失败/解码错误，该档数据不可用。")
    lines.append("\n> 说明：mock 从站为纯 Python 实现，即便多进程也存在上限；若网关吞吐停涨而 CPU 未饱和，"
                 "说明压力仍由测试端施加不足，真实上限不低于本报告数值。")
    return "\n".join(lines) + "\n"


if __name__ == "__main__":
    sys.exit(main())
