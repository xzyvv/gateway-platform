#!/usr/bin/env bash
set -euo pipefail

# ────────────────────────────────────────────────────────────
# 串口稳定名生成器
#
# 解决的问题：USB 串口重启后设备名会漂移。
#   插着两个 CP210x 时，重启一次可能变成 /dev/ttyUSB0 /dev/ttyUSB1，
#   再重启一次顺序又可能反过来。配置里的 port_name 是**裸字符串**，
#   直接进 tokio_serial::new()，设备名一变 → open() 失败 →
#   采集全线静默停摆，且日志里只有「找不到设备」，现场极易误判为硬件故障。
#
# 做法：给每个物理串口生成一个**跨重启稳定**的名字 /dev/gateway-serialN，
# 由 udev 规则维护（设备真正出现时才创建软链）。
#
# ── 为什么不用 ENV{ID_SERIAL} ──
# 这是一个踩过的坑，改动前务必读完，否则会重犯：
#
#   1) 属性导入时机 —— systemd 的 /usr/lib/udev/rules.d/60-serial.rules 才是
#      导入 ID_SERIAL / ID_PATH 的地方，它的写法是「先 IMPORT{builtin} 导入，
#      再用 ENV{...} 匹配」。udev 按**文件名字典序**处理规则文件，本脚本生成
#      的 60-gateway-serial.rules 排在 60-serial.rules **之前**（g < s），
#      我方规则执行时 tty 设备自身还没被导入这些属性 → ENV{ID_SERIAL} 永远
#      匹配不上 → 软链永不生成，整套机制静默失效。
#      （udev 属性不跨设备继承：父设备 usb_device 上的 ID_SERIAL 不会自动
#        出现在子设备 ttyUSB0 的属性里。）
#
#   2) ID_SERIAL 本身不是唯一标识 —— 它是「厂商_型号[_序列号]」的复合串，
#      **设备没有序列号时它依然非空**（只有 厂商_型号）。两块同型号、无序列号
#      的转换器（如 CH340 克隆）会得到完全相同的 ID_SERIAL → 同一条规则同时
#      命中两个设备 → 抢同一个 /dev/gateway-serialN，后写覆盖前写，
#      配置里的 port_name 指向错误设备。
#
# ── 本脚本采用的稳定标识 ──
# 优先级从高到低：
#   1. 原始 USB 序列号 —— 规则用 ATTRS{serial} + ATTRS{idVendor} +
#      ATTRS{idProduct} + ATTRS{bInterfaceNumber} 匹配。
#      ATTRS{} 直接匹配 sysfs 属性、沿父链向上查找，**不依赖任何 udev builtin
#      导入、不依赖规则文件顺序**；匹配到的 serial 是原始序列号（真正唯一），
#      而不是 ID_SERIAL 那种复合串。sysfs 的 serial 属性只在设备**真的有**
#      序列号时才存在，天然避免「无序列号却当成唯一」的误判。
#      代价：设备换 USB 口后序列号仍不变（优点），但物理路径会变——
#      本方案用序列号而非路径，正是为了扛住换口。
#
#   2. 多口设备（同一 USB 设备出多个串口，如 FTDI 四口）**不能用序列号**：
#      各口序列号相同。此时退到 3（物理路径）。
#      判定方式：sysfs 下该 usb_device 的接口数 > 1，或单接口下端口数 > 1。
#
#   3. 物理路径（sysfs 设备路径）—— 跨重启稳定，换 USB 口会变。
#      规则用 ENV{DEVPATH} 匹配（DEVPATH 是 udev 内核属性，任何规则都能用，
#      不需要 IMPORT{builtin}）。取 tty 设备的**父目录**（usb 接口或接口端口），
#      因此多口设备的各个口也能分开。
#
#   4. 都没有 → 该设备无法稳定命名，脚本会明确报出来，不猜。
#
# 已有绑定必须保留：现场设备换了 /dev/ttyUSB* 名字（甚至换了物理位置），
# 配置里的 port_name 不能跟着变。因此绑定关系存在状态文件里，
# 重新生成时按「稳定标识」复用原来的序号，不做重新洗牌。
#
# 用法：
#   sudo bash serial-rules.sh                 # 生成规则 + 打印映射表
#   sudo bash serial-rules.sh --list          # 只看当前设备与拟分配名
#   bash serial-rules.sh --dry-run            # 只打印，不落盘
#   bash serial-rules.sh --rules-file /tmp/x.rules --state-file /tmp/x.tsv
#
# 无硬件测试：设置 GW_SERIAL_DEVICES_FILE 指向一份设备清单，
# 即可在 CI 上跑完整逻辑。清单每行 7 段，用 | 分隔：
#   设备名 | 厂商ID | 产品ID | 原始序列号 | 接口号 | 多口标记 | 设备路径
#   例：/dev/ttyUSB0|0403|6001|A10K7VNA|00|0|/devices/pci0000:00/0000:00:14.0/usb1/1-1/1-1:1.0
# ────────────────────────────────────────────────────────────

