#!/usr/bin/env bash
set -uo pipefail

# ────────────────────────────────────────────────────────────
# 工业网关一键交付（G5）
#
#   install.sh → 等待就绪 → verify-deploy.sh → 交付检查单导出
#
# 用法（发布包内，一条命令）:
#   sudo bash deliver.sh [--port 8080] [--token xxx] [--host 127.0.0.1] \
#                        [--report path] [--timeout 45] [--no-start]
#
#   --port/--host 省略时自动读 /etc/gateway/config.json 的 web.listen_addr。
#
#   或显式指定产物（源码/包布局均可）:
#   sudo bash deliver.sh gatewayd-<target> ./ui-dist
#
# 为什么由本脚本串起来：install 只负责落盘，verify 只负责判定；交付方
# 真正需要的是「一条命令跑完并留下一份可签字的检查单」。检查单由
# verify-deploy.sh 生成（判定逻辑唯一来源），本脚本只做编排与兜底：
#   - install 失败时不中止，仍跑 verify 以便把失败项与诊断命令写进检查单
#   - 退出码取 install/verify 中最严重的那个，CI 或人工都能据此拦发布
# ────────────────────────────────────────────────────────────

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# 发布包：install.sh / verify-deploy.sh 与 deliver.sh 同级；
# 仓库：deliver.sh 在 deploy/，install.sh 在 gateway-core/deploy/
INSTALL="$DIR/install.sh"
VERIFY="$DIR/verify-deploy.sh"
if [ ! -f "$INSTALL" ]; then
  INSTALL="$DIR/../gateway-core/deploy/install.sh"
fi

PORT=""
HOST=""
TOKEN=""
WAIT_TIMEOUT=45
REPORT="$DIR/out/delivery-checklist-$(date +%Y%m%d-%H%M%S).md"
BINARY=""
UI_DIST=""
NO_START=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --port)     PORT="$2"; shift 2 ;;
    --port=*)   PORT="${1#*=}"; shift ;;
    --host)     HOST="$2"; shift 2 ;;
    --host=*)   HOST="${1#*=}"; shift ;;
    --token)    TOKEN="$2"; shift 2 ;;
    --token=*)  TOKEN="${1#*=}"; shift ;;
    --report)   REPORT="$2"; shift 2 ;;
    --report=*) REPORT="${1#*=}"; shift ;;
    --timeout)  WAIT_TIMEOUT="$2"; shift 2 ;;
    --timeout=*) WAIT_TIMEOUT="${1#*=}"; shift ;;
    --no-start) NO_START=1; shift ;;
    -h|--help)
      sed -n '2,16p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}/  /'
      exit 0
      ;;
    -*) echo "错误: 未知参数 '$1'"; exit 1 ;;
    *)
      if [ -z "$BINARY" ]; then BINARY="$1"
      elif [ -z "$UI_DIST" ]; then UI_DIST="$1"
      else echo "错误: 多余的参数 '$1'"; exit 1
      fi
      shift ;;
  esac
done

echo "═══════════════════════════════════════════════"
echo " 工业网关一键交付"
echo "═══════════════════════════════════════════════"
echo "安装脚本: $INSTALL"
echo "验证脚本: $VERIFY"
echo "检查单:   $REPORT"
echo ""

if [ "$(id -u)" -ne 0 ]; then
  echo "❌ 需要 root 权限（安装 systemd 服务与 /opt/gatewayd）；请用 sudo 重新执行"
  exit 1
fi
if [ ! -f "$INSTALL" ]; then
  echo "❌ 找不到 install.sh（期望: $INSTALL）"
  exit 1
fi
if [ ! -f "$VERIFY" ]; then
  echo "❌ 找不到 verify-deploy.sh（期望: $VERIFY）"
  exit 1
fi

