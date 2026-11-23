# bug 修复验证脚本（PowerShell 5+）
#
# 用途：把「确认大模型真的消除了 bug」的五步闭环里能机器化的部分跑掉：
#   1) 改动范围体检 —— 查「注水信号」（吞错、断言被改软、门禁口径被改、契约面被改宽）；
#   2) 证据链检查  —— 这次改动到底有没有留下回归网（测试文件改动 / 断言数量）；
#   3) 全量门禁执行 —— Rust fmt+clippy+test、前端 lint+type-check+test、部署 lint、接口覆盖、api-surface 一致性。
#
# 退出码：0 = 通过 / 1 = 有阻断项（证据链缺证据 或 门禁失败）
# 用法：
#   powershell -ExecutionPolicy Bypass -File scripts/verify_bugfix.ps1
#   powershell -ExecutionPolicy Bypass -File scripts/verify_bugfix.ps1 -Base origin/main
#   powershell -ExecutionPolicy Bypass -File scripts/verify_bugfix.ps1 -SkipRust -SkipWeb
#
# 说明：本脚本只读不改代码；门禁失败不会自动修改任何文件。

param(
    # 对比基线：HEAD（默认，只看工作区未提交改动）或任意 git ref（如 origin/main）
    [string]$Base = "HEAD",
    [switch]$SkipRust,
    [switch]$SkipWeb,
    [switch]$SkipExtra,
    # 允许本次改动没有测试文件（纯文档/纯配置改动时使用）
    [switch]$AllowNoTest,
    # 只采集门禁结果、不因门禁失败判负（证据链缺证据仍然判负）
    [switch]$AllowGateFailure,
    # 可选：把汇总写到指定文件
    [string]$SummaryPath = ""
)

$ErrorActionPreference = "Continue"
$repoRoot = (Resolve-Path "$PSScriptRoot/..").Path
Push-Location $repoRoot

$script:results = New-Object System.Collections.ArrayList

function Add-Result {
    param([string]$Stage, [string]$Name, [string]$Status, [string]$Detail)
    [void]$script:results.Add([pscustomobject]@{
        Stage = $Stage; Name = $Name; Status = $Status; Detail = $Detail
    })
}

function Write-Step {
    param([string]$Title)
    Write-Host ""
    Write-Host "=== $Title ===" -ForegroundColor White
}

# ---------------------------------------------------------------------------
# 1. 改动范围体检
# ---------------------------------------------------------------------------

$gateFiles = @(
    'scripts/check_coverage.py', 'scripts/line_budget_frozen.txt', 'scripts/deploy_lint.ps1',
    'gateway-web/vitest.config.ts', 'Cargo.toml', 'Cargo.lock', 'docs/api-surface.md'
)
$contractPatterns = @(
    'gateway-web/src/api/generated/', 'docs/api-devices.md', 'docs/api-error-codes.md',
    'gateway-api/src/web/api/.*/model\.rs$'
)

function Is-TestFile {
    param([string]$Path)
    return ($Path -match '(^|/)tests?/') -or ($Path -match 'tests\.rs$') -or
           ($Path -match '_test\.rs$') -or ($Path -match '(^|/)test_[^/]*\.rs$') -or
           ($Path -match '\.spec\.ts$') -or ($Path -match '\.test\.ts$') -or
           ($Path -match '(^|/)__tests__/')
}

Write-Step "1. 改动范围体检"

# core.quotepath=false：让含中文的路径以原文返回，而不是八进制转义
$changed = @()
if ($Base -and $Base -ne "HEAD") { $changed += @(git -c core.quotepath=false diff --name-only "$Base..HEAD" 2>$null) }
$changed += @(git -c core.quotepath=false diff --name-only $Base 2>$null)
$changed += @(git -c core.quotepath=false ls-files --others --exclude-standard 2>$null)
$changed = @($changed | Where-Object { $_ -and $_.Trim() -ne "" } | Sort-Object -Unique)

if ($changed.Count -eq 0) {
    Write-Host "  未检测到任何改动（基线 $Base）" -ForegroundColor Yellow
    Add-Result "范围" "改动文件" "WARN" "基线 $Base 下没有改动"
} else {
    Write-Host "  改动文件 $($changed.Count) 个（基线 $Base）：" -ForegroundColor Cyan
    foreach ($f in $changed) { Write-Host "    $f" }
}

