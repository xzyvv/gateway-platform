#!/usr/bin/env bash
set -euo pipefail

# ────────────────────────────────────────────────────────────
# 工业网关部署后验证脚本
# 用法: sudo bash verify-deploy.sh [--port 8080] [--token xxx] [--build-info path] [--report path]
#                                  [--retention-signed]
#       --port/--host 省略时自动读 /etc/gateway/config.json 的 web.listen_addr
#       --retention-signed  数据留存边界已由客户签字确认（不放行则检查单打「未签字」并计 WARN）
#
# 在目标 Linux 上跑完 install.sh 后执行。
# 覆盖：systemd 服务（含开机自启）/ 环境变量 / 配置可解析性 / 时区 / 二进制兼容性 /
#       端口 / 健康检查 / API 契约 / 运行态配置 / 授权 / 前端静态 / 硬件能力
# 输出：彩色 PASS/FAIL 清单 + 最终总分 + Markdown 交付检查单（默认 deploy/out/）
# ────────────────────────────────────────────────────────────

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

PORT=""
TOKEN=""
HOST=""
# A4：出厂构建信息，默认取同仓库 deploy/out/BUILD-INFO.txt，用于核对协议裁剪
BUILD_INFO="$SCRIPT_DIR/out/BUILD-INFO.txt"
# 交付检查单：随安装包一起留档，验收时直接勾选签字
REPORT="$SCRIPT_DIR/out/delivery-checklist-$(date +%Y%m%d-%H%M%S).md"
# 数据留存边界是单机形态下的**必答项**：客户未签字确认「无异地副本」之前不得验收。
# 这里不用交互式提示（现场常由脚本批量执行），而是用显式开关记录签字事实：
# 不传 --retention-signed 时检查单会打成「未签字」并在清单里给一条 WARN。
RETENTION_SIGNED=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --port)  PORT="$2"; shift 2 ;;
    --port=*) PORT="${1#*=}"; shift ;;
    --token) TOKEN="$2"; shift 2 ;;
    --host)  HOST="$2"; shift 2 ;;
    --host=*) HOST="${1#*=}"; shift ;;
    --build-info) BUILD_INFO="$2"; shift 2 ;;
    --build-info=*) BUILD_INFO="${1#*=}"; shift ;;
    --report) REPORT="$2"; shift 2 ;;
    --report=*) REPORT="${1#*=}"; shift ;;
    --retention-signed) RETENTION_SIGNED=1; shift ;;
    *) shift ;;
  esac
done

# ── 生效配置路径 ──
# 默认与代码 paths::default_config_path 一致；服务里若用 GATEWAY_CONFIG_PATH 覆盖则以它为准。
# 必须先于端口探测解析：否则「配置被指到别处」的现场会用旧文件的 listen_addr 探测，
# 把装在 9000 上的正常网关判成未监听。
CFG_PATH="/etc/gateway/config.json"
SVC_ENV=$(systemctl show -p Environment --value gateway 2>/dev/null || true)
# 纯 bash 遍历，不走 `grep | head` 管道：脚本开着 pipefail，
# `head` 提前退出会让上游 grep 收到 SIGPIPE，导致「明明匹配到了却判为失败」。
svc_env_val() {
  local kv
  for kv in $SVC_ENV; do
    case "$kv" in
      "$1="*) printf '%s' "${kv#*=}"; return 0 ;;
    esac
  done
  return 1
}
svc_env_set() { svc_env_val "$1" >/dev/null; }
if svc_env_set "GATEWAY_CONFIG_PATH"; then
  CFG_PATH=$(svc_env_val "GATEWAY_CONFIG_PATH")
fi

# ── 监听地址：显式 --port/--host 优先，否则读生效配置文件 ──
# 自定义端口/绑定时若仍按 8080 探测，会把「装在 9000 上的正常网关」判成未监听。
# 通配绑定（0.0.0.0 / * / [::]）在本机验证一律走回环；IPv6 字面量保留方括号。
config_bind() {
  [ -f "$CFG_PATH" ] || return 0
  command -v python3 >/dev/null 2>&1 || return 0
  python3 -c 'import json, sys
try:
    d = json.load(open(sys.argv[1]))
except Exception:
    sys.exit(0)
a = (d.get("web") or {}).get("listen_addr") or ""
if not a:
    sys.exit(0)
host, sep, port = a.rpartition(":")
if not sep:
    sys.exit(0)
host = host.strip("[]")
if host in ("", "0.0.0.0", "*", "::"):
    host = "127.0.0.1"
elif ":" in host:
    host = "[" + host + "]"
print(host, port)' "$CFG_PATH" 2>/dev/null || true
}
if [ -z "$PORT" ] || [ -z "$HOST" ]; then
  BIND_FROM_CONFIG="$(config_bind)"
  if [ -n "$BIND_FROM_CONFIG" ]; then
    if [ -z "$HOST" ]; then HOST="${BIND_FROM_CONFIG% *}"; fi
    if [ -z "$PORT" ]; then PORT="${BIND_FROM_CONFIG##* }"; fi
  fi
fi
PORT="${PORT:-8080}"
HOST="${HOST:-127.0.0.1}"
BIND_ADDR=""
API="http://$HOST:$PORT"
PASS=0; FAIL=0; WARN=0
RESULTS=()

# ── 输出样式 ──
GREEN=$'\033[32m'; RED=$'\033[31m'; YELLOW=$'\033[33m'; CYAN=$'\033[36m'; BOLD=$'\033[1m'; RESET=$'\033[0m'
ok()   { echo "  ${GREEN}✓${RESET} $1"; PASS=$((PASS+1)); RESULTS+=("PASS $1"); }
fail() { echo "  ${RED}✗${RESET} $1"; FAIL=$((FAIL+1)); RESULTS+=("FAIL $1"); }
warn() { echo "  ${YELLOW}!${RESET} $1"; WARN=$((WARN+1)); RESULTS+=("WARN $1"); }
info() { echo "  ${CYAN}→${RESET} $1"; }

