#!/usr/bin/env bash
# 串口稳定名生成器的功能测试（无需真实硬件）
#
# 用夹具（GW_SERIAL_DEVICES_FILE）喂给 serial-rules.sh，验证：
#   T1  序列号优先：规则用 ATTRS{serial} 匹配，分配序号 0/1
#   T2  无序列号时退到物理路径（ENV{DEVPATH}）
#   T3  多口设备（各口序列号相同）必须退到物理路径，否则抢同一个软链
#   T4  既无序列号又无物理路径 → 明确报「无法稳定命名」，不瞎猜
#   T5  绑定保留：设备换了 /dev/ttyUSB* 名字后，序号不变（配置不用改）
#   T6  离线设备的规则被保留（设备插回来名字不变）
#   T7  幂等：同一份输入跑两次，输出完全一致
#   T8  同序列号克隆设备：撞车后自动退到物理路径，序号不重叠
#   T9  生成的规则不得出现 ENV{ID_SERIAL} / ENV{ID_PATH}
#       （它们由 systemd 的 60-serial.rules 导入，而我方规则文件名字典序在它
#        之前 → 匹配永远不中；ID_SERIAL 还不是唯一标识）
#
# 夹具格式（| 分隔，7 段）:
#   设备名 | 厂商ID | 产品ID | 原始序列号 | 接口号 | 多口标记 | 设备路径
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GEN="$SCRIPT_DIR/../gateway-core/deploy/serial-rules.sh"
if [ ! -f "$GEN" ]; then
    echo "找不到 serial-rules.sh ($GEN)"; exit 1
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

PASS=0
FAIL=0
ok()   { echo "  [OK] $1"; PASS=$((PASS+1)); }
bad()  { echo "  [FAIL] $1"; FAIL=$((FAIL+1)); }
check_contains() { # file needle desc
    if grep -Fq "$2" "$1"; then ok "$3"; else bad "$3（未在 $1 中找到 '$2'）"; fi
}
check_not_contains() {
    if grep -Fq "$2" "$1"; then bad "$3（不应出现 '$2'）"; else ok "$3"; fi
}

echo "[serial-rules] 生成器: $GEN"

P1="/devices/pci0000:00/0000:00:14.0/usb1/1-1/1-1:1.0"
P2="/devices/pci0000:00/0000:00:14.0/usb1/1-2/1-2:1.0"
P3="/devices/pci0000:00/0000:00:14.0/usb1/1-3/1-3:1.0"

# ── 夹具 1：两个有序列号的设备 + 一个只有物理路径的设备 ──
cat > "$WORK/dev1.tsv" <<EOF
# 设备名 | 厂商ID | 产品ID | 原始序列号 | 接口号 | 多口 | 设备路径
/dev/ttyUSB0|0403|6001|A10K7VNA|00|0|$P1
/dev/ttyUSB1|10c4|ea60|1234|00|0|$P2
/dev/ttyACM0|1a86|7523||00|0|$P3
EOF

GW_SERIAL_DEVICES_FILE="$WORK/dev1.tsv" \
    bash "$GEN" --rules-file "$WORK/r1.rules" --state-file "$WORK/s1.tsv" \
    > "$WORK/out1.txt" 2>&1
rc=$?
if [ $rc -eq 0 ]; then ok "夹具1 执行成功"; else bad "夹具1 执行失败 rc=$rc: $(cat "$WORK/out1.txt")"; fi

check_contains "$WORK/r1.rules" 'SYMLINK+="gateway-serial0"' "T1 序号0 生成软链 gateway-serial0"
check_contains "$WORK/r1.rules" 'SYMLINK+="gateway-serial1"' "T1 序号1 生成软链 gateway-serial1"
check_contains "$WORK/r1.rules" 'SYMLINK+="gateway-serial2"' "T2 无序列号设备分配到序号2"
check_contains "$WORK/r1.rules" 'ATTRS{serial}=="A10K7VNA"' "T1 规则按原始序列号 ATTRS{serial} 匹配"
check_contains "$WORK/r1.rules" 'ATTRS{idVendor}=="0403"' "T1 规则带 ATTRS{idVendor}"
check_contains "$WORK/r1.rules" 'ATTRS{idProduct}=="6001"' "T1 规则带 ATTRS{idProduct}"
check_contains "$WORK/r1.rules" "ENV{DEVPATH}==\"$P3/tty/*\"" "T2 无序列号设备按物理路径 ENV{DEVPATH} 匹配"
check_contains "$WORK/out1.txt" 'gateway-serial0 -> /dev/ttyUSB0' "映射表打印出序号与设备的对应关系"
check_contains "$WORK/s1.tsv" 'serial:A10K7VNA@0403:6001#00' "T1 状态文件记录稳定标识"

