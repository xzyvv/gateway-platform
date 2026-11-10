#!/usr/bin/env bash
set -euo pipefail

# ────────────────────────────────────────────────────────────
# 开机自启 / 断电自启 行为验证脚本（两阶段）
#
#   sudo bash verify-boot.sh before [--point p1]     # 重启/断电前，留基线
#   sudo systemctl reboot                            # 或直接拉闸（断电）
#   sudo bash verify-boot.sh after  [--point p1]     # 上电后，出判定
#
# 通用参数：
#   --host/--port    省略时读 /etc/gateway/config.json 的 web.listen_addr
#   --token          省略时读配置里的 web.api_token
#   --state          基线文件，默认 /var/lib/gateway/boot-check.env
#   --point          用于核对「重启前已落库的样本仍在」的点位 ID（强烈建议给）
#   --max-uptime     after 阶段允许的最大开机时长（秒，默认 900）
#   --wait           after 阶段等待 /health/live 就绪的秒数（默认 60）
#   --report         把结果写成 Markdown，便于贴进交付检查单
#
# 为什么需要它：`systemctl is-enabled` 只能证明「配置了自启」，
# 证明不了「开机后真的自己起来了」。而 CI 的托管 runner 一旦 reboot
# 连 job 一起死掉，所以这件事只能在真机做。仅靠人眼也容易被糊过去：
# 运维手工 `systemctl start` 同样能让服务 active，因此必须用
# boot_id 变化 + ActiveEnterTimestamp 落在本次 boot 内来区分「自启」与「人启」。
# ────────────────────────────────────────────────────────────

STAGE="${1:-}"
case "$STAGE" in
  before|after) shift ;;
  -h|--help|"")
    sed -n '5,28p' "$0" | sed 's/^# \{0,1\}//'
    exit 0
    ;;
  *) echo "未知阶段: $STAGE（只支持 before / after）" >&2; exit 2 ;;
esac

HOST=""
PORT=""
TOKEN=""
STATE="/var/lib/gateway/boot-check.env"
POINT=""
MAX_UPTIME=900
WAIT_SECS=60
REPORT=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --host) HOST="$2"; shift 2 ;;
    --host=*) HOST="${1#*=}"; shift ;;
    --port) PORT="$2"; shift 2 ;;
    --port=*) PORT="${1#*=}"; shift ;;
    --token) TOKEN="$2"; shift 2 ;;
    --token=*) TOKEN="${1#*=}"; shift ;;
    --state) STATE="$2"; shift 2 ;;
    --state=*) STATE="${1#*=}"; shift ;;
    --point) POINT="$2"; shift 2 ;;
    --point=*) POINT="${1#*=}"; shift ;;
    --max-uptime) MAX_UPTIME="$2"; shift 2 ;;
    --max-uptime=*) MAX_UPTIME="${1#*=}"; shift ;;
    --wait) WAIT_SECS="$2"; shift 2 ;;
    --wait=*) WAIT_SECS="${1#*=}"; shift ;;
    --report) REPORT="$2"; shift 2 ;;
    --report=*) REPORT="${1#*=}"; shift ;;
    *) shift ;;
  esac
done

# ── 生效配置：与 verify-deploy.sh 同口径（GATEWAY_CONFIG_PATH 覆盖优先）──
CFG_PATH="/etc/gateway/config.json"
SVC_ENV=$(systemctl show -p Environment --value gateway 2>/dev/null || true)
for kv in $SVC_ENV; do
  case "$kv" in
    GATEWAY_CONFIG_PATH=*) CFG_PATH="${kv#*=}" ;;
  esac
done

cfg_get() { # $1=dotted.path  —— 只用 python3（Debian/Ubuntu 自带），无 python3 时返回空
  [ -f "$CFG_PATH" ] || return 0
  command -v python3 >/dev/null 2>&1 || return 0
  python3 -c 'import json,sys
try:
    d=json.load(open(sys.argv[1]))
except Exception:
    sys.exit(0)
for k in sys.argv[2].split("."):
    d=(d or {}).get(k) if isinstance(d,dict) else None
print(d if isinstance(d,str) else "")' "$CFG_PATH" "$1" 2>/dev/null || true
}