# ── 交付检查单：把屏幕上的 PASS/FAIL/WARN 落成可签字留档的 Markdown ──
# 之所以由本脚本导出而不是另写脚本：清单条目与判定逻辑只维护一份，
# 避免「页面说通过、检查单却漏项」。
write_report() {
  local verdict="$1"
  mkdir -p "$(dirname "$REPORT")" 2>/dev/null || true
  {
    echo "# 工业网关交付检查单"
    echo ""
    echo "- 生成时间：$(date -Is)"
    echo "- 目标地址：$API"
    echo "- 监听绑定：${BIND_ADDR:-（未取到）}"
    echo "- 生效时区：${TZ_LINE:-（未取到，请人工核对 journalctl -u gateway | grep 'device timezone'）}"
    echo "- 二进制链接：${LINK_KIND:-unknown}${BIN_GLIBC:+（要求 glibc ≥ $BIN_GLIBC）}${SYS_GLIBC:+，本机 glibc $SYS_GLIBC}"
    echo "- 设备时钟：${CLOCK_VERDICT:-（未取到）}"
    echo "- 主机：$(uname -s) / $(uname -m)"
    echo "- 结论：$verdict"
    echo "- 统计：PASS $PASS / FAIL $FAIL / WARN $WARN"
    if [ -f "$BUILD_INFO" ]; then
      echo ""
      echo "## 构建信息"
      echo ""
      echo '```'
      cat "$BUILD_INFO"
      echo '```'
    fi
    echo ""
    echo "## 检查清单"
    echo ""
    echo "| # | 结果 | 检查项 |"
    echo "|---|---|---|"
    local i=1 r tag item
    for r in "${RESULTS[@]}"; do
      tag="${r%% *}"
      item="${r#* }"
      case "$tag" in
        PASS) echo "| $i | ✅ PASS | $item |" ;;
        WARN) echo "| $i | ⚠️ WARN | $item |" ;;
        *)    echo "| $i | ❌ FAIL | $item |" ;;
      esac
      i=$((i + 1))
    done
    echo ""
    echo "## 数据留存与恢复边界（**必答项**，未逐项勾选并签字不得验收）"
    echo ""
    echo "本产品为**单机工业边缘网关**（无云平台、无双机热备），以下为形态边界，**不是缺陷**："
    echo ""
    echo "> 请逐项勾选。任一项未勾选，即视为客户尚未理解数据留存风险，不得进入验收。"
    echo ""
    echo "- [ ] 1. 历史数据的**唯一副本**在本机 \`${DB_PATH}\`，**无异地副本**。"
    echo "         设备磁盘物理损坏或整机丢失时，该时段历史数据**不可恢复**。"
    echo "- [ ] 2. 同盘备份 \`${DB_BAK}\` 可恢复 **SQLite 文件逻辑损坏**，"
    echo "         **不可**恢复磁盘物理损坏或整机丢失。"
    echo "- [ ] 3. 本产品**不做**：双机热备 / 高可用 / 集群 / 多租户 / 异地备份 / 云平台。"
    echo "- [ ] 4. 队列容量触顶时会按 FIFO 淘汰**最旧未送达**数据；磁盘剩余空间不足时会暂停入库。"
    echo "         客户已知悉并接受该淘汰策略。"
    echo "- [ ] 5. 需审计留存的场景，唯一出路是定期用 \`GET /api/history/export\` 导出到"
    echo "         **网关之外**的介质；落回本机不构成任何冗余。"
    echo ""
    echo "本机实况（由脚本探测，供勾选时核对）："
    echo ""
    echo "| 数据留存项 | 实测结论 |"
    echo "|---|---|"
    echo "| 数据队列库 | \`${DB_PATH}\`（${RET_DB_SIZE}） |"
    echo "| 同盘备份 | ${RET_BAK_LINE} |"
    echo "| 备份可恢复范围 | ${RET_BAK_SCOPE} |"
    echo "| 数据分区剩余 | ${RET_FREE_LINE} |"
    if [ "$RETENTION_SIGNED" -eq 1 ]; then
      echo "| 签字状态 | ✅ 已签字确认 |"
    else
      echo "| 签字状态 | ❌ **未签字**（本次以 WARN 计入清单） |"
    fi
    echo ""
    echo "客户确认：☐ 已逐项阅读并接受上述 5 条边界"
    echo ""
    echo "签字：________________　职务：________________　日期：____________"
    echo ""
    echo "详见 \`docs/architecture/data-flow.md\` §7.3 与 \`docs/architecture/constraints.md\` §11。"
    echo ""
    echo "## 处置建议"
    echo ""
    if [ "$FAIL" -eq 0 ]; then
      # 回环绑定不能写成「浏览器打开 http://<host>:PORT」——现场用 LAN 一定打不开
      case "$BIND_ADDR" in
        127.0.0.1:*|"[::1]:"*)
          echo "- 本机验证通过，但网关仅监听 $BIND_ADDR：远程浏览器目前打不开控制台。"
          echo "  需要 LAN 访问时改 /etc/gateway/config.json 的 web.listen_addr 为 0.0.0.0:$PORT，"
          echo "  同时设置 web.password_hash / web.api_token，然后 sudo systemctl restart gateway。"
          echo "- 上线前建议复查：浏览器打开 <http://$HOST:$PORT/> 只能在本机确认 Vue 控制台完整。"
          ;;
        *)
          echo "- 网关上线就绪：浏览器打开 <http://$HOST:$PORT/> 确认 Vue 控制台完整。"
          ;;
      esac
      echo "- 有硬件时：短接 DI0→+24V 验证 DI0 变 HIGH；插 Modbus 从站后新建设备跑发现向导。"
    else
      echo "失败项按顺序处置，常用诊断命令："
      echo ""
      echo '```bash'
      echo "sudo journalctl -u gateway -f"
      echo "ss -ltnp | grep gatewayd"
      echo "curl -v $API/health/live"
      echo '```'
    fi
    # 签字是独立于技术检查的验收闸门：技术上全过也不等于可以交付，
    # 因为「数据无异地副本」这件事只能靠客户书面确认来闭环。
    if [ "$RETENTION_SIGNED" -eq 0 ]; then
      echo ""
      echo "---"
      echo "**⚠️ 交付闸门未放行**：数据留存边界（上方 5 条）尚未签字确认。"
      echo "该项与技术检查结果无关，技术全 PASS 也不能跳过。"
      echo "客户签字后执行 \`sudo bash verify-deploy.sh --retention-signed\` 重新生成本检查单。"
    fi
  } > "$REPORT"
  echo ""
  echo "  ${CYAN}📄 交付检查单已导出: $REPORT${RESET}"
}

echo ""
echo "${BOLD}═══════════════════════════════════════════════${RESET}"
echo "${BOLD} 工业网关部署验证${RESET}"
echo "${BOLD}═══════════════════════════════════════════════${RESET}"
echo "目标: $API"
echo "时间: $(date -Is)"
echo ""

# ── 0. 前置 ──
if ! command -v curl &>/dev/null; then
  echo "❌ curl 未安装，无法发起 HTTP 请求"
  exit 1
fi

# ── 1. systemd 服务 ──
echo "${BOLD}[1/7] systemd 服务${RESET}"
if systemctl is-active --quiet gateway; then
  ok "gateway.service 运行中"
  # 「正在运行」不等于「开机自启」：现场断电重启后网关不起来，
  # 看起来像服务崩溃，实际是安装时漏了 enable。install.sh 已执行 enable，这里核对结果。
  if systemctl is-enabled --quiet gateway 2>/dev/null; then
    ok "gateway.service 已设开机自启"
  else
    fail "gateway.service 未设开机自启（is-enabled: $(systemctl is-enabled gateway 2>&1 || true)）"
    echo "    影响: 设备断电重启后网关不会自动启动"
    echo "    处置: sudo systemctl enable gateway"
  fi
  # 最近 500 条日志里的 panic / ERROR 数
  ERR_COUNT=$(journalctl -u gateway -n 500 --no-pager 2>/dev/null | grep -ci "panic\|ERROR\|FATAL" || true)
  if [ "$ERR_COUNT" -eq 0 ]; then
    ok "最近 500 条日志无 panic/ERROR"
  else
    warn "最近 500 条日志里有 $ERR_COUNT 条 ERROR（可能正常业务日志里的 error 级别，人工确认）"
  fi
else
  fail "gateway.service 未运行 (systemctl is-active: $(systemctl is-active gateway 2>&1 || true))"
  echo "    诊断: sudo journalctl -u gateway -n 50 --no-pager"
fi

# ── 1.5 时区（计划任务按墙钟解释 cron，时区错了就是整体差 8 小时）──
echo "${BOLD}[1b] 时区${RESET}"
TZ_LINE=$(journalctl -u gateway -n 200 --no-pager 2>/dev/null | grep -o "device timezone: .*" | tail -n 1 || true)
if [ -n "$TZ_LINE" ]; then
  ok "网关生效时区: ${TZ_LINE#device timezone: }"
else
  warn "未在日志中找到 \"device timezone:\"（服务刚启动或日志已轮转），请人工执行 journalctl -u gateway | grep 'device timezone' 核对"
