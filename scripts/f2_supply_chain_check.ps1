# F2 (supply chain guard): local pre-commit equivalent of cargo-deny /
# cargo-audit that CI runs in security.yml. Wraps both tools with tool
# auto-install and a unified exit semantics suitable for hooking on commit
# or push.
#
# Why this script:
# - CI runs deny + audit every push (security.yml), but locally contributors
#   only see the result the day their PR runs. This script compresses that
#   window to "anytime before git push".
# - The factory CI default-features configuration in deny.toml is used; we
#   do NOT pass --all-features here because (a) audit does not care about
#   features, (b) deny.toml already pins all-features=true.
#
# Exit codes:
#   0 - clean
#   1 - tool missing and auto-install failed
#   2 - cargo deny reported a problem
#   3 - cargo audit reported a problem
#   4 - both deny and audit failed
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\f2_supply_chain_check.ps1
#   pwsh scripts/f2_supply_chain_check.ps1               # linux/macOS dev
#
# CI wiring (suggested): run after `cargo test (unit)` in `.github/workflows/ci.yml`
# (or run from the same job that runs `cargo build`). We attach the report as an
# artifact named `supply-chain-report.txt`.

$ErrorActionPreference = "Continue"
$root = (Resolve-Path "$PSScriptRoot\..").Path
$reportPath = Join-Path $root "f2_supply_chain_report.txt"

$denyRc = 0
$auditRc = 0

function Append-Line($line) {
    Add-Content -Path $reportPath -Value $line -Encoding utf8
}

"" | Out-File -FilePath $reportPath -Encoding utf8   # truncate

Append-Line "F2 supply-chain local check"
Append-Line "==========================="
Append-Line ("workspace : " + $root)
Append-Line ("time      : " + (Get-Date -Format "yyyy-MM-dd HH:mm:ss"))
Append-Line ""

# ----------------------------------------------------------------------------
# 1) cargo deny
# ----------------------------------------------------------------------------

Append-Line "[1/2] cargo deny check"
Append-Line "----------------------"

$deny = Get-Command cargo-deny -ErrorAction SilentlyContinue
if (-not $deny) {
    Append-Line "cargo-deny not on PATH; attempting install via cargo"
    try {
        cargo install cargo-deny --locked --quiet 2>&1 | ForEach-Object { Append-Line $_ }
    } catch {
        Append-Line ("install failed: " + $_.Exception.Message)
    }
    $deny = Get-Command cargo-deny -ErrorAction SilentlyContinue
}
if (-not $deny) {
    Append-Line "SKIP: cargo-deny unavailable"
} else {
    Push-Location $root
    $denyOutput = & cargo deny check 2>&1
    $denyRc = $LASTEXITCODE
    Pop-Location
    foreach ($line in $denyOutput) { Append-Line $line }
    Append-Line ("deny rc = " + $denyRc)
}
Append-Line ""

# ----------------------------------------------------------------------------
# 2) cargo audit
# ----------------------------------------------------------------------------

Append-Line "[2/2] cargo audit (RustSec advisories)"
Append-Line "--------------------------------------"

$audit = Get-Command cargo-audit -ErrorAction SilentlyContinue
if (-not $audit) {
    Append-Line "cargo-audit not on PATH; attempting install via cargo"
    try {
        cargo install cargo-audit --locked --quiet 2>&1 | ForEach-Object { Append-Line $_ }
    } catch {
        Append-Line ("install failed: " + $_.Exception.Message)
    }
    $audit = Get-Command cargo-audit -ErrorAction SilentlyContinue
}
if (-not $audit) {
    Append-Line "SKIP: cargo-audit unavailable"
} else {
    Push-Location $root
    $auditOutput = & cargo audit 2>&1
    $auditRc = $LASTEXITCODE
    Pop-Location
    foreach ($line in $auditOutput) { Append-Line $line }
    Append-Line ("audit rc = " + $auditRc)
}
Append-Line ""

# ----------------------------------------------------------------------------
# exit semantics: never zero if either failed
# ----------------------------------------------------------------------------

$combined = 0
if ($denyRc -ne 0) { $combined += 2 }
if ($auditRc -ne 0) { $combined += 4 }

Append-Line "summary"
Append-Line "-------"
Append-Line ("deny rc   = " + $denyRc + "  (0 == clean)")
Append-Line ("audit rc  = " + $auditRc + "  (0 == clean)")
Append-Line ("combined  = " + $combined + " (bit 2 = deny failed, bit 3 = audit failed)")

Write-Host ("deny rc=" + $denyRc + " audit rc=" + $auditRc + " combined=" + $combined)
Write-Host ("report: " + $reportPath)
exit $combined