if [ -z "$PORT" ] || [ -z "$HOST" ]; then
  LISTEN="$(cfg_get web.listen_addr)"
  if [ -n "$LISTEN" ]; then
    lh="${LISTEN%:*}"; lp="${LISTEN##*:}"
    lh="${lh#[}"; lh="${lh%]}"
    case "$lh" in ""|0.0.0.0|"*"|::) lh="127.0.0.1" ;; esac
    [ -z "$HOST" ] && HOST="$lh"
    [ -z "$PORT" ] && PORT="$lp"
  fi
fi
HOST="${HOST:-127.0.0.1}"
PORT="${PORT:-8080}"
API="http://$HOST:$PORT"
[ -n "$TOKEN" ] || TOKEN="$(cfg_get web.api_token)"

PASS=0; FAIL=0; WARN=0
GREEN=$'\033[32m'; RED=$'\033[31m'; YELLOW=$'\033[33m'; CYAN=$'\033[36m'; BOLD=$'\033[1m'; RESET=$'\033[0m'
ok()   { echo "  ${GREEN}✓${RESET} $1"; PASS=$((PASS+1)); }
fail() { echo "  ${RED}✗${RESET} $1"; FAIL=$((FAIL+1)); }
warn() { echo "  ${YELLOW}!${RESET} $1"; WARN=$((WARN+1)); }
info() { echo "  ${CYAN}→${RESET} $1"; }

boot_id() { cat /proc/sys/kernel/random/boot_id 2>/dev/null || true; }
uptime_secs() { awk '{printf "%d", $1}' /proc/uptime 2>/dev/null || echo 0; }
svc() { systemctl show -p "$1" --value gateway 2>/dev/null || true; }
to_epoch() { # systemd 风格时间戳 → epoch；解析失败返回空
  [ -n "$1" ] && date -d "$1" +%s 2>/dev/null || true
}

# 历史样本核对：用 python3 + urllib，避免依赖 jq（目标机不一定装）
# 输出 YES / NO / ERR:<原因>
last_sample_ts() { # $1=point
  python3 - "$API" "$TOKEN" "$1" <<'PY' 2>/dev/null || echo "ERR:python3"
import json,sys,urllib.request,urllib.parse
api,token,point=sys.argv[1],sys.argv[2],sys.argv[3]
q=urllib.parse.urlencode({'point_id':point,'limit':1})
req=urllib.request.Request(api+'/api/history?'+q)
if token: req.add_header('Authorization','Bearer '+token)
try:
    d=json.load(urllib.request.urlopen(req,timeout=8))
except Exception as e:
    print('ERR:'+str(e)); sys.exit(0)
items=d.get('items') or []
print(items[-1]['timestamp'] if items else 'NO')
PY
}
history_has_ts() { # $1=point $2=ts(ms)
  python3 - "$API" "$TOKEN" "$1" "$2" <<'PY' 2>/dev/null || echo "ERR:python3"
import json,sys,urllib.request,urllib.parse
api,token,point,ts=sys.argv[1],sys.argv[2],sys.argv[3],int(sys.argv[4])
q=urllib.parse.urlencode({'point_id':point,'start':ts-1,'end':ts+1,'limit':10})
req=urllib.request.Request(api+'/api/history?'+q)
if token: req.add_header('Authorization','Bearer '+token)
try:
    d=json.load(urllib.request.urlopen(req,timeout=8))
except Exception as e:
    print('ERR:'+str(e)); sys.exit(0)
items=d.get('items') or []
print('YES' if any(int(i.get('timestamp',-1))==ts for i in items) else 'NO')
PY
}

now_epoch() { date +%s; }

