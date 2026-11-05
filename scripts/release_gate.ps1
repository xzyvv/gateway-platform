# 发布门禁（Windows 本地一键）：按 CI 同口径串起后端 / 前端 / 端到端 / 仓库一致性检查
#
# 定位：
#   scripts/e2e_smoke.ps1      -> 真实进程冒烟（单个场景）
#   gateway-core/scripts/ci.sh -> 仅 Rust 五阶段（Linux 口径）
#   本脚本                     -> 发版前「全仓一次跑完」，覆盖 CI 的 check / e2e / 前端 /
#                                 api-surface / 仓库健康度，输出统一结论与日志目录
#
# 与 CI 的差异（刻意为之，不是遗漏）：
#   - cargo check/clippy 用默认 features：Windows 上 --all-features 会带上 encryption
#     （bundled-sqlcipher），依赖本机 OpenSSL 构建环境；该路径由 CI 的
#     encryption-check job 单独覆盖，本地发布机不重复承担。
#   - 协议 feature 矩阵 / SKU 矩阵 / chaos / 覆盖率基线等 Linux CI 专属 job 不在此处
#     重复；需要时用 -IncludeChaos 单独跑 chaos，其余以 CI 结论为准。
#
# 用法：
#   .\scripts\release_gate.ps1                     # 全量门禁（默认 fail-fast）
#   .\scripts\release_gate.ps1 -KeepGoing          # 全部跑完再汇总（发布前拿全貌）
#   .\scripts\release_gate.ps1 -SkipFrontend       # 只跑后端 + 仓库一致性
#   .\scripts\release_gate.ps1 -SkipE2E -SkipSmoke # 快速回归（不跑浏览器/真实进程）
#   .\scripts\release_gate.ps1 -IncludeChaos       # 追加 chaos 场景
#   .\scripts\release_gate.ps1 -InstallDeps        # 先 npm ci（缺 node_modules 时）
#   .\scripts\release_gate.ps1 -InstallPlaywright  # 先装 Playwright chromium
#   .\scripts\release_gate.ps1 -Only fmt,web-lint  # 只跑指定步骤（重跑失败项）
#   .\scripts\release_gate.ps1 -ListOnly           # 只看步骤清单，不执行
#
# 退出码：0 = 通过（允许有 SKIP）；1 = 有 FAIL 或前置检查失败。
#
# 日志：默认写 target\release-gate\<时间戳>\，含每步 *.log 与 summary.md；
#       target/ 已被 .gitignore 忽略，不会污染仓库。

[CmdletBinding()]
param(
    [switch]$SkipBackend,
    [switch]$SkipFrontend,
    [switch]$SkipE2E,
    [switch]$SkipSmoke,
    [switch]$SkipCoverage,
    [switch]$IncludeChaos,
    [switch]$KeepGoing,
    [switch]$InstallDeps,
    [switch]$InstallPlaywright,
    [string[]]$Only = @(),
    [int]$E2EPort = 18081,
    [int]$SmokePort = 18080,
    [string]$LogDir = "",
    [switch]$ListOnly
)

$ErrorActionPreference = "Continue"
$repoRoot = Split-Path -Parent $PSScriptRoot
$webRoot = Join-Path $repoRoot "gateway-web"
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
if ([string]::IsNullOrWhiteSpace($LogDir)) {
    $LogDir = Join-Path $repoRoot "target\release-gate\$stamp"
}
New-Item -ItemType Directory -Force -Path $LogDir | Out-Null

# ===== 环境变量：统一口径，结束还原 =====
$script:savedRustflags = $null
if (Test-Path "Env:RUSTFLAGS") {
    $script:savedRustflags = $env:RUSTFLAGS
    Remove-Item "Env:RUSTFLAGS"
}
$script:savedCargoColor = $null
if (Test-Path "Env:CARGO_TERM_COLOR") {
    $script:savedCargoColor = $env:CARGO_TERM_COLOR
}
$env:CARGO_TERM_COLOR = "never"

