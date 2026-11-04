#!/usr/bin/env bash
#
# soak.sh —— scripts/soak.ps1 的 Linux/macOS 等价实现。
#
# 为什么要 .sh 版：仓库约定「同一脚本两侧逻辑等价」（.ps1 供 Windows、
# .sh 供 Linux/macOS/bash，见 scripts/README.md）。CI 跑在 ubuntu-latest 上，
# 没有 PowerShell，压测农场就只能靠 .sh 驱动——否则 soak 永远只能本地跑，
# 「跑三天内存涨到爆」这类现场事故要到发货后才暴露。
#
# 与 soak.ps1 的差异仅在两处平台相关实现，其余流程完全一致：
#   1. 进程采样：PowerShell 用 Get-Process 读 WorkingSet64/HandleCount；
#      这里读 /proc/<pid>/statm（RSS 实际驻留）与 /proc/<pid>/fd（句柄数）。
#   2. 时间戳/路径：Windows 用 [DateTimeOffset]::UtcNow 与 $PSScriptRoot，
#      这里用 date 与 dirname $0。
#
# 用法（环境变量与 soak.ps1 完全同名同义）：
#   SOAK_DURATION_SECS=3600 scripts/soak.sh          # 1 小时
#   SOAK_SKIP_RUN=1 scripts/soak.sh                   # 只对已有产物重新判定
#
# 退出码：0 判定通过；1 判定未通过（硬指标 fail）或运行阶段失败；2 前置缺失/编译失败。

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# 切到仓库根：编译 / 运行 / 判定三步都按 workspace 根解析，
# 且相对形式的 SOAK_OUT_DIR 也以此为基准，避免调用方 cwd 影响结果。
cd "$ROOT"

OUT_DIR="${SOAK_OUT_DIR:-$ROOT/gateway-core/soak-out}"
mkdir -p "$OUT_DIR"
OUT_DIR="$(cd "$OUT_DIR" && pwd)"

DURATION_SECS="${SOAK_DURATION_SECS:-86400}"
# 注意：Rust 侧读的是 SOAK_BUSES（见 gateway-core/tests/soak/spec.rs），
# 不是 SOAK_BUSSES——名字写错时 Rust 会静默回落到默认值 4，
# 跑出来的规模与预期不符且不报错，是最容易漏掉的一类偏差。
BUS_COUNT="${SOAK_BUSES:-4}"
SLAVES_PER_BUS="${SOAK_SLAVES_PER_BUS:-10}"
POINTS_PER_SLAVE="${SOAK_POINTS_PER_SLAVE:-50}"
INTERVAL_SECS="${SOAK_INTERVAL_SECS:-5}"
SAMPLE_MS="${SOAK_SAMPLE_MS:-10000}"
SKIP_RUN="${SOAK_SKIP_RUN:-0}"

say() { printf '[soak] %s\n' "$*"; }

# ---------- 0. 前置检查 ----------
if ! command -v cargo >/dev/null 2>&1; then
  echo "[soak] ✗ 找不到 cargo，请先安装 Rust 工具链" >&2
  exit 2
fi

# ---------- 1. 环境变量（Rust 侧读 SOAK_*，口径见 gateway-core/tests/soak/spec.rs） ----------
# 预热时长：未显式给出时交给 Rust 侧按「总时长 / 5（上限 1800s）」推导，
# 脚本不硬写默认值——两侧推导规则不同会导致判定基线漂移。
export SOAK_DURATION_SECS="$DURATION_SECS"
export SOAK_BUSES="$BUS_COUNT"
export SOAK_SLAVES_PER_BUS="$SLAVES_PER_BUS"
export SOAK_POINTS_PER_SLAVE="$POINTS_PER_SLAVE"
export SOAK_INTERVAL_SECS="$INTERVAL_SECS"
export SOAK_SAMPLE_MS="$SAMPLE_MS"
export SOAK_OUT_DIR="$OUT_DIR"
if [ -n "${SOAK_WARMUP_SECS:-}" ] && [ "$SOAK_WARMUP_SECS" -gt 0 ]; then
  export SOAK_WARMUP_SECS
fi

# 采样周期（进程级）：与 soak.ps1 一致，取 ceil(SAMPLE_MS / 1000) 秒。
SAMPLE_INTERVAL=$(( (SAMPLE_MS + 999) / 1000 ))
[ "$SAMPLE_INTERVAL" -lt 1 ] && SAMPLE_INTERVAL=1

say "输出目录 : $OUT_DIR"
say "规模     : ${BUS_COUNT} 总线 × ${SLAVES_PER_BUS} 从站 × ${POINTS_PER_SLAVE} 点 = $((BUS_COUNT * SLAVES_PER_BUS * POINTS_PER_SLAVE)) 点 / 周期 ${INTERVAL_SECS}s"
say "时长     : ${DURATION_SECS}s / 采样 ${SAMPLE_MS}ms"