# 逐文件收集新增行
$addedByFile = @{}
$cur = $null
foreach ($l in @(git -c core.quotepath=false diff --unified=0 $Base 2>$null)) {
    if ($l -match '^\+\+\+ "?b/(.+?)"?$') {
        $cur = $matches[1]
        if (-not $addedByFile.ContainsKey($cur)) { $addedByFile[$cur] = New-Object System.Collections.ArrayList }
        continue
    }
    if ($cur -and $l.StartsWith("+") -and -not $l.StartsWith("+++")) { [void]$addedByFile[$cur].Add($l.Substring(1)) }
}
foreach ($f in @(git -c core.quotepath=false ls-files --others --exclude-standard 2>$null)) {
    if (-not $f) { continue }
    if (-not $addedByFile.ContainsKey($f)) { $addedByFile[$f] = New-Object System.Collections.ArrayList }
    try {
        Get-Content -LiteralPath $f -Encoding UTF8 | ForEach-Object { [void]$addedByFile[$f].Add($_) }
    } catch { }
}

# 吞错信号：把错误藏起来 = 把证据藏起来
$swallowPatterns = @(
    @{ name = 'unwrap_or_default()'; re = 'unwrap_or_default\s*\(' },
    @{ name = 'unwrap_or(0/0.0)'; re = 'unwrap_or\s*\(\s*0(\.0)?\s*\)' },
    @{ name = '.ok() 丢弃 Result'; re = '\.ok\(\)' },
    @{ name = '空 catch 块'; re = 'catch\s*\(\s*[^)]*\)\s*\{\s*\}' },
    @{ name = 'except: 裸吞'; re = 'except\s*:\s*$' },
    @{ name = '空实现体 / no-op'; re = '^\s*(pass|return None)\s*(#.*)?$' }
)

$swallowHits = New-Object System.Collections.ArrayList
foreach ($file in $addedByFile.Keys) {
    $lines = $addedByFile[$file]
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $line = $lines[$i]
        foreach ($p in $swallowPatterns) {
            if ($line -match $p.re) {
                [void]$swallowHits.Add(("{0}  [{1}] {2}" -f $file, $p.name, $line.Trim()))
            }
        }
    }
}
if ($swallowHits.Count -gt 0) {
    Write-Host "  [检查] 新增行里出现吞错 / no-op 模式 $($swallowHits.Count) 处，需人工确认是否为掩盖证据：" -ForegroundColor Yellow
    foreach ($h in ($swallowHits | Select-Object -First 15)) { Write-Host "    $h" -ForegroundColor DarkYellow }
    if ($swallowHits.Count -gt 15) { Write-Host "    ...（另有 $($swallowHits.Count - 15) 处）" }
    Add-Result "范围" "吞错模式" "WARN" "$($swallowHits.Count) 处待人工确认"
} else {
    Write-Host "  [检查] 未发现吞错 / no-op 新增行" -ForegroundColor Green
    Add-Result "范围" "吞错模式" "PASS" ""
}

# 门禁口径文件被改
$gateHits = @($changed | Where-Object {
    $p = $_
    ($gateFiles | Where-Object { $p -like "*$_*" }).Count -gt 0 -or $p -like '.github/workflows/*'
})
if ($gateHits.Count -gt 0) {
    Write-Host "  [检查] 改动了门禁 / 阈值口径文件，必须在说明里单独交代：" -ForegroundColor Yellow
    foreach ($h in $gateHits) { Write-Host "    $h" -ForegroundColor DarkYellow }
    Add-Result "范围" "门禁口径变更" "WARN" ($gateHits -join ', ')
} else {
    Write-Host "  [检查] 未改动门禁 / 阈值口径文件" -ForegroundColor Green
    Add-Result "范围" "门禁口径变更" "PASS" ""
}

# 契约面被改
$contractHits = @($changed | Where-Object {
    $p = $_
    ($contractPatterns | Where-Object { $p -match $_ }).Count -gt 0
})
if ($contractHits.Count -gt 0) {
    Write-Host "  [检查] 改动了前后端契约面，确认不是把契约改宽来让测试通过：" -ForegroundColor Yellow
    foreach ($h in $contractHits) { Write-Host "    $h" -ForegroundColor DarkYellow }
    Add-Result "范围" "契约面变更" "WARN" ($contractHits -join ', ')
} else {
    Write-Host "  [检查] 未改动前后端契约面" -ForegroundColor Green
    Add-Result "范围" "契约面变更" "PASS" ""
}