# ════════════════════════ before ════════════════════════
if [ "$STAGE" = "before" ]; then
  echo "${BOLD}== 阶段 1/2：记录重启前基线 ==${RESET}"
  info "配置: $CFG_PATH    API: $API    point: ${POINT:-<未指定>}"

  BID="$(boot_id)"
  if [ -z "$BID" ]; then
    fail "读不到 /proc/sys/kernel/random/boot_id，无法做 boot 比对"
  else
    ok "当前 boot_id: $BID"
  fi

  ACTIVE="$(svc ActiveState)"
  if [ "$ACTIVE" = "active" ]; then
    ok "gateway.service 当前 active（基线有效）"
  else
    fail "gateway.service 当前 $ACTIVE，请先确保服务在跑再记录基线"
  fi

  UP="$(uptime_secs)"
  ENTER="$(svc ActiveEnterTimestamp)"
  NRESTARTS="$(svc NRestarts)"
  info "开机时长 ${UP}s，本次 active 进入时间: ${ENTER:-未知}，NRestarts=${NRESTARTS:-0}"

  LAST_TS=""
  if [ -n "$POINT" ]; then
    R="$(last_sample_ts "$POINT")"
    case "$R" in
      ERR:*|NO) warn "点位 $POINT 暂无历史样本（$R），after 阶段将跳过数据留存核对" ;;
      *) LAST_TS="$R"; ok "点位 $POINT 最新落库样本: $LAST_TS" ;;
    esac
  else
    warn "未给 --point，after 阶段无法核对「重启前已落库的样本仍在」"
  fi

  # 基线无效就不写：写进去会让 after 阶段拿到空 boot_id，把「没重启」误判成通过
  if [ -z "$BID" ] || [ "$ACTIVE" != "active" ]; then
    echo "${RED}基线无效，未写入 $STATE。请先修好再跑 before。${RESET}" >&2
    exit 1
  fi

  mkdir -p "$(dirname "$STATE")"
  cat > "$STATE" <<EOF
boot_id=$BID
captured_at=$(date -Is)
epoch=$(now_epoch)
uptime_secs=$UP
active_enter=${ENTER:-}
nrestarts=${NRESTARTS:-0}
point=$POINT
last_sample_ts=$LAST_TS
EOF
  chmod 644 "$STATE"
  ok "基线已写入 $STATE"

  echo
  echo "${BOLD}下一步（二选一）${RESET}"
  echo "  重启: ${CYAN}sudo systemctl reboot${RESET}"
  echo "  断电: 物理拉闸/拔电（必须是真断电，不要用 poweroff 冒充）"
  echo "  上电并等到能 SSH 后执行: ${CYAN}sudo bash verify-boot.sh after${POINT:+ --point $POINT}${RESET}"
  exit 0
fi

# ════════════════════════ after ════════════════════════
echo "${BOLD}== 阶段 2/2：核对开机自启行为 ==${RESET}"

BASE_BID=""; BASE_TS=""; BASE_POINT=""; BASE_LAST_TS=""; BASE_NRESTARTS=""
if [ -f "$STATE" ]; then
  # 基线是 key=value 纯文本：目标机不一定有 jq，但一定有 bash
  while IFS='=' read -r k v; do
    case "$k" in
      boot_id) BASE_BID="$v" ;;
      epoch) BASE_TS="$v" ;;
      point) BASE_POINT="$v" ;;
      last_sample_ts) BASE_LAST_TS="$v" ;;
      nrestarts) BASE_NRESTARTS="$v" ;;
    esac
  done < "$STATE"
  [ -n "$POINT" ] || POINT="$BASE_POINT"
else
  fail "找不到基线文件 $STATE —— 必须先跑 before，否则无法证明「真的重启过」"
fi

# ① boot_id 变了 = 真的重启过；没变 = 只是重启了进程，不能算自启证据
CUR_BID="$(boot_id)"
if [ -n "$BASE_BID" ] && [ -n "$CUR_BID" ]; then
  if [ "$BASE_BID" != "$CUR_BID" ]; then
    ok "boot_id 已变化（${BASE_BID:0:8}… → ${CUR_BID:0:8}…），确实是新的开机周期"
  else
    fail "boot_id 未变化 —— 机器没有重启；本次只能说明服务在跑，不能说明自启"
  fi
