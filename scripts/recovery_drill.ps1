# 升级 / 强杀恢复演练（真实二进制 + 真实 SQLite 文件）
#
# 覆盖两件「部署前必须验证、单测无法证明」的事：
#   1. 升级演练：用最老版本 schema 的 data_queue.db 直接启动新二进制，
#      验证 8 个新增列全部迁移、历史行不丢、API 正常；
#   2. 强杀恢复：进程被 TerminateProcess 强制杀死（无优雅停机、无 WAL
#      checkpoint）后重启，验证已 commit 的行不丢、无重复、无幽灵 in_flight。
#
# 与 e2e_smoke.ps1 的分工：那个验证「能起来」，本脚本验证「升级/断电后还能起来
# 且数据对得上」。
#
# 用法：
#   .\scripts\recovery_drill.ps1                 # 默认端口 18090
#   .\scripts\recovery_drill.ps1 -Build          # 先 cargo build -p gatewayd
#   .\scripts\recovery_drill.ps1 -KeepArtifacts  # 保留临时目录便于排查

[CmdletBinding()]
param(
    [int]$Port = 18090,
    [string]$ExePath = "",
    [switch]$Build,
    [switch]$KeepArtifacts,
    [int]$StartupTimeoutSec = 60
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$baseUrl = "http://127.0.0.1:$Port"
if ([string]::IsNullOrWhiteSpace($ExePath)) {
    $ExePath = Join-Path $repoRoot "target\debug\gatewayd.exe"
}

# ===== 环境隔离：清空继承的 GATEWAY_* 路径覆盖，全部落到临时目录 =====
$pathVars = @(
    "GATEWAY_CONFIG_PATH", "GATEWAY_DB_PATH", "GATEWAY_OUTBOX_DB", "GATEWAY_VERSION_DB",
    "GATEWAY_LICENSE_CLOCK_DB", "GATEWAY_WATCHDOG_STATE", "GATEWAY_DIAG_LOG", "GATEWAY_LOG_DIR",
    "GATEWAY_AUDIT_LOG", "GATEWAY_CERT_DIR", "GATEWAY_OTA_STAGING",
    "GATEWAY_TEMPLATE_BUILTIN", "GATEWAY_TEMPLATE_CUSTOM", "GATEWAY_REQUIRE_EXTERNAL_CERT"
)
$savedEnv = @{}
foreach ($name in $pathVars) {
    if (Test-Path "Env:$name") { $savedEnv[$name] = [Environment]::GetEnvironmentVariable($name); Remove-Item "Env:$name" }
}

$workDir = Join-Path $env:TEMP ("gateway-recovery-drill-" + (Get-Date -Format "yyyyMMdd-HHmmss"))
New-Item -ItemType Directory -Force -Path $workDir | Out-Null
$configPath = Join-Path $workDir "config.json"
$dbPath = Join-Path $workDir "data_queue.db"
$helperPath = Join-Path $workDir "drill_db.py"
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
    return (Invoke-WebRequest @params).Content | ConvertFrom-Json
}

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw "断言失败：$Message" }
}

# 通过 Python 直连 DB（默认无 SQLCipher key，即普通 SQLite 文件），
# 避免为「造老库 / 读校验」在 Rust 侧再开测试接口。
function Invoke-DbHelper {
    param([string]$Command)
    $raw = & python $helperPath $Command $dbPath 2>&1
    if ($LASTEXITCODE -ne 0) { throw "python drill_db.py $Command 失败：$raw" }
    return ($raw | Out-String).Trim()
}

