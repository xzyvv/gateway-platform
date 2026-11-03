#!/usr/bin/env bash
# 仓库健康度扫描脚本
#
# 用途：CI / 本地 dev 防回归检测，扫描 4 类潜在问题：
#   1. bare CR 检测（\r\r\n 损坏换行符）
#   2. rust-toolchain.toml 与 ci.yml 工具链版本一致性
#   3. ts-rs generated 完整性检测（gateway-web/src/api/generated/ 应由 cargo test 生成）
#   4. CRLF/LF 一致性检测（同类型文件不应混用）
#
# 退出码：0 = 通过 / 1 = 有阻断性失败
#
# 接入 CI：在 .github/workflows/ci.yml 的 check job 末尾追加
#   - name: Repo health scan
#     run: bash scripts/check_repo_health.sh

set -euo pipefail

cd "$(dirname "$0")/.."

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
NC='\033[0m'
fail=0
warn=0

echo "=== 1. bare CR 检测（\\r\\r\\n 损坏换行符）==="
bare_cr_files=""
while IFS= read -r f; do
    if grep -qP '\r\r\n' "$f" 2>/dev/null; then
        bare_cr_files="$bare_cr_files $f"
    fi
done < <(find gateway-core/src gateway-api/src gateway-web/src -type f \( -name '*.rs' -o -name '*.ts' -o -name '*.vue' \) 2>/dev/null)
if [[ -n "${bare_cr_files// }" ]]; then
    echo -e "${RED}FAIL${NC}: 检测到 \\r\\r\\n 损坏换行符（一般是编辑 CRLF 文件时 PowerShell 传输层导致）"
    echo "$bare_cr_files" | tr ' ' '\n' | sed 's/^/  /'
    fail=1
else
    echo -e "${GREEN}OK${NC}: 无 bare CR"
fi

echo ""
echo "=== 2. rust-toolchain 版本一致性 ==="
toolchain_ver=$(grep -oP '^channel = "\K[^"]+' rust-toolchain.toml 2>/dev/null || echo "")
if [[ -z "$toolchain_ver" ]]; then
    echo -e "${RED}FAIL${NC}: rust-toolchain.toml 未找到 channel 配置"
    fail=1
fi
ci_vers=$(grep -oP 'dtolnay/rust-toolchain@\K[0-9.]+' .github/workflows/ci.yml | sort -u)
ci_count=$(echo "$ci_vers" | wc -l)
if [[ $ci_count -gt 1 ]]; then
    echo -e "${RED}FAIL${NC}: ci.yml 里存在多个不同的 toolchain 版本:"
    echo "$ci_vers" | sed 's/^/  /'
    fail=1
elif [[ -n "$toolchain_ver" && "$toolchain_ver" != "$ci_vers" ]]; then
    echo -e "${RED}FAIL${NC}: rust-toolchain.toml channel=$toolchain_ver != ci.yml @$ci_vers"
    fail=1
else
    echo -e "${GREEN}OK${NC}: 都是 $toolchain_ver"
fi

echo ""
echo "=== 3. ts-rs generated 完整性检测 ==="
gen_dir="gateway-web/src/api/generated"
if [[ -d "$gen_dir" ]]; then
    gen_count=$(find "$gen_dir" -maxdepth 1 -name '*.ts' | wc -l)
    if [[ $gen_count -gt 0 ]]; then
        echo -e "${GREEN}OK${NC}: generated/ 有 $gen_count 个 .ts 文件"
    else
        echo -e "${YELLOW}WARN${NC}: gateway-web/src/api/generated/ 为空，运行 cargo test export_ts_types 生成"
        warn=1
    fi
else
    echo -e "${YELLOW}WARN${NC}: gateway-web/src/api/generated/ 不存在，运行 cargo test export_ts_types 生成"
    warn=1
fi

echo ""
echo "=== 4. CRLF/LF 一致性检测 ==="
crlf_files=0
lf_files=0
while IFS= read -r f; do
    if grep -qP '\r\n' "$f" 2>/dev/null; then
        crlf_files=$((crlf_files + 1))
    else
        lf_files=$((lf_files + 1))
    fi
done < <(find gateway-core/src gateway-api/src -type f -name '*.rs' 2>/dev/null)
if [[ $crlf_files -gt 0 && $lf_files -gt 0 ]]; then
    echo -e "${YELLOW}WARN${NC}: CRLF 文件 $crlf_files 个，LF 文件 $lf_files 个 —— 建议统一"
    echo "  建议加 .gitattributes 统一换行符："
    echo '    *.rs text eol=lf'
    echo "  然后执行：git add --renormalize . && git commit -m 'normalize line endings'"
    warn=1
elif [[ $crlf_files -gt 0 ]]; then
    echo -e "${YELLOW}INFO${NC}: 全 CRLF ($crlf_files 个 .rs) —— 不影响编译但建议统一为 LF"
    warn=1
elif [[ $lf_files -gt 0 ]]; then
    echo -e "${GREEN}OK${NC}: 全 LF ($lf_files 个 .rs)"
else
    echo -e "${GREEN}OK${NC}: 无 .rs 文件"
fi

echo ""
echo "=== 5. build artifact leak 检测 ==="
# 检查不应出现在 git 工作树的构建产物 / 自动生成文件
# 这些已加入 .gitignore，若出现在工作树说明 .gitignore 失效或被绕过
artifact_paths=(
  "target"
  "gateway-web/dist"
  "gateway-web/auto-imports.d.ts"
  "gateway-web/components.d.ts"
  "gateway-desktop/src-tauri/Cargo.lock"
)
leaked=0
for p in "${artifact_paths[@]}"; do
  if [[ -e "$p" ]]; then
    echo -e "${RED}LEAK${NC}: $p 存在（应被 .gitignore 排除）"
    leaked=1
  fi
done
# vue-tsc-errors*.txt glob
for f in gateway-web/vue-tsc-errors*.txt; do
  if [[ -f "$f" ]]; then
    echo -e "${RED}LEAK${NC}: $f 存在（应被 .gitignore 排除）"
    leaked=1
  fi
done
if [[ $leaked -eq 0 ]]; then
  echo -e "${GREEN}OK${NC}: 无构建产物 / 自动生成文件泄漏"
else
  fail=1
fi
if [[ $fail -eq 1 ]]; then
    echo -e "${RED}FAILED${NC}: 阻断性错误，请修复后重试"
    exit 1
elif [[ $warn -eq 1 ]]; then
    echo -e "${YELLOW}WARN${NC}: 仅有警告（不阻断 CI）"
    exit 0
else
    echo -e "${GREEN}All checks passed${NC}"
    exit 0
fi

echo ""