# ===== 能力探测：缺什么就把对应步骤标 SKIP，而不是让整条门禁莫名失败 =====
$npmCmd = Get-Command npm -ErrorAction SilentlyContinue
$gitCmd = Get-Command git -ErrorAction SilentlyContinue
$pythonCmd = Get-Command python -ErrorAction SilentlyContinue
if ($null -eq $pythonCmd) { $pythonCmd = Get-Command python3 -ErrorAction SilentlyContinue }
$nodeCmd = Get-Command node -ErrorAction SilentlyContinue
$llvmCovReady = $null -ne (Get-Command cargo-llvm-cov -ErrorAction SilentlyContinue)
$gatewaydReady = Test-Path (Join-Path $repoRoot "target\debug\gatewayd.exe")
$nodeModulesReady = Test-Path (Join-Path $webRoot "node_modules")
$configReady = Test-Path (Join-Path $repoRoot "config.json")
$playwrightBrowsersReady = Test-Path (Join-Path $env:LOCALAPPDATA "ms-playwright")

$script:steps = New-Object System.Collections.ArrayList
$script:results = New-Object System.Collections.ArrayList
$script:stepNo = 0
$script:stop = $false
$script:currentLog = ""

# 子脚本（e2e_smoke / check_repo_health）以 `exit` 结束，必须放子进程跑，
# 否则会把宿主脚本一起退出。
$psExe = Join-Path $PSHOME "powershell.exe"
if (-not (Test-Path $psExe)) { $psExe = Join-Path $PSHOME "pwsh.exe" }

function Add-Step {
    param(
        [string]$Id,
        [string]$Title,
        [scriptblock]$Action,
        [string]$SkipReason = ""
    )
    [void]$script:steps.Add([pscustomobject]@{
            Id         = $Id
            Title      = $Title
            Action     = $Action
            SkipReason = $SkipReason
        })
}

function Add-Result {
    param([string]$Id, [string]$Title, [string]$Status, [double]$Seconds, [string]$Note)
    [void]$script:results.Add([pscustomobject]@{
            Id      = $Id
            Title   = $Title
            Status  = $Status
            Seconds = $Seconds
            Note    = $Note
        })
}

function Run-Native {
    param(
        [string]$Exe,
        [string[]]$Arguments,
        [string]$WorkDir = $repoRoot,
        [hashtable]$EnvVars = @{}
    )
    $backup = @{}
    foreach ($k in $EnvVars.Keys) {
        $backup[$k] = [Environment]::GetEnvironmentVariable($k)
        [Environment]::SetEnvironmentVariable($k, $EnvVars[$k])
    }
    Push-Location $WorkDir
    try {
        & $Exe @Arguments 2>&1 | Tee-Object -FilePath $script:currentLog | ForEach-Object { Write-Host $_ }
        $code = $LASTEXITCODE
    }
    finally {
        Pop-Location
        foreach ($k in $backup.Keys) { [Environment]::SetEnvironmentVariable($k, $backup[$k]) }
    }
    if ($null -eq $code) { $code = 0 }
    return ($code -eq 0)
}

function Run-PsScript {
    param([string]$ScriptPath, [string[]]$Arguments = @())
    $allArgs = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $ScriptPath) + $Arguments
    return (Run-Native -Exe $psExe -Arguments $allArgs)
}

function Invoke-Step {
    param([pscustomobject]$Step)

    if (-not [string]::IsNullOrWhiteSpace($Step.SkipReason)) {
        Write-Host "[SKIP] $($Step.Title)：$($Step.SkipReason)" -ForegroundColor Yellow
        Add-Result $Step.Id $Step.Title "SKIP" 0 $Step.SkipReason
        return
    }

    if ($script:stop) {
        Add-Result $Step.Id $Step.Title "SKIP" 0 "前序步骤失败，按 fail-fast 停止"
        return
    }

    $script:stepNo++
    $script:currentLog = Join-Path $LogDir ("{0:00}-{1}.log" -f $script:stepNo, $Step.Id)
    Write-Host ""
    Write-Host (("[{0}] {1}" -f $script:stepNo, $Step.Title)) -ForegroundColor Cyan

    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $status = "PASS"
    $note = ""
    try {
        $ok = & $Step.Action
        if (-not $ok) {
            $status = "FAIL"
            $note = "命令返回非 0，详见 $(Split-Path -Leaf $script:currentLog)"
        }
    }
    catch {
        $status = "FAIL"
        $note = $_.Exception.Message
    }
    $sw.Stop()
    $seconds = [math]::Round($sw.Elapsed.TotalSeconds, 1)
    Add-Result $Step.Id $Step.Title $status $seconds $note

    if ($status -eq "FAIL") {
        Write-Host ("[FAIL] {0}（{1}s）{2}" -f $Step.Title, $seconds, $note) -ForegroundColor Red
        if (-not $KeepGoing) { $script:stop = $true }
    }
    else {
        Write-Host ("[PASS] {0}（{1}s）" -f $Step.Title, $seconds) -ForegroundColor Green
    }
}