PREFIX="gateway-serial"
DEFAULT_RULES="/etc/udev/rules.d/60-gateway-serial.rules"
DEFAULT_STATE="/etc/gateway/serial-bindings.tsv"
FIXTURE="${GW_SERIAL_DEVICES_FILE:-}"
SEP="|"

RULES_FILE="$DEFAULT_RULES"
STATE_FILE="$DEFAULT_STATE"
DRY_RUN=0
LIST_ONLY=0

usage() {
    # 打印文件头部的注释块（跳过 shebang 与第一段连续注释之间的代码行）
    awk '
        NR == 1 { next }
        /^#/   { if (!started) started = 1; sub(/^# ?/, ""); print; next }
        started { exit }
    ' "${BASH_SOURCE[0]}"
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --rules-file) RULES_FILE="$2"; shift 2 ;;
        --rules-file=*) RULES_FILE="${1#*=}"; shift ;;
        --state-file) STATE_FILE="$2"; shift 2 ;;
        --state-file=*) STATE_FILE="${1#*=}"; shift ;;
        --dry-run) DRY_RUN=1; shift ;;
        --list) LIST_ONLY=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "错误: 未知参数 '$1'"; exit 1 ;;
    esac
done

# udev 规则里双引号字符串需要转义 \ 与 "
udev_escape() {
    local s="$1"
    s="${s//\\/\\\\}"
    s="${s//\"/\\\"}"
    printf '%s' "$s"
}

# ── 采集设备列表 ──
# 输出每行: 设备路径 | 厂商ID | 产品ID | 原始序列号 | 接口号 | 多口标记 | 设备路径(DEVPATH)
#
# 全部从 sysfs 直接读，不依赖 udev 属性是否已被导入：
#   * /sys/class/tty/ttyUSBN 是符号链接，readlink -f 后取其父目录 = usb 接口
#     （多口设备则是「接口:端口」子目录），这个路径就是 DEVPATH 的前缀。
#   * 沿父链向上找带 idVendor/idProduct 的目录 = usb_device，序列号在其上。
#   * bInterfaceNumber 在 usb 接口上。
collect_devices() {
    if [ -n "$FIXTURE" ]; then
        # 测试夹具：跳过注释与空行
        grep -v '^[[:space:]]*#' "$FIXTURE" 2>/dev/null | grep -v '^[[:space:]]*$' || true
        return 0
    fi

    local dev base rp parent usbdev vendor product serial ifnum multi devpath d n iface
    for dev in /dev/ttyUSB* /dev/ttyACM*; do
        [ -e "$dev" ] || continue
        base="$(basename "$dev")"
        [ -d "/sys/class/tty/$base" ] || continue

        rp="$(readlink -f "/sys/class/tty/$base" 2>/dev/null || true)"
        [ -n "$rp" ] || continue
        parent="$(dirname "$rp")"

        # 向上找到 usb_device（唯一带 idVendor/idProduct 的那一层）
        usbdev=""; d="$parent"
        while [ -n "$d" ] && [ "$d" != "/" ] && [ "$d" != "." ]; do
            if [ -f "$d/idVendor" ] && [ -f "$d/idProduct" ]; then usbdev="$d"; break; fi
            d="$(dirname "$d")"
        done
        [ -n "$usbdev" ] || continue

        vendor="$(cat "$usbdev/idVendor" 2>/dev/null || true)"
        product="$(cat "$usbdev/idProduct" 2>/dev/null || true)"

        # 原始序列号：sysfs 的 serial 属性只在设备真的有序列号时才存在
        serial=""
        if [ -f "$usbdev/serial" ]; then
            serial="$(cat "$usbdev/serial" 2>/dev/null || true)"
        fi

        # 接口号：最近的带 bInterfaceNumber 的祖先
        ifnum=""; d="$parent"
        while [ -n "$d" ] && [ "$d" != "/" ] && [ "$d" != "." ]; do
            if [ -f "$d/bInterfaceNumber" ]; then
                ifnum="$(cat "$d/bInterfaceNumber" 2>/dev/null || true)"
                break
            fi
            d="$(dirname "$d")"
        done

        # 多口判定：接口数 > 1，或单接口下的端口数 > 1
        multi=0
        n="$(ls -d "$usbdev":*:1.[0-9] 2>/dev/null | wc -l)"
        [ "$n" -gt 1 ] && multi=1
        iface="$parent"
        case "$(basename "$parent")" in
            *:1.[0-9]:*) iface="$(dirname "$parent")" ;;
        esac
        n="$(ls -d "$iface":*:1.[0-9]:* 2>/dev/null | wc -l)"
        [ "$n" -gt 1 ] && multi=1

        # DEVPATH = sysfs 路径去掉 /sys 前缀
        devpath="${parent#/sys}"

        printf '%s|%s|%s|%s|%s|%s|%s\n' "$dev" "$vendor" "$product" "$serial" "$ifnum" "$multi" "$devpath"
    done
}