fi
# 服务实际采用的时区偏移 vs 系统时区偏移：不一致说明存在静默偏移
SYS_OFF=$(date "+%z" 2>/dev/null || echo "")
if [ -n "$TZ_LINE" ]; then
  case "$TZ_LINE" in
    *"UTC+00:00"*|*"UTC-00:00"*)
      # 网关按 UTC 运行：只有在系统本身也是 UTC 时才正常
      case "$SYS_OFF" in
        "+0000"|"-0000"|"") ok "网关按 UTC 运行，与系统时区一致（$SYS_OFF）" ;;
        *) warn "网关按 UTC 运行，但系统时区偏移为 $SYS_OFF：计划任务「每天 02:00」会与现场预期错位；请在 gateway.service 加 Environment=TZ=<时区名> 或 Environment=GATEWAY_TZ=<+HH:MM> 后重启" ;;
      esac
      ;;
    *)
      ok "系统时区偏移 $SYS_OFF"
      ;;
  esac
else
  [ -n "$SYS_OFF" ] && info "系统时区偏移 $SYS_OFF（未能取到网关生效时区，无法比对）" || true
fi

# ── 1.5 二进制兼容性（静态链接 / glibc 版本）──
# 服务能起来不代表这个产物能装到别的设备上：gnu 动态链接的二进制会带上
# 构建机的 glibc 版本要求，换到更旧的发行版就直接 "GLIBC_2.xx not found"。
# 这里核对已安装产物，让「能装到哪些设备」变成可交付的结论，而不是现场试错。
echo "${BOLD}[1c] 二进制兼容性${RESET}"
GW_BIN="${GW_BIN:-/opt/gatewayd/gatewayd}"
if [ -x "$GW_BIN" ]; then
  LINK_KIND="unknown"
  if command -v file >/dev/null 2>&1; then
    case "$(file "$GW_BIN" 2>/dev/null)" in
      # x86_64 musl 默认 PIE，`file` 报 "static-pie linked"；非 PIE 报 "statically linked"
      *"statically linked"*|*"static-pie linked"*) LINK_KIND="static" ;;
      *"ELF"*) LINK_KIND="dynamic" ;;
    esac
  fi
  if [ "$LINK_KIND" = "static" ]; then
    ok "产物静态链接：不依赖设备 glibc，任意 Linux 发行版可运行"
  elif [ "$LINK_KIND" = "dynamic" ]; then
    BIN_GLIBC=""
    if command -v objdump >/dev/null 2>&1; then
      BIN_GLIBC=$(objdump -T "$GW_BIN" 2>/dev/null | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/^GLIBC_//' | sort -V | tail -n1 || true)
    elif command -v readelf >/dev/null 2>&1; then
      BIN_GLIBC=$(readelf --version-info "$GW_BIN" 2>/dev/null | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/^GLIBC_//' | sort -V | tail -n1 || true)
    fi
    SYS_GLIBC=$(ldd --version 2>/dev/null | head -n1 | grep -o '[0-9][0-9.]*[0-9]' | tail -n1 || true)
    if [ -n "$BIN_GLIBC" ] && [ -n "$SYS_GLIBC" ]; then
      # sort -V 比大小：较大者等于产物要求 → 设备 glibc 不够
      BIGGER=$(printf '%s\n%s\n' "$BIN_GLIBC" "$SYS_GLIBC" | sort -V | tail -n1)
      if [ "$BIGGER" = "$BIN_GLIBC" ] && [ "$BIN_GLIBC" != "$SYS_GLIBC" ]; then
        warn "产物要求 glibc ≥ $BIN_GLIBC，本机仅 $SYS_GLIBC：此包无法在本机运行；换设备前务必确认目标系统 glibc，或改用 musl 静态包（--target linux-*-musl）"
      else
        ok "动态链接，产物要求 glibc ≥ $BIN_GLIBC，本机 $SYS_GLIBC 满足；换到更旧的系统（Debian 10/11、Yocto）可能需静态包"
      fi
    else
      warn "无法解析 glibc 版本（缺 objdump/readelf/ldd），请人工确认目标系统 glibc"
    fi
  else
    warn "无法识别产物链接方式（缺 file 命令），请人工执行 file $GW_BIN 核对"
  fi
  # BUILD-INFO.txt 若随包提供，直接给出构建时的自检结论
  for f in "$SCRIPT_DIR/BUILD-INFO.txt" /opt/gatewayd/BUILD-INFO.txt; do
    if [ -f "$f" ]; then
      BI_LINK=$(grep -E '^link_mode=' "$f" 2>/dev/null | cut -d= -f2 || true)
      BI_GLIBC=$(grep -E '^glibc_min=' "$f" 2>/dev/null | cut -d= -f2 || true)
      if [ -n "$BI_LINK" ]; then
        info "构建信息: link_mode=${BI_LINK}, glibc_min=${BI_GLIBC:-none}"
      fi
      break
    fi
  done
else
  warn "未找到可执行产物 $GW_BIN，跳过二进制兼容性检查"
fi

# ── 1d. 时钟有效性 ──
# 计划任务按墙钟解释 cron：设备时间不对，任务要么在错误时刻执行、要么静默停摆。
# 无 RTC（无电池）的板子掉电后时间常回落到 1970/厂商默认时刻，网关会自动暂停
# 计划任务（见 gateway-api 调度器日志 "system clock not trustworthy"）。
# 这里从设备侧读时间，把「时间不可信」变成交付检查单上看得见的一条结论。
echo "${BOLD}[1d] 时钟有效性${RESET}"
DEV_EPOCH=$(date +%s 2>/dev/null || echo 0)
# 2020-01-01T00:00:00Z：任何真实部署都应晚于该时刻
MIN_EPOCH=1577836800
if [ "$DEV_EPOCH" -lt "$MIN_EPOCH" ]; then
  CLOCK_VERDICT="不可信（$(date -Is 2>/dev/null || echo "$DEV_EPOCH")，早于 2020-01-01；计划任务已暂停）"
  fail "设备系统时间为 $(date -Is 2>/dev/null || echo "$DEV_EPOCH")，早于 2020-01-01（疑似无 RTC 掉电回退）"
  echo "    影响: 计划任务已被网关自动暂停（不会在错误时刻执行），校时恢复后自动重排"
  echo "    处置: 配置 OS 级校时（systemd-timesyncd / chrony）或写入 RTC；仅网关自身 NTP 只记录偏差、不改系统时钟"
else
  CLOCK_VERDICT="正常（$(date -Is 2>/dev/null || echo "$DEV_EPOCH")）"
  ok "设备系统时间正常：$(date -Is 2>/dev/null || echo "$DEV_EPOCH")"
fi

# ── 1e. 服务环境变量 ──
# 环境变量缺失/写错不会让服务起不来，只会「静默走默认值」：
# 例如把变量名写成 GATEWAY_CONFIG（代码认的是 GATEWAY_CONFIG_PATH），
# 改配置路径的意图完全不生效，现场表现为「我明明改了配置，网关读的还是旧的」。
echo "${BOLD}[1e] 服务环境变量${RESET}"
# SVC_ENV / svc_env_val / svc_env_set 与 CFG_PATH 已在脚本开头解析（端口探测要用到）

if svc_env_set "GATEWAY_CONFIG"; then
  fail "服务环境里有 GATEWAY_CONFIG=$(svc_env_val GATEWAY_CONFIG)，但程序只读 GATEWAY_CONFIG_PATH —— 该设置不生效"
  echo "    处置: 改成 Environment=GATEWAY_CONFIG_PATH=... 后 daemon-reload + restart"
elif svc_env_set "GATEWAY_CONFIG_PATH"; then
  if [ -f "$CFG_PATH" ]; then
    ok "GATEWAY_CONFIG_PATH=$CFG_PATH（文件存在）"
  else
    fail "GATEWAY_CONFIG_PATH=$CFG_PATH 指向的文件不存在 —— 服务会改写成默认配置并重启丢失现场设置"
  fi
else
  info "未覆盖 GATEWAY_CONFIG_PATH，使用默认 $CFG_PATH"
fi

if svc_env_set "GATEWAY_TZ"; then
  ok "时区变量 GATEWAY_TZ=$(svc_env_val GATEWAY_TZ)"
elif svc_env_set "TZ"; then
  ok "时区变量 TZ=$(svc_env_val TZ)"
else
  warn "服务环境里既无 GATEWAY_TZ 也无 TZ：计划任务按系统时区解释；系统无 /etc/localtime 时会回退 UTC"
fi

if svc_env_set "GATEWAY_WEB_DIR"; then
  WEB_DIR=$(svc_env_val "GATEWAY_WEB_DIR")
  if [ -f "$WEB_DIR/index.html" ]; then
    ok "GATEWAY_WEB_DIR=$WEB_DIR（含 index.html）"
  else
    fail "GATEWAY_WEB_DIR=$WEB_DIR 下没有 index.html —— 控制台会退化为内置兜底页"
  fi
else
  info "未设 GATEWAY_WEB_DIR，按 WorkingDirectory 下的 ./ui-dist 查找前端"
fi

if svc_env_set "RUST_LOG"; then
  ok "RUST_LOG=$(svc_env_val RUST_LOG)"
else
  warn "未设 RUST_LOG：日志级别走默认，现场排障可能缺关键信息（建议 info）"
fi

# ── 1f. 配置文件可解析性 ──
# 「文件存在」不等于「能解析」。现场最常见的两类故障：
# 1) 手工编辑把 JSON 改坏（少逗号 / 中文引号 / 记事本写入 BOM）；
# 2) include 引用的分片文件被删或改名——主配置还在，展开时整份加载失败。
# 程序加载链路（gateway-core config::ConfigLoader）为
# expand_includes → schema → 反序列化 → migrate → 解密 → validate_all，
# 任一步失败都会让服务起不来或热重载失败。这里以无副作用的方式把前两类变成可见结论，
# 运行态是否真的解析成功由 [4c] 用 GET /api/config 反证。
echo "${BOLD}[1f] 配置文件可解析性${RESET}"
if [ ! -f "$CFG_PATH" ]; then
  fail "配置文件 $CFG_PATH 不存在"
else
  # BOM 必须像代码里的 strip_bom 一样先剥掉，否则会把「代码能读」的配置误判为坏 JSON
  CFG_VERDICT=$(python3 - "$CFG_PATH" <<'PY' 2>/dev/null || echo "NO_PYTHON|"
import glob, json, os, sys
path = sys.argv[1]
raw = open(path, "rb").read()
if raw.startswith(b"\xef\xbb\xbf"):
    raw = raw[3:]
try:
    data = json.loads(raw.decode("utf-8"))
except Exception as e:
    print("BAD_JSON|" + str(e))
    raise SystemExit
shards = data.get("include") or []
if isinstance(shards, str):
    shards = [shards]
base = os.path.dirname(os.path.abspath(path))
missing = []
for s in shards:
    if not isinstance(s, str) or not s:
        continue
    if any(ch in s for ch in "*?["):
        if not glob.glob(os.path.join(base, s)):
            missing.append(s + "(glob 无匹配)")
    elif not os.path.exists(os.path.join(base, s)):
        missing.append(s)
print(("SHARD_MISSING|" + ",".join(missing)) if missing else ("OK|" + str(len(shards))))
PY
)
  CFG_KIND="${CFG_VERDICT%%|*}"
  CFG_DETAIL="${CFG_VERDICT#*|}"
  case "$CFG_KIND" in
    OK)
      if [ "$CFG_DETAIL" = "0" ]; then
        ok "配置 JSON 语法正确（无 include 分片）"
      else
        ok "配置 JSON 语法正确，include 引用的 $CFG_DETAIL 个分片文件都存在"
      fi
      ;;
    BAD_JSON)
      fail "配置 JSON 解析失败：$CFG_DETAIL"
      echo "    影响: 网关重启/热重载会直接失败（不会拿着坏配置启动）"
      echo "    处置: python3 -m json.tool $CFG_PATH 定位行号，修好后 sudo systemctl restart gateway"
      ;;
    SHARD_MISSING)
      fail "include 引用的分片文件缺失：$CFG_DETAIL"
      echo "    影响: 加载时 expand_includes 报错，网关启动/热重载失败"
      ;;
    *)
      warn "无法解析 $CFG_PATH（缺 python3 或读取失败），请人工执行 python3 -m json.tool $CFG_PATH"
      ;;
  esac