fi

# ② 开机时长与 baseline 的关系：after 应尽快执行，否则证据力变弱
UP="$(uptime_secs)"
if [ "$UP" -le "$MAX_UPTIME" ]; then
  ok "当前开机时长 ${UP}s ≤ 阈值 ${MAX_UPTIME}s（本阶段在开机后不久执行，证据有效）"
else
  warn "当前开机时长 ${UP}s > 阈值 ${MAX_UPTIME}s —— 结论仍有效，但无法排除「中途人工 start 过」"
fi

# ③ 服务状态
ACTIVE="$(svc ActiveState)"
SUB="$(svc SubState)"
if [ "$ACTIVE" = "active" ]; then
  ok "gateway.service → active(${SUB:-running})"
else
  fail "gateway.service → ${ACTIVE:-unknown}（自启失败，先看 journalctl -b --unit=gateway）"
fi

# ④ 关键：本次 boot 内就自己起来了，而不是人手工 start
#    boot_time = now - uptime；ActiveEnterTimestamp 落在 boot_time 之后很短窗口内即判定自启。
BOOT_EPOCH=$(( $(now_epoch) - UP ))
ENTER_EPOCH="$(to_epoch "$(svc ActiveEnterTimestamp)")"
if [ -n "$ENTER_EPOCH" ]; then
  DELAY=$(( ENTER_EPOCH - BOOT_EPOCH ))
  if [ "$DELAY" -lt 0 ]; then
    warn "ActiveEnterTimestamp 早于本次开机时间（时钟可能被校过），改按 process 运行时长判断"
  elif [ "$DELAY" -le 180 ]; then
    ok "开机后 ${DELAY}s 内进入 active —— 属开机自启（非人工 start）"
  elif [ "$DELAY" -le 600 ]; then
    warn "开机后 ${DELAY}s 才进入 active —— 偏慢，检查依赖服务/设备探测耗时"
  else
    fail "开机后 ${DELAY}s 才进入 active —— 更像人工启动或反复重启，不满足自启判据"
  fi
else
  warn "读不到 ActiveEnterTimestamp，跳过「自启 vs 人启」判定"
fi

# ⑤ 重启循环：NRestarts 较基线增长说明进过 crash-loop
CUR_NR="$(svc NRestarts)"; BASE_NRESTARTS="${BASE_NRESTARTS:-0}"
# 两个值都必须先收敛成纯数字，否则 `[ -le ]` 报错 + `$(( ))` 在 set -e 下直接中断
case "$CUR_NR" in ''|*[!0-9]*) CUR_NR=0 ;; esac
case "$BASE_NRESTARTS" in ''|*[!0-9]*) BASE_NRESTARTS=0 ;; esac
if [ "$CUR_NR" -le "$BASE_NRESTARTS" ]; then
  ok "NRestarts 未增长（基线 ${BASE_NRESTARTS} → 当前 ${CUR_NR}），无重启循环"
else
  DIFF=$(( CUR_NR - BASE_NRESTARTS ))
  if [ "$DIFF" -ge 3 ]; then
    fail "本次开机内自动重启 ${DIFF} 次，属 crash-loop"
  else
    warn "本次开机内自动重启 ${DIFF} 次，检查 journalctl -b --unit=gateway"
  fi
fi