# ── 读入已有绑定状态 ──
# 状态文件每行: 序号 | 稳定标识 | 设备路径 | udev 匹配片段
# 它是「序号 ↔ 物理设备」的唯一权威：设备换了 /dev/ttyUSB* 名字后，
# 靠这里的稳定标识把序号找回来，配置里的 port_name 因此不用改。
STATE_LINES=()

load_state() {
    STATE_LINES=()
    [ -f "$STATE_FILE" ] || return 0
    local line
    while IFS= read -r line; do
        [ -n "$line" ] || continue
        case "$line" in \#*) continue ;; esac
        STATE_LINES+=("$line")
    done < "$STATE_FILE"
}

# 按稳定标识查已有序号；查不到返回空
lookup_index() {
    local key="$1" line k
    for line in "${STATE_LINES[@]:-}"; do
        [ -n "$line" ] || continue
        k="$(printf '%s\n' "$line" | cut -d"$SEP" -f2)"
        if [ "$k" = "$key" ]; then
            printf '%s' "$(printf '%s\n' "$line" | cut -d"$SEP" -f1)"
            return 0
        fi
    done
    return 1
}

# 已有状态里的最大序号；没有状态时返回 -1
max_index() {
    local line idx m=-1
    for line in "${STATE_LINES[@]:-}"; do
        [ -n "$line" ] || continue
        idx="$(printf '%s\n' "$line" | cut -d"$SEP" -f1)"
        case "$idx" in
            ''|*[!0-9]*) continue ;;
        esac
        [ "$idx" -gt "$m" ] && m="$idx"
    done
    printf '%s' "$m"
}

# ── 主流程 ──
load_state

DEV_ROWS=()
while IFS= read -r line; do
    [ -n "$line" ] || continue
    DEV_ROWS+=("$line")
done < <(collect_devices)

# ── 第一遍：为每个设备算「候选稳定标识 + 候选匹配片段」──
# 序列号可用且不是多口设备 → 用序列号；否则退到设备路径。
A_DEV=()       # 设备路径
A_KEY=()       # 候选稳定标识
A_RULE=()      # 候选 udev 匹配片段
A_LAYER=()     # 1=序列号 2=路径
A_DEVPATH=()   # 设备路径（退到路径层时使用）
UNSTABLE=()

for row in "${DEV_ROWS[@]:-}"; do
    [ -n "$row" ] || continue
    IFS="$SEP" read -r dev vendor product serial ifnum multi devpath <<< "$row"
    [ -n "$dev" ] || continue

    if [ -n "$serial" ] && [ "$multi" != "1" ]; then
        # 序列号优先：换 USB 口也不变。
        key="serial:${serial}@${vendor}:${product}#${ifnum}"
        rule="SUBSYSTEM==\"tty\", ATTRS{idVendor}==\"$(udev_escape "$vendor")\", ATTRS{idProduct}==\"$(udev_escape "$product")\", ATTRS{serial}==\"$(udev_escape "$serial")\""
        if [ -n "$ifnum" ]; then
            rule="${rule}, ATTRS{bInterfaceNumber}==\"$(udev_escape "$ifnum")\""
        fi
        A_DEV+=("$dev"); A_KEY+=("$key"); A_RULE+=("$rule"); A_LAYER+=(1); A_DEVPATH+=("$devpath")
    elif [ -n "$devpath" ]; then
        # 无序列号，或多口设备（各口序列号相同）→ 退到物理路径
        key="path:${devpath}"
        rule="SUBSYSTEM==\"tty\", ENV{DEVPATH}==\"$(udev_escape "$devpath")/tty/*\""
        A_DEV+=("$dev"); A_KEY+=("$key"); A_RULE+=("$rule"); A_LAYER+=(2); A_DEVPATH+=("$devpath")
    else
        UNSTABLE+=("$dev")
    fi
