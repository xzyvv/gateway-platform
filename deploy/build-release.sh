#!/usr/bin/env bash
set -euo pipefail

# ────────────────────────────────────────────────────────────
# 工业网关一键构建脚本
# 产物：前端 dist/ + Rust gatewayd release
# 可选：交叉编译到 Linux ARM64 / ARMv7（在 x86 开发机上）
# 用法：
#   ./deploy/build-release.sh                    # 本地 host，默认标准版 SKU
#   ./deploy/build-release.sh --target linux-arm64
#   ./deploy/build-release.sh --sku building     # 楼宇版（BACnet/IP + Modbus + CJT188 + OPC UA）
#   ./deploy/build-release.sh --sku power        # 电力版（IEC104/IEC101 + DLT645 + Modbus + OPC UA）
#   ./deploy/build-release.sh --sku plc          # PLC 版（S7comm + 三菱 MC + Modbus + OPC UA）
#   ./deploy/build-release.sh --sku minimal      # 精简版（仅 Modbus + DLT645 + CJT188）
#   ./deploy/build-release.sh --protocols modbus,cjt188,dlt645
#   ./deploy/build-release.sh --protocols all    # 默认发行集全部协议（仅调试/自测）
#
# 静态链接（musl）：现场设备的 glibc 版本千差万别（Debian 10 = 2.28、
# Debian 11 = 2.31、Yocto 常见 musl），gnu 动态产物会带着构建机的 glibc 版本要求，
# 在旧系统上直接报 "GLIBC_2.xx not found"。要免掉这层耦合用 musl target：
#   ./deploy/build-release.sh --target linux-x86_64-musl
#   ./deploy/build-release.sh --target linux-arm64-musl
#   ./deploy/build-release.sh --target linux-armv7-musl
# 构建机需有对应 musl C 工具链（bundled sqlite3.c 要过 C 编译器）：
#   x86_64: apt install musl-tools        → CC_x86_64_unknown_linux_musl=musl-gcc
#   ARM64/armv7: 建议用 cross（自带 musl 交叉工具链）
# 构建结束后会自动做二进制兼容性自检，见下方 MAX_GLIBC。
#
# A4 出厂镜像按协议清单裁剪：默认不再依赖 `default = ["all-protocols"]`，
# 而是显式传 `--no-default-features --features <清单>`。清单会写入产物目录的
# BUILD-INFO.txt，作为发布检查项（verify-deploy.sh 会据此核对线上协议表）。
#
# P0-1 SKU 分级：发行包默认使用 --sku standard；各行业 SKU 按南向协议互斥切片，
# 不再以标准版为底座叠加。FINS 与 BACnet MS/TP 已移出默认集合并进入「退役观察」，
# 只能通过 --protocols 显式指定构建（不再出现在任何 SKU 中）。
# ────────────────────────────────────────────────────────────

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FRONTEND="$ROOT/gateway-web"
RUST_ROOT="$ROOT"
OUT="$ROOT/deploy/out"

TARGET=""
# A4：出厂镜像协议清单。默认走标准版 SKU（见下方 SKU_PROTOCOLS）；
# 需要行业子集时用 --sku / --protocols 覆盖，默认发行集全量用 --protocols all。
SKU="${FACTORY_SKU:-standard}"
PROTOCOLS=""
PROTOCOLS_CLI=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --sku)         SKU="${2:-}"; shift 2 ;;
    --sku=*)       SKU="${1#*=}"; shift ;;
    --protocols)   PROTOCOLS="${2:-}"; PROTOCOLS_CLI=1; shift 2 ;;
    --protocols=*) PROTOCOLS="${1#*=}"; PROTOCOLS_CLI=1; shift ;;
    --target)      TARGET="${2:-}"; shift 2 ;;
    --target=*)    TARGET="${1#*=}"; shift ;;
    -*)            echo "错误: 未知参数 '$1'"; exit 1 ;;
    *)             TARGET="$1"; shift ;;
  esac
done