fi

# ── 1g. 数据留存与恢复边界 ──
# 本产品形态是**单机边缘网关**（无云平台、无双机热备）：历史数据的唯一副本
# 在本机 data_queue.db，没有异地副本。这不是缺陷，是形态的必然结果，
# 但客户默认会以为"系统有备份"——所以必须让它变成检查单上看得见的一条结论。
# 这里核的是「当前这台机器的实际状态」，边界声明本身写进检查单的固定段落。
echo "${BOLD}[1g] 数据留存${RESET}"
# 默认路径必须与代码 default_db_path（gateway-core/src/support/paths.rs）逐字一致，
# 否则脚本会检查一个根本不存在的库。deploy_lint C21 会核对这一点。
DB_PATH="${GATEWAY_DB_PATH:-/var/lib/gateway/data_queue.db}"
# 路径可被 GATEWAY_DB_PATH 覆盖，与代码 default_db_path 一致
if svc_env_set "GATEWAY_DB_PATH"; then
  DB_PATH=$(svc_env_val "GATEWAY_DB_PATH")
fi
# 备份文件由代码 with_extension("db.bak") 派生：data_queue.db → data_queue.db.bak
DB_BAK="${DB_PATH%.*}.db.bak"
# 报告段落要用到的事实，先全部留空再按实际探测结果填充，
# 避免「探测失败时报告里沿用上一次循环的脏值」。
RET_DB_SIZE="（未取到）"
RET_BAK_LINE="未找到"
RET_BAK_SCOPE="（未取到）"
RET_FREE_MB=""
RET_FREE_LINE="（未取到）"
RET_STATE="unknown"
if [ ! -f "$DB_PATH" ]; then
  warn "数据队列库 $DB_PATH 不存在 —— 若尚未采集过数据属正常；若无数据且已上线，请检查 GATEWAY_DB_PATH 是否被误配"
else
  DB_SIZE=$(du -h "$DB_PATH" 2>/dev/null | awk '{print $1}' || echo "?")
  RET_DB_SIZE="$DB_SIZE"
  RET_STATE="present"
  ok "数据队列库存在：$DB_PATH（$DB_SIZE）"
  # 同盘备份：能救 SQLite 逻辑损坏，救不回磁盘物理损坏 —— 这条区别必须让客户知道
  if [ -f "$DB_BAK" ]; then
    BAK_SIZE=$(du -h "$DB_BAK" 2>/dev/null | awk '{print $1}' || echo "?")
    ok "备份文件存在：$DB_BAK（$BAK_SIZE）"
    RET_BAK_LINE="存在（$BAK_SIZE）"
    DB_DIR_PATH=$(dirname "$DB_PATH")
    BAK_DIR_PATH=$(dirname "$DB_BAK")
    if [ "$DB_DIR_PATH" = "$BAK_DIR_PATH" ]; then
      warn "备份与主库位于同一分区（$DB_DIR_PATH）：可恢复 SQLite 逻辑损坏，**不可**恢复磁盘物理损坏或整机丢失；需审计留存的客户应定期用 GET /api/history/export 导出到网关之外"
      RET_BAK_SCOPE="仅逻辑损坏（与主库同分区，无冗余）"
    else
      ok "备份与主库不在同一分区（$BAK_DIR_PATH vs $DB_DIR_PATH）"
      RET_BAK_SCOPE="仅逻辑损坏（跨分区，仍非异地副本）"
    fi
  else
    warn "未找到备份文件 $DB_BAK —— 启动时若主库逻辑损坏将无法自动恢复；网关会在首次写入后生成该备份"
    RET_BAK_SCOPE="无备份文件"
  fi
  # 数据分区剩余空间：队列满了会 FIFO 丢最旧数据，磁盘满会触发退避
  DB_PART=$(df -P "$DB_PATH" 2>/dev/null | tail -1 | awk '{print $4}' || echo "")
  if [ -n "$DB_PART" ] && [ "$DB_PART" -gt 0 ] 2>/dev/null; then
    DB_FREE_MB=$((DB_PART / 1024))
    RET_FREE_MB="$DB_FREE_MB"
    RET_FREE_LINE="${DB_FREE_MB} MB"
    if [ "$DB_FREE_MB" -lt 512 ]; then
      warn "数据分区剩余空间仅 ${DB_FREE_MB}MB —— 队列满后会按 FIFO 丢弃最旧数据，磁盘满还会触发退避；请清理或扩容"
    else
      ok "数据分区剩余空间充足（${DB_FREE_MB}MB）"
    fi
  else
    info "未能取到 $DB_PATH 所在分区剩余空间，跳过空间检查"
  fi
