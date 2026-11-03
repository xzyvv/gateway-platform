#!/usr/bin/env bash
# T0.1: CI local gate
#
# 防止"注释吞代码""配置损坏"类 bug 再次出现。
# PR 合入前必须全部通过。
#
# N-12：本脚本是「fmt → check → clippy → test --lib → build」五阶段的**唯一实现**，
# GitHub Actions 的 check job 直接调用它（不再在 ci.yml 内联同一串命令），
# 避免两份门禁逐步漂移。
#
# 用法（在工作区任意目录）：bash gateway-core/scripts/ci.sh
#
# 退出码：0 = 全部通过；非 0 = 任一阶段失败

set -euo pipefail

# 脚本位于 <workspace>/gateway-core/scripts/ → 上两级即工作区根
cd "$(dirname "$0")/../.."

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log() { echo -e "${GREEN}[CI]${NC} $*"; }
warn() { echo -e "${YELLOW}[CI][warn]${NC} $*"; }
fail() { echo -e "${RED}[CI][FAIL]${NC} $*"; exit 1; }

# 1. fmt check
log "1/5 cargo fmt --check"
cargo fmt --all -- --check || fail "代码格式不符（运行 cargo fmt 修复）"

# 2. check
log "2/5 cargo check --all-targets"
cargo check --all-targets --all-features || fail "编译检查失败"

# 3. clippy (D warnings)
log "3/6 cargo clippy --all-targets -- -D warnings"
cargo clippy --all-targets --all-features -- -D warnings || fail "clippy 警告存在"

# 4. unit tests
log "4/6 cargo test --lib"
cargo test --lib --quiet || fail "单元测试失败"

# 4b. license-issuer unit tests（F-08）
# 该 crate 只有 [[bin]] 没有 lib target，`cargo test --lib` 覆盖不到它的
# 签发侧单测（签名/哈希/编码），必须显式按包名跑。
log "5/6 cargo test -p license-issuer"
cargo test -p license-issuer --quiet || fail "license-issuer 单元测试失败"

# 6. debug build
log "6/6 cargo build (debug)"
cargo build --quiet || fail "调试构建失败"

log "全部通过 ✅"
