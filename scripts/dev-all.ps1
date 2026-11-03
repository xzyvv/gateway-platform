# 一键联调：同时启动 Rust 网关 + Vite dev server
# 用法：pwsh ./scripts/dev-all.ps1
#
# 设计要点：
# 1. Start-Process -PassThru 拿真实 PID；Stop-Job 不会传播到子进程，旧版 cargo/node 残留
# 2. finally 用 Win32_Process 走进程树，连同 rustc/node/vite 一起 Stop-Process -Force
# 3. 前置检查 cargo / node / npm
# 4. 配色日志 + stdout/stderr 分离到 dev-*.log，方便 Get-Content -Wait 跟踪

param(
    [switch]$Desktop
)

$ErrorActionPreference = 'Stop'

# B10: 注册 PowerShell 退出事件，确保 Ctrl+C / 异常退出也走 finally 清理子进程。
# 原 try/finally 在 Ctrl+C 时可能被 PowerShell 直接终止而不执行；
# Register-ObjectEvent 挂到 PowerShell 引擎的 Exiting 事件，作为兜底清理。
$cleanupScript = {
    Stop-ProcessTree $backendProc.Id  "后端"
    Stop-ProcessTree $frontendProc.Id "前端"
    if ($desktopProc) {
        Stop-ProcessTree $desktopProc.Id "桌面端"
    }
}
$null = Register-EngineEvent PowerShell.Exiting -Action $cleanupScript -SupportEvent

# === 路径定位 ===
# $PSScriptRoot = scripts/ 目录本身；上一层才是仓库根
$root = Split-Path -Parent $PSScriptRoot
$backend = Join-Path $root "gateway-core"
$frontend = Join-Path $root "gateway-web"

if (-not (Test-Path $backend)) {
    Write-Error "gateway-core 不存在: $backend"
    exit 1
}
if (-not (Test-Path $frontend)) {
    Write-Error "gateway-web 不存在: $frontend"
    exit 1
}

# 模板唯一源在仓库根 templates/，而后端 cwd 必须留在 gateway-core 才能找到 ./config.json
# （默认模板路径是 cwd 相对的 ./templates/*）。这里显式指向仓库根，避免
# "template not found: modbus-sim-meter" 导致启动失败；已手动设置的变量优先。
if (-not $env:GATEWAY_TEMPLATE_BUILTIN) {
    $env:GATEWAY_TEMPLATE_BUILTIN = Join-Path $root "templates\builtin"
}
if (-not $env:GATEWAY_TEMPLATE_CUSTOM) {
    $env:GATEWAY_TEMPLATE_CUSTOM = Join-Path $root "templates\custom"
}

# === 前置依赖检查 ===
$missing = @()
foreach ($cmd in @('cargo', 'node', 'npm')) {
    if (-not (Get-Command $cmd -ErrorAction SilentlyContinue)) {
        $missing += $cmd
    }
}
if ($missing.Count -gt 0) {
    Write-Error ("缺少依赖: " + ($missing -join ', ') + "（请先安装 Rust 工具链和 Node.js）")
    exit 1
}

# === 配色辅助 ===
function Write-Step($color, $text) {
    Write-Host $text -ForegroundColor $color
}

# === 启动进程（拿 PID，不用 Job）===
$backendExe = (Get-Command cargo).Source
$frontendExe = (Get-Command npm).Source

Write-Step Green "==> 启动 Rust 网关 (cargo run -p gatewayd)..."
# 二进制由 gatewayd crate 提供；WorkingDirectory 保持 gateway-core，
# 使默认 ./config.json 仍解析到 gateway-core/config.json（与历史 dev 约定一致）。
$backendProc = Start-Process -FilePath $backendExe `
    -ArgumentList 'run','-p','gatewayd' `
    -WorkingDirectory $backend `
    -RedirectStandardOutput "$backend\dev-stdout.log" `
    -RedirectStandardError  "$backend\dev-stderr.log" `
    -NoNewWindow -PassThru

Write-Step Green "==> 启动 Vite dev server (npm run dev)..."
$frontendProc = Start-Process -FilePath $frontendExe `
    -ArgumentList 'run','dev' `
    -WorkingDirectory $frontend `
    -RedirectStandardOutput "$frontend\dev-stdout.log" `
    -RedirectStandardError  "$frontend\dev-stderr.log" `
    -NoNewWindow -PassThru