fi
# 必答项：单机形态下「无异地副本」是产品边界，必须由客户书面确认，
# 否则事后任何一次数据丢失都会变成责任争议。未签字一律按 WARN 计入清单。
if [ "$RETENTION_SIGNED" -eq 1 ]; then
  ok "数据留存边界已由客户签字确认（--retention-signed）"
else
  warn "数据留存边界【必答项】尚未签字确认 —— 单机网关历史数据无异地副本，验收前必须由客户在检查单上逐项勾选并签字；确认后可用 --retention-signed 重新生成本检查单"
fi

# ── 2. 端口监听 ──
# 同时取本地绑定地址：仅回环说明「本机验证能过，但车间 LAN 打不开」，
# 检查单不能再写「浏览器打开 http://<host>:PORT」而不加限定。
echo "${BOLD}[2/7] 端口监听${RESET}"
if command -v ss >/dev/null 2>&1; then
  BIND_ADDR=$(ss -ltn 2>/dev/null | awk -v p="$PORT" '$4 ~ (":" p "$") {print $4; exit}' || true)
elif command -v netstat >/dev/null 2>&1; then
  BIND_ADDR=$(netstat -ltn 2>/dev/null | awk -v p="$PORT" '$4 ~ (":" p "$") {print $4; exit}' || true)
fi
case "$BIND_ADDR" in
  "") fail "TCP $PORT 未监听" ;;
  127.0.0.1:*|"[::1]:"*)
    warn "TCP $PORT 仅监听回环（$BIND_ADDR）：本机验证可用，但车间 LAN 浏览器打不开；需把 config.json 的 web.listen_addr 设为 0.0.0.0:$PORT 后重启" ;;
  0.0.0.0:*|"*:"*|"[::]:"*|":::"*)
    ok "TCP $PORT 已监听（$BIND_ADDR，全部网卡）" ;;
  *)
    ok "TCP $PORT 已监听（$BIND_ADDR，请确认车间网段可达该地址）" ;;
esac

# ── 3. 根健康检查（不需要 token） ──
echo "${BOLD}[3/7] 健康检查端点${RESET}"
HTTP_OK=0
http_code() { curl -s -o /dev/null -w "%{http_code}" "$1" 2>/dev/null || echo "000"; }

CODE=$(http_code "$API/health/live")
[ "$CODE" = "200" ] && ok "GET /health/live → 200" || fail "GET /health/live → $CODE (期望 200)"

CODE=$(http_code "$API/ok")
[ "$CODE" = "200" ] && ok "GET /ok → 200" || fail "GET /ok → $CODE (期望 200)"

# /health/ready 需要 api_token 可能（看配置 allow_anon_read）
CODE=$(http_code "$API/health/ready")
if [ "$CODE" = "200" ]; then
  ok "GET /health/ready → 200"
elif [ "$CODE" = "401" ] || [ "$CODE" = "403" ]; then
  warn "GET /health/ready → $CODE (需要 api_token，后续用 token 再测)"
else
  fail "GET /health/ready → $CODE (期望 200 或 401)"
fi

# /boom 是故意 panic 测试点（有 service 的 --help 提到）
CODE=$(http_code "$API/boom")
[ "$CODE" = "500" ] && ok "GET /boom → 500 (故意 panic 端点正常)" || warn "GET /boom → $CODE (非 500，panic handler 可能被禁用)"

# ── 4. API 端点（带 token） ──
echo "${BOLD}[4/7] API 端点契约${RESET}"
AUTH_HEADER=()
if [ -n "$TOKEN" ]; then
  AUTH_HEADER=(-H "Authorization: Bearer $TOKEN")
  info "使用提供的 api_token"
else
  info "未提供 --token，尝试 config.json 里读取..."
  if [ -f /etc/gateway/config.json ]; then
    TOKEN=$(python3 -c "import json,sys; print(json.load(open('/etc/gateway/config.json')).get('web',{}).get('api_token',''))" 2>/dev/null || true)
    if [ -n "$TOKEN" ]; then
      AUTH_HEADER=(-H "Authorization: Bearer $TOKEN")
      ok "从 /etc/gateway/config.json 读取到 api_token"
    else
      warn "config.json 里 api_token 为空，跳过需要鉴权的端点测试"
    fi
  else
    warn "config.json 不存在，跳过鉴权端点测试"
  fi
fi

# ── 4b. 网关校时状态 ──
# 设备时间对得上还不够：网关自己的校时结果决定「计划任务保护」是否处于暂停状态。
# 放这里是因为 /api/ntp/status 需要鉴权（token 在上一段才准备好）。
echo "${BOLD}[4b] 网关校时状态${RESET}"
NTP_JSON=$(curl -fsS --max-time 5 "${AUTH_HEADER[@]}" "$API/api/ntp/status" 2>/dev/null || true)
if [ -n "$NTP_JSON" ]; then
  NTP_STATE=$(printf '%s' "$NTP_JSON" | grep -o '"status":"[^"]*"' | cut -d'"' -f4 || true)
  NTP_SRC=$(printf '%s' "$NTP_JSON" | grep -o '"source":"[^"]*"' | cut -d'"' -f4 || true)
  NTP_OFF=$(printf '%s' "$NTP_JSON" | grep -o '"offset_ms":[-0-9]*' | cut -d: -f2 || true)
  # 网关直接下发时钟可信结论（与调度器同一判据），优先于本地按偏差猜测；
  # 旧版本网关无该字段，回退到「synced + 偏差」推断。
  NTP_TRUST=$(printf '%s' "$NTP_JSON" | grep -o '"clock_trusted":[a-z]*' | cut -d: -f2 || true)
  NTP_REASON=$(printf '%s' "$NTP_JSON" | grep -o '"clock_untrust_reason":"[^"]*"' | cut -d'"' -f4 || true)
  if [ "$NTP_TRUST" = "false" ]; then
    warn "系统时钟不可信（${NTP_REASON:-原因未下发}）：计划任务已暂停，校时恢复后自动重排"
    CLOCK_VERDICT="${CLOCK_VERDICT:-正常}；系统时钟不可信（计划任务暂停）"
    CLOCK_TRUST_DONE=1
  elif [ "$NTP_TRUST" = "true" ]; then
    ok "系统时钟可信（计划任务正常运行）"
    CLOCK_TRUST_DONE=1
  else
    CLOCK_TRUST_DONE=0
  fi
  NTP_ABS_OFF=${NTP_OFF:-0}
  if [ "$NTP_ABS_OFF" -lt 0 ] 2>/dev/null; then NTP_ABS_OFF=$((-NTP_ABS_OFF)); fi
  case "$NTP_STATE" in
    synced)
      if [ "$NTP_ABS_OFF" -gt 300000 ] 2>/dev/null; then
        warn "网关 NTP 偏差 ${NTP_OFF} ms（>5 分钟）：系统时钟不可信，计划任务处于暂停状态"
        if [ "${CLOCK_TRUST_DONE:-0}" = "0" ]; then
          CLOCK_VERDICT="${CLOCK_VERDICT:-正常}；NTP 偏差 ${NTP_OFF} ms（计划任务暂停）"
        fi
      else
        ok "网关 NTP 已同步（来源 ${NTP_SRC:-NTP}，偏差 ${NTP_OFF:-0} ms）"
      fi
      ;;
    "")
      warn "未取到 /api/ntp/status 的 status 字段，无法判断网关校时状态"
      ;;
    *)
      warn "网关 NTP 状态为 ${NTP_STATE}（来源 ${NTP_SRC:-未知}）：若设备时间本身不可信，计划任务会持续暂停"
      ;;
  esac