done

# ── 第二遍：稳定标识撞车的设备退到物理路径 ──
# 典型场景：两块同型号、伪造了相同序列号的转换器。序列号相同 → 同一条规则会
# 同时命中两个设备 → 抢同一个软链。此时必须用物理路径把它们分开。
declare -A KEYCOUNT=()
for k in "${A_KEY[@]:-}"; do
    [ -n "$k" ] || continue
    KEYCOUNT["$k"]=$(( ${KEYCOUNT["$k"]:-0} + 1 ))
done

for i in "${!A_DEV[@]}"; do
    key="${A_KEY[$i]}"
    if [ "${KEYCOUNT["$key"]:-0}" -le 1 ]; then
        continue
    fi
    devpath="${A_DEVPATH[$i]}"
    if [ -z "$devpath" ]; then
        UNSTABLE+=("${A_DEV[$i]}（稳定标识与其它设备冲突：${key}）")
        continue
    fi
    A_KEY[$i]="path:${devpath}"
    A_RULE[$i]="SUBSYSTEM==\"tty\", ENV{DEVPATH}==\"$(udev_escape "$devpath")/tty/*\""
    A_LAYER[$i]=2
done

# ── 第三遍：分配序号 ──
# 已有绑定优先复用（lookup_index），新设备才拿新序号；
# 序号用 CUR_MAX 递增分配，保证同一批多个新设备不会拿到同一个号。
NEW_IDX=()
NEW_KEY=()
NEW_DEV=()
NEW_RULE=()
CUR_MAX="$(max_index)"

for i in "${!A_DEV[@]}"; do
    key="${A_KEY[$i]}"
    idx="$(lookup_index "$key" || true)"
    if [ -z "$idx" ]; then
        CUR_MAX=$(( CUR_MAX + 1 ))
        idx="$CUR_MAX"
    elif [ "$idx" -gt "$CUR_MAX" ]; then
        CUR_MAX="$idx"
    fi
    NEW_IDX+=("$idx")
    NEW_KEY+=("$key")
    NEW_DEV+=("${A_DEV[$i]}")
    NEW_RULE+=("${A_RULE[$i]}")
done