function Write-Summary {
    param([string]$Conclusion)
    $lines = New-Object System.Collections.ArrayList
    [void]$lines.Add("# 发布门禁报告")
    [void]$lines.Add("")
    [void]$lines.Add("- 时间：$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')")
    [void]$lines.Add("- 仓库：$repoRoot")
    [void]$lines.Add("- 日志：$LogDir")
    [void]$lines.Add("")
    [void]$lines.Add("| # | 步骤 | 结果 | 耗时(s) | 说明 |")
    [void]$lines.Add("|---|------|------|---------|------|")
    $i = 0
    foreach ($r in $script:results) {
        $i++
        $noteText = $r.Note -replace "`r?`n", " "
        [void]$lines.Add("| $i | $($r.Title) | $($r.Status) | $($r.Seconds) | $noteText |")
    }
    [void]$lines.Add("")
    [void]$lines.Add("## 结论")
    [void]$lines.Add("")
    [void]$lines.Add($Conclusion)
    $path = Join-Path $LogDir "summary.md"
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($path, ($lines -join "`r`n"), $utf8NoBom)
    return $path
}

# =============================================================================
# 步骤清单（顺序 = 由快到慢，尽早暴露廉价问题）
# =============================================================================

Add-Step -Id "preflight" -Title "前置检查（目录 / 工具链 / 可选依赖）" -Action {
    $ok = $true
    $required = @(
        "gateway-web\package.json",
        "gateway-core\Cargo.toml",
        "gateway-api\Cargo.toml",
        "gatewayd\Cargo.toml",
        "gateway-desktop\src-tauri\Cargo.toml"
    )
    foreach ($p in $required) {
        if (-not (Test-Path (Join-Path $repoRoot $p))) {
            Write-Host "缺少必需路径：$p" -ForegroundColor Red
            $ok = $false
        }
    }
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Write-Host "未检测到 cargo（请确认 rustup 工具链在 PATH 中）" -ForegroundColor Red
        $ok = $false
    }
    Write-Host ("工具链：cargo={0} / node={1} / npm={2} / python={3}" -f
        $(if (Get-Command cargo -ErrorAction SilentlyContinue) { (cargo --version) } else { "缺失" }),
        $(if (Get-Command node -ErrorAction SilentlyContinue) { (node --version) } else { "缺失" }),
        $(if ($npmCmd) { (& npm --version) } else { "缺失" }),
        $(if ($pythonCmd) { (& $pythonCmd.Source --version) } else { "缺失" }))
    Write-Host ("gateway-web/node_modules={0} / config.json={1} / Playwright浏览器={2}" -f
        $nodeModulesReady, $configReady, $playwrightBrowsersReady)
    return $ok
}

Add-Step -Id "repo-health" -Title "仓库健康度扫描（bare CR / toolchain / ts-rs / CRLF / 产物泄漏）" -Action {
    return (Run-PsScript -ScriptPath (Join-Path $repoRoot "scripts\check_repo_health.ps1"))
}

Add-Step -Id "api-surface" -Title "API 端点清单防漂移（gen_api_surface.py --check）" -Action {
    return (Run-Native -Exe $pythonCmd.Source -Arguments @("scripts/gen_api_surface.py", "--check"))
} -SkipReason $(if ($null -eq $pythonCmd) { "未检测到 python/python3，无法校验 docs/api-surface.md（CI check job 覆盖此项）" } else { "" })

