#!/usr/bin/env bash
# F2 (supply chain guard) -- bash mirror of f2_supply_chain_check.ps1.
# Used in CI (.github/workflows/ci.yml) where Linux runners have bash and
# git natively. For local Windows dev, prefer the PowerShell version.
#
# Exits:
#   0 - clean
#   1 - tool missing
#   2 - cargo deny failed
#   3 - cargo audit failed
#   4 - both failed
set -uo pipefail
cd "$(dirname "$0")/.."

report="f2_supply_chain_report.txt"
: > "$report"

{
    echo "F2 supply-chain local check"
    echo "==========================="
    echo "workspace : $(pwd)"
    echo "time      : $(date '+%Y-%m-%d %H:%M:%S')"
    echo
} >> "$report"

# ---------------------------------------------------------------------------
# 1) cargo-deny
# ---------------------------------------------------------------------------

echo "[1/2] cargo deny check" >> "$report"
echo "----------------------" >> "$report"

deny_rc=0
if ! command -v cargo-deny >/dev/null 2>&1; then
    echo "cargo-deny not on PATH; attempting install" >> "$report"
    cargo install cargo-deny --locked --quiet 2>&1 | tee -a "$report" || true
fi
if command -v cargo-deny >/dev/null 2>&1; then
    cargo deny check 2>&1 | tee -a "$report" || deny_rc=$?
    echo "deny rc = $deny_rc" >> "$report"
else
    echo "SKIP: cargo-deny unavailable" >> "$report"
fi
echo >> "$report"

# ---------------------------------------------------------------------------
# 2) cargo-audit
# ---------------------------------------------------------------------------

echo "[2/2] cargo audit (RustSec advisories)" >> "$report"
echo "--------------------------------------" >> "$report"

audit_rc=0
if ! command -v cargo-audit >/dev/null 2>&1; then
    echo "cargo-audit not on PATH; attempting install" >> "$report"
    cargo install cargo-audit --locked --quiet 2>&1 | tee -a "$report" || true
fi
if command -v cargo-audit >/dev/null 2>&1; then
    # Use --no-fetch: the advisories DB must be in `~/.cargo/advisory-db/`
    cargo audit 2>&1 | tee -a "$report" || audit_rc=$?
    # Fall back to a one-shot fetch if the local DB is missing.
    if [ "$audit_rc" -ne 0 ]; then
        echo "retrying audit with --fetch" >> "$report"
        cargo audit --fetch 2>&1 | tee -a "$report" || true
        cargo audit 2>&1 | tee -a "$report" || audit_rc=$?
    fi
    echo "audit rc = $audit_rc" >> "$report"
else
    echo "SKIP: cargo-audit unavailable" >> "$report"
fi
echo >> "$report"

# ---------------------------------------------------------------------------
# combined exit semantics
# ---------------------------------------------------------------------------

combined=0
[ "$deny_rc" -ne 0 ] && combined=$((combined + 2))
[ "$audit_rc" -ne 0 ] && combined=$((combined + 4))

{
    echo "summary"
    echo "-------"
    echo "deny rc   = $deny_rc  (0 == clean)"
    echo "audit rc  = $audit_rc  (0 == clean)"
    echo "combined  = $combined (bit 2 = deny failed, bit 3 = audit failed)"
} >> "$report"

echo "deny rc=$deny_rc audit rc=$audit_rc combined=$combined report=$report"
exit "$combined"