else
  warn "未取到 /api/ntp/status（可能需要 api_token），无法判断网关校时状态"
fi

# ── 4c. 运行态配置一致性 ──
# /api/config 能返回，说明进程内的配置对象是**程序自己解析成功**的
# （启动时已走完 expand_includes → schema → 反序列化 → migrate → 解密 → validate_all）。
# 但磁盘文件若在服务启动后被改过，运行态就没包含这些改动——「我改了配置没生效」正是如此。
# /proc/<pid> 的 mtime 即进程创建时间，与 config.json 的 mtime 比对，全程无副作用。
echo "${BOLD}[4c] 运行态配置${RESET}"
CFG_CODE=$(curl -s -o /dev/null -w "%{http_code}" "${AUTH_HEADER[@]}" "$API/api/config" 2>/dev/null || echo "000")
if [ "$CFG_CODE" = "200" ]; then
  ok "GET /api/config → 200（当前进程配置已成功解析）"
  MAIN_PID=$(systemctl show -p MainPID --value gateway 2>/dev/null || true)
  PROC_START=$(stat -c %Y "/proc/$MAIN_PID" 2>/dev/null || true)
  CFG_MTIME=$(stat -c %Y "$CFG_PATH" 2>/dev/null || true)
  if [ -n "$PROC_START" ] && [ -n "$CFG_MTIME" ] && [ "$CFG_MTIME" -gt "$PROC_START" ] 2>/dev/null; then
    warn "磁盘配置在服务启动后被改动（晚于进程启动 $((CFG_MTIME - PROC_START))s）：运行态未包含这些改动，也可能解析失败"
    echo "    处置: sudo systemctl restart gateway（或调用配置热重载接口）后重跑本脚本确认"
  elif [ -n "$PROC_START" ] && [ -n "$CFG_MTIME" ]; then
    ok "运行态配置与磁盘一致（配置文件最后修改早于服务启动）"
  else
    info "未能取到进程启动时间或配置 mtime，跳过一致性比对"
  fi
elif [ "$CFG_CODE" = "401" ] || [ "$CFG_CODE" = "403" ]; then
  warn "GET /api/config → $CFG_CODE（无 token，无法读取运行态配置）"
else
  fail "GET /api/config → $CFG_CODE（期望 200；服务读不到运行态配置）"
fi

# ── 4d. 授权状态 ──
# 授权失效不会让服务报错——程序静默降级到 absent 兜底配额继续跑，现场表现为
# 「点数上限变小」却没有任何告警。这里把降级原因落成检查单结论。
# 注意 GET /api/license 返回的是扁平 JSON（不是 ApiResponse 信封），且需要 token。
echo "${BOLD}[4d] 授权状态${RESET}"
LIC_JSON=$(curl -fsS --max-time 5 "${AUTH_HEADER[@]}" "$API/api/license" 2>/dev/null || true)
if [ -z "$LIC_JSON" ]; then
  warn "未取到 /api/license（需要 api_token），无法确认授权状态"
else
  LIC_STATUS=$(printf '%s' "$LIC_JSON" | grep -o '"status":"[^"]*"' | cut -d'"' -f4 || true)
  LIC_TIER=$(printf '%s' "$LIC_JSON" | grep -o '"tier":"[^"]*"' | cut -d'"' -f4 || true)
  LIC_SRC=$(printf '%s' "$LIC_JSON" | grep -o '"quota_source":"[^"]*"' | cut -d'"' -f4 || true)
  LIC_PRESENT=$(printf '%s' "$LIC_JSON" | grep -o '"present":[a-z]*' | cut -d: -f2 || true)
  LIC_EXP=$(printf '%s' "$LIC_JSON" | grep -o '"expires_at":[0-9]*' | cut -d: -f2 || true)
  LIC_EXP_TXT=""
  if [ -n "$LIC_EXP" ]; then
    LIC_EXP_TXT="，到期 $(date -d "@$LIC_EXP" +%Y-%m-%d 2>/dev/null || echo "$LIC_EXP")"
  fi
  case "$LIC_STATUS" in
    ok)
      if [ "$LIC_PRESENT" = "false" ]; then
        ok "未安装授权文件，按 absent 兜底配额运行（tier=${LIC_TIER:-core}）"
      else
        ok "授权有效（tier=${LIC_TIER:-?}$LIC_EXP_TXT）"
      fi
      ;;
    expiring)
      warn "授权即将到期$LIC_EXP_TXT（tier=${LIC_TIER:-?}）：到期后自动降级到 absent 兜底配额" ;;
    expired|mismatch|tampered|invalid)
      fail "授权无效（status=$LIC_STATUS$LIC_EXP_TXT）：已降级到 absent 兜底配额，点数与协议数受限"
      echo "    说明: mismatch=授权与本机设备码不符 / tampered=文件被改动 / invalid=格式或签名无效"
      echo "    处置: GET /api/license 取 machine.machine_key → 向供应商重新签发 → POST /api/license/import"
      ;;
    "")
      warn "未解析到授权状态字段，请人工查看 GET /api/license" ;;
    *)
      warn "授权状态为 $LIC_STATUS（未知取值）" ;;
  esac
  if [ "$LIC_SRC" = "tier_missing" ]; then
    warn "配额来源 tier_missing：档位 ${LIC_TIER:-?} 未在配置 tier_quotas 中定义，配额取自代码内置默认表（如需按现场调整请补齐该档位）"
  fi
fi