# 协议名白名单：拼错会让 cargo 直接失败，但错误信息不可读；这里提前给出可用清单。
# bacnet-mstp / fins 属「退役观察」，仅允许通过 --protocols 显式构建。
ALL_PROTOCOLS="modbus dlt645 cjt188 opcua iec104 bacnet s7comm bacnet-mstp mc fins iec101"

# SKU → 实际协议 feature 清单（用于白名单校验与 BUILD-INFO.protocols；
# 构建时仍使用聚合 feature `sku-<name>`）。
#
# 单一来源：SKU 表只在 `gateway-core/Cargo.toml` 的 `sku-*` 聚合 feature 中维护，
# 本脚本运行时解析该文件，不再保留第二份 case 映射——新增/调整 SKU 只改 Cargo.toml。
SKU_CARGO_TOML="$RUST_ROOT/gateway-core/Cargo.toml"

# 从 Cargo.toml 列出全部 sku-* 名称（去掉前缀），用于错误提示
SKU_LIST="$(awk -F'=' '/^sku-[a-z0-9-]+[[:space:]]*=/ { k=$1; gsub(/[[:space:]]/, "", k); sub(/^sku-/, "", k); print k }' "$SKU_CARGO_TOML" | tr '\n' ' ')"

# 读取指定 SKU 的协议清单（feature 数组里的元素，逗号连接）
sku_protocols() {
  awk -v key="sku-$1" '
    $0 ~ "^" key "[[:space:]]*=" {
      v = $0
      sub(/^[^=]*=[[:space:]]*\[/, "", v)
      sub(/\].*$/, "", v)
      gsub(/[",]/, "", v)
      gsub(/[[:space:]]+/, ",", v)
      gsub(/,+/, ",", v)
      sub(/^,/, "", v)
      sub(/,$/, "", v)
      print v
      exit
    }
  ' "$SKU_CARGO_TOML"
}

SKU_PROTOCOLS="$(sku_protocols "$SKU")"
if [ -z "$SKU_PROTOCOLS" ]; then
  echo "错误: 未知 SKU '$SKU'"
  echo "可用: ${SKU_LIST:-(无，请先检查 gateway-core/Cargo.toml)}"
  exit 1
fi

if [ "$PROTOCOLS_CLI" -eq 1 ]; then
  SKU="custom"
  PROTOCOLS="${PROTOCOLS//[[:space:]]/}"
  if [ -z "$PROTOCOLS" ]; then
    echo "错误: --protocols 不能为空（默认发行集全量请显式写 all）"
    exit 1
  fi
  if [ "$PROTOCOLS" != "all" ]; then
    IFS=',' read -ra REQ_PROTOCOLS <<< "$PROTOCOLS"
    for p in "${REQ_PROTOCOLS[@]}"; do
      case " $ALL_PROTOCOLS " in
        *" $p "*) ;;
        *)
          echo "错误: 未知协议 '$p'"
          echo "可用: $ALL_PROTOCOLS（或 all 表示默认发行集全量）"
          exit 1
          ;;
      esac
    done
  fi
else
  PROTOCOLS="$SKU_PROTOCOLS"
fi

echo "═══════════════════════════════════════════════"
echo " 工业网关 Release 构建"
echo "═══════════════════════════════════════════════"
echo "仓库根: $ROOT"
echo "目标:  ${TARGET:-host 本机}"
if [ "$SKU" = "custom" ] || [ "$PROTOCOLS" = "all" ]; then
  echo "SKU:   ${SKU}（未按行业 SKU 打包）"
else
  echo "SKU:   ${SKU}"
fi
echo "协议:  $PROTOCOLS"
echo ""

# ── 解析 Rust target ──
case "$TARGET" in
  ""|"host")      RUST_TARGET="" ;;
  "linux-arm64")  RUST_TARGET="aarch64-unknown-linux-gnu" ;;
  "linux-armv7")  RUST_TARGET="armv7-unknown-linux-gnueabihf" ;;
  "linux-x86_64") RUST_TARGET="x86_64-unknown-linux-gnu" ;;
  # musl = 静态链接，产物不依赖设备 glibc 版本（现场兼容性首选）
  "linux-x86_64-musl") RUST_TARGET="x86_64-unknown-linux-musl" ;;
  "linux-arm64-musl")  RUST_TARGET="aarch64-unknown-linux-musl" ;;
  "linux-armv7-musl")  RUST_TARGET="armv7-unknown-linux-musleabihf" ;;
  *)
    echo "错误: 未知 target '$TARGET'"
    echo "支持: host (默认) | linux-arm64 | linux-armv7 | linux-x86_64"
    echo "      linux-x86_64-musl | linux-arm64-musl | linux-armv7-musl（静态链接）"
    exit 1
    ;;
