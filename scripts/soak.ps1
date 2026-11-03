# 浸泡测试驱动脚本（Windows / PowerShell 5+）
#
# 职责分工：
#   Rust（cargo test --test soak soak_run）  -> 跑真实总线/调度/管道链路、注入故障、写业务指标
#   本脚本                                  -> 采样进程级 RSS / 句柄数、串起“运行 -> 判定”两阶段
#
# 用法：
#   .\scripts\soak.ps1                          # 默认 24h
#   .\scripts\soak.ps1 -DurationSecs 120        # 2 分钟冒烟
#   .\scripts\soak.ps1 -SkipRun                 # 只对已有产物重新判定
#
# 产物（默认 .\soak-out）：metrics.jsonl / proc.jsonl / events.jsonl / final.json / report.md

[CmdletBinding()]
param(
    [int]$DurationSecs = 86400,
    [int]$WarmupSecs = 0,           # 0 = 由 Rust 侧按总时长自动推导
    [int]$Buses = 4,
    [int]$SlavesPerBus = 10,
    [int]$PointsPerSlave = 50,
    [int]$IntervalSecs = 5,
    [int]$SampleMs = 10000,
    [string]$OutDir = "",
    [switch]$SkipRun,
    [switch]$NoBuild
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$coreDir = Join-Path $repoRoot "gateway-core"
if ([string]::IsNullOrWhiteSpace($OutDir)) {
    $OutDir = Join-Path $coreDir "soak-out"
}
$OutDir = [System.IO.Path]::GetFullPath($OutDir)
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

# ---- 环境变量：Rust 侧读取 SOAK_* ----
$env:SOAK_DURATION_SECS = $DurationSecs
if ($WarmupSecs -gt 0) { $env:SOAK_WARMUP_SECS = $WarmupSecs }
$env:SOAK_BUSES = $Buses
$env:SOAK_SLAVES_PER_BUS = $SlavesPerBus
$env:SOAK_POINTS_PER_SLAVE = $PointsPerSlave
$env:SOAK_INTERVAL_SECS = $IntervalSecs
$env:SOAK_SAMPLE_MS = $SampleMs
$env:SOAK_OUT_DIR = $OutDir
# 隔离 cargo 目标目录，避免与日常构建互相踢锁
if (-not $env:CARGO_TARGET_DIR) {
    $env:CARGO_TARGET_DIR = Join-Path $repoRoot "target"
}

Write-Host "[soak] 输出目录 : $OutDir"
Write-Host "[soak] 规模     : $Buses 总线 x $SlavesPerBus 从站 x $PointsPerSlave 点 = $($Buses * $SlavesPerBus * $PointsPerSlave) 点 / 周期 ${IntervalSecs}s"
Write-Host "[soak] 时长     : ${DurationSecs}s / 采样 ${SampleMs}ms"

$exitCode = 0

if (-not $SkipRun) {
    if (-not $NoBuild) {
        Write-Host "[soak] 编译测试二进制 ..."
        # PS5 会把 cargo 的 stderr 进度输出当成 ErrorRecord；在 Stop 策略下会直接终止脚本，
        # 因此 cargo 调用期间临时切到 Continue，用 $LASTEXITCODE 判断真实结果。
        $prevEap = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        & cargo test --test soak --no-run 2>&1 | Tee-Object -FilePath (Join-Path $OutDir "build.log")
        $buildCode = $LASTEXITCODE
        $ErrorActionPreference = $prevEap
        if ($buildCode -ne 0) {
            throw "编译失败，见 build.log"
        }
    }

    $pidFile = Join-Path $OutDir "pid.txt"
    Remove-Item $pidFile -ErrorAction SilentlyContinue

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = "cargo"
    # PowerShell 5.1 基于 .NET Framework，没有 ProcessStartInfo.ArgumentList，只能用 Arguments 字符串
    $psi.Arguments = "test --test soak -- --ignored --exact soak_run --nocapture --test-threads=1"
    $psi.WorkingDirectory = $coreDir
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.CreateNoWindow = $true

    $runLog = Join-Path $OutDir "run.log"
    $proc = [System.Diagnostics.Process]::Start($psi)
    # 异步读取，避免管道写满导致子进程阻塞
    $stdoutTask = $proc.StandardOutput.ReadToEndAsync()
    $stderrTask = $proc.StandardError.ReadToEndAsync()

    # ---- 进程级采样：等 pid.txt 出现后，按采样间隔抓 RSS / 句柄 ----
    $procFile = Join-Path $OutDir "proc.jsonl"
    Remove-Item $procFile -ErrorAction SilentlyContinue
    $targetPid = 0
    $deadline = (Get-Date).AddMinutes(2)
    while ($targetPid -eq 0 -and -not $proc.HasExited -and (Get-Date) -lt $deadline) {
        if (Test-Path $pidFile) {
            $raw = (Get-Content $pidFile -Raw).Trim()
            if ($raw -match '^\d+$') { $targetPid = [int]$raw }
        }
        if ($targetPid -eq 0) { Start-Sleep -Milliseconds 200 }
    }
    if ($targetPid -eq 0) {
        Write-Warning "[soak] 未能读取 pid.txt，进程级指标将缺失"
    } else {
        Write-Host "[soak] 被测进程 PID = $targetPid"
    }

    $sampleInterval = [Math]::Max(1, [int]([Math]::Ceiling($SampleMs / 1000.0)))
    while (-not $proc.HasExited) {
        if ($targetPid -ne 0) {
            try {
                $p = Get-Process -Id $targetPid -ErrorAction Stop
                $rec = [ordered]@{
                    ts_ms     = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
                    pid       = $p.Id
                    rss_bytes = [int64]$p.WorkingSet64
                    handles   = [int64]$p.HandleCount
                }
                # 不用 Add-Content -Encoding UTF8：PS5 会给新文件写 BOM，导致 Rust 侧 JSONL 首行解析失败
                $line = ($rec | ConvertTo-Json -Compress) + "`n"
                $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
                [System.IO.File]::AppendAllText($procFile, $line, $utf8NoBom)
            } catch {
                # 进程在两次采样之间退出属正常
            }
        }
        Start-Sleep -Seconds $sampleInterval
    }

    $stdout = $stdoutTask.GetAwaiter().GetResult()
    $stderr = $stderrTask.GetAwaiter().GetResult()
    ($stdout + "`n" + $stderr) | Out-File -FilePath $runLog -Encoding UTF8
    $proc.WaitForExit()

    Write-Host "[soak] 运行阶段退出码 = $($proc.ExitCode)"
    if ($proc.ExitCode -ne 0) {
        Write-Warning "[soak] 运行阶段失败，仍尝试判定以保留现场信息（见 run.log）"
        $exitCode = $proc.ExitCode
    }
}

# ---- 判定阶段 ----
Write-Host "[soak] 开始判定 ..."
$verdictLog = Join-Path $OutDir "verdict.log"
$prevEap = $ErrorActionPreference
$ErrorActionPreference = "Continue"
& cargo test --test soak -- --ignored --exact soak_verdict --nocapture --test-threads=1 2>&1 |
    Tee-Object -FilePath $verdictLog
$verdictCode = $LASTEXITCODE
$ErrorActionPreference = $prevEap

$report = Join-Path $OutDir "report.md"
if (Test-Path $report) {
    Write-Host ""
    Write-Host "[soak] 报告已生成: $report"
}

if ($verdictCode -ne 0) {
    Write-Warning "[soak] 判定未通过（见 $verdictLog）"
    if ($exitCode -eq 0) { $exitCode = $verdictCode }
}

exit $exitCode