if [ -n "$TOKEN" ]; then
  # 核心状态端点
  CODE=$(curl -s -o /dev/null -w "%{http_code}" "${AUTH_HEADER[@]}" "$API/api/auth/whoami" 2>/dev/null || echo "000")
  [ "$CODE" = "200" ] && ok "GET /api/auth/whoami → 200" || fail "GET /api/auth/whoami → $CODE"

  CODE=$(curl -s -o /dev/null -w "%{http_code}" "${AUTH_HEADER[@]}" "$API/api/devices" 2>/dev/null || echo "000")
  [ "$CODE" = "200" ] && ok "GET /api/devices → 200" || fail "GET /api/devices → $CODE"

  # 点位：没有全局 GET /api/points（点位按设备挂载：GET /api/devices/:id/points），
  # 这里用全局可用的回收站列表做契约探针——原来打 /api/points 恒 404，会让每次交付都判 FAIL。
  CODE=$(curl -s -o /dev/null -w "%{http_code}" "${AUTH_HEADER[@]}" "$API/api/point-trash" 2>/dev/null || echo "000")
  [ "$CODE" = "200" ] && ok "GET /api/point-trash → 200" || fail "GET /api/point-trash → $CODE"

  CODE=$(curl -s -o /dev/null -w "%{http_code}" "${AUTH_HEADER[@]}" "$API/api/io/status" 2>/dev/null || echo "000")
  if [ "$CODE" = "200" ]; then
    ok "GET /api/io/status → 200"
    # 解析 JSON 结构
    IO_JSON=$(curl -s "${AUTH_HEADER[@]}" "$API/api/io/status" 2>/dev/null || echo "{}")
    # 响应是 {code,data,message} 信封，di/do_state/di_quality 在 data 下
    if echo "$IO_JSON" | python3 -c "import json,sys; d=json.load(sys.stdin); d=d.get('data',d) if isinstance(d,dict) else d; assert 'di' in d and 'do_state' in d and 'di_quality' in d and len(d['di'])==4 and len(d['do_state'])==4" 2>/dev/null; then
      ok "GPIO 状态 JSON 结构正确 (di[4] + di_quality[4] + do_state[4])"
    else
      fail "GPIO 状态 JSON 结构不正确: $(echo "$IO_JSON" | head -c 200)"
    fi
  elif [ "$CODE" = "401" ] || [ "$CODE" = "403" ]; then
    warn "GET /api/io/status → $CODE (无权限)"
  else
    fail "GET /api/io/status → $CODE"
  fi

  # 第 1 期冗余收口：原 /api/north/servers 已删除，Server 存活列表并入 /api/north/status
  CODE=$(curl -s -o /dev/null -w "%{http_code}" "${AUTH_HEADER[@]}" "$API/api/north/status" 2>/dev/null || echo "000")
  [ "$CODE" = "200" ] && ok "GET /api/north/status → 200" || fail "GET /api/north/status → $CODE"

  CODE=$(curl -s -o /dev/null -w "%{http_code}" "${AUTH_HEADER[@]}" "$API/api/health/disk/history" 2>/dev/null || echo "000")
  [ "$CODE" = "200" ] && ok "GET /api/health/disk/history → 200" || fail "GET /api/health/disk/history → $CODE"

  CODE=$(curl -s -o /dev/null -w "%{http_code}" "${AUTH_HEADER[@]}" "$API/api/templates" 2>/dev/null || echo "000")
  [ "$CODE" = "200" ] && ok "GET /api/templates → 200" || fail "GET /api/templates → $CODE"

  # 计划任务在 access 命名空间下：GET /api/access/scheduled-tasks
  CODE=$(curl -s -o /dev/null -w "%{http_code}" "${AUTH_HEADER[@]}" "$API/api/access/scheduled-tasks" 2>/dev/null || echo "000")
  [ "$CODE" = "200" ] && ok "GET /api/access/scheduled-tasks → 200" || fail "GET /api/access/scheduled-tasks → $CODE"

  # A4：协议裁剪一致性（BUILD-INFO.txt 的 protocols= 与线上 /api/protocols 比对）
  # 出厂镜像按协议清单裁剪后，线上注册的协议应恰好等于构建清单，多/少都说明发布物不对。
  CODE=$(curl -s -o /dev/null -w "%{http_code}" "${AUTH_HEADER[@]}" "$API/api/protocols" 2>/dev/null || echo "000")
  if [ "$CODE" != "200" ]; then
    warn "GET /api/protocols → $CODE，跳过协议裁剪一致性检查"
  elif [ ! -f "$BUILD_INFO" ]; then
    warn "未找到构建信息 $BUILD_INFO，跳过协议裁剪一致性检查（可用 --build-info 指定）"
  else
    PROTO_JSON=$(curl -s "${AUTH_HEADER[@]}" "$API/api/protocols" 2>/dev/null || echo "")
    EXPECTED=$(grep -E '^protocols=' "$BUILD_INFO" | head -1 | cut -d= -f2- | tr -d '[:space:]' || true)
    ACTUAL=$(echo "$PROTO_JSON" | python3 -c "
import json,sys
d = json.load(sys.stdin)
rows = d.get('data', d) if isinstance(d, dict) else d
print(','.join(sorted(r['name'].replace('-','_') for r in rows if r.get('name') and r['name'] != 'gpio')))
" 2>/dev/null || true)
    if [ -z "$EXPECTED" ] || [ -z "$ACTUAL" ]; then
      warn "无法解析协议清单（BUILD-INFO: '${EXPECTED}' / API: '${ACTUAL}'），跳过一致性检查"
    elif [ "$EXPECTED" = "all" ]; then
      info "出厂构建为全量协议（未裁剪），线上注册协议: $ACTUAL"
    else
      EXPECTED_NORM=$(python3 -c "import sys; print(','.join(sorted(x.replace('-','_') for x in sys.argv[1].split(',') if x)))" "$EXPECTED")
      if [ "$EXPECTED_NORM" = "$ACTUAL" ]; then
        ok "协议裁剪一致: $ACTUAL"
      else
        fail "协议裁剪不一致: 构建清单 [$EXPECTED_NORM] ≠ 线上 [$ACTUAL]"
      fi
    fi
  fi
fi

# ── 5. 前端静态资源 ──
echo "${BOLD}[5/7] 前端静态资源${RESET}"
# index.html（SPA fallback 也会返回）
CODE=$(http_code "$API/")
if [ "$CODE" = "200" ]; then
  BODY=$(curl -s "$API/" 2>/dev/null | head -c 500)
  if echo "$BODY" | grep -q "<!DOCTYPE html>\|<html"; then
    ok "GET / → HTML 文档 (SPA index.html)"
  else
    warn "GET / → 200 但 body 不包含 <html（可能返回 Web 控制台 fallback）"
  fi
else
  fail "GET / → $CODE"
fi

# 找个 .js 资源
INDEX_BODY=$(curl -s "$API/" 2>/dev/null)
JS_FILE=$(echo "$INDEX_BODY" | grep -oE 'src="/assets/[^"]+\.js"' | head -1 | sed 's/src="//;s/"$//')
if [ -n "$JS_FILE" ]; then
  CODE=$(http_code "$API$JS_FILE")
  SIZE=$(curl -sI "$API$JS_FILE" 2>/dev/null | grep -i content-length | awk '{print $2}' | tr -d '\r')
  [ "$CODE" = "200" ] && ok "GET $JS_FILE → 200 (size ~${SIZE} bytes)" || fail "GET $JS_FILE → $CODE"
else
  warn "index.html 里没找到 /assets/*.js 引用（可能是 fallback 控制台）"
fi

# ── 6. 硬件能力探测 ──
echo "${BOLD}[6/7] 硬件能力 (platform introspect)${RESET}"
KERNEL=$(uname -s)
ARCH=$(uname -m)
info "内核: $KERNEL / $ARCH"

# GPIO chip
if [ -e /dev/gpiochip0 ]; then
  ok "/dev/gpiochip0 存在 → GPIO 硬件可用"
else
  warn "/dev/gpiochip0 不存在 → GPIO 将走 stub（Windows / 非工控 Linux 正常）"
fi

# 硬件看门狗
if [ -e /dev/watchdog ]; then
  ok "/dev/watchdog 存在 → 硬件看门狗可用"
else
  warn "/dev/watchdog 不存在 → 看门狗走 stub"
fi

# 串口（Modbus 关键设备）
if ls /dev/ttyUSB* /dev/ttyACM* /dev/ttyS* 2>/dev/null | grep -q .; then
  ok "串口设备存在: $(ls /dev/ttyUSB* /dev/ttyACM* /dev/ttyS* 2>/dev/null | head -5 | tr '\n' ' ')"
else
  warn "未发现 /dev/ttyUSB* / /dev/ttyACM* 串口（插设备后重测）"
fi

# ── 6b. 总线 port_name 存在性与稳定性 ──
# config.json 的 buses[].port_name 是裸字符串，直接进 tokio_serial::new()。
# 两个坑在这里都能被静态发现，不必等到采集静默失效：
#   1) 设备不存在 → 该总线一启动就 open() 失败，采集全程无数据
#   2) 写成 /dev/ttyUSB* 这类会漂移的名字 → 重启后编号可能变成别的，
#      现场表现是「昨天还好好的，今天全采不上」，极易误判为硬件故障
# 正确做法是填 /dev/gateway-serialN（由 udev 规则生成，见 serial-rules.sh）。
echo "${BOLD}[6b] 总线 port_name${RESET}"
PORT_VERDICT=$(python3 - "$CFG_PATH" <<'PY' 2>/dev/null || echo "NO_PYTHON"
import json, sys
try:
    raw = open(sys.argv[1], "rb").read()
except Exception:
    print("NO_PYTHON"); raise SystemExit
if raw.startswith(b"\xef\xbb\xbf"):
    raw = raw[3:]
try:
    d = json.loads(raw.decode("utf-8"))
except Exception:
    print("BAD_JSON"); raise SystemExit
buses = d.get("buses") or []
if not isinstance(buses, list):
    print("BAD_JSON"); raise SystemExit
rows = []
for i, b in enumerate(buses):
    if not isinstance(b, dict):
        continue
    pn = (b.get("port_name") or "").strip()
    if not pn:
        continue
    rows.append("%d\t%s\t%s" % (i, pn, b.get("protocol") or "?"))
print("\n".join(rows))
PY
)
if [ -z "$PORT_VERDICT" ]; then
  info "config.json 里没有配置任何 buses[].port_name（无串口总线，跳过）"
elif [ "$PORT_VERDICT" = "NO_PYTHON" ]; then
  warn "无法读取 $CFG_PATH（缺 python3 或文件不可读），请人工核对 port_name"
elif [ "$PORT_VERDICT" = "BAD_JSON" ]; then
  warn "无法解析 $CFG_PATH 的 buses（JSON 有误），请人工核对 port_name"
else
  BUS_COUNT=0
  while IFS=$'\t' read -r bidx bport bproto; do
    [ -n "$bport" ] || continue
    BUS_COUNT=$((BUS_COUNT + 1))
    # 1) 存在性
    if [ ! -e "$bport" ]; then
      fail "buses[$bidx]（$bproto）port_name=$bport 不存在 —— 该总线采集必然失败"
      echo "    处置: 插好设备后执行 sudo bash serial-rules.sh 看稳定名映射，改成 /dev/gateway-serialN 后重启服务"
      continue
    fi
    # 2) 稳定性：裸 /dev/ttyUSB* / ttyACM* 编号会漂移
    case "$bport" in
      /dev/ttyUSB*|/dev/ttyACM*)
        warn "buses[$bidx]（$bproto）port_name=$bport 是会漂移的裸设备名：USB 枚举顺序一变（重启/换口）就可能指向别的设备，采集会静默中断"
        echo "    处置: 改用 /dev/gateway-serialN（sudo bash serial-rules.sh 生成 udev 规则）"
        ;;
      /dev/gateway-serial*)
        if [ -L "$bport" ]; then
          ok "buses[$bidx]（$bproto）port_name=$bport 使用 udev 稳定名（指向 $(readlink -f "$bport" 2>/dev/null || echo '?')）"
        else
          ok "buses[$bidx]（$bproto）port_name=$bport 存在"
        fi
        ;;
      *)
        ok "buses[$bidx]（$bproto）port_name=$bport 存在"
        ;;
    esac
  done < <(printf '%s\n' "$PORT_VERDICT")
  [ "$BUS_COUNT" -gt 0 ] && info "共核对 $BUS_COUNT 条串口总线的 port_name"
