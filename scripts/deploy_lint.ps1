<#
.SYNOPSIS
    部署脚本静态检查 + 与代码默认值的一致性断言。

.DESCRIPTION
    CI 的 deploy-lint 用 actionlint + shellcheck（本机没有）。
    本脚本做两件事，都在本机 Windows PowerShell 上跑：

    1. **语法检查**：用 Git for Windows 自带的 bash 对所有 deploy/*.sh
       跑 `bash -n`（只解析不执行）。

    2. **一致性断言**（真正有价值的部分）：部署脚本里写死的路径 /
       协议清单 / API 端点，必须和**代码里的默认值**一致。
       这类漂移正是 S3 检出的真实缺陷形态（systemd DeviceAllow 白名单
       漏掉代码会探测的看门狗候选路径 → 设备上被 EPERM 静默拒绝）。
       所以这里把「代码改了就必须同步改脚本」变成机器可验证的规则。

    检查项：
      C1  bash -n 语法检查（deploy/*.sh + gateway-core/deploy/install.sh）
      C2  gateway.service 的 DeviceAllow 白名单覆盖代码里全部看门狗候选路径
      C3  gateway.service 的 DeviceAllow 覆盖代码里 OTA 默认分区设备
      C4  gateway.service / install.sh / verify-deploy.sh 的配置路径一致
          （= 代码里 default_config_path 的 Linux 值）
      C5  build-release.sh 的协议白名单与 Cargo.toml 的协议 feature 一致
      C6  verify-deploy.sh 探测的 API 端点在 gateway-api 路由里真实存在
      C7  verify-deploy.sh 里的固定路径（GW_BIN / ui-dist / systemd 名）
          与 gateway.service / install.sh 一致

.PARAMETER Strict
    任何检查失败都返回 1；否则只打印警告并返回 0。

.PARAMETER Probe
    只探测 bash 是否可用（退出码 0 = 就绪，3 = 不可用）。

.EXAMPLE
    powershell -File scripts/deploy_lint.ps1
#>
param(
    [switch]$Strict,
    [switch]$Probe
)

$ErrorActionPreference = "Continue"
$repoRoot = Split-Path -Parent $PSScriptRoot

$failures = New-Object System.Collections.Generic.List[string]
$warnings = New-Object System.Collections.Generic.List[string]
$passes = 0

function Ok([string]$msg) {
    $script:passes++
    Write-Host "  [OK] $msg" -ForegroundColor Green
}
function Fail([string]$msg) {
    $script:failures.Add($msg)
    Write-Host "  [FAIL] $msg" -ForegroundColor Red
}
function Warn([string]$msg) {
    $script:warnings.Add($msg)
    Write-Host "  [WARN] $msg" -ForegroundColor Yellow
}

# ── 定位 bash ──
# Git for Windows 自带 bash（MSYS2）。不依赖 WSL / Linux 机器。
function Find-Bash {
    $candidates = @()
    $gitCmd = Get-Command git -ErrorAction SilentlyContinue
    if ($gitCmd) {
        # git.exe 通常在 <install>\cmd\git.exe，bash 在 <install>\bin\bash.exe
        $gitRoot = Split-Path -Parent (Split-Path -Parent $gitCmd.Source)
        $candidates += (Join-Path $gitRoot "bin\bash.exe")
        $candidates += (Join-Path $gitRoot "usr\bin\bash.exe")
    }
    $candidates += @(
        "C:\Program Files\Git\bin\bash.exe",
        "C:\Program Files (x86)\Git\bin\bash.exe",
        "$env:LOCALAPPDATA\Programs\Git\bin\bash.exe"
    )
    foreach ($c in $candidates) {
        if ($c -and (Test-Path $c)) { return $c }
    }
    return $null
}

$bash = Find-Bash
if ($null -eq $bash) {
    Write-Host "[deploy-lint] 未找到 bash（需要 Git for Windows 自带的 Git Bash）—— 跳过本脚本。" -ForegroundColor Yellow
    Write-Host "[deploy-lint] 安装：https://git-scm.com/download/win （或 winget install Git.Git）" -ForegroundColor Yellow
    if ($Probe) { exit 3 }
    if ($Strict) { exit 1 }
    exit 0
}
if ($Probe) {
    Write-Host "[deploy-lint] bash 可用: $bash"
    exit 0
}

Write-Host "[deploy-lint] bash: $bash"
Write-Host ""

# 收集要检查的脚本
$shFiles = @()
foreach ($dir in @("deploy", "gateway-core\deploy")) {
    $d = Join-Path $repoRoot $dir
    if (Test-Path $d) {
        $shFiles += Get-ChildItem $d -Filter "*.sh" -File -ErrorAction SilentlyContinue
    }
}
# 仓库根及其它位置的 .sh
$shFiles += Get-ChildItem $repoRoot -Filter "*.sh" -File -ErrorAction SilentlyContinue
$shFiles = $shFiles | Sort-Object FullName -Unique

# ══════════════════════════════════════════════════════════
# C1  bash -n 语法检查
# ══════════════════════════════════════════════════════════
Write-Host "[C1] bash -n 语法检查" -ForegroundColor Cyan
if ($shFiles.Count -eq 0) {
    Fail "没找到任何 .sh 脚本（deploy/*.sh 不存在？）"
}
foreach ($f in $shFiles) {
    $rel = $f.FullName.Substring($repoRoot.Length + 1).Replace('\', '/')
    $out = & $bash -n $f.FullName 2>&1
    if ($LASTEXITCODE -eq 0) {
        Ok "$rel 语法通过"
    }
    else {
        Fail "$rel 语法错误: $($out -join ' ')"
    }
}
Write-Host ""

# 读文件内容的小工具。
# 必须显式 -Encoding UTF8：Windows PowerShell 5.1 的 Get-Content 默认按 ANSI
# （GBK）解码，仓库里的中文注释是多字节 UTF-8 → 解码后字节数错位，
# 会把换行吃掉，导致「行首匹配」类的断言莫名其妙地失败
# （曾表现为 DeviceAllow=/dev/watchdog 那行匹配不上，而 /dev/watchdog0 能匹配）。
function ReadText([string]$relPath) {
    $p = Join-Path $repoRoot $relPath
    if (-not (Test-Path $p)) { return $null }
    return (Get-Content $p -Raw -Encoding UTF8 -ErrorAction SilentlyContinue)
}

# ══════════════════════════════════════════════════════════
# C2  DeviceAllow 覆盖代码里的看门狗候选路径
# ══════════════════════════════════════════════════════════
# systemd 的 DeviceAllow 是**白名单**语义：一旦出现任何 DeviceAllow=，
# 未列出的设备一律拒绝。代码按候选顺序探测，systemd 少放行一个，
# 设备上就在 open() 之前被 EPERM 拒绝 → 看门狗静默失效。
Write-Host "[C2] gateway.service DeviceAllow 覆盖代码看门狗候选路径" -ForegroundColor Cyan
$serviceText = ReadText "gateway-core\deploy\gateway.service"
$collectionText = ReadText "gatewayd\src\bootstrap\helpers\collection.rs"
if ($null -eq $serviceText) {
    Fail "找不到 gateway-core/deploy/gateway.service"
}
elseif ($null -eq $collectionText) {
    Warn "找不到 gatewayd/src/bootstrap/helpers/collection.rs（跳过看门狗候选路径断言）"
}
else {
    $m = [regex]::Match($collectionText, '(?s)WATCHDOG_DEVICE_CANDIDATES[^=]*=\s*&\[(.*?)\]')
    if (-not $m.Success) {
        Warn "无法从 collection.rs 解析 WATCHDOG_DEVICE_CANDIDATES（跳过）"
    }
    else {
        $cands = [regex]::Matches($m.Groups[1].Value, '"([^"]+)"') | ForEach-Object { $_.Groups[1].Value }
        foreach ($c in $cands) {
            if ($serviceText -match "(?m)^\s*DeviceAllow=$([regex]::Escape($c))\s") {
                Ok "DeviceAllow 已放行 $c"
            }
            else {
                Fail "DeviceAllow 未放行看门狗候选路径 $c（代码会探测它，但 systemd 会先 EPERM 拒绝 → 看门狗静默失效）"
            }
        }
    }
}
Write-Host ""

# ══════════════════════════════════════════════════════════
# C3  DeviceAllow 覆盖代码里的 OTA 默认分区设备
# ══════════════════════════════════════════════════════════
Write-Host "[C3] gateway.service DeviceAllow 覆盖 OTA 默认分区设备" -ForegroundColor Cyan
$otaText = ReadText "gateway-core\src\platform\ota\mod.rs"
if ($null -eq $serviceText) {
    # 已在 C2 报过
}
elseif ($null -eq $otaText) {
    Warn "找不到 gateway-core/src/platform/ota/mod.rs（跳过 OTA 分区断言）"
}
else {
    $m = [regex]::Match($otaText, '(?s)impl Default for AbPartitionConfig.*?\{(.*?)\n\s*\}')
    if (-not $m.Success) {
        Warn "无法从 ota/mod.rs 解析 AbPartitionConfig::default（跳过）"
    }
    else {
        $devs = [regex]::Matches($m.Groups[1].Value, '"(/dev/[^"]+)"') | ForEach-Object { $_.Groups[1].Value }
        if ($devs.Count -eq 0) {
            Warn "未从 AbPartitionConfig::default 解析到分区设备（跳过）"
        }
        foreach ($d in $devs) {
            if ($serviceText -match "(?m)^\s*DeviceAllow=$([regex]::Escape($d))\s") {
                Ok "DeviceAllow 已放行 $d"
            }
            else {
                Fail "DeviceAllow 未放行 OTA 分区设备 $d（OTA 现场直接失败：systemd 在 open() 前 EPERM）"
            }
        }
    }
}
Write-Host ""

# ══════════════════════════════════════════════════════════
# C4  配置路径一致（代码默认值 vs 脚本里的硬编码）
# ══════════════════════════════════════════════════════════
Write-Host "[C4] 部署脚本里的配置路径 = 代码 default_config_path" -ForegroundColor Cyan
$pathsText = ReadText "gateway-core\src\support\paths.rs"
if ($null -eq $pathsText) {
    Warn "找不到 gateway-core/src/support/paths.rs（跳过配置路径断言）"
}
else {
    $m = [regex]::Match($pathsText, '(?s)default_config_path,\s*"[^"]*",\s*"([^"]+)"')
    if (-not $m.Success) {
        Warn "无法从 paths.rs 解析 default_config_path 的 Linux 值（跳过）"
    }
    else {
        $cfgPath = $m.Groups[1].Value
        $cfgDir = $cfgPath.Substring(0, $cfgPath.LastIndexOf('/'))
        Ok "代码 default_config_path = $cfgPath"
        foreach ($f in $shFiles) {
            $rel = $f.FullName.Substring($repoRoot.Length + 1).Replace('\', '/')
            $txt = Get-Content $f.FullName -Raw -Encoding UTF8 -ErrorAction SilentlyContinue
            # 脚本里出现的所有 /etc/gateway/... 路径
            $used = [regex]::Matches($txt, '/etc/gateway/[A-Za-z0-9_\-\./]*') | ForEach-Object { $_.Value } | Sort-Object -Unique
            foreach ($u in $used) {
                if ($u -eq $cfgPath) {
                    Ok "$rel 使用 $u（与代码一致）"
                }
                elseif ($u.StartsWith("$cfgDir/")) {
                    # 同目录下的其它运行时文件（模板目录 / 证书等）：只要在配置目录下就合法
                    Ok "$rel 使用 $u（位于配置目录 $cfgDir 下）"
                }
                else {
                    Fail "$rel 使用配置路径 $u，但代码 default_config_path 是 $cfgPath —— 网关读不到它"
                }
            }
        }
    }
}
Write-Host ""

# ══════════════════════════════════════════════════════════
# C5  build-release.sh 协议白名单 vs Cargo.toml 协议 feature
# ══════════════════════════════════════════════════════════
# build-release.sh 维护了一份手写的 ALL_PROTOCOLS 白名单，
# 与 gateway-core/Cargo.toml 的 [features] 是两份来源 —— 加协议时漏改一处
# 会让「能编译但脚本拒绝」或「脚本放行但 cargo 不认」。
Write-Host "[C5] build-release.sh 协议白名单 = Cargo.toml 协议 feature" -ForegroundColor Cyan
$buildSh = ReadText "deploy\build-release.sh"
$cargoToml = ReadText "gateway-core\Cargo.toml"
if ($null -eq $buildSh) {
    Fail "找不到 deploy/build-release.sh"
}
elseif ($null -eq $cargoToml) {
    Warn "找不到 gateway-core/Cargo.toml（跳过协议白名单断言）"
}
else {
    $m = [regex]::Match($buildSh, '(?m)^ALL_PROTOCOLS="([^"]+)"')
    if (-not $m.Success) {
        Warn "未在 build-release.sh 中找到 ALL_PROTOCOLS 定义（跳过）"
    }
    else {
        $scriptProtos = $m.Groups[1].Value -split '\s+' | Where-Object { $_ } | Sort-Object -Unique
        # Cargo.toml [features] 段里的 feature 名
        $featStart = $cargoToml.IndexOf("[features]")
        $featEnd = if ($featStart -ge 0) { $cargoToml.IndexOf("[dev-dependencies]", $featStart) } else { -1 }
        if ($featStart -lt 0 -or $featEnd -lt 0) {
            Warn "无法定位 Cargo.toml 的 [features] 段（跳过）"
        }
        else {
            $featBlock = $cargoToml.Substring($featStart, $featEnd - $featStart)
            $cargoFeats = @{}
            foreach ($fm in [regex]::Matches($featBlock, '(?m)^([A-Za-z0-9_\-]+)\s*=\s*\[(.*?)\]')) {
                $name = $fm.Groups[1].Value
                $deps = [regex]::Matches($fm.Groups[2].Value, '"([^"]+)"') |
                    ForEach-Object { $_.Groups[1].Value }
                $cargoFeats[$name] = $deps
            }
            # 展开某个 feature 能带出的全部子 feature（忽略 dep: 与 dep/feature 形式）
            function Expand-Feature([string]$name) {
                $out = New-Object System.Collections.Generic.HashSet[string]
                $stack = New-Object System.Collections.Generic.Stack[string]
                $stack.Push($name)
                while ($stack.Count -gt 0) {
                    $cur = $stack.Pop()
                    if (-not $cargoFeats.ContainsKey($cur)) { continue }
                    foreach ($d in $cargoFeats[$cur]) {
                        if ($d -like "dep:*") { continue }
                        if ($d -match "/") { continue }
                        if ($out.Add($d)) { $stack.Push($d) }
                    }
                }
                return $out
            }

            # 「协议 feature」的权威来源有两处，缺一不可：
            #  1) south/protocol/mod.rs 里 gate 住的协议模块（新增协议模块必然在这里出现）
            #  2) all-protocols / sku-* 的成员（发行集口径）
            # 不能用「所有非聚合 feature」——那会把 default / tracing /
            # remote-shell / test-support 也算进来。
            $protoSet = New-Object System.Collections.Generic.HashSet[string]
            $protoModText = ReadText "gateway-core\src\south\protocol\mod.rs"
            if ($null -ne $protoModText) {
                foreach ($pm in [regex]::Matches($protoModText, '#\[cfg\(feature\s*=\s*"([^"]+)"\)\]')) {
                    [void]$protoSet.Add($pm.Groups[1].Value)
                }
            }
            foreach ($root in (@("all-protocols") + @($cargoFeats.Keys | Where-Object { $_ -like "sku-*" }))) {
                if (-not $cargoFeats.ContainsKey($root)) { continue }
                foreach ($d in $cargoFeats[$root]) {
                    if ($d -like "dep:*") { continue }
                    if ($d -match "/") { continue }
                    [void]$protoSet.Add($d)
                }
            }
            $cargoProtos = $protoSet | Sort-Object -Unique

            # 脚本白名单必须覆盖所有协议 feature：
            # 要么直接列出，要么由白名单里某个条目展开而来
            # （opcua → opcua-client / opcua-server）。
            $covered = New-Object System.Collections.Generic.HashSet[string]
            foreach ($w in $scriptProtos) {
                [void]$covered.Add($w)
                foreach ($e in (Expand-Feature $w)) { [void]$covered.Add($e) }
            }
            $missingInScript = $cargoProtos | Where-Object { -not $covered.Contains($_) }
            # 反向：白名单里的每个条目都必须是真实存在的 cargo feature
            $missingInCargo = $scriptProtos | Where-Object { -not $cargoFeats.ContainsKey($_) }

            if ($missingInScript.Count -gt 0) {
                Fail "代码里存在协议 feature 但 build-release.sh 白名单没覆盖: $($missingInScript -join ', ')（--protocols 会误报「未知协议」）"
            }
            else {
                Ok "所有协议 feature 都被 build-release.sh 白名单覆盖"
            }
            if ($missingInCargo.Count -gt 0) {
                Fail "build-release.sh 白名单有 Cargo.toml 里不存在的 feature: $($missingInCargo -join ', ')（cargo 会直接编译失败）"
            }
            else {
                Ok "脚本白名单里的 feature 在 Cargo.toml 中都存在"
            }
        }
    }
}
Write-Host ""

# ══════════════════════════════════════════════════════════
# C6  verify-deploy.sh 探测的 API 端点真实存在
# ══════════════════════════════════════════════════════════
# verify-deploy.sh 里写死了一串 API 路径。网关删/改端点后，脚本会
# 「验证失败」但看不出是脚本过期了还是网关坏了 —— 现场浪费大量排查时间。
Write-Host "[C6] verify-deploy.sh 的 API 端点在 gateway-api 路由里存在" -ForegroundColor Cyan
$verifySh = ReadText "deploy\verify-deploy.sh"
if ($null -eq $verifySh) {
    Fail "找不到 deploy/verify-deploy.sh"
}
else {
    # 只取脚本里形如 $API/xxx 的端点（排除变量插值路径与诊断命令）
    $eps = [regex]::Matches($verifySh, '\$API(/[A-Za-z0-9_\-\./\{\}]*)') |
        ForEach-Object { $_.Groups[1].Value } |
        Where-Object { $_ -notmatch '\{' } |
        Sort-Object -Unique
    if ($eps.Count -eq 0) {
        Warn "未从 verify-deploy.sh 解析出 API 端点（跳过）"
    }
    else {
        # 在 gateway-api 源码里找路由定义
        $apiSrc = Join-Path $repoRoot "gateway-api\src"
        $apiFiles = Get-ChildItem $apiSrc -Filter "*.rs" -Recurse -File -ErrorAction SilentlyContinue
        $apiText = ($apiFiles | ForEach-Object { Get-Content $_.FullName -Raw -Encoding UTF8 -ErrorAction SilentlyContinue }) -join "`n"
        foreach ($e in $eps) {
            # axum 路由写成 .route("/api/xxx", ...)，路径参数写成 :id
            $needle = $e.TrimStart('/')
            $axumPat = '/' + ($needle -replace ':', ':')
            if ($apiText -match [regex]::Escape($axumPat)) {
                Ok "端点 $e 在 gateway-api 路由中存在"
            }
            else {
                Fail "端点 $e 在 gateway-api 源码里找不到对应路由（verify-deploy.sh 会恒 FAIL —— 脚本过期或网关改了 API）"
            }
        }
    }
}
Write-Host ""

# ══════════════════════════════════════════════════════════
# C7  verify-deploy.sh / install.sh 的固定路径与服务名一致
# ══════════════════════════════════════════════════════════
Write-Host "[C7] 部署脚本的固定路径与服务名互相一致" -ForegroundColor Cyan
if ($null -eq $serviceText) {
    Fail "找不到 gateway-core/deploy/gateway.service"
}
else {
    # systemd 服务名（systemctl is-active gateway / journalctl -u gateway）
    $svcName = "gateway"
    $execStart = [regex]::Match($serviceText, '(?m)^ExecStart=(\S+)').Groups[1].Value
    if ($execStart) {
        Ok "gateway.service ExecStart = $execStart"
        $binDir = $execStart.Substring(0, $execStart.LastIndexOf('/'))
        $binName = $execStart.Substring($execStart.LastIndexOf('/') + 1)
        # verify-deploy.sh 的 GW_BIN 默认值
        $v = ""
        if ($null -ne $verifySh) { $v = $verifySh }
        if ($v -match 'GW_BIN="\$\{GW_BIN:-([^}]+)\}"') {
            $gwBin = $Matches[1]
            if ($gwBin -eq $execStart) {
                Ok "verify-deploy.sh 的 GW_BIN 默认值与 ExecStart 一致（$gwBin）"
            }
            else {
                Fail "verify-deploy.sh 默认查 $gwBin，但 systemd 跑的是 $execStart —— 二进制兼容性检查会跳过"
            }
        }
        else {
            Warn "未从 verify-deploy.sh 解析到 GW_BIN 默认值（跳过）"
        }
        # ui-dist 目录（与 ExecStart 同级）。注意用正斜杠：
        # Split-Path / Join-Path 在 Windows 上会产出反斜杠，和脚本里的 Linux 路径对不上。
        $uiDist = ($binDir.TrimEnd('/') + "/ui-dist")
        if ($v -and $v.Contains($uiDist)) {
            Ok "verify-deploy.sh 检查的 ui-dist 路径与 ExecStart 同级（$uiDist）"
        }
        elseif ($v) {
            Fail "verify-deploy.sh 未检查 $uiDist（与 ExecStart 同级的 ui-dist）—— 前端缺失时不会被发现"
        }
    }
    else {
        Fail "无法从 gateway.service 解析 ExecStart"
    }

    # 服务名一致性：install.sh / verify-deploy.sh 里出现的 systemctl/journalctl 服务名。
    # 只认「命令末尾的服务名参数」：systemctl <verb> [选项] <服务名>
    # 选项以 - 开头必须跳过（`systemctl is-active --quiet gateway` 的 --quiet 不是服务名）。
    foreach ($f in $shFiles) {
        $rel = $f.FullName.Substring($repoRoot.Length + 1).Replace('\', '/')
        $txt = Get-Content $f.FullName -Raw -Encoding UTF8 -ErrorAction SilentlyContinue
        $names = @()
        foreach ($sm in [regex]::Matches($txt, 'systemctl\s+(?:is-active|status|restart|enable|show)([^\n;|&)]*)')) {
            $toks = @($sm.Groups[1].Value -split '\s+' | Where-Object { $_ })
            # 从后往前找第一个既不是选项、也不是选项参数的 token。
            # 服务名只允许 [A-Za-z0-9_.-]，带引号/中文标点的（如 echo 里的
            # `gateway。"`）不是服务名参数。
            for ($i = $toks.Count - 1; $i -ge 0; $i--) {
                $tok = $toks[$i]
                if ($tok.StartsWith('-')) { continue }
                if ($tok -match '^[A-Za-z0-9_\-\.]+=') { continue }   # --value=xxx
                if ($tok -notmatch '^[A-Za-z0-9_\-\.]+$') { continue } # 含 / & " 中文 等 → 不是服务名
                if ($tok -match '^\d') { continue }                   # 数字（重定向 2>、PID 等）
                $names += $tok
                break
            }
        }
        foreach ($jm in [regex]::Matches($txt, 'journalctl\s+-u\s+([A-Za-z0-9_\-\.]+)')) {
            $names += $jm.Groups[1].Value
        }
        $names = $names | Sort-Object -Unique
        foreach ($n in $names) {
            if ($serviceText -match "(?m)^SyslogIdentifier=$([regex]::Escape($n))\s") {
                Ok "$rel 引用的服务名 $n 与 gateway.service 一致"
            }
            else {
                Fail "$rel 引用 systemd 服务名 $n，但 gateway.service 的 SyslogIdentifier 不是它 —— 验证脚本查的是另一个服务"
            }
        }
    }
}
Write-Host ""

# ══════════════════════════════════════════════════════════
# 汇总
# ══════════════════════════════════════════════════════════
Write-Host "[deploy-lint] 通过 $passes 项，失败 $($failures.Count) 项，警告 $($warnings.Count) 项"
if ($failures.Count -gt 0) {
    Write-Host ""
    foreach ($f in $failures) { Write-Host "  [FAIL] $f" -ForegroundColor Red }
}
if ($warnings.Count -gt 0) {
    Write-Host ""
    foreach ($w in $warnings) { Write-Host "  [WARN] $w" -ForegroundColor Yellow }
}

if ($failures.Count -gt 0) {
    exit 1
}
exit 0