# ---------------------------------------------------------------------------
# 2. 证据链检查（有没有留下回归网）
# ---------------------------------------------------------------------------

Write-Step "2. 证据链检查"

$testFiles = @($changed | Where-Object { Is-TestFile $_ })
$srcFiles = @($changed | Where-Object { -not (Is-TestFile $_) -and $_ -notmatch '(^|/)docs/' -and $_ -notmatch '\.md$' })

if ($testFiles.Count -eq 0) {
    if ($AllowNoTest) {
        Write-Host "  [检查] 本次改动没有测试文件（已用 -AllowNoTest 放行）" -ForegroundColor Yellow
        Add-Result "证据" "回归网" "WARN" "无测试改动（显式放行）"
    } else {
        Write-Host "  [检查] 本次改动没有测试文件 —— 没有回归网，无法证明 bug 被消除" -ForegroundColor Red
        Add-Result "证据" "回归网" "FAIL" "无测试改动"
    }
} else {
    Write-Host "  [检查] 测试改动 $($testFiles.Count) 个文件：" -ForegroundColor Green
    foreach ($t in $testFiles) { Write-Host "    $t" }
    Add-Result "证据" "回归网" "PASS" "$($testFiles.Count) 个测试文件"
}

# 断言数量是否被改软（当前 vs 基线）
$weakened = New-Object System.Collections.ArrayList
foreach ($t in $testFiles) {
    $baseText = @(git -c core.quotepath=false show "$Base`:$t" 2>$null) -join "`n"
    $nowText = ""
    try { $nowText = (Get-Content -LiteralPath $t -Raw -Encoding UTF8) } catch { }
    if ($nowText -eq "") { continue }
    $re = 'assert|expect\(|assert_eq!|assert_ne!|should'
    $baseCount = ([regex]::Matches($baseText, $re)).Count
    $nowCount = ([regex]::Matches($nowText, $re)).Count
    if ($nowCount -lt $baseCount) {
        [void]$weakened.Add(("{0}  断言数 {1} -> {2}" -f $t, $baseCount, $nowCount))
    }
}
if ($weakened.Count -gt 0) {
    Write-Host "  [检查] 测试断言数量减少，可能是把断言改软来迁就实现：" -ForegroundColor Yellow
    foreach ($w in $weakened) { Write-Host "    $w" -ForegroundColor DarkYellow }
    Add-Result "证据" "断言强度" "WARN" "$($weakened.Count) 个文件断言减少"
} else {
    Write-Host "  [检查] 没有测试文件的断言数量下降" -ForegroundColor Green
    Add-Result "证据" "断言强度" "PASS" ""
}

if ($srcFiles.Count -gt 0) {
    Write-Host "  [提示] 源码改动 $($srcFiles.Count) 个文件；反证步骤（人为破坏修复看用例是否重新变红）需人工完成" -ForegroundColor Cyan
}

# ---------------------------------------------------------------------------
# 3. 门禁执行
# ---------------------------------------------------------------------------

function Invoke-Gate {
    param([string]$Name, [string]$WorkDir, [string]$Exe, [string[]]$CmdArgs)
    Write-Host "  > $Name" -ForegroundColor Cyan
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    Push-Location $WorkDir
    $code = 1
    try {
        # 用 Out-Host 回显子进程输出，避免它的 stdout 混进函数返回值
        & $Exe @CmdArgs 2>&1 | Out-Host
        $code = $LASTEXITCODE
    } catch {
        Write-Host "    执行异常: $($_.Exception.Message)" -ForegroundColor Red
        $code = 1
    } finally {
        Pop-Location
    }
    $sw.Stop()
    $secs = [int]$sw.Elapsed.TotalSeconds
    if ($code -eq 0) {
        Write-Host "    PASS (${secs}s)" -ForegroundColor Green
        Add-Result "门禁" $Name "PASS" "${secs}s"
    } else {
        Write-Host "    FAIL (exit=$code, ${secs}s)" -ForegroundColor Red
        Add-Result "门禁" $Name "FAIL" "exit=$code"
    }
    return $code
}

$gateFail = 0