fi

# zigbee 协调器 / 蜂窝模块（USB 设备概览）
if lsusb 2>/dev/null | grep -qi "zigbee\|1a86.*7523\|qualcomm\|simcom"; then
  ok "USB 有通信模块: $(lsusb 2>/dev/null | grep -i 'zigbee\|1a86\|qualcomm\|simcom' | head -3 | awk -F: '{print $2}' | xargs)"
else
  info "无明显 Zigbee/蜂窝 USB 模块"
fi

# dialout 组
if id gateway 2>/dev/null | grep -q dialout; then
  ok "gateway 用户在 dialout 组（串口访问）"
else
  warn "gateway 用户不在 dialout 组，串口访问可能被拒"
fi

# /opt/gatewayd/ui-dist 是否存在
if [ -d /opt/gatewayd/ui-dist ] && [ -f /opt/gatewayd/ui-dist/index.html ]; then
  ok "前端静态已部署到 /opt/gatewayd/ui-dist"
else
  warn "/opt/gatewayd/ui-dist 缺失 → Rust 将返回 WEB_UI_HTML fallback（不是完整 Vue 控制台）"
fi

# ── 7. 资源占用 ──
echo "${BOLD}[7/7] 资源占用${RESET}"
RSS=$(ps -o rss= -p "$(systemctl show -p MainPID --value gateway 2>/dev/null)" 2>/dev/null | awk '{printf "%.0f MB", $1/1024}' || echo "?")
info "gatewayd RSS: $RSS"
DISK_CFG=$(df -h /etc/gateway 2>/dev/null | tail -1 | awk '{print "config partition "$5" used"}' || echo "?")
DISK_DATA=$(df -h /var/lib/gateway 2>/dev/null | tail -1 | awk '{print "data partition "$5" used"}' || echo "?")
info "磁盘: $DISK_CFG, $DISK_DATA"

# ── 汇总 ──
echo ""
echo "${BOLD}═══════════════════════════════════════════════${RESET}"
TOTAL=$((PASS + FAIL + WARN))
echo "${BOLD} 结果汇总${RESET}"
echo "  ${GREEN}PASS${RESET}: $PASS"
echo "  ${RED}FAIL${RESET}: $FAIL"
echo "  ${YELLOW}WARN${RESET}: $WARN"
echo "  总计: $TOTAL"

SCORE=0
[ "$FAIL" -eq 0 ] && SCORE=$((SCORE + 80))
[ "$FAIL" -eq 0 ] && SCORE=$((SCORE + (PASS * 20 / TOTAL))) 2>/dev/null || true

if [ "$FAIL" -eq 0 ]; then
  echo ""
  echo "  ${GREEN}部署健康度: ✅ PASS${RESET} （$PASS 通过，$WARN 提示）"
  echo "  网关上线就绪。建议下一步："
  # 回环绑定单独提示：现场用 LAN 浏览器一定打不开，别让验收人白跑
  case "$BIND_ADDR" in
    127.0.0.1:*|"[::1]:"*)
      echo "    • 当前仅监听 $BIND_ADDR：控制台只能在本机打开；LAN 访问需改 web.listen_addr=0.0.0.0:$PORT 并重启" ;;
    *)
      echo "    • 浏览器打开 http://$HOST:$PORT/ 确认 Vue 控制台完整" ;;
  esac
  echo "    • 若有硬件：短接 DI0→+24V，验证页面 DI0 变 HIGH"
  echo "    • 插 Modbus 从站 /dev/ttyUSB0，新建设备 → 发现向导扫"
  write_report "✅ 通过"
  exit 0
else
  echo ""
  echo "  ${RED}部署健康度: ❌ $FAIL 项失败${RESET}，需要修复"
  echo ""
  for r in "${RESULTS[@]}"; do
    case "$r" in
      FAIL*) echo "    ${RED}$r${RESET}" ;;
    esac
  done
  echo ""
  echo "  常用诊断命令:"
  echo "    sudo journalctl -u gateway -f            # 实时日志"
  echo "    ss -ltnp | grep gatewayd                  # 端口监听"
  echo "    curl -v $API/health/live                  # 健康检查"
  write_report "❌ 不通过（$FAIL 项失败）"
  exit 1
fi