# ⑥ 本次 boot 的启动日志：有 Started、无 Failed
if command -v journalctl >/dev/null 2>&1; then
  BOOT_LOG="$(journalctl -b --unit=gateway --no-pager -o cat 2>/dev/null || true)"
  if [ -z "$BOOT_LOG" ]; then
    warn "本次 boot 没有 gateway 的 journal 记录（服务未自启，或日志被清）"
  else
    STARTED=$(printf '%s\n' "$BOOT_LOG" | grep -c 'Started\|主进程\|listening on' || true)
    FAILED=$(printf '%s\n' "$BOOT_LOG" | grep -ci 'Failed with result\|Main process exited' || true)
    if [ "$FAILED" -ge 3 ]; then
      fail "本次 boot 日志出现 ${FAILED} 次异常退出"
    elif [ "$FAILED" -ge 1 ]; then
      warn "本次 boot 日志出现 ${FAILED} 次异常退出（随后自愈则属正常）"
    else
      ok "本次 boot 日志无异常退出记录"
    fi
    info "本次 boot 关键行数: ${STARTED}"
  fi
fi

# ⑦ 存活探针（刚上电时可能还在起，给 --wait 秒）
DEADLINE=$(( $(now_epoch) + WAIT_SECS ))
CODE="000"
while :; do
  CODE=$(curl -s -o /dev/null -w '%{http_code}' --max-time 3 "$API/health/live" 2>/dev/null || echo 000)
  [ "$CODE" = "200" ] && break
  [ "$(now_epoch)" -ge "$DEADLINE" ] && break
  sleep 2
done
if [ "$CODE" = "200" ]; then
  ok "GET $API/health/live → 200"
else
  fail "GET $API/health/live → $CODE（${WAIT_SECS}s 内未就绪）"
fi

# ⑧ 数据留存：重启/断电前最后一条已落库样本必须仍在（断电场景最关键）
if [ -n "$BASE_LAST_TS" ] && [ -n "$POINT" ]; then
  R="$(history_has_ts "$POINT" "$BASE_LAST_TS")"
  case "$R" in
    YES) ok "重启前最后一条样本仍在（point=$POINT ts=$BASE_LAST_TS），已落库数据未丢" ;;
    NO)  fail "重启前最后一条样本查不到了（point=$POINT ts=$BASE_LAST_TS）—— 数据丢失或留存被清" ;;
    *)   warn "历史核对未完成（$R），需手工确认 point=$POINT ts=$BASE_LAST_TS" ;;
  esac
elif [ -z "$POINT" ]; then
  warn "基线未记录点位，跳过分重启前后的数据留存核对"
else
  warn "基线里没有可用的最后样本时间戳，跳过数据留存核对"
fi

echo
echo "${BOLD}结果${RESET}: ${GREEN}PASS=$PASS${RESET}  ${YELLOW}WARN=$WARN${RESET}  ${RED}FAIL=$FAIL${RESET}"
if [ -n "$BASE_TS" ]; then
  DUR=$(( $(now_epoch) - BASE_TS ))
  info "基线时间 $(date -d "@$BASE_TS" -Is 2>/dev/null || echo "$BASE_TS")，距今 ${DUR}s"
fi

if [ -n "$REPORT" ]; then
  mkdir -p "$(dirname "$REPORT")"
  {
    echo "## 开机自启验证（$(date -Is)）"
    echo
    echo "| 项 | 结果 |"
    echo "|---|---|"
    echo "| boot_id 变化 | $( [ "$BASE_BID" != "$CUR_BID" ] && echo PASS || echo FAIL ) |"
    echo "| 开机时长 | ${UP}s |"
    echo "| 服务状态 | ${ACTIVE:-unknown}(${SUB:-}) |"
    echo "| 自启 vs 人启 | ${DELAY:-n/a}s 内 active |"
    echo "| NRestarts | ${BASE_NRESTARTS} → ${CUR_NR} |"
    echo "| /health/live | ${CODE} |"
    echo "| 数据留存 | ${BASE_LAST_TS:-未核对} |"
    echo
    echo "**汇总**：PASS=$PASS WARN=$WARN FAIL=$FAIL"
  } > "$REPORT"
  echo "  检查单片段: $REPORT"
fi

[ "$FAIL" -eq 0 ] || exit 1
echo "${GREEN}${BOLD}开机自启验证通过${RESET}"
