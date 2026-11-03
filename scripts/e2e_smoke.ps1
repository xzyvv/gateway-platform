# 端到端冒烟（真实进程）：拉起 gatewayd.exe -> 轮询探活 -> 登录 -> 主链路只读校验 -> 清理
#
# 与 `cargo test -p gateway-api --test e2e_journey` 的分工：
#   集成测试  -> 进程内（oneshot 路由 + 假总线），校验 API 契约细节
#   本脚本    -> 真实二进制 + 真实配置装载 + 真实 TCP 监听，校验「交付现场能起来」
#
# 用法：
#   .\scripts\e2e_smoke.ps1                       # 默认端口 18080
#   .\scripts\e2e_smoke.ps1 -Port 18081           # 端口被占时换一个
#   .\scripts\e2e_smoke.ps1 -Build                # 先 cargo build -p gatewayd
#   .\scripts\e2e_smoke.ps1 -KeepTemp             # 失败排查时保留临时目录与日志
#
# 隔离约定：工作目录设为临时目录，清空继承的 GATEWAY_* 路径覆盖，
# GATEWAY_CONFIG_PATH 指向临时配置；队列库/日志/证书等默认路径随之落在临时目录，
# 不读写仓库根目录的 config.json / data_queue.db / logs。

[CmdletBinding()]
param(
    [int]$Port = 18080,
    [string]$ExePath = "",
    [switch]$Build,
    [switch]$KeepTemp,
    [int]$StartupTimeoutSec = 60
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$baseUrl = "http://127.0.0.1:$Port"
if ([string]::IsNullOrWhiteSpace($ExePath)) {
    $ExePath = Join-Path $repoRoot "target\debug\gatewayd.exe"
}

# ===== 环境变量隔离（子进程继承父进程环境，PS5 无 -Environment）=====
$pathVars = @(
    "GATEWAY_CONFIG_PATH", "GATEWAY_DB_PATH", "GATEWAY_OUTBOX_DB", "GATEWAY_VERSION_DB",
    "GATEWAY_LICENSE_CLOCK_DB", "GATEWAY_WATCHDOG_STATE", "GATEWAY_DIAG_LOG", "GATEWAY_LOG_DIR",
    "GATEWAY_AUDIT_LOG", "GATEWAY_CERT_DIR", "GATEWAY_OTA_STAGING",
    "GATEWAY_TEMPLATE_BUILTIN", "GATEWAY_TEMPLATE_CUSTOM"
)
$savedEnv = @{}
foreach ($name in $pathVars) {
    if (Test-Path "Env:$name") { $savedEnv[$name] = [Environment]::GetEnvironmentVariable($name); Remove-Item "Env:$name" }
}

$workDir = Join-Path $env:TEMP ("gateway-e2e-smoke-" + (Get-Date -Format "yyyyMMdd-HHmmss"))
New-Item -ItemType Directory -Force -Path $workDir | Out-Null
$configPath = Join-Path $workDir "config.json"
$outLog = Join-Path $workDir "gatewayd.out.log"
$errLog = Join-Path $workDir "gatewayd.err.log"
$proc = $null
$failed = $false

function Invoke-Api {
    param([string]$Method, [string]$Path, [string]$Token, [string]$Body)
    $headers = @{}
    if (-not [string]::IsNullOrWhiteSpace($Token)) { $headers["Authorization"] = "Bearer $Token" }
    $params = @{
        Method = $Method; Uri = "$baseUrl$Path"; Headers = $headers;
        UseBasicParsing = $true; TimeoutSec = 10
    }
    if (-not [string]::IsNullOrWhiteSpace($Body)) {
        $params["Body"] = $Body
        $params["ContentType"] = "application/json; charset=utf-8"
    }
    try {
        $resp = Invoke-WebRequest @params
    }
    catch {
        throw "HTTP $Method $Path 请求失败：$($_.Exception.Message)"
    }
    return $resp.Content | ConvertFrom-Json
}

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw "断言失败：$Message" }
}