esac

# 允许现场按目标系统放宽/收紧 glibc 基线（默认 2.31 ≈ Debian 11 / Ubuntu 20.04）
MAX_GLIBC="${MAX_GLIBC:-2.31}"

mkdir -p "$OUT"

# ── 1. 前端构建（纯静态产物，跨平台通用） ──
echo "[1/3] 构建前端..."
cd "$FRONTEND"
if [ ! -d node_modules ]; then
  echo "  安装依赖..."
  npm install --no-audit --no-fund
fi
npm run build  # vue-tsc + vite build
echo "  ✓ 前端产物: $FRONTEND/dist/"

# ── 2. Rust release 构建 ──
echo "[2/3] 构建 Rust gatewayd..."
cd "$RUST_ROOT"

# musl target 需要 musl C 工具链来编译 bundled sqlite3.c。
# cc-rs 通过 CC_<target> 找编译器；找到就显式指定，找不到交给 cargo 报错。
if [ -n "$RUST_TARGET" ] && [[ "$RUST_TARGET" == *musl* ]]; then
  MUSL_CC=""
  case "$RUST_TARGET" in
    x86_64-*)  MUSL_CC="musl-gcc" ;;
    aarch64-*) MUSL_CC="aarch64-linux-musl-gcc" ;;
    armv7-*)   MUSL_CC="arm-linux-musleabihf-gcc" ;;
  esac
  if [ -n "$MUSL_CC" ] && command -v "$MUSL_CC" >/dev/null 2>&1; then
    export "CC_${RUST_TARGET//-/_}=$MUSL_CC"
    echo "  musl C 工具链: $MUSL_CC"
  else
    echo "  ⚠ 未找到 ${MUSL_CC:-musl 交叉工具链}：bundled sqlite3.c 可能编译失败"
    echo "    x86_64 可 apt install musl-tools；ARM 建议用 cross 构建"
  fi
fi

# A4：协议 feature 参数。all 显式走 all-protocols（等价默认，但参数形态统一，
# 避免空数组在 bash 3.2 + `set -u` 下展开报 unbound variable）；行业 SKU 走聚合
# feature `sku-<name>`；自定义清单一律关掉默认 feature 显式裁剪。
FEATURE_ARGS=()
if [ "$PROTOCOLS" = "all" ]; then
  FEATURE_ARGS=(--features all-protocols)
  echo "  ⚠ 默认发行集全量构建（体积基准，未裁剪）"
elif [ "$SKU" != "custom" ]; then
  FEATURE_ARGS=(--no-default-features --features "sku-$SKU")
  echo "  SKU 裁剪: --features sku-$SKU（包含 $PROTOCOLS）"
else
  FEATURE_ARGS=(--no-default-features --features "$PROTOCOLS")
  echo "  协议裁剪: --features $PROTOCOLS"
fi

if [ -n "$RUST_TARGET" ]; then
  # 交叉编译：确保 target 已安装
  if ! rustup target list --installed | grep -q "$RUST_TARGET"; then
    echo "  安装 Rust target: $RUST_TARGET"
    rustup target add "$RUST_TARGET"
  fi

  # ARM 交叉编译需要 linker（windows 上用 zig 最省事）
  if [[ "$OSTYPE" == "msys" || "$OSTYPE" == "win32" ]]; then
    # Windows 上：尝试用 zig-cc（需要 zig 已安装），否则降级到仅提示
    if command -v zig &>/dev/null; then
      echo "  Windows + zig 交叉编译: $RUST_TARGET"
      CC_aarch64_unknown_linux_gnu="zig cc -target aarch64-linux-gnu" \
      CC_armv7_unknown_linux_gnueabihf="zig cc -target arm-linux-gnueabihf" \
      CC_x86_64_unknown_linux_gnu="zig cc -target x86_64-linux-gnu" \
        cargo build --release -p gatewayd --target "$RUST_TARGET" "${FEATURE_ARGS[@]}"
    else
      echo "  ⚠ Windows 上交叉编译到 $RUST_TARGET 需要 zig 或预配 linker"
      echo "  推荐：在 Linux 目标机上直接 ./deploy/build-release.sh"
      echo "  或者：安装 zig (https://ziglang.org) 后重跑"
      exit 2
    fi
  else
    # Linux / macOS 交叉编译
    cargo build --release -p gatewayd --target "$RUST_TARGET" "${FEATURE_ARGS[@]}"
  fi