function Start-Gateway {
    $script:proc = Start-Process -FilePath $ExePath -WorkingDirectory $workDir -PassThru `
        -RedirectStandardOutput $outLog -RedirectStandardError $errLog
    $deadline = (Get-Date).AddSeconds($StartupTimeoutSec)
    while ((Get-Date) -lt $deadline) {
        if ($script:proc.HasExited) {
            throw "gatewayd 启动后立即退出（ExitCode=$($script:proc.ExitCode)），见 $errLog"
        }
        try {
            $r = Invoke-WebRequest -Uri "$baseUrl/health/live" -UseBasicParsing -TimeoutSec 2
            if ($r.StatusCode -eq 200) { return }
        }
        catch { }
        Start-Sleep -Milliseconds 500
    }
    throw "在 ${StartupTimeoutSec}s 内 /health/live 未就绪"
}

function Stop-GatewayHard {
    if ($null -ne $script:proc -and -not $script:proc.HasExited) {
        # TerminateProcess：模拟断电/被强杀，不走 graceful_shutdown
        Stop-Process -Id $script:proc.Id -Force -ErrorAction SilentlyContinue
        $null = $script:proc.WaitForExit(5000)
    }
    $script:proc = $null
}

try {
    if ($Build) {
        Write-Host "[drill] cargo build -p gatewayd ..."
        $prevEap = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        & cargo build -p gatewayd 2>&1 | Select-Object -Last 5
        $buildCode = $LASTEXITCODE
        $ErrorActionPreference = $prevEap
        if ($buildCode -ne 0) { throw "构建失败（cargo build -p gatewayd exit=$buildCode）" }
    }
    if (-not (Test-Path $ExePath)) { throw "找不到 $ExePath，请先 cargo build -p gatewayd 或加 -Build" }

    try {
        $probe = New-Object System.Net.Sockets.TcpListener -ArgumentList ([System.Net.IPAddress]::Loopback), $Port
        $probe.Start(); $probe.Stop()
    }
    catch { throw "端口 $Port 已被占用，换一个端口（-Port）后重试" }

    # ---- 生成隔离配置（基线来自仓库 config.json）----
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
    $env:GATEWAY_DB_PATH = $dbPath

    # ---- Python 助手：造最老 schema / 造 in_flight 残留 / 插入一行 / 出报告 ----
    $helperSource = @'
import json, sqlite3, sys, time

cmd, db = sys.argv[1], sys.argv[2]
conn = sqlite3.connect(db)
NOW = int(time.time() * 1000)

ANCIENT = """CREATE TABLE data_queue (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    point_id TEXT NOT NULL,
    value REAL NOT NULL,
    raw_value REAL NOT NULL,
    raw_type TEXT NOT NULL,
    quality TEXT NOT NULL,
    timestamp INTEGER NOT NULL
)"""

FULL_INSERT = """INSERT INTO data_queue
    (point_id, device_id, value, raw_value, raw_type, quality, quality_reason,
     timestamp, source_ts, stale_count, in_flight, dequeued_at, delivered)
    VALUES (?1, 'dev1', ?2, ?2, 'u16', 'good', '', ?3, 0, 0, ?4, 0, 0)"""

if cmd == "seed-ancient":
    conn.executescript(ANCIENT)
    for i in range(3):
        # 时间戳用「刚刚」：启动时会按 license 存储配额（7 天）清理历史，
        # 用 2023 年的固定时间会被判为过期数据而清空，导致演练结论失真。
        conn.execute(
            "INSERT INTO data_queue (point_id, value, raw_value, raw_type, quality, timestamp)"
            " VALUES ('p-old', ?, ?, 'u16', 'good', ?)",
            (float(i), float(i), NOW - 3000 + i),
        )
elif cmd == "seed-inflight":
    # 模拟「上一轮运行中被强杀」：5 行 delivered=0 且 in_flight=1
    for i in range(5):
        conn.execute(FULL_INSERT, ("p-flight", float(i), NOW + 1000 + i, 1))
elif cmd == "insert-one":
    # 在网关运行期间从另一连接写入并 commit：WAL 中已 fsync 的可恢复行
    conn.execute(FULL_INSERT, ("p-wal", 9.0, NOW + 2000, 0))
elif cmd == "report":
    cols = [row[1] for row in conn.execute("PRAGMA table_info(data_queue)")]
    total = conn.execute("SELECT COUNT(*) FROM data_queue").fetchone()[0]
    distinct = conn.execute("SELECT COUNT(DISTINCT id) FROM data_queue").fetchone()[0]
    inflight = conn.execute(
        "SELECT COUNT(*) FROM data_queue WHERE in_flight = 1").fetchone()[0]
    points = [row[0] for row in conn.execute(
        "SELECT DISTINCT point_id FROM data_queue ORDER BY point_id")]
    print(json.dumps({
        "columns": cols, "total": total, "distinct_ids": distinct,
        "in_flight": inflight, "points": points,
    }))
else:
    raise SystemExit("unknown command: " + cmd)

conn.commit()
conn.close()
'@
    [System.IO.File]::WriteAllText($helperPath, $helperSource, $utf8NoBom)

    # ================= 阶段 1：升级演练 =================
    Write-Host "[drill] 阶段 1/2 升级演练：最老 schema 库 + 新二进制"
    Invoke-DbHelper "seed-ancient" | Out-Null

    Start-Gateway
    $login = Invoke-Api "POST" "/api/auth/login" "" '{"username":"admin","password":"admin"}'
    Assert-True (-not [string]::IsNullOrWhiteSpace($login.token)) "登录响应缺少 token"
    $auth = $login.token
    $status = Invoke-Api "GET" "/api/status" $auth
    Assert-True ($status.code -eq 0) "/api/status code=$($status.code)"
    Stop-GatewayHard

    $r1 = Invoke-DbHelper "report" | ConvertFrom-Json
    foreach ($col in @("device_id", "quality_reason", "source_ts", "stale_count",
            "in_flight", "dequeued_at", "delivered", "value_text")) {
        Assert-True (@($r1.columns) -contains $col) "升级后缺少列 $col"
    }
    Assert-True ($r1.total -eq 3) "升级后历史行应为 3，实际 $($r1.total)"
    Assert-True ($r1.distinct_ids -eq $r1.total) "升级后出现重复 id"
    Write-Host "[ok] 1/2 旧库升级：8 列补齐、3 行保留、API 正常"

    # ================= 阶段 2：强杀 / WAL 恢复 =================
    Write-Host "[drill] 阶段 2/2 强杀恢复：in_flight 残留 + 运行中提交 + TerminateProcess"
    Invoke-DbHelper "seed-inflight" | Out-Null

    Start-Gateway
    $r2a = Invoke-DbHelper "report" | ConvertFrom-Json
    Assert-True ($r2a.in_flight -eq 0) "启动应把 delivered=0 的残留 in_flight 复位，实际 $($r2a.in_flight)"

    # 运行中写入一行并 commit，随后立刻强杀：该行只存在于 WAL，靠 synchronous=FULL
    # 的 fsync 保证重启后仍可恢复
    Invoke-DbHelper "insert-one" | Out-Null
    Stop-GatewayHard

    Start-Gateway
    $status2 = Invoke-Api "GET" "/api/status" $auth
    Assert-True ($status2.code -eq 0) "重启后 /api/status code=$($status2.code)"
    Stop-GatewayHard

    $r2 = Invoke-DbHelper "report" | ConvertFrom-Json
    $expected = 3 + 5 + 1
    Assert-True ($r2.total -eq $expected) "强杀重启后行数应为 $expected，实际 $($r2.total)（丢数或重复）"
    Assert-True ($r2.distinct_ids -eq $r2.total) "强杀重启后出现重复 id：total=$($r2.total) distinct=$($r2.distinct_ids)"
    Assert-True ($r2.in_flight -eq 0) "重启后不应残留 in_flight=1，实际 $($r2.in_flight)"
    Assert-True (@($r2.points) -contains "p-wal") "运行中提交的行（WAL）在强杀后丢失"
    Write-Host "[ok] 2/2 强杀恢复：$expected 行不丢不重、无幽灵 in_flight、WAL 行可恢复"

    Write-Host "[drill] PASS 升级与强杀恢复演练通过"
}
catch {
    $failed = $true
    Write-Host "[drill] FAIL $($_.Exception.Message)" -ForegroundColor Red
    foreach ($log in @($errLog, $outLog)) {
        if (Test-Path $log) {
            Write-Host "[drill] --- $([System.IO.Path]::GetFileName($log)) (末 15 行) ---" -ForegroundColor DarkYellow
            Get-Content -Path $log -Tail 15
        }
    }
}
finally {
    Stop-GatewayHard
    Remove-Item "Env:GATEWAY_CONFIG_PATH" -ErrorAction SilentlyContinue
    Remove-Item "Env:GATEWAY_DB_PATH" -ErrorAction SilentlyContinue
    foreach ($name in $savedEnv.Keys) { [Environment]::SetEnvironmentVariable($name, $savedEnv[$name]) }
    if ($KeepArtifacts -or $failed) {
        Write-Host "[drill] 临时目录保留：$workDir"
    }
    else {
        Remove-Item -Path $workDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

if ($failed) { exit 1 }
exit 0
