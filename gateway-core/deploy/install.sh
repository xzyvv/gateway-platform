#!/bin/bash
set -euo pipefail

# ────────────────────────────────────────────────────────────
# 探测系统时区名（用于写进 systemd 服务的 Environment=TZ=）
# 依次尝试：timedatectl → /etc/timezone → /etc/localtime 软链目标。
# 都拿不到时返回空串（调用方给出「按 UTC 运行」的告警）。
detect_system_tz() {
  local tz=""
  if command -v timedatectl >/dev/null 2>&1; then
    tz="$(timedatectl show --property=Timezone --value 2>/dev/null || true)"
  fi
  if [ -z "$tz" ] && [ -f /etc/timezone ]; then
    tz="$(tr -d '[:space:]' < /etc/timezone)"
  fi
  if [ -z "$tz" ] && [ -L /etc/localtime ]; then
    # /etc/localtime -> /usr/share/zoneinfo/Asia/Shanghai
    tz="$(readlink /etc/localtime 2>/dev/null | sed -n 's#.*zoneinfo/##p' || true)"
  fi
  # UTC 本身不算「探测到时区」：它正是需要告警的场景
  case "$tz" in
    UTC|Etc/UTC|Etc/GMT|"") echo "" ;;
    *) echo "$tz" ;;
  esac
}

# ────────────────────────────────────────────────────────────
# 工业网关一键部署脚本
# 用法: sudo bash install.sh [binary_path] [ui_dist_path] [--no-start]
#
# 路径解析以「本脚本所在目录」为基准，同时兼容两种布局：
#   A. 发布包（build-release.sh 产物）：gatewayd / ui-dist / gateway.service /
#      install.sh / verify-deploy.sh / deliver.sh / templates 全部同级
#   B. 仓库源码（gateway-core/deploy/）：二进制在 target/、前端在 gateway-web/dist、
#      service 与脚本同级、templates 在仓库根
#
# --no-start：只安装不启动（留给编排脚本或手工排障；默认安装后启动并等待健康检查）
#
# 前置：已用 build-release.sh 或手动构建出前端 dist/ 和 gatewayd
# 本脚本安装二进制 + 前端静态资源 + systemd 服务
# ────────────────────────────────────────────────────────────

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

START_SERVICE=1
ARGS=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-start) START_SERVICE=0; shift ;;
    -h|--help)
      sed -n '2,18p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}/  /'
      exit 0
      ;;
    -*) echo "错误: 未知参数 '$1'"; exit 1 ;;
    *) ARGS+=("$1"); shift ;;
  esac
done

BINARY="${ARGS[0]:-}"
UI_DIST="${ARGS[1]:-}"

# 前置自检：差一个依赖时尽早失败，别装到一半才报错（现场没有回滚条件）
if [ "$(id -u)" -ne 0 ]; then
  echo "❌ 需要 root 权限（安装 /opt/gatewayd、/etc/gateway 与 systemd 服务），请用 sudo 执行"
  exit 1
fi
for dep in systemctl curl; do
  if ! command -v "$dep" >/dev/null 2>&1; then
    echo "❌ 缺少依赖: $dep（Debian/Ubuntu: apt-get install -y curl systemd）"
    exit 1
  fi
done

INSTALL_DIR="/opt/gatewayd"
UI_DIR="$INSTALL_DIR/ui-dist"
CONFIG_DIR="/etc/gateway"
DATA_DIR="/var/lib/gateway"
LOG_DIR="/var/log/gateway"
SERVICE_FILE="/etc/systemd/system/gateway.service"
PORT="${GATEWAY_PORT:-}"
# OTA A/B 分区设备：现场介质不一定是 eMMC（可能是 SD / NVMe），分区节点名随之变化。
# 这两个值必须同时体现在两处，缺一 OTA 直接失败：
#   1) systemd unit 的 DeviceAllow（不放行 → 程序 open() 之前被 EPERM 拒绝）
#   2) 进程环境变量 GATEWAY_OTA_SLOT_A / GATEWAY_OTA_SLOT_B（代码读它决定写哪个分区）
# 部署时用环境变量指定：GATEWAY_OTA_SLOT_A=/dev/sda2 GATEWAY_OTA_SLOT_B=/dev/sda3 bash install.sh
OTA_SLOT_A="${GATEWAY_OTA_SLOT_A:-/dev/mmcblk0p2}"
OTA_SLOT_B="${GATEWAY_OTA_SLOT_B:-/dev/mmcblk0p3}"
# 健康检查目标主机：gatewayd 只读 config.json，不读 GATEWAY_PORT；
# 未显式指定端口时下面会按已落盘的 web.listen_addr 探测（自定义端口才能验证到真实监听）
HEALTH_HOST="127.0.0.1"