if (-not $SkipBackend) {
    Add-Step -Id "fmt" -Title "Rust 格式检查（cargo fmt --check）" -Action {
        return (Run-Native -Exe "cargo" -Arguments @("fmt", "--all", "--", "--check"))
    }

    Add-Step -Id "check" -Title "Rust 编译检查（cargo check --all-targets，Windows CI 口径）" -Action {
        return (Run-Native -Exe "cargo" -Arguments @("check", "--all-targets"))
    }

    Add-Step -Id "clippy" -Title "Rust clippy（-D warnings）" -Action {
        return (Run-Native -Exe "cargo" -Arguments @("clippy", "--all-targets", "--", "-D", "warnings"))
    }

    Add-Step -Id "test-unit" -Title "Rust 单元测试（cargo test --lib，全 workspace）" -Action {
        return (Run-Native -Exe "cargo" -Arguments @("test", "--lib", "--quiet"))
    }

    Add-Step -Id "test-license" -Title "license-issuer 单元测试（无 lib target，单独跑）" -Action {
        return (Run-Native -Exe "cargo" -Arguments @("test", "-p", "license-issuer", "--quiet"))
    }

    Add-Step -Id "test-contract" -Title "集成 / 契约测试（arch_deps / api_contract / license ...）" -Action {
        $ok1 = Run-Native -Exe "cargo" -Arguments @("test", "-p", "gateway-core", "--quiet", "--test", "arch_deps")
        if (-not $ok1) { return $false }
        $ok2 = Run-Native -Exe "cargo" -Arguments @(
            "test", "-p", "gateway-api", "--quiet",
            "--test", "integration",
            "--test", "api_contract",
            "--test", "arch_deps",
            "--test", "self_heal",
            "--test", "license_api",
            "--test", "license_feature_free",
            "--test", "license_quota_checks"
        )
        return $ok2
    }

    Add-Step -Id "ts-bindings" -Title "ts-rs 生成物是否已提交（git status）" -Action {
        $out = & $gitCmd.Source status --porcelain -- gateway-web/src/api/generated/ 2>&1
        if ($LASTEXITCODE -ne 0) {
            Write-Host "git status 执行失败，跳过该项" -ForegroundColor Yellow
            return $true
        }
        if ($out) {
            Write-Host "以下生成物未提交，请运行 cargo test --lib 后提交 gateway-web/src/api/generated/：" -ForegroundColor Red
            $out | ForEach-Object { Write-Host $_ }
            return $false
        }
        return $true
    } -SkipReason $(if ($null -eq $gitCmd) { "未检测到 git，无法判定 ts-rs 生成物是否已提交（CI check job 覆盖此项）" } else { "" })

    Add-Step -Id "build-gatewayd" -Title "构建发布入口二进制（cargo build -p gatewayd）" -Action {
        return (Run-Native -Exe "cargo" -Arguments @("build", "-p", "gatewayd", "--quiet"))
    }

    Add-Step -Id "check-deadcode" -Title "dead_code 严格门禁（RUSTFLAGS=-D dead_code）" -Action {
        return (Run-Native -Exe "cargo" -Arguments @("check", "--all-targets") -EnvVars @{ RUSTFLAGS = "-D dead_code" })
    }

    if ($IncludeChaos) {
        Add-Step -Id "chaos" -Title "chaos 故障恢复场景（cargo test --test chaos）" -Action {
            return (Run-Native -Exe "cargo" -Arguments @("test", "-p", "gateway-core", "--test", "chaos", "--quiet"))
        }
    }

    if (-not $SkipCoverage) {
        # 与 CI 的差异：本地不带 --all-features（Windows 上 encryption/bundled-sqlcipher
        # 需要本机 OpenSSL 构建环境），故用默认 features 的口径；阈值来自 CI 实测值，
        # 只作"防回落地板"，不放宽也不收紧。
        # 只测 gateway-core / gateway-api：check_coverage.py 的分桶只认这两个 crate 的源文件，
        # 而 --workspace 会把 gateway-desktop（Tauri）一起编译，白等十几分钟且与阈值无关。
        Add-Step -Id "backend-coverage" -Title "后端覆盖率门禁（cargo llvm-cov + check_coverage.py）" -Action {
            $lcovPath = Join-Path $LogDir "lcov.info"
            $covOk = Run-Native -Exe "cargo" -Arguments @("llvm-cov", "-p", "gateway-core", "-p", "gateway-api", "--lcov", "--output-path", $lcovPath)
            if (-not $covOk) { return $false }
            return (Run-Native -Exe $pythonCmd.Source -Arguments @("scripts/check_coverage.py", $lcovPath))
        } -SkipReason $(if ($null -eq $pythonCmd) { "未检测到 python/python3，无法运行 check_coverage.py" } elseif (-not $llvmCovReady) { "未安装 cargo-llvm-cov，先执行 cargo install cargo-llvm-cov 并 rustup component add llvm-tools-preview" } else { "" })
    }
}