try {
    if ($Build) {
        Write-Host "[smoke] cargo build -p gatewayd ..."
        $prevEap = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        & cargo build -p gatewayd 2>&1 | Select-Object -Last 5
        $buildCode = $LASTEXITCODE
        $ErrorActionPreference = $prevEap
        if ($buildCode -ne 0) { throw "构建失败（cargo build -p gatewayd exit=$buildCode）" }
    }
    if (-not (Test-Path $ExePath)) { throw "找不到 $ExePath，请先 cargo build -p gatewayd 或加 -Build" }

    # ---- 端口预检：避免把「端口被占」误判成「网关启动失败」----
    try {
        $probe = New-Object System.Net.Sockets.TcpListener -ArgumentList ([System.Net.IPAddress]::Loopback), $Port
        $probe.Start(); $probe.Stop()
    }
    catch {
        throw "端口 $Port 已被占用，换一个端口（-Port）后重试"
    }

    # ---- 生成隔离配置：以仓库根 config.json 为基线，清空现场相关项 ----
    $sourceConfig = Join-Path $repoRoot "config.json"
    if (-not (Test-Path $sourceConfig)) { throw "缺少配置基线 $sourceConfig" }
    $cfg = Get-Content -Path $sourceConfig -Raw -Encoding UTF8 | ConvertFrom-Json
    $cfg.buses = @()
    $cfg.points = @()
    $cfg.devices = $null
    $cfg.mqtt.enabled = $false
    $cfg.sim_consumer.enabled = $false
    $cfg.ntp = [pscustomobject]@{ server = "127.0.0.1"; sync_interval = 3600 }
    $cfg.webhook = $null
    $cfg.cellular = $null
    $cfg.opcua = $null
    $cfg.modbus_server = $null
    $cfg.modbus_passthrough = $null
    $cfg.telemetry = $null
    $cfg.license = $null
    $cfg.self_heal = $null
    $token = -join (1..32 | ForEach-Object { "{0:x}" -f (Get-Random -Maximum 16) })
    $cfg.web = [pscustomobject]@{
        api_token = $token
        username = "admin"
        password_hash = ""
        listen_addr = "127.0.0.1:$Port"
        allow_anon_read = $false
        allow_default_token = $true
        realtime_buffer = 1024
        cors_origins = @()
        demo_replay_sessions = $false
    }
    $json = $cfg | ConvertTo-Json -Depth 20
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($configPath, $json, $utf8NoBom)
    $env:GATEWAY_CONFIG_PATH = $configPath

    Write-Host "[smoke] 启动 gatewayd : $ExePath"
    Write-Host "[smoke] 工作目录      : $workDir"
    Write-Host "[smoke] 监听地址      : $baseUrl"
    $proc = Start-Process -FilePath $ExePath -WorkingDirectory $workDir -PassThru `
        -RedirectStandardOutput $outLog -RedirectStandardError $errLog

    # ---- 1. 探活轮询（/health/live 无信封、无鉴权）----
    $deadline = (Get-Date).AddSeconds($StartupTimeoutSec)
    $live = $false
    while ((Get-Date) -lt $deadline) {
        if ($proc.HasExited) { throw "gatewayd 启动后立即退出（ExitCode=$($proc.ExitCode)），见 $errLog" }
        try {
            $r = Invoke-WebRequest -Uri "$baseUrl/health/live" -UseBasicParsing -TimeoutSec 2
            if ($r.StatusCode -eq 200) { $live = $true; break }
        }
        catch { }
        Start-Sleep -Milliseconds 500
    }
    Assert-True $live "在 ${StartupTimeoutSec}s 内 /health/live 未就绪"
    Write-Host "[ok] 1/8 进程探活通过"

    # ---- 2. 登录（无信封，直接返回 {token}）----
    $login = Invoke-Api "POST" "/api/auth/login" "" '{"username":"admin","password":"admin"}'
    Assert-True (-not [string]::IsNullOrWhiteSpace($login.token)) "登录响应缺少 token"
    Assert-True ($login.token.Length -ge 16) "登录 token 长度不足 16"
    $auth = $login.token
    Write-Host "[ok] 2/8 登录通过"

    # ---- 3. 运行状态（信封 {code,data}）----
    $status = Invoke-Api "GET" "/api/status" $auth
    Assert-True ($status.code -eq 0) "/api/status code=$($status.code)"
    Assert-True ($null -ne $status.data) "/api/status 返回空 data"
    Write-Host "[ok] 3/8 运行状态通过"

    # ---- 4. 配置读取 + 5. 配置校验（交付现场最常用的两个入口）----
    $configResp = Invoke-Api "GET" "/api/config" $auth
    Assert-True ($configResp.code -eq 0) "/api/config code=$($configResp.code)"
    $validateBody = @{ config = $configResp.data; templates = @() } | ConvertTo-Json -Depth 30
    $validated = Invoke-Api "POST" "/api/config/validate" $auth $validateBody
    Assert-True ($validated.code -eq 0) "/api/config/validate code=$($validated.code)"
    Assert-True ($null -ne $validated.data.valid) "/api/config/validate 缺少 valid"
    Assert-True (@($validated.data.issues).Count -ge 0) "/api/config/validate 缺少 issues 数组"
    Write-Host "[ok] 4-5/8 配置读取/校验通过"

    # ---- 6. R1 回归：生产默认不得注入预置演示会话 ----
    $replay = Invoke-Api "GET" "/api/protocol-tools/packet-replay/sessions" $auth
    Assert-True ($replay.code -eq 0) "回放会话列表 code=$($replay.code)"
    Assert-True (@($replay.data).Count -eq 0) "默认配置下回放会话应为空，实际 $(@($replay.data).Count) 条（web.demo_replay_sessions 未关闭？）"
    Write-Host "[ok] 6/8 回放会话未被演示数据污染"

    # ---- 7. 协议注册表 + 诊断 + 健康报告 ----
    $protocols = Invoke-Api "GET" "/api/protocols" $auth
    Assert-True ($protocols.code -eq 0) "/api/protocols code=$($protocols.code)"
    Assert-True (@($protocols.data).Count -ge 1) "/api/protocols 未返回任何协议"
    $diag = Invoke-Api "GET" "/api/diagnostics/gateway" $auth
    Assert-True ($diag.code -eq 0) "/api/diagnostics/gateway code=$($diag.code)"
    $health = Invoke-Api "GET" "/api/health/report" $auth
    Assert-True ($health.code -eq 0) "/api/health/report code=$($health.code)"
    Assert-True ($null -ne $health.data) "/api/health/report 返回空 data"
    Write-Host "[ok] 7-8/8 协议/诊断/健康报告通过"

    Write-Host "[smoke] PASS 真实进程端到端冒烟通过"
}
catch {
    $failed = $true
    Write-Host "[smoke] FAIL $($_.Exception.Message)" -ForegroundColor Red
    foreach ($log in @($errLog, $outLog)) {
        if (Test-Path $log) {
            Write-Host "[smoke] --- $([System.IO.Path]::GetFileName($log)) (末 20 行) ---" -ForegroundColor DarkYellow
            Get-Content -Path $log -Tail 20
        }
    }
}
finally {
    if ($null -ne $proc -and -not $proc.HasExited) {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
        $null = $proc.WaitForExit(5000)
    }
    # 还原被隔离的环境变量
    Remove-Item "Env:GATEWAY_CONFIG_PATH" -ErrorAction SilentlyContinue
    foreach ($name in $savedEnv.Keys) { [Environment]::SetEnvironmentVariable($name, $savedEnv[$name]) }
    if ($KeepTemp -or $failed) {
        Write-Host "[smoke] 临时目录保留：$workDir"
    }
    else {
        Remove-Item -Path $workDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

if ($failed) { exit 1 }
exit 0