# ── T4：既无序列号又无物理路径 ──
cat > "$WORK/dev2.tsv" <<'EOF'
/dev/ttyUSB9||||||
EOF
GW_SERIAL_DEVICES_FILE="$WORK/dev2.tsv" \
    bash "$GEN" --rules-file "$WORK/r2.rules" --state-file "$WORK/s2.tsv" \
    > "$WORK/out2.txt" 2>&1
check_contains "$WORK/out2.txt" '无法生成稳定名' "T4 无稳定标识的设备被明确告警"
check_not_contains "$WORK/r2.rules" 'SYMLINK+="gateway-serial0"' "T4 无法稳定命名的设备不生成软链"

# ── T3：多口设备（各口序列号相同）必须退到物理路径 ──
cat > "$WORK/dev3.tsv" <<EOF
/dev/ttyUSB0|0403|6011|FT4232H_A1|00|1|$P1
/dev/ttyUSB1|0403|6011|FT4232H_A1|01|1|/devices/pci0000:00/0000:00:14.0/usb1/1-1/1-1:1.1
EOF
GW_SERIAL_DEVICES_FILE="$WORK/dev3.tsv" \
    bash "$GEN" --rules-file "$WORK/r3.rules" --state-file "$WORK/s3.tsv" \
    > "$WORK/out3.txt" 2>&1
check_contains "$WORK/r3.rules" "ENV{DEVPATH}==\"$P1/tty/*\"" "T3 多口设备按物理路径区分（口0）"
check_contains "$WORK/r3.rules" "ENV{DEVPATH}==\"/devices/pci0000:00/0000:00:14.0/usb1/1-1/1-1:1.1/tty/*\"" "T3 多口设备按物理路径区分（口1）"
check_not_contains "$WORK/r3.rules" 'ATTRS{serial}=="FT4232H_A1"' "T3 多口设备不使用序列号匹配（各口序列号相同，会抢同一个软链）"
check_contains "$WORK/r3.rules" 'SYMLINK+="gateway-serial0"' "T3 多口设备分配到不同序号"
check_contains "$WORK/r3.rules" 'SYMLINK+="gateway-serial1"' "T3 多口设备分配到不同序号"

# ── T5：设备换了 /dev/ttyUSB* 名字，序号必须不变 ──
cat > "$WORK/dev4.tsv" <<EOF
/dev/ttyUSB1|0403|6001|A10K7VNA|00|0|$P1
/dev/ttyUSB0|10c4|ea60|1234|00|0|$P2
/dev/ttyACM0|1a86|7523||00|0|$P3
EOF
cp "$WORK/s1.tsv" "$WORK/s5.tsv"
GW_SERIAL_DEVICES_FILE="$WORK/dev4.tsv" \
    bash "$GEN" --rules-file "$WORK/r5.rules" --state-file "$WORK/s5.tsv" \
    > "$WORK/out5.txt" 2>&1
# FTDI 原来是 ttyUSB0（序号0），现在变成 ttyUSB1 → 仍应是 gateway-serial0
if grep -Fq 'gateway-serial0 | serial:A10K7VNA@0403:6001#00 | /dev/ttyUSB1' "$WORK/r5.rules"; then
    ok "T5 设备改名后序号保持不变（FTDI 仍是 gateway-serial0）"
else
    bad "T5 设备改名后序号被洗牌: $(grep -F 'A10K7VNA' "$WORK/r5.rules" | head -1)"