if (-not $SkipFrontend) {
    $webSkipReason = ""
    if ($null -eq $npmCmd) {
        $webSkipReason = "未检测到 npm，跳过前端检查"
    }
    elseif (-not $nodeModulesReady -and -not $InstallDeps) {
        $webSkipReason = "gateway-web/node_modules 缺失，先执行 npm ci（或加 -InstallDeps）"
    }

    if ($InstallDeps) {
        Add-Step -Id "web-deps" -Title "安装前端依赖（npm ci）" -Action {
            return (Run-Native -Exe "npm" -Arguments @("ci") -WorkDir $webRoot)
        } -SkipReason $(if ($null -eq $npmCmd) { "未检测到 npm" } else { "" })
    }

    Add-Step -Id "web-lint" -Title "前端 lint（eslint，warning 零容忍）" -Action {
        return (Run-Native -Exe "npm" -Arguments @("run", "lint", "--", "--max-warnings", "0") -WorkDir $webRoot)
    } -SkipReason $webSkipReason

    Add-Step -Id "web-typecheck" -Title "前端类型检查（vue-tsc --noEmit）" -Action {
        return (Run-Native -Exe "npm" -Arguments @("run", "type-check") -WorkDir $webRoot)
    } -SkipReason $webSkipReason

    Add-Step -Id "web-build" -Title "前端构建（vue-tsc + vite build）" -Action {
        return (Run-Native -Exe "npm" -Arguments @("run", "build") -WorkDir $webRoot)
    } -SkipReason $webSkipReason

    Add-Step -Id "web-bundle" -Title "前端产物体积预算（check:bundle）" -Action {
        return (Run-Native -Exe "npm" -Arguments @("run", "check:bundle") -WorkDir $webRoot)
    } -SkipReason $webSkipReason

    if ($SkipCoverage) {
        Add-Step -Id "web-test" -Title "前端单元测试（vitest run，不统计覆盖率）" -Action {
            return (Run-Native -Exe "npm" -Arguments @("test", "--", "--maxWorkers=2") -WorkDir $webRoot)
        } -SkipReason $webSkipReason
    }
    else {
        # 固定 2 workers：本机 4 workers 下 vmThreads 池偶发 worker RPC 提前关闭
        # （表现为 theme.spec 某个用例卡到超时 + EnvironmentTeardownError），
        # 2 workers 实测 176 文件 / 1573 用例全绿且更快（44s vs 153s）。
        # CI 仍用默认并发，本地门禁优先稳定。
        Add-Step -Id "web-test" -Title "前端单元测试 + 覆盖率门禁（vitest coverage，2 workers）" -Action {
            return (Run-Native -Exe "npm" -Arguments @("run", "test:coverage", "--", "--reporter=dot", "--maxWorkers=2") -WorkDir $webRoot)
        } -SkipReason $webSkipReason
    }

    if (-not $SkipE2E) {
        if ($InstallPlaywright) {
            Add-Step -Id "web-e2e-install" -Title "安装 Playwright chromium" -Action {
                return (Run-Native -Exe "npm" -Arguments @("exec", "--", "playwright", "install", "chromium") -WorkDir $webRoot)
            } -SkipReason $webSkipReason
        }

        Add-Step -Id "web-e2e" -Title ("Playwright E2E（chromium，隔离端口 {0}）" -f $E2EPort) -Action {
            return (Run-Native -Exe "npm" -Arguments @("run", "e2e") -WorkDir $webRoot -EnvVars @{ GATEWAY_E2E_PORT = "$E2EPort" })
        } -SkipReason $(if ($webSkipReason) { $webSkipReason } elseif (-not $playwrightBrowsersReady -and -not $InstallPlaywright) { "未检测到 Playwright 浏览器，先执行 npm run e2e:install（或加 -InstallPlaywright）" } else { "" })
    }
}