# S15: -Desktop 开关——条件性启动 Tauri 桌面端
$desktopProc = $null
if ($Desktop) {
    $desktopDir = Join-Path $root "gateway-desktop"
    if (Test-Path $desktopDir) {
        $tauriExe = (Get-Command npx -ErrorAction SilentlyContinue).Source
        if ($tauriExe) {
            Write-Step Green "==> 启动 Tauri 桌面端 (npx tauri dev)..."
            $desktopProc = Start-Process -FilePath $tauriExe `
                -ArgumentList 'tauri','dev' `
                -WorkingDirectory $desktopDir `
                -RedirectStandardOutput "$desktopDir\dev-stdout.log" `
                -RedirectStandardError  "$desktopDir\dev-stderr.log" `
                -NoNewWindow -PassThru
        } else {
            Write-Warning "npx 未找到，跳过桌面端启动"
        }
    } else {
        Write-Warning "gateway-desktop 不存在: $desktopDir"
    }
}

Write-Host ""
Write-Step Cyan "进程已启动（PID 追踪，方便强制清理）："
Write-Host "  后端 PID: $($backendProc.Id)    cargo run -p gatewayd    ($backend\dev-*.log)"
Write-Host "  前端 PID: $($frontendProc.Id)    npm run dev  ($frontend\dev-*.log)"
if ($desktopProc) {
    Write-Host "  桌面端 PID: $($desktopProc.Id)    npx tauri dev  ($desktopDir\dev-*.log)"
}
Write-Host ""
Write-Step Cyan "常用命令："
Write-Host "  Get-Content $backend\dev-stdout.log -Wait -Tail 30"
Write-Host "  Get-Content $frontend\dev-stdout.log -Wait -Tail 30"
Write-Host "  按 Ctrl+C 停止并清理两个进程树"
Write-Host ""
Write-Step Cyan "Web UI: http://localhost:5173"
Write-Host "API:    http://localhost:8080/api/..."

# === 进程树清理函数 ===
function Stop-ProcessTree($rootPid, [string]$label) {
    if ($null -eq $rootPid) { return }

    try {
        $allPids = New-Object System.Collections.Generic.HashSet[int]
        $queue = New-Object System.Collections.Queue
        $queue.Enqueue([int]$rootPid)

        while ($queue.Count -gt 0) {
            $current = $queue.Dequeue()
            $null = $allPids.Add($current)
            $children = Get-CimInstance Win32_Process -Filter "ParentProcessId=$current" `
                -ErrorAction SilentlyContinue
            foreach ($c in $children) {
                if (-not $allPids.Contains([int]$c.ProcessId)) {
                    $queue.Enqueue([int]$c.ProcessId)
                }
            }
        }

        foreach ($pid in $allPids) {
            Stop-Process -Id $pid -Force -ErrorAction SilentlyContinue
        }
        Write-Step Yellow "==> 已清理 $label 进程树 ($($allPids.Count) 个进程)"
    } catch {
        Write-Warning "清理 $label 进程树时出错: $_"
    }
}

# === 主循环 + Ctrl+C 优雅退出 ===
try {
    while ($true) {
        Start-Sleep -Milliseconds 500

        if ($backendProc.HasExited -or $frontendProc.HasExited) {
            Write-Host ""
            Write-Step Yellow "==> 检测到子进程退出，停止联调"
            break
        }
    }
} finally {
    Write-Host ""
    Write-Step Yellow "==> 停止进程树..."
    Stop-ProcessTree $backendProc.Id  "后端"
    Stop-ProcessTree $frontendProc.Id "前端"
    if ($desktopProc) {
        Stop-ProcessTree $desktopProc.Id "桌面端"
    }
}