if [ ${#NEW_DEV[@]} -eq 0 ] && [ ${#UNSTABLE[@]} -eq 0 ]; then
    echo "未发现 USB 串口设备（/dev/ttyUSB* / /dev/ttyACM*）"
    echo "  若现场使用 USB 串口，请确认驱动已加载（lsmod | grep -E 'cp210x|ch341|ftdi|pl2303|cdc_acm'）"
    exit 0
fi

# ── 输出映射表 ──
# 现场交付方需要照着这张表填 config.json 的 port_name，所以必须打印。
echo "串口稳定名映射（写进 config.json 的 buses[].port_name）:"
if [ ${#NEW_DEV[@]} -gt 0 ]; then
    for i in "${!NEW_DEV[@]}"; do
        printf '  %s%s -> %-18s [%s]\n' "$PREFIX" "${NEW_IDX[$i]}" "${NEW_DEV[$i]}" "${NEW_KEY[$i]}"
    done
else
    echo "  （无）"
fi
for u in "${UNSTABLE[@]:-}"; do
    [ -n "$u" ] || continue
    echo "  ⚠ $u —— 无法生成稳定名（设备既无序列号也无物理路径）"
done

if [ "$LIST_ONLY" -eq 1 ]; then
    exit 0
fi

# ── 合并「本次设备」与「离线保留的旧条目」──
# 离线设备必须保留规则：设备插回来时名字不变，
# 否则现场换根线重启一次，配置里的 port_name 就指向了别人的设备。
MERGED=()
for i in "${!NEW_IDX[@]}"; do
    MERGED+=("${NEW_IDX[$i]}${SEP}${NEW_KEY[$i]}${SEP}${NEW_DEV[$i]}${SEP}${NEW_RULE[$i]}")
done
for line in "${STATE_LINES[@]:-}"; do
    [ -n "$line" ] || continue
    key="$(printf '%s\n' "$line" | cut -d"$SEP" -f2)"
    still=0
    for i in "${!NEW_KEY[@]}"; do
        [ "${NEW_KEY[$i]}" = "$key" ] && still=1 && break
    done
    [ "$still" -eq 1 ] && continue
    MERGED+=("$line")
done

# 按序号升序排序（序号是十进制数字，用 sort -n）
SORTED=()
while IFS= read -r line; do
    [ -n "$line" ] || continue
    SORTED+=("$line")
done < <(printf '%s\n' "${MERGED[@]:-}" | sort -t"$SEP" -k1,1n)

# ── 写规则文件 ──
# 规则整体重写（而不是追加）：避免历史遗留规则与当前绑定不一致，
# 也保证同一份输入总是产出同一份文件（幂等）。
render_rules() {
    echo "# 由 gateway serial-rules.sh 生成，请勿手工编辑"
    echo "# 用途：为 USB 串口固定名字 /dev/${PREFIX}N，避免重启后 /dev/ttyUSB0 变成 /dev/ttyUSB1"
    echo "# 匹配一律用 sysfs 属性（ATTRS{}/DEVPATH），不依赖 udev builtin 导入顺序"
    echo "# 注释行记录：序号 | 稳定标识 | 当前设备路径"
    local line idx key dev rule offline found i
    for line in "${SORTED[@]:-}"; do
        [ -n "$line" ] || continue
        IFS="$SEP" read -r idx key dev rule <<< "$line"
        offline=""
        found=0
        for i in "${!NEW_IDX[@]}"; do
            [ "${NEW_IDX[$i]}" = "$idx" ] && found=1 && break
        done
        [ "$found" -eq 0 ] && offline=" (当前离线，规则保留)"
        echo "# ${PREFIX}${idx} | ${key} | ${dev}${offline}"
        echo "${rule}, SYMLINK+=\"${PREFIX}${idx}\""
    done
}

if [ "$DRY_RUN" -eq 1 ]; then
    echo ""
    echo "--- 规则预览（--dry-run，未写入）---"
    render_rules
    exit 0
fi

RULES_DIR="$(dirname "$RULES_FILE")"
STATE_DIR="$(dirname "$STATE_FILE")"
mkdir -p "$RULES_DIR" "$STATE_DIR"

render_rules > "$RULES_FILE.tmp"
mv "$RULES_FILE.tmp" "$RULES_FILE"
chmod 0644 "$RULES_FILE"

# ── 写状态文件 ──
printf '%s\n' "${SORTED[@]:-}" | grep -v '^$' > "$STATE_FILE.tmp" || true
mv "$STATE_FILE.tmp" "$STATE_FILE"
chmod 0644 "$STATE_FILE"

echo ""
echo "  ✓ 规则已写入: $RULES_FILE"
echo "  ✓ 绑定状态:   $STATE_FILE"

# ── 生效 ──
# 规则是「设备出现时才建软链」，所以重载规则后还要 trigger 一次，
# 让当前已经在位的设备立刻拿到软链（否则要等下次热插拔）。
if command -v udevadm >/dev/null 2>&1; then
    udevadm control --reload-rules 2>/dev/null || echo "  ⚠ udevadm control --reload-rules 失败（规则将在下次重启生效）"
    udevadm trigger --subsystem-match=tty 2>/dev/null || echo "  ⚠ udevadm trigger 失败（新设备需热插拔后生效）"
    sleep 1
    MISSING=0
    for i in "${!NEW_IDX[@]}"; do
        if [ ! -e "/dev/${PREFIX}${NEW_IDX[$i]}" ]; then
            echo "  ⚠ /dev/${PREFIX}${NEW_IDX[$i]} 未生成（udev 未生效，请人工检查 $RULES_FILE）"
            MISSING=1
        fi
    done
    [ "$MISSING" -eq 0 ] && echo "  ✓ 稳定名软链已就绪"
else
    echo "  ⚠ 未找到 udevadm：规则已写入，重启后生效"
fi