if (-not $SkipRust) {
    Write-Step "3. Rust 门禁"
    $gateFail += (Invoke-Gate "cargo fmt --all -- --check" $repoRoot "cargo" @("fmt", "--all", "--", "--check"))
    $gateFail += (Invoke-Gate "cargo clippy --workspace --all-targets -- -D warnings" $repoRoot "cargo" @("clippy", "--workspace", "--all-targets", "--", "-D", "warnings"))
    $gateFail += (Invoke-Gate "cargo test --workspace" $repoRoot "cargo" @("test", "--workspace"))
} else {
    Write-Host "  （-SkipRust：跳过 Rust 门禁）" -ForegroundColor DarkGray
}

if (-not $SkipWeb) {
    Write-Step "4. 前端门禁"
    $webDir = Join-Path $repoRoot "gateway-web"
    if (Test-Path (Join-Path $webDir "node_modules")) {
        $gateFail += (Invoke-Gate "npm run lint" $webDir "npm" @("run", "lint"))
        $gateFail += (Invoke-Gate "npm run type-check" $webDir "npm" @("run", "type-check"))
        $gateFail += (Invoke-Gate "npm run test" $webDir "npm" @("run", "test"))
    } else {
        Write-Host "  [跳过] gateway-web/node_modules 缺失，前端门禁未执行（CI 侧仍会全量执行）" -ForegroundColor Yellow
        Add-Result "门禁" "前端门禁" "WARN" "node_modules 缺失"
    }
} else {
    Write-Host "  （-SkipWeb：跳过前端门禁）" -ForegroundColor DarkGray
}

if (-not $SkipExtra) {
    Write-Step "5. 附加门禁"
    $gateFail += (Invoke-Gate "deploy_lint" $repoRoot "powershell" @("-ExecutionPolicy", "Bypass", "-File", "scripts/deploy_lint.ps1"))
    $gateFail += (Invoke-Gate "api 测试覆盖盘点" $repoRoot "python" @("scripts/check_api_test_coverage.py"))
    $gateFail += (Invoke-Gate "api-surface 一致性" $repoRoot "python" @("scripts/gen_api_surface.py", "--check"))
} else {
    Write-Host "  （-SkipExtra：跳过附加门禁）" -ForegroundColor DarkGray
}

# ---------------------------------------------------------------------------
# 4. 汇总
# ---------------------------------------------------------------------------

Write-Step "6. 汇总"

$script:results | Format-Table -AutoSize | Out-String | Write-Host

$evidenceFail = @($script:results | Where-Object { $_.Stage -ne "门禁" -and $_.Status -eq "FAIL" }).Count
$warnCount = @($script:results | Where-Object { $_.Status -eq "WARN" }).Count

Write-Host "证据链阻断: $evidenceFail    门禁失败: $gateFail    警告: $warnCount"
Write-Host ""
Write-Host "判据：证据链完整 = 复现输出 + 新增回归用例 + 全量门禁 + 一次反证（人工）。" -ForegroundColor White
Write-Host "      本脚本只覆盖前两项与全量门禁；「复现输出」与「反证」仍需人工给出证据。" -ForegroundColor DarkGray

if ($SummaryPath -ne "") {
    $out = @()
    $out += "# bug 修复验证汇总（基线 $Base）"
    $out += ""
    $out += "| 阶段 | 项目 | 状态 | 说明 |"
    $out += "|---|---|---|---|"
    foreach ($r in $script:results) {
        $out += "| $($r.Stage) | $($r.Name) | $($r.Status) | $($r.Detail) |"
    }
    $out += ""
    $out += "证据链阻断: $evidenceFail    门禁失败: $gateFail    警告: $warnCount"
    $summaryDir = Split-Path -Parent $SummaryPath
    if ($summaryDir -and -not (Test-Path $summaryDir)) { New-Item -ItemType Directory -Path $summaryDir -Force | Out-Null }
    Set-Content -LiteralPath $SummaryPath -Value ($out -join "`n") -Encoding UTF8
    Write-Host "汇总已写入: $SummaryPath" -ForegroundColor Cyan
}

Pop-Location

if ($evidenceFail -gt 0) { exit 1 }
if ($gateFail -gt 0) {
    if ($AllowGateFailure) {
        Write-Host "门禁失败已按 -AllowGateFailure 降级为警告。" -ForegroundColor Yellow
        exit 0
    }
    exit 1
}
exit 0