else
  cargo build --release -p gatewayd "${FEATURE_ARGS[@]}"
fi

# 定位产物
if [ -n "$RUST_TARGET" ]; then
  BIN_SRC="$RUST_ROOT/target/$RUST_TARGET/release/gatewayd"
else
  BIN_SRC="$RUST_ROOT/target/release/gatewayd"
fi
echo "  ✓ 二进制: $BIN_SRC"

# ── 二进制兼容性自检（B） ──
# 两件事必须在上机前就确定，而不是等设备上执行时报错：
#   1) 是静态链接还是动态链接；
#   2) 动态链接时，产物要求的最低 glibc 版本（= 构建机的 glibc）。
# 现场设备若是 Debian 10(2.28)/11(2.31)，在 ubuntu-latest 上构建的 gnu 产物
# （glibc 2.39）会直接 "GLIBC_2.39 not found"，连启动都到不了。
LINK_MODE="unknown"
GLIBC_REQ=""
if command -v file >/dev/null 2>&1; then
  if file "$BIN_SRC" | grep -q "statically linked"; then
    LINK_MODE="static"
  else
    LINK_MODE="dynamic"
  fi
fi
if [ "$LINK_MODE" = "dynamic" ]; then
  # 取产物引用到的最高 GLIBC 符号版本（objdump/readelf 二者取其一）
  if command -v objdump >/dev/null 2>&1; then
    GLIBC_REQ="$(objdump -T "$BIN_SRC" 2>/dev/null \
      | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/^GLIBC_//' | sort -V | tail -n1 || true)"
  elif command -v readelf >/dev/null 2>&1; then
    GLIBC_REQ="$(readelf --version-info "$BIN_SRC" 2>/dev/null \
      | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/^GLIBC_//' | sort -V | tail -n1 || true)"
  fi
fi
if [ "$LINK_MODE" = "static" ]; then
  echo "  ✓ 链接方式: 静态（不依赖设备 glibc，任意 Linux 发行版可运行）"
elif [ -n "$GLIBC_REQ" ]; then
  echo "  链接方式: 动态，要求设备 glibc ≥ $GLIBC_REQ（基线 $MAX_GLIBC）"
  # 用 sort -V 判大小：把两个版本排序后，较大者若等于产物要求则说明超基线
  BIGGER="$(printf '%s\n%s\n' "$MAX_GLIBC" "$GLIBC_REQ" | sort -V | tail -n1)"
  if [ "$BIGGER" = "$GLIBC_REQ" ] && [ "$GLIBC_REQ" != "$MAX_GLIBC" ]; then
    echo "  ✗ 产物要求的 glibc 高于基线 $MAX_GLIBC：旧设备（Debian 10/11、Yocto）会启动失败"
    echo "    解决：改用静态链接 target（--target linux-x86_64-musl / -arm64-musl / -armv7-musl），"
    echo "          在更旧的构建环境里构建，或显式放宽基线 MAX_GLIBC=$GLIBC_REQ"
    exit 1
  fi
  echo "  ✓ glibc 基线满足（≤ $MAX_GLIBC）"
else
  echo "  ⚠ 未能检测链接方式与 glibc 版本（构建机缺 file/objdump/readelf），请人工核对"
fi