# ---------- 2. 编译 ----------
# soak 用例整体 #[ignore]，需显式 --ignored；整体按 modbus feature 门控
#（gateway-core 的 default = ["all-protocols"] 含 modbus，默认构建即可编译）。
if [ "$SKIP_RUN" != "1" ]; then
  say "编译 soak 测试二进制 ..."
  if ! cargo test -p gateway-core --test soak --no-run; then
    echo "[soak] ✗ 编译失败" >&2
    exit 2
  fi
fi

# ---------- 3. 跑农场 ----------
# pid.txt 由 Rust 侧写入（runner.rs: std::process::id()），内容是**测试二进制**
# 的 PID（cargo test 会把它作为子进程拉起），因此采样目标就是它。
PID_FILE="$OUT_DIR/pid.txt"
PROC_FILE="$OUT_DIR/proc.jsonl"

now_ms() {
  local t
  t="$(date +%s%3N 2>/dev/null || true)"
  case "$t" in
    ''|*[!0-9]*) t="$(python3 -c 'import time; print(int(time.time()*1000))' 2>/dev/null || echo 0)" ;;
  esac
  printf '%s' "$t"
}

page_size() {
  local p
  p="$(getconf PAGESIZE 2>/dev/null || true)"
  case "$p" in
    ''|*[!0-9]*) p=4096 ;;
  esac
  printf '%s' "$p"
}
PAGE_SIZE="$(page_size)"

# 采一条进程级样本：/proc/<pid>/statm 第 2 个字段是常驻页数，
# /proc/<pid>/fd 的条目数即打开的文件描述符数（对应 Windows 的 HandleCount）。
sample_proc() {
  local pid="$1" ts rss_pages rss_bytes handles
  ts="$(now_ms)"
  [ -d "/proc/$pid" ] || return 1
  # 每条读取都单独兜底：被测进程可能在两次读取之间退出，
  # 此时 /proc/<pid> 消失，裸命令会让 set -e + pipefail 直接终止整个脚本。
  rss_pages="$(awk '{print $2}' "/proc/$pid/statm" 2>/dev/null || echo 0)"
  case "$rss_pages" in ''|*[!0-9]*) rss_pages=0 ;; esac
  rss_bytes=$((rss_pages * PAGE_SIZE))
  handles="$(ls -1 "/proc/$pid/fd" 2>/dev/null | wc -l | tr -d ' ' || true)"
  case "$handles" in ''|*[!0-9]*) handles=0 ;; esac
  printf '{"ts_ms":%s,"pid":%s,"rss_bytes":%s,"handles":%s}\n' \
    "$ts" "$pid" "$rss_bytes" "$handles" >> "$PROC_FILE"
}

if [ "$SKIP_RUN" != "1" ]; then
  rm -f "$PID_FILE" "$PROC_FILE"
  say "启动浸泡农场 ..."
  ( cd "$ROOT" && cargo test -p gateway-core --test soak -- \
      --ignored --exact soak_run --nocapture --test-threads=1 ) \
    > "$OUT_DIR/run.log" 2>&1 &
  RUN_PID=$!

  # 等 pid.txt 出现（与 soak.ps1 的 2 分钟截止一致）
  TARGET_PID=""
  for _ in $(seq 1 120); do
    if [ -s "$PID_FILE" ]; then
      TARGET_PID="$(tr -dc '0-9' < "$PID_FILE" | head -c 12)"
      [ -n "$TARGET_PID" ] && break
    fi
    kill -0 "$RUN_PID" 2>/dev/null || break
    sleep 0.5
  done

  if [ -z "$TARGET_PID" ]; then
    say "⚠ 未能读取 pid.txt，进程级指标将缺失（RSS / 句柄判定将被跳过）"
  else
    say "被测进程 PID = $TARGET_PID"
    while [ -d "/proc/$TARGET_PID" ]; do
      sample_proc "$TARGET_PID" || break
      sleep "$SAMPLE_INTERVAL"
    done
  fi

  set +e
  wait "$RUN_PID"
  RUN_RC=$?
  set -e
  say "运行阶段退出码 = $RUN_RC"
  tail -n 40 "$OUT_DIR/run.log" || true

  if [ ! -s "$OUT_DIR/metrics.jsonl" ]; then
    echo "[soak] ✗ 未生成 metrics.jsonl，浸泡农场未正常完成（见 run.log）" >&2
    exit 2
  fi
  if [ "$RUN_RC" -ne 0 ]; then
    echo "[soak] ✗ 运行阶段失败（退出码 $RUN_RC），仍尝试判定以保留现场信息" >&2
    exit "$RUN_RC"
  fi
fi

# ---------- 4. 判定 ----------
# 判定读 metrics.jsonl + proc.jsonl，可在跑完后离线重算，无需重跑农场。
say "开始判定 ..."
set +e
cargo test -p gateway-core --test soak -- \
  --ignored --exact soak_verdict --nocapture --test-threads=1
VERDICT_RC=$?
set -e

if [ -f "$OUT_DIR/report.md" ]; then
  say "报告已生成: $OUT_DIR/report.md"
fi

if [ "$VERDICT_RC" -ne 0 ]; then
  echo "[soak] ✗ 浸泡判定未通过" >&2
  exit 1
fi
say "✓ 浸泡判定通过"
exit 0