# ── 1. 安装 ──
INSTALL_ARGS=()
[ -n "$BINARY" ] && INSTALL_ARGS+=("$BINARY")
if [ -n "$UI_DIST" ]; then
  # 未显式给二进制却给了前端目录时，用空串占出第一个位置
  [ ${#INSTALL_ARGS[@]} -eq 0 ] && INSTALL_ARGS+=("")
  INSTALL_ARGS+=("$UI_DIST")
fi
[ "$NO_START" -eq 1 ] && INSTALL_ARGS+=("--no-start")

echo "── [1/3] 安装 ──"
INSTALL_RC=0
# 只在显式 --port 时钳住 install.sh 的探测端口；未指定则交给 install.sh
# 读 config.json 的 web.listen_addr（gatewayd 不认 GATEWAY_PORT 环境变量）
if [ ${#INSTALL_ARGS[@]} -gt 0 ]; then
  if [ -n "$PORT" ]; then
    GATEWAY_PORT="$PORT" bash "$INSTALL" "${INSTALL_ARGS[@]}" || INSTALL_RC=$?
  else
    bash "$INSTALL" "${INSTALL_ARGS[@]}" || INSTALL_RC=$?
  fi
else
  if [ -n "$PORT" ]; then
    GATEWAY_PORT="$PORT" bash "$INSTALL" || INSTALL_RC=$?
  else
    bash "$INSTALL" || INSTALL_RC=$?
  fi
fi
if [ "$INSTALL_RC" -ne 0 ]; then
  echo ""
  echo "⚠ 安装阶段返回 $INSTALL_RC，仍继续验证以便把失败项写进检查单"
fi

# ── 解析监听地址（安装后 /etc/gateway/config.json 已就位）──
# 未显式 --port/--host 时按配置探测，避免自定义端口被 8080 的默认值覆盖
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

# ── 2. 等待就绪（安装已启动服务时） ──
HEALTH_URL="http://$HOST:$PORT/health/live"
if [ "$NO_START" -eq 0 ]; then
  echo ""
  echo "── [2/3] 等待网关就绪（最长 ${WAIT_TIMEOUT}s）──"
  READY=0
  for _ in $(seq 1 "$WAIT_TIMEOUT"); do
    CODE=$(curl -s -o /dev/null -w "%{http_code}" "$HEALTH_URL" 2>/dev/null || echo "000")
    if [ "$CODE" = "200" ]; then READY=1; break; fi
    sleep 1
  done
  if [ "$READY" -eq 1 ]; then
    echo "  ✓ $HEALTH_URL → 200"
  else
    echo "  ✗ $HEALTH_URL 在 ${WAIT_TIMEOUT}s 内未返回 200（继续验证以定位原因）"
  fi
else
  echo ""
  echo "── [2/3] 已按 --no-start 跳过启动与等待 ──"
fi

# ── 3. 验证 + 导出检查单 ──
echo ""
echo "── [3/3] 部署验证与检查单导出 ──"
VERIFY_ARGS=(--host "$HOST" --port "$PORT" --report "$REPORT")
[ -n "$TOKEN" ] && VERIFY_ARGS+=(--token "$TOKEN")
if [ -f "$DIR/BUILD-INFO.txt" ]; then
  VERIFY_ARGS+=(--build-info "$DIR/BUILD-INFO.txt")
fi
VERIFY_RC=0
bash "$VERIFY" "${VERIFY_ARGS[@]}" || VERIFY_RC=$?

echo ""
echo "═══════════════════════════════════════════════"
if [ -f "$REPORT" ]; then
  echo " 📄 交付检查单: $REPORT"
else
  echo " ⚠ 未生成交付检查单（验证脚本提前退出，先看上方输出）"
fi

if [ "$INSTALL_RC" -ne 0 ] || [ "$VERIFY_RC" -ne 0 ]; then
  echo " ❌ 交付未通过（安装 $INSTALL_RC / 验证 $VERIFY_RC）"
  echo "═══════════════════════════════════════════════"
  [ "$INSTALL_RC" -ne 0 ] && exit "$INSTALL_RC"
  exit "$VERIFY_RC"
fi

echo " ✅ 交付完成：安装、验证、检查单导出全部通过"
echo "═══════════════════════════════════════════════"
exit 0