if (-not $SkipSmoke) {
    Add-Step -Id "smoke" -Title "真实进程端到端冒烟（gatewayd + 隔离端口 $SmokePort）" -Action {
        $smokeScript = Join-Path $repoRoot "scripts\e2e_smoke.ps1"
        return (Run-PsScript -ScriptPath $smokeScript -Arguments @("-Port", "$SmokePort", "-Build"))
    } -SkipReason $(if (-not $configReady) { "缺少 config.json（e2e_smoke 以它为基线生成隔离配置）" } else { "" })

    # 与上一步的分工：e2e_smoke 校验 API 契约（总线为空，不采集）；
    # chain-smoke 校验**采集链路**——真实 Modbus TCP 从站 + 真实轮询 + 入队 + 历史查回。
    Add-Step -Id "chain-smoke" -Title "采集全链路冒烟（Modbus TCP 从站 + 真实轮询 + /api/history）" -Action {
        return (Run-Native -Exe "node" -Arguments @("scripts/smoke/chain-smoke.mjs"))
    } -SkipReason $(if ($null -eq $nodeCmd) { "未检测到 node，无法运行 scripts/smoke/chain-smoke.mjs" } elseif (-not $gatewaydReady) { "未找到 target\debug\gatewayd.exe，先 cargo build -p gatewayd" } else { "" })
}

# =============================================================================
# 执行
# =============================================================================

$selected = @($script:steps | Where-Object {
        if ($Only.Count -eq 0) { $true } else { $Only -contains $_.Id }
    })

if ($ListOnly) {
    Write-Host "发布门禁步骤清单（$($selected.Count) 项，未执行）：" -ForegroundColor Cyan
    foreach ($s in $selected) {
        $mark = if ([string]::IsNullOrWhiteSpace($s.SkipReason)) { "RUN " } else { "SKIP" }
        $suffix = if ([string]::IsNullOrWhiteSpace($s.SkipReason)) { "" } else { "（$($s.SkipReason)）" }
        Write-Host ("  [{0}] {1,-16} {2}{3}" -f $mark, $s.Id, $s.Title, $suffix)
    }
    exit 0
}

Write-Host "发布门禁开始：$repoRoot" -ForegroundColor Cyan
Write-Host "日志目录：$LogDir"
Write-Host ("跳过项：后端={0} 前端={1} E2E={2} 冒烟={3}" -f $SkipBackend, $SkipFrontend, $SkipE2E, $SkipSmoke)

try {
    foreach ($step in $selected) {
        Invoke-Step -Step $step
    }
}
finally {
    if ($null -ne $script:savedRustflags) { $env:RUSTFLAGS = $script:savedRustflags } else { Remove-Item "Env:RUSTFLAGS" -ErrorAction SilentlyContinue }
    if ($null -ne $script:savedCargoColor) { $env:CARGO_TERM_COLOR = $script:savedCargoColor } else { Remove-Item "Env:CARGO_TERM_COLOR" -ErrorAction SilentlyContinue }
}

$failCount = @($script:results | Where-Object { $_.Status -eq "FAIL" }).Count
$passCount = @($script:results | Where-Object { $_.Status -eq "PASS" }).Count
$skipCount = @($script:results | Where-Object { $_.Status -eq "SKIP" }).Count
$conclusion = if ($failCount -eq 0) {
    "通过（PASS $passCount / SKIP $skipCount）"
}
else {
    "未通过（FAIL $failCount / PASS $passCount / SKIP $skipCount）"
}
$summaryPath = Write-Summary -Conclusion $conclusion

Write-Host ""
Write-Host "================ 发布门禁结果 ================" -ForegroundColor Cyan
foreach ($r in $script:results) {
    $color = switch ($r.Status) {
        "PASS" { "Green" }
        "FAIL" { "Red" }
        default { "Yellow" }
    }
    Write-Host ("[{0}] {1}（{2}s）{3}" -f $r.Status, $r.Title, $r.Seconds, $r.Note) -ForegroundColor $color
}
Write-Host ""
Write-Host "结论：$conclusion" -ForegroundColor $(if ($failCount -eq 0) { "Green" } else { "Red" })
Write-Host "报告：$summaryPath"
Write-Host "日志：$LogDir"

if ($failCount -ne 0) { exit 1 }
exit 0