fi
# CP2102 原来是 ttyUSB1（序号1），现在变成 ttyUSB0 → 仍应是 gateway-serial1
if grep -Fq 'gateway-serial1 | serial:1234@10c4:ea60#00 | /dev/ttyUSB0' "$WORK/r5.rules"; then
    ok "T5 设备改名后序号保持不变（CP2102 仍是 gateway-serial1）"
else
    bad "T5 设备改名后序号被洗牌: $(grep -F 'serial:1234' "$WORK/r5.rules" | head -1)"
fi

# ── T6：设备离线后规则保留 ──
cat > "$WORK/dev5.tsv" <<EOF
/dev/ttyUSB0|0403|6001|A10K7VNA|00|0|$P1
EOF
cp "$WORK/s1.tsv" "$WORK/s6.tsv"
GW_SERIAL_DEVICES_FILE="$WORK/dev5.tsv" \
    bash "$GEN" --rules-file "$WORK/r6.rules" --state-file "$WORK/s6.tsv" \
    > "$WORK/out6.txt" 2>&1
check_contains "$WORK/r6.rules" 'SYMLINK+="gateway-serial1"' "T6 离线设备的软链规则被保留"
check_contains "$WORK/r6.rules" '当前离线' "T6 离线设备在规则里有明确标记"

# ── T7：幂等 ──
cp "$WORK/s1.tsv" "$WORK/s7.tsv"
GW_SERIAL_DEVICES_FILE="$WORK/dev1.tsv" \
    bash "$GEN" --rules-file "$WORK/r7a.rules" --state-file "$WORK/s7.tsv" \
    > "$WORK/out7.txt" 2>&1
GW_SERIAL_DEVICES_FILE="$WORK/dev1.tsv" \
    bash "$GEN" --rules-file "$WORK/r7b.rules" --state-file "$WORK/s7.tsv" \
    > "$WORK/out7b.txt" 2>&1
if diff -q "$WORK/r7a.rules" "$WORK/r7b.rules" >/dev/null 2>&1; then
    ok "T7 同一份输入重复生成，规则文件完全一致（幂等）"
else
    bad "T7 重复生成结果不一致: $(diff "$WORK/r7a.rules" "$WORK/r7b.rules" | head -5)"
fi

# ── T8：两块同型号、伪造了相同序列号的克隆设备 ──
cat > "$WORK/dev6.tsv" <<EOF
/dev/ttyUSB0|0403|6001|CLONESAME|00|0|$P1
/dev/ttyUSB1|0403|6001|CLONESAME|00|0|$P2
EOF
GW_SERIAL_DEVICES_FILE="$WORK/dev6.tsv" \
    bash "$GEN" --rules-file "$WORK/r8.rules" --state-file "$WORK/s8.tsv" \
    > "$WORK/out8.txt" 2>&1
check_not_contains "$WORK/r8.rules" 'ATTRS{serial}=="CLONESAME"' "T8 同序列号克隆设备不使用序列号匹配（否则抢同一个软链）"
check_contains "$WORK/r8.rules" "ENV{DEVPATH}==\"$P1/tty/*\"" "T8 克隆设备之一按物理路径匹配"
check_contains "$WORK/r8.rules" "ENV{DEVPATH}==\"$P2/tty/*\"" "T8 克隆设备之二按物理路径匹配"
check_contains "$WORK/r8.rules" 'SYMLINK+="gateway-serial0"' "T8 克隆设备分配到序号0"
check_contains "$WORK/r8.rules" 'SYMLINK+="gateway-serial1"' "T8 克隆设备分配到序号1"

# ── T9：不得使用依赖 builtin 导入顺序的属性 ──
check_not_contains "$WORK/r1.rules" 'ENV{ID_SERIAL}' "T9 规则不使用 ENV{ID_SERIAL}（导入顺序不保证，匹配永不命中）"
check_not_contains "$WORK/r1.rules" 'ENV{ID_PATH}' "T9 规则不使用 ENV{ID_PATH}（导入顺序不保证，匹配永不命中）"
check_not_contains "$WORK/r3.rules" 'ENV{ID_SERIAL}' "T9 多口设备规则同样不使用 ENV{ID_SERIAL}"

echo ""
echo "[serial-rules] 通过 $PASS 项，失败 $FAIL 项"
[ "$FAIL" -eq 0 ] || exit 1
exit 0