# ── 3. 打产物包 ──
echo "[3/3] 打产物包..."
# 先清掉上次的产物：ui-dist 已存在时 `cp -r dist ui-dist` 会变成 ui-dist/dist
# （index.html 找不到，install.sh 判定前端缺失）；多架构 gatewayd-* 残留会让
# install.sh 的 `gatewayd-*` 通配选到别的架构。检查单（*.md）保留不动。
rm -rf "$OUT/ui-dist" "$OUT/templates" "$OUT/gatewayd" "$OUT"/gatewayd-* \
       "$OUT/deploy-linux.sh" "$OUT/BUILD-INFO.txt"
BIN_NAME="gatewayd"
[ -n "$RUST_TARGET" ] && BIN_NAME="gatewayd-$RUST_TARGET"
cp "$BIN_SRC" "$OUT/$BIN_NAME"
cp -r "$FRONTEND/dist" "$OUT/ui-dist"
cp "$ROOT/gateway-core/deploy/gateway.service" "$OUT/"
cp "$ROOT/gateway-core/deploy/install.sh" "$OUT/"
cp "$ROOT/gateway-core/deploy/install.sh" "$OUT/deploy-linux.sh"  # 别名方便远程
# G5 交付链路随包分发：一条命令 install → verify → 导出检查单。
# 三者必须在同一目录，deliver.sh 才按「同目录」布局解析；缺任何一个都交付不了。
cp "$ROOT/deploy/verify-deploy.sh" "$OUT/"
cp "$ROOT/deploy/deliver.sh" "$OUT/"
# 内置设备模板唯一源在仓库根 templates/（gateway-core/templates 副本已删除）；
# install.sh 会从包内 templates/builtin 拷贝到 /etc/gateway/templates/builtin
cp -r "$ROOT/templates" "$OUT/"

# 完整 config.json 不随包提供：/etc/gateway/config.json 由交付方按现场填写
# （web.listen_addr / api_token / password_hash）。仓库里的
# gateway-core/config.json.example 只是「配置片段示例」，文件自身写明不会被网关加载，
# 不能当整份配置安装。install.sh 仅在同级目录真的存在 config.json 时才落盘。

# A4：写出厂元信息，作为发布检查依据（verify-deploy.sh 据此核对线上协议表）。
# 注意 protocols 用的是 cargo feature 名（bacnet-mstp），线上 /api/protocols 返回
# 协议名（bacnet_mstp），校验脚本会做 `-` → `_` 归一化；gpio 不随 feature 裁剪，不参与比对。
if [ "$PROTOCOLS" = "all" ]; then
  FEATURE_DESC="--features all-protocols（未裁剪）"
elif [ "$SKU" != "custom" ]; then
  FEATURE_DESC="--no-default-features --features sku-$SKU"
else
  FEATURE_DESC="--no-default-features --features $PROTOCOLS"
fi
cat > "$OUT/BUILD-INFO.txt" <<EOF
# 出厂镜像构建信息（由 deploy/build-release.sh 生成）
build_time=$(date -Is)
target=${TARGET:-host}
rust_target=${RUST_TARGET:-host}
sku=$SKU
protocols=$PROTOCOLS
feature_args=$FEATURE_DESC
link_mode=$LINK_MODE
glibc_min=${GLIBC_REQ:-none}
glibc_policy=$MAX_GLIBC
verify=包内 sudo bash deliver.sh 可一键完成 install + verify-deploy 并导出交付检查单
EOF
echo "  ✓ 构建信息: $OUT/BUILD-INFO.txt (sku=$SKU, protocols=$PROTOCOLS)"

echo ""
echo "═══════════════════════════════════════════════"
echo " ✓ 构建完成！产物: $OUT/"
echo ""
ls -lh "$OUT"
echo ""
echo "部署到目标 Linux:"
echo "  scp -r deploy/out/ gateway@<target>:/tmp/gw-out"
echo "  ssh gateway@<target> 'cd /tmp/gw-out && sudo bash deliver.sh'   # 安装 + 验证 + 检查单"
echo "  （或分步: sudo bash install.sh ./$BIN_NAME ./ui-dist && sudo bash verify-deploy.sh）"
echo "═══════════════════════════════════════════════"
