#!/usr/bin/env bash
set -euo pipefail

# ────────────────────────────────────────────────────────────
# 工业网关部署后验证脚本
# 用法: sudo bash verify-deploy.sh [--port 8080] [--token xxx] [--build-info path] [--report path]
#       --port/--host 省略时自动读 /etc/gateway/config.json 的 web.listen_addr
#
# 在目标 Linux 上跑完 install.sh 后执行。
# 覆盖：systemd 服务 / 端口 / 健康检查 / API 契约 / 前端静态 / 硬件能力
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
    *) shift ;;
  esac
done

# ── 监听地址：显式 --port/--host 优先，否则读 /etc/gateway/config.json ──
# 自定义端口/绑定时若仍按 8080 探测，会把「装在 9000 上的正常网关」判成未监听。
# 通配绑定（0.0.0.0 / * / [::]）在本机验证一律走回环；IPv6 字面量保留方括号。
config_bind() {
  [ -f /etc/gateway/config.json ] || return 0
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
print(host, port)' /etc/gateway/config.json 2>/dev/null || true
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

if [ -n "$TOKEN" ]; then
  # 核心状态端点
  CODE=$(curl -s -o /dev/null -w "%{http_code}" "${AUTH_HEADER[@]}" "$API/api/auth/whoami" 2>/dev/null || echo "000")
  [ "$CODE" = "200" ] && ok "GET /api/auth/whoami → 200" || fail "GET /api/auth/whoami → $CODE"

  CODE=$(curl -s -o /dev/null -w "%{http_code}" "${AUTH_HEADER[@]}" "$API/api/devices" 2>/dev/null || echo "000")
  [ "$CODE" = "200" ] && ok "GET /api/devices → 200" || fail "GET /api/devices → $CODE"

  # 点位：没有全局 GET /api/points（点位按设备挂载：GET /api/devices/:id/points），
  # 这里用全局可用的回收站列表做契约探针——原来打 /api/points 恒 404，会让每次交付都判 FAIL。
  CODE=$(curl -s -o /dev/null -w "%{http_code}" "${AUTH_HEADER[@]}" "$API/api/points/trash" 2>/dev/null || echo "000")
  [ "$CODE" = "200" ] && ok "GET /api/points/trash → 200" || fail "GET /api/points/trash → $CODE"

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