# ── 二进制：参数优先，否则在包目录 / 仓库 target 里探测 ──
if [ -z "$BINARY" ]; then
  for candidate in "$SCRIPT_DIR/gatewayd" "$SCRIPT_DIR"/gatewayd-* \
                   "$SCRIPT_DIR/../target/release/gatewayd" \
                   "$SCRIPT_DIR/../../target/release/gatewayd"; do
    if [ -f "$candidate" ]; then
      BINARY="$candidate"
      break
    fi
  done
fi

# ── 前端产物：参数优先，否则按包布局 / 源码布局探测 ──
if [ -z "$UI_DIST" ]; then
  for candidate in "$SCRIPT_DIR/ui-dist" "$SCRIPT_DIR/../ui-dist" \
                   "$SCRIPT_DIR/../../gateway-web/dist"; do
    if [ -d "$candidate" ] && [ -f "$candidate/index.html" ]; then
      UI_DIST="$candidate"
      break
    fi
  done
fi

# ── systemd unit：包布局在脚本同级，源码布局同样在 gateway-core/deploy ──
SERVICE_SRC=""
for candidate in "$SCRIPT_DIR/gateway.service" "$SCRIPT_DIR/deploy/gateway.service"; do
  if [ -f "$candidate" ]; then
    SERVICE_SRC="$candidate"
    break
  fi
done

# ── 串口稳定名生成器：包布局在脚本同级，源码布局同样在 gateway-core/deploy ──
# 4.6 步要调用它，5.x 步要把它装进 /opt/gatewayd，所以这里先解析出来。
SERIAL_RULES_SRC=""
for candidate in "$SCRIPT_DIR/serial-rules.sh" "$SCRIPT_DIR/deploy/serial-rules.sh"; do
  if [ -f "$candidate" ]; then
    SERIAL_RULES_SRC="$candidate"
    break
  fi
done

# ── 设备模板：包布局在 scripts 同级，源码布局在仓库根 ──
TEMPLATES_SRC=""
for candidate in "$SCRIPT_DIR/templates" "$SCRIPT_DIR/../templates" "$SCRIPT_DIR/../../templates"; do
  if [ -d "$candidate/builtin" ]; then
    TEMPLATES_SRC="$candidate"
    break
  fi
done

echo "═══════════════════════════════════════════════"
echo " 工业网关部署"
echo "═══════════════════════════════════════════════"
echo "二进制: ${BINARY:-（未找到）}"
echo "前端:   ${UI_DIST:-（未找到，将回退内置控制台）}"
echo "服务:   ${SERVICE_SRC:-（未找到）}"
echo ""

if [ ! -f "$BINARY" ]; then
  echo "❌ 错误: 找不到 gatewayd 二进制"
  echo "   发布包布局：把 gatewayd（或 gatewayd-<target>）放在本脚本同级目录"
  echo "   源码布局：  ./deploy/build-release.sh 后在 target/release/gatewayd"
  echo "   交叉编译：  ./deploy/build-release.sh --target linux-arm64"
  exit 1
fi

# 1. 创建系统用户
if ! id -u gateway &>/dev/null; then
    echo "[1/7] 创建系统用户 gateway..."
    useradd --system --no-create-home --shell /usr/sbin/nologin gateway
else
    echo "[1/7] 用户 gateway 已存在"
fi
usermod -aG dialout gateway 2>/dev/null || true

# 2. 创建目录结构
echo "[2/7] 创建目录结构..."
mkdir -p "$INSTALL_DIR"
mkdir -p "$UI_DIR"
mkdir -p "$CONFIG_DIR/templates/builtin" "$CONFIG_DIR/templates/custom" "$CONFIG_DIR/scripts"
mkdir -p "$DATA_DIR/certs" "$DATA_DIR/ota"
mkdir -p "$LOG_DIR"

# 3. 拷贝二进制
echo "[3/7] 安装 gatewayd..."
# 升级场景：运行中的二进制无法覆盖（ETXTBSY），先停服再替换；
# --no-start 时这里只停不启，交给调用方决定何时启动
if systemctl is-active --quiet gateway 2>/dev/null; then
    echo "  → gateway 正在运行，先停止以便替换二进制"
    systemctl stop gateway
fi
cp "$BINARY" "$INSTALL_DIR/gatewayd"
chmod 755 "$INSTALL_DIR/gatewayd"

# 把串口稳定名生成器一并装到 /opt/gatewayd：现场后插设备、或换了 USB 口时
# 需要能直接重跑它重新生成映射，不能依赖交付包还在手边。
if [ -n "$SERIAL_RULES_SRC" ] && [ -f "$SERIAL_RULES_SRC" ]; then
    cp "$SERIAL_RULES_SRC" "$INSTALL_DIR/serial-rules.sh"
    chmod 755 "$INSTALL_DIR/serial-rules.sh"
fi

# 4. 安装前端静态资源
echo "[4/7] 安装前端..."
if [ -n "$UI_DIST" ] && [ -d "$UI_DIST" ] && [ -f "$UI_DIST/index.html" ]; then
    rm -rf "$UI_DIR"/*
    cp -r "$UI_DIST"/. "$UI_DIR"/
    chmod -R a+r "$UI_DIR"
    echo "  ✓ 前端已安装到 $UI_DIR (index.html + $(find "$UI_DIR" -type f | wc -l) 文件)"
else
    echo "  ⚠ 未找到前端产物，网关将返回内置调试控制台 (WEB_UI_HTML fallback)"
    echo "    生产部署应把 ui-dist（含 index.html）放在本脚本同级目录"
fi

# 4.5 预加载 USB 串口驱动模块
#
# 为什么要这一步：gateway.service 用 `DeviceAllow=char-ttyUSB rw` 这种
# **设备组形式**放行串口（路径形式 /dev/ttyUSB 在 systemd v255 会被 stat
# 失败后整条丢弃，见 gateway.service 的注释）。设备组名必须在服务启动那一刻
# 能在 /proc/devices 里解析 —— 即驱动模块已加载。若 cp210x / ch341 等模块
# 还没加载，/proc/devices 里没有 ttyUSB 组名，systemd 解析失败 →
# 整条 DeviceAllow 被丢弃 → 程序 open() 之前就被 EPERM 拒绝 →
# **全部串口协议采集不上来**（且无任何报错日志）。
#
# 写进 /etc/modules-load.d/ 让 systemd-modules-load.service 在开机时加载，
# 早于我们的服务启动。
echo "[4.5/7] 预加载 USB 串口驱动模块..."
MODULES_DIR="/etc/modules-load.d"
MODULES_FILE="$MODULES_DIR/gateway-serial.conf"
# 只写实际存在（或可能存在）的模块名：modprobe 找不到的模块会在
# systemd-modules-load.service 里报错，虽不影响开机但会污染 journal。
# 这里覆盖常见的几种 USB 转串口芯片。
SERIAL_MODULES=()
# modinfo 不存在（极简容器 / BusyBox）时不要刷 "command not found" 噪声
if command -v modinfo >/dev/null 2>&1; then
    for m in cp210x ch341 ftdi_sio pl2303 cdc_acm; do
        if modinfo "$m" >/dev/null 2>&1; then
            SERIAL_MODULES+=("$m")
        fi
    done
fi
if [ ${#SERIAL_MODULES[@]} -gt 0 ]; then
    mkdir -p "$MODULES_DIR"
    printf '%s\n' "${SERIAL_MODULES[@]}" > "$MODULES_FILE"
    # 立即加载，不等下次重启（否则本次安装后服务仍可能起不来）
    for m in "${SERIAL_MODULES[@]}"; do
        modprobe "$m" 2>/dev/null || true
    done
    echo "  ✓ 已写入 $MODULES_FILE 并立即加载：${SERIAL_MODULES[*]}"
else
    echo "  ⚠ 未发现可用的 USB 串口驱动模块（cp210x/ch341/ftdi_sio/pl2303/cdc_acm）"
    echo "    若现场用 USB 串口，请手动确认驱动已加载，否则串口采集会被 systemd 拒绝"
fi

# 4.6 生成串口稳定名 udev 规则
#
# 为什么要这一步：4.5 只保证**驱动模块**已加载，但设备名本身仍会漂移。
# USB 串口的 /dev/ttyUSB* 编号由枚举顺序决定，重启一次可能变成
# /dev/ttyUSB0 → /dev/ttyUSB1。config.json 里的 buses[].port_name 是
# 裸字符串，直接进 tokio_serial::new()，名字对不上 → open() 失败 →
# **采集全线静默停摆**，且日志里只有「找不到设备」，现场极易误判为硬件故障。
#
# 这里为每个物理串口生成一个跨重启稳定的名字 /dev/gateway-serialN，
# 并把「序号 ↔ 物理设备」的绑定记在 /etc/gateway/serial-bindings.tsv：
# 设备下次枚举成别的名字时，绑定不洗牌，配置里的 port_name 不用改。
echo "[4.6/7] 生成串口稳定名 udev 规则..."
if [ -n "$SERIAL_RULES_SRC" ]; then
  # 失败不阻断安装：没有稳定名只是退回到「人工保证设备名不变」的老办法，
  # 阻断安装反而会让现场完全装不上。
  if bash "$SERIAL_RULES_SRC" --rules-file /etc/udev/rules.d/60-gateway-serial.rules \
       --state-file "$CONFIG_DIR/serial-bindings.tsv"; then
    echo ""
    echo "  → 请把上面映射里的 /dev/gateway-serialN 填进 config.json 的 buses[].port_name"
    echo "    （不要直接填 /dev/ttyUSB*，重启后编号会变）"
  else
    echo "  ⚠ 串口稳定名生成失败，继续安装（port_name 仍需人工保证设备名不变）"
  fi
else
  echo "  ⚠ 未找到 serial-rules.sh，跳过稳定名生成"
fi

# 5. 拷贝配置和模板
echo "[5/7] 安装配置和模板..."
# 升级不覆盖：/etc/gateway/config.json 已存在则保留
# 只接受完整的 config.json：仓库里的 config.json.example 是「配置片段示例」，
# 文件本身明确写着不会被网关加载，装成 config.json 反而会得到一份没有 web 段的配置。
if [ ! -f "$CONFIG_DIR/config.json" ]; then
    if [ -f "$SCRIPT_DIR/config.json" ]; then
        cp "$SCRIPT_DIR/config.json" "$CONFIG_DIR/config.json"
        chmod 640 "$CONFIG_DIR/config.json"
        echo "  ✓ 写入 config.json（来源: 包内 config.json）"
    else
        # 包内没有现场 config.json 时，用出厂完整模板（P3-1b）兜底：
        # 它是一份能被网关真加载的完整配置（一条示例总线 + 一个示例点位 + web +
        # license 四档），交付方只需改现场相关字段，不必从零手写。
        # 注意 priority：包内 config.json（现场定制）始终优先于模板。
        if [ -f "$SCRIPT_DIR/config.factory.json" ]; then
            cp "$SCRIPT_DIR/config.factory.json" "$CONFIG_DIR/config.json"
            chmod 640 "$CONFIG_DIR/config.json"
            echo "  ✓ 写入 config.json（来源: 包内出厂模板 config.factory.json）"
            echo "    投运前请按现场改：buses[].port_name（建议用 serial-rules.sh 生成的稳定名）、"
            echo "    points（示例点位 demo_modbus_1 仅用于开箱验链路）、mqtt.broker。"
            echo "    web.api_token 留空 → 启动时随机生成并打印在 journalctl 日志。"
            echo "    web.listen_addr 默认 127.0.0.1:8080（仅本机可访问）。要开放车间 LAN："
            echo "    先用 admin/admin 登录 → POST /api/auth/password 改密（哈希写回 config.json）"
            echo "    → 再把 web.listen_addr 改成 0.0.0.0:8080 并重启。"
            echo "    ⚠ 未设 password_hash 就改成 0.0.0.0 会触发 fail-closed，网关拒绝启动。"
        else
            echo "  ⚠ 未提供 config.json：gatewayd 将以安全默认值启动"
            echo "    （仅监听 127.0.0.1:8080，api_token 自动生成并打印在 journalctl 日志）"
            echo "    需要 LAN 访问/固定 token 时，请按现场补一份完整 /etc/gateway/config.json 后重启服务"
        fi
    fi
else
    echo "  跳过 config.json（已存在，保留原值）"
fi
if [ -n "$TEMPLATES_SRC" ]; then
    cp -r "$TEMPLATES_SRC/builtin/." "$CONFIG_DIR/templates/builtin/" 2>/dev/null || true
fi

# ── 解析健康检查地址（此时 config.json 已落盘）──
# gatewayd 的监听地址来自配置而非环境变量；自定义端口/绑定时若仍打 8080，
# 会把「服务其实已正常」误判成安装失败。通配绑定退回回环，IPv6 字面量加方括号。
if [ -z "$PORT" ]; then
    PORT=8080
    if [ -f "$CONFIG_DIR/config.json" ] && command -v python3 >/dev/null 2>&1; then
        BIND_PAIR=$(python3 -c 'import json, sys
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
print(host, port)' "$CONFIG_DIR/config.json" 2>/dev/null || true)
        if [ -n "$BIND_PAIR" ]; then
            read -r DETECT_HOST DETECT_PORT <<< "$BIND_PAIR"
            [ -n "$DETECT_HOST" ] && HEALTH_HOST="$DETECT_HOST"
            case "$DETECT_PORT" in
                ''|*[!0-9]*) : ;;
                *) PORT="$DETECT_PORT" ;;
            esac
        fi
    fi
fi

# 6. 设置权限
echo "[6/7] 设置权限..."
chown -R gateway:gateway "$DATA_DIR" "$LOG_DIR"
# 配置不能被普通用户读（含 api_token），但 gateway 进程（User=gateway）必须能读：
# 归 root:gateway + 组读即可，旧实现 root:root 640 会让服务启动后读不到配置
chown -R root:gateway "$CONFIG_DIR"
chmod -R g+rX,o-rwx "$CONFIG_DIR"
# 目录额外需要**组可写**：ConfigLoader::save() → atomic_write() 的临时文件建在
# 目标同目录（/etc/gateway/）再 rename 覆盖，目录不可写时所有在线改配置端点
# 都会 500「runtime reloaded but disk save failed」——运行时已改、磁盘未改，重启回滚。
# 文件本身保持组只读（g+r，不给 g+w）：写回走的是 rename 替换，不需要文件写权限。
find "$CONFIG_DIR" -type d -exec chmod g+rwx,o-rwx {} +
chown -R root:root "$INSTALL_DIR"
# gateway 进程需要能读 UI
chmod -R a+rX "$UI_DIR"

# 6.5 journald 日志限额
#
# 为什么必须在这一步落地：
#   gateway.service 用 StandardOutput=journal，日志落在 /var/log/journal，
#   与 /var/lib/gateway 的 SQLite 数据队列**同一个盘**。journald 没有磁盘占用
#   上限时，长周期运行 + 异常刷屏会把盘写满；而磁盘满会直接卡住 SQLite 入库 →
#   表现为「数据停采而不是报错」，现场极难定位。
#
# SystemMaxUse= / SystemKeepFree= 是 journald.conf 的 [Journal] 段选项，
# **写进 gateway.service 无效**（unit 里没有 [Journal] 段，systemd 会忽略）。
# 正确位置是 /etc/systemd/journald.conf.d/*.conf（drop-in）：
#   - 不污染发行版自带的 /etc/systemd/journald.conf（升级不冲突）
#   - drop-in 优先级高于主文件；同名选项按文件名字典序取最后一个，
#     所以用 60- 前缀（官方建议 /etc/ 下用 60-90 区间，压过发行版自带的 10-40）
# 运行时限流（LogRateLimitIntervalSec/LogRateLimitBurst）写在 unit 里，
# 只挡「日志风暴」，不限制磁盘占用 —— 两者是不同层面的保险，缺一不可。
echo "[6.5/7] 配置 journald 日志限额..."
JOURNAL_CONF_DIR="/etc/systemd/journald.conf.d"
JOURNAL_CONF="$JOURNAL_CONF_DIR/60-gateway-journal.conf"
# 可用环境变量覆盖（不同 eMMC 容量差异大）：
#   GATEWAY_JOURNAL_SYSTEM_MAX_USE  持久化 journal 占用上限（/var/log/journal）
#   GATEWAY_JOURNAL_SYSTEM_KEEP_FREE 为数据盘预留的空闲空间（journald 取两者较小值）
#   GATEWAY_JOURNAL_RUNTIME_MAX_USE 易失 journal 上限（/run/log/journal，内存盘）
JOURNAL_SYSTEM_MAX_USE="${GATEWAY_JOURNAL_SYSTEM_MAX_USE:-512M}"
JOURNAL_SYSTEM_KEEP_FREE="${GATEWAY_JOURNAL_SYSTEM_KEEP_FREE:-1G}"
JOURNAL_RUNTIME_MAX_USE="${GATEWAY_JOURNAL_RUNTIME_MAX_USE:-64M}"
JOURNAL_CONF_BODY="[Journal]
# SystemMaxUse 只作用于持久化 journal（/var/log/journal）。
# 若 Storage=volatile（发行版常发 #Storage=auto），日志落在 /run 的内存盘，
# 上限根本用不上 —— 显式写 persistent 让限额真正生效。
# 持久化目录不可写时 systemd 会自动回落到 /run，不影响服务启动。
Storage=persistent
SystemMaxUse=$JOURNAL_SYSTEM_MAX_USE
SystemKeepFree=$JOURNAL_SYSTEM_KEEP_FREE
RuntimeMaxUse=$JOURNAL_RUNTIME_MAX_USE"
# 写 drop-in 失败不阻断安装（日志限额是保护措施，不该让整个装机流程挂掉）；
# 但必须明确告警 —— 否则现场会以为限额已生效。
JOURNAL_WRITE_OK=1
mkdir -p "$JOURNAL_CONF_DIR" 2>/dev/null || JOURNAL_WRITE_OK=0
if [ "$JOURNAL_WRITE_OK" = "1" ]; then
  {
    echo "# 由 gateway install.sh 生成（$(date +%F)）"
    echo "# 服务日志（StandardOutput=journal）与 SQLite 数据队列同盘，"
    echo "# 必须限制 journal 占用上限，否则日志写满磁盘会导致入库暂停（表现为数据停采）。"
    printf '%s\n' "$JOURNAL_CONF_BODY"
  } > "$JOURNAL_CONF" 2>/dev/null || JOURNAL_WRITE_OK=0
fi
if [ "$JOURNAL_WRITE_OK" = "1" ]; then
  chmod 644 "$JOURNAL_CONF" 2>/dev/null || true
  echo "  ✓ 已写入 $JOURNAL_CONF"
  echo "    SystemMaxUse=$JOURNAL_SYSTEM_MAX_USE  SystemKeepFree=$JOURNAL_SYSTEM_KEEP_FREE  RuntimeMaxUse=$JOURNAL_RUNTIME_MAX_USE"
  # 立即生效需要重启 journald；失败不阻断安装（重启机器后同样生效）
  if systemctl restart systemd-journald 2>/dev/null; then
    echo "  ✓ systemd-journald 已重启，日志限额立即生效"
  else
    echo "  ⚠ 重启 systemd-journald 失败，限额将在下次重启后生效"
  fi
else
  echo "  ⚠ 写入 $JOURNAL_CONF 失败 —— 日志限额未生效，日志仍可能占满数据盘"
  echo "    处置: 手动创建 /etc/systemd/journald.conf.d/60-gateway-journal.conf，"
  echo "          写入 [Journal] / Storage=persistent / SystemMaxUse=$JOURNAL_SYSTEM_MAX_USE / SystemKeepFree=$JOURNAL_SYSTEM_KEEP_FREE，"
  echo "          然后 sudo systemctl restart systemd-journald"
fi

# 7. 安装 systemd 服务
echo "[7/7] 安装 systemd 服务..."
if [ -z "$SERVICE_SRC" ]; then
    echo "❌ 错误: 找不到 gateway.service（应位于脚本同级的 gateway.service）"
    exit 1
fi
# 把 GATEWAY_WEB_DIR 注入 service（让 Rust 从 /opt/gatewayd/ui-dist 找前端）
SERVICE_CONTENT="$(cat "$SERVICE_SRC")"
# 如果 service 里没写 Environment=GATEWAY_WEB_DIR，补上
if ! echo "$SERVICE_CONTENT" | grep -q "GATEWAY_WEB_DIR"; then
    SERVICE_CONTENT="$(echo "$SERVICE_CONTENT" | sed '/^Environment=RUST_LOG=/a Environment=GATEWAY_WEB_DIR='"$UI_DIR"'')"
fi
# 把时区显式注入 service：计划任务按**墙钟**解释 cron 表达式，
# systemd 服务不带 TZ 时跟随 /etc/localtime；精简镜像缺 tzdata 时会静默退化成 UTC，
# 表现为「每天凌晨 2 点」整体差 8 小时且无任何报错。
# 优先级：GATEWAY_TZ（固定偏移，不依赖 tzdata） > TZ（系统时区名，含夏令时）
if [ -n "${GATEWAY_TZ:-}" ]; then
    TZ_ENV_LINE="Environment=GATEWAY_TZ=${GATEWAY_TZ}"
    echo "  ✓ 时区: GATEWAY_TZ=${GATEWAY_TZ}（固定 UTC 偏移）"
elif [ -n "${TZ:-}" ]; then
    TZ_ENV_LINE="Environment=TZ=${TZ}"
    echo "  ✓ 时区: TZ=${TZ}（系统时区名）"
else
    # 现场没给 TZ：探测系统时区并显式写入，避免服务跟随不到 /etc/localtime
    SYSTEM_TZ="$(detect_system_tz)"
    if [ -n "$SYSTEM_TZ" ]; then
        TZ_ENV_LINE="Environment=TZ=${SYSTEM_TZ}"
        echo "  ✓ 时区: TZ=${SYSTEM_TZ}（自动探测）"
    else
        # 探测不到就不写空值：Environment=TZ= 会被 systemd 当成空时区，反而更糟
        TZ_ENV_LINE=""
        echo "  ⚠ 未探测到系统时区，服务将按 UTC 运行"
        echo "    计划任务「每天 02:00」会按 UTC 解释（本地为 UTC+8 时差 8 小时）"
        echo "    修复: 重新执行时带上 GATEWAY_TZ=+08:00，或在 /etc/systemd/system/gateway.service"
        echo "          里加 Environment=TZ=Asia/Shanghai 后 systemctl restart gateway"
    fi
fi
if [ -n "$TZ_ENV_LINE" ] && ! echo "$SERVICE_CONTENT" | grep -qE "^Environment=(GATEWAY_TZ|TZ)="; then
    SERVICE_CONTENT="$(echo "$SERVICE_CONTENT" | sed '/^Environment=RUST_LOG=/a '"$TZ_ENV_LINE")"
fi
# OTA A/B 分区：把 **DeviceAllow 白名单** 与 **Environment 变量** 同步改写到实际介质。
# 这两处必须同时改，缺一 OTA 现场直接失败：
#   - 只改 Environment 不同步 DeviceAllow → systemd 在 open() 之前 EPERM 拒绝
#   - 只改 DeviceAllow 不同步 Environment → 进程仍去写默认的 /dev/mmcblk0p2
# 规则：
#   1) 标记块（# OTA-PARTITIONS-BEGIN/END）内的 DeviceAllow 整块替换为本机分区
#   2) 块外的同款行也替换（防止标记被手工删掉后留下默认值）
#   3) Environment 赋值必须**有且只有一组**：
#      - unit 里已有注释态示例（# Environment=GATEWAY_OTA_SLOT_A=…）→ 就地取消注释并替换值
#      - 没有示例 → 在 Environment=RUST_LOG= 之后补写；连 RUST_LOG 都没有 → 写进标记块内
#      （曾经两条路径都会命中，导致同一变量被写两遍 —— systemd 取最后一行，
#        看似能跑，但 unit 与实际介质不一致时排查极困难）
if printf '%s\n' "$SERVICE_CONTENT" | grep -E '^#? *Environment=GATEWAY_OTA_SLOT_A=' >/dev/null 2>&1; then
    HAVE_SLOT_ENV=1
else
    HAVE_SLOT_ENV=0
fi
SERVICE_CONTENT="$(printf '%s\n' "$SERVICE_CONTENT" | awk -v a="$OTA_SLOT_A" -v b="$OTA_SLOT_B" -v haveenv="$HAVE_SLOT_ENV" '
    /^# OTA-PARTITIONS-BEGIN/ {
        inblk=1; print
        print "DeviceAllow=" a " rw"; print "DeviceAllow=" b " rw"
        if (haveenv + 0 == 0 && !placed) {
            print "Environment=GATEWAY_OTA_SLOT_A=" a
            print "Environment=GATEWAY_OTA_SLOT_B=" b
            placed=1
        }
        next
    }
    /^# OTA-PARTITIONS-END/   { inblk=0; print; next }
    inblk && /^DeviceAllow=/   { next }
    /^DeviceAllow=\/dev\/mmcblk0p2/ { print "DeviceAllow=" a " rw"; next }
    /^DeviceAllow=\/dev\/mmcblk0p3/ { print "DeviceAllow=" b " rw"; next }
    /^# *Environment=GATEWAY_OTA_SLOT_A=/ { print "Environment=GATEWAY_OTA_SLOT_A=" a; placed=1; next }
    /^# *Environment=GATEWAY_OTA_SLOT_B=/ { print "Environment=GATEWAY_OTA_SLOT_B=" b; placed=1; next }
    /^Environment=GATEWAY_OTA_SLOT_A=/ { print "Environment=GATEWAY_OTA_SLOT_A=" a; placed=1; next }
    /^Environment=GATEWAY_OTA_SLOT_B=/ { print "Environment=GATEWAY_OTA_SLOT_B=" b; placed=1; next }
    /^Environment=RUST_LOG=/ && haveenv + 0 == 0 && !placed {
        print
        print "Environment=GATEWAY_OTA_SLOT_A=" a
        print "Environment=GATEWAY_OTA_SLOT_B=" b
        placed=1
        next
    }
    { print }
')"
echo "  ✓ OTA 分区: $OTA_SLOT_A / $OTA_SLOT_B（已同步写入 DeviceAllow 与 Environment）"
echo "$SERVICE_CONTENT" > "$SERVICE_FILE"
systemctl daemon-reload
systemctl enable gateway

if [ "$START_SERVICE" -eq 1 ]; then
    echo "  → 启动并等待健康检查..."
    systemctl restart gateway
    HEALTH_URL="http://$HEALTH_HOST:$PORT/health/live"
    READY=0
    for _ in $(seq 1 30); do
        CODE=$(curl -s -o /dev/null -w "%{http_code}" "$HEALTH_URL" 2>/dev/null || echo "000")
        if [ "$CODE" = "200" ]; then READY=1; break; fi
        sleep 1
    done
    if [ "$READY" -eq 1 ]; then
        echo "  ✓ gateway.service 已启动，$HEALTH_URL → 200"
    else
        echo "  ❌ gateway.service 已安装但健康检查未通过（$HEALTH_URL 未返回 200）"
        echo "     诊断: sudo journalctl -u gateway -n 50 --no-pager"
        exit 1
    fi
else
    echo "  → 已跳过启动（--no-start），可稍后手动 sudo systemctl start gateway"
fi

echo ""
echo "═══════════════════════════════════════════════"
echo " ✓ 部署完成！"
echo ""
echo " 状态: sudo systemctl status gateway"
echo " 日志: sudo journalctl -u gateway -f"
echo " 前端: http://<host>:$PORT/  (GATEWAY_WEB_DIR=$UI_DIR)"
echo " 配置: $CONFIG_DIR/config.json"
echo ""
echo " 下一步（一条命令完成验证 + 交付检查单导出）:"
echo "   sudo bash deliver.sh"
echo "═══════════════════════════════════════════════"
