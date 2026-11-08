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
      C8  gateway.service / install.sh 里的 Environment=GATEWAY_* 变量名
          必须在 Rust 源码里真实存在（写错名字 = 静默不生效）
      C9  代码里真正会 open 的 /dev/* 路径都在 DeviceAllow 白名单内
          （精确路径形式，或 char-<组名> 设备组形式）
      C10 gateway.service 的 ReadWritePaths 覆盖配置目录
          （= default_config_path 所在目录；ProtectSystem=strict 下唯一的写通道）
      C11 install.sh 对配置目录的组权限 与 gateway.service 的写权限一致
          （组可写 ⇔ ReadWritePaths 含该目录；且组名 = service 的 Group）
      C12 DeviceAllow 的路径形式不得写成「非真实节点」的裸设备名
          （/dev/ttyUSB 这类在 systemd v255 会被 stat 失败后整条丢弃）
      C13 install.sh 的 OTA 分区改写幂等，且每个 SLOT 变量恰好一条赋值
      C14 串口稳定名链路完整：生成器按稳定标识匹配并保留已有绑定、
          install.sh 接入、build-release.sh 打包、verify-deploy.sh 核对 port_name

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
# Windows：Git for Windows 自带 bash（MSYS2），不依赖 WSL。
# Linux  ：系统自带 /usr/bin/bash（CI runner 上直接可用）。
function Find-Bash {
    $isWindows = ($env:OS -eq "Windows_NT")
    $candidates = @()
    $gitCmd = Get-Command git -ErrorAction SilentlyContinue
    if ($gitCmd) {
        # git.exe 通常在 <install>\cmd\git.exe，bash 在 <install>\bin\bash.exe
        $gitRoot = Split-Path -Parent (Split-Path -Parent $gitCmd.Source)
        $candidates += (Join-Path $gitRoot "bin/bash.exe")
        $candidates += (Join-Path $gitRoot "usr/bin/bash.exe")
    }
    $candidates += @(
        "C:\Program Files\Git\bin\bash.exe",
        "C:\Program Files (x86)\Git\bin\bash.exe",
        "$env:LOCALAPPDATA\Programs\Git\bin\bash.exe"
    )
    if (-not $isWindows) {
        # Linux / macOS：系统 bash（CI runner 上是 /usr/bin/bash）
        $candidates += @("/usr/bin/bash", "/bin/bash", "/usr/local/bin/bash")
    }
    foreach ($c in $candidates) {
        if ($c -and (Test-Path $c)) { return $c }
    }
    if (-not $isWindows) {
        # PATH 兜底（Linux 上 bash 可能装在别处）
        $onPath = Get-Command bash -ErrorAction SilentlyContinue
        if ($onPath) { return $onPath.Source }
    }
    return $null
}

# ── 仓库内路径拼接 ──
# 统一用正斜杠：Linux 上 Join-Path 不会把反斜杠当分隔符，
# Windows 上正斜杠同样被 API 接受（Get-Content / Test-Path 都行）。
function Join-Repo([string]$relPath) {
    return (Join-Path $repoRoot ($relPath -replace '\\', '/'))
}

$bash = Find-Bash
if ($null -eq $bash) {
    # 只跳过 C1（bash -n），C2~C12 是纯文本断言，不需要 bash —— 必须继续跑，
    # 否则「没装 Git」就会让整套一致性断言（含 CI 门禁）静默失效。
    Write-Host "[deploy-lint] 未找到 bash —— 跳过 C1 语法检查，其余断言照常执行。" -ForegroundColor Yellow
    if ($Strict) { Fail "缺少 bash，无法执行 C1 的 bash -n 语法检查" }
}
if ($Probe) {
    if ($null -eq $bash) {
        Write-Host "[deploy-lint] bash 不可用"
        exit 3
    }
    Write-Host "[deploy-lint] bash 可用: $bash"
    exit 0
}

if ($bash) { Write-Host "[deploy-lint] bash: $bash" }
Write-Host ""

# 收集要检查的脚本
$shFiles = @()
foreach ($dir in @("deploy", "gateway-core/deploy")) {
    $d = Join-Repo $dir
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
if ($null -eq $bash) {
    Warn "跳过 C1（未找到 bash）"
}
else {
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
}
Write-Host ""

# 读文件内容的小工具。
# 必须显式 -Encoding UTF8：Windows PowerShell 5.1 的 Get-Content 默认按 ANSI
# （GBK）解码，仓库里的中文注释是多字节 UTF-8 → 解码后字节数错位，
# 会把换行吃掉，导致「行首匹配」类的断言莫名其妙地失败
# （曾表现为 DeviceAllow=/dev/watchdog 那行匹配不上，而 /dev/watchdog0 能匹配）。
function ReadText([string]$relPath) {
    $p = Join-Repo $relPath
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
$serviceText = ReadText "gateway-core/deploy/gateway.service"
$collectionText = ReadText "gatewayd/src/bootstrap/helpers/collection.rs"
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
$otaText = ReadText "gateway-core/src/platform/ota/mod.rs"
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
$pathsText = ReadText "gateway-core/src/support/paths.rs"
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
$buildSh = ReadText "deploy/build-release.sh"
$cargoToml = ReadText "gateway-core/Cargo.toml"
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
            $protoModText = ReadText "gateway-core/src/south/protocol/mod.rs"
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
$verifySh = ReadText "deploy/verify-deploy.sh"
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
        $apiSrc = Join-Repo "gateway-api/src"
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
        foreach ($sm in [regex]::Matches($txt, 'systemctl\s+(?:is-active|is-enabled|status|restart|enable|show)([^\n;|&)]*)')) {
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
# C8 环境变量名一致性：服务里设的 GATEWAY_* 必须是代码真的读的名字
# ══════════════════════════════════════════════════════════
# 形态：gateway.service 里注释掉的示例曾写成 Environment=GATEWAY_CONFIG
# （代码只读 GATEWAY_CONFIG_PATH），运维照抄取消注释后完全不生效、也没有任何报错。
# 规则：只扫 `Environment=<NAME>=` 这类真正赋值的位置（含被注释的示例），
# NAME 以 GATEWAY_ 开头时，必须能在 Rust 源码里作为完整 token 找到。
Write-Host "[C8] 服务环境变量名在代码中存在"
$envNames = @()
foreach ($f in @(
        (Join-Repo "gateway-core/deploy/gateway.service"),
        (Join-Repo "gateway-core/deploy/install.sh")
    )) {
    if (-not (Test-Path $f)) { continue }
    $rel = $f.Substring($repoRoot.Length + 1).Replace('\', '/')
    $txt = Get-Content $f -Raw -Encoding UTF8 -ErrorAction SilentlyContinue
    foreach ($m in [regex]::Matches($txt, '(?m)^\s*#?\s*Environment=([A-Za-z_][A-Za-z0-9_]*)')) {
        $envNames += [pscustomobject]@{ Name = $m.Groups[1].Value; File = $rel }
    }
}
$envNames = @($envNames | Sort-Object Name, File -Unique)
$gwEnvNames = @($envNames | Where-Object { $_.Name -match '^GATEWAY_' })

if ($gwEnvNames.Count -eq 0) {
    Fail "未从 gateway.service / install.sh 提取到任何 GATEWAY_* 环境变量，C8 失效（正则或文件路径需要更新）"
}
else {
    $rsFiles = Get-ChildItem $repoRoot -Recurse -File -Filter *.rs -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -notmatch '[\\/](target|node_modules|\.git)[\\/]' }
    $rsText = ($rsFiles | ForEach-Object { Get-Content $_.FullName -Raw -Encoding UTF8 }) -join "`n"
    foreach ($e in $gwEnvNames) {
        # \b 配合下划线：GATEWAY_CONFIG 不会误匹配到 GATEWAY_CONFIG_PATH
        if ($rsText -match "\b$([regex]::Escape($e.Name))\b") {
            Ok "$($e.File) 的 Environment=$($e.Name) 在代码中存在"
        }
        else {
            Fail "$($e.File) 的 Environment=$($e.Name) 在代码里不存在 —— 该变量不生效（多半是名字写错）"
        }
    }
}
Write-Host ""

# ══════════════════════════════════════════════════════════
# C9  代码会 open 的设备路径 ⊆ DeviceAllow 白名单
# ══════════════════════════════════════════════════════════
# systemd 的 DeviceAllow 有两套语义（v255，src/core/bpf-devices.c）：
#   · 路径形式  /dev/xxx      → 应用期先 stat(node)，stat 失败**整条丢弃**
#                                （"Couldn't stat device %s: %m"）
#   · 设备组形式 char-<组名>  → 按 /proc/devices 的设备组名匹配，
#                                一次放行该 major 的全部次设备
# 因此「代码里 open 的路径」必须能被白名单覆盖，否则进程在 open() 之前
# 就被 EPERM 拒绝 → 该协议静默不上报数据。
#
# 扫描方式：遍历 Rust 源码，去掉注释与 #[cfg(test)] 之后的测试代码，
# 再取被引号包裹的完整 "/dev/xxx" 字面量（避免 format! 拼接串误判）。
function Remove-RustComments([string]$s) {
    $sb = New-Object System.Text.StringBuilder
    $i = 0; $n = $s.Length
    $inStr = $false; $inLine = $false; $inBlock = 0
    while ($i -lt $n) {
        $c = $s[$i]
        $c2 = if ($i + 1 -lt $n) { $s[$i + 1] } else { [char]0 }
        if ($inLine) {
            if ($c -eq "`n") { $inLine = $false; [void]$sb.Append($c) }
            $i++; continue
        }
        if ($inBlock -gt 0) {
            # 保留换行：调用方按行号对齐原文与去注释后的文本
            if ($c -eq "`n") { [void]$sb.Append($c) }
            if ($c -eq '/' -and $c2 -eq '*') { $inBlock++; $i += 2; continue }
            if ($c -eq '*' -and $c2 -eq '/') { $inBlock--; $i += 2; continue }
            $i++; continue
        }
        if ($inStr) {
            [void]$sb.Append($c)
            if ($c -eq '\' -and $i + 1 -lt $n) { [void]$sb.Append($s[$i + 1]); $i += 2; continue }
            if ($c -eq '"') { $inStr = $false }
            $i++; continue
        }
        if ($c -eq '/' -and $c2 -eq '/') { $inLine = $true; $i += 2; continue }
        if ($c -eq '/' -and $c2 -eq '*') { $inBlock = 1; $i += 2; continue }
        if ($c -eq '"') { $inStr = $true; [void]$sb.Append($c); $i++; continue }
        if ($c -eq "'") {
            # 字符字面量 'x' / '\n' 吃掉三个字符；'a 这样的生命周期标注保留引号本身
            if ($i + 2 -lt $n -and $s[$i + 2] -eq "'") {
                [void]$sb.Append($c); [void]$sb.Append($s[$i + 1]); [void]$sb.Append($s[$i + 2]); $i += 3; continue
            }
            [void]$sb.Append($c); $i++; continue
        }
        [void]$sb.Append($c); $i++
    }
    return $sb.ToString()
}

# 标记属于 #[cfg(test)] 条目的行号。
# 结果写入 $script:testLineSet（不用返回值：HashSet 经管道返回会被展开成 int 数组）。
function Get-TestLineSet([string[]]$lines) {
    $test = New-Object 'System.Collections.Generic.HashSet[int]'
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -notmatch '#\[cfg\(test\)\]') { continue }
        # 从该行往后找条目体（函数 / 模块 / 单行声明）
        $depth = 0; $started = $false; $j = $i
        while ($j -lt $lines.Count) {
            [void]$test.Add($j)
            $code = ($lines[$j] -replace '//.*$', '')
            $opens = ([regex]::Matches($code, '\{')).Count
            $closes = ([regex]::Matches($code, '\}')).Count
            if (-not $started) {
                if ($opens -gt 0) {
                    $started = $true; $depth += $opens - $closes
                    if ($depth -le 0) { break }
                    $j++; continue
                }
                if ($code -match ';') { break }   # 单行声明（use / const …）
                $j++; continue
            }
            $depth += $opens - $closes
            if ($depth -le 0) { break }
            $j++
        }
        $i = $j
    }
    $script:testLineSet = $test
}

Write-Host "[C9] 代码 open 的设备路径都在 DeviceAllow 白名单内" -ForegroundColor Cyan
# DeviceAllow 条目解析：路径形式与设备组形式分开收集
$allowPaths = @()
$allowGroups = @()
if ($null -ne $serviceText) {
    foreach ($m in [regex]::Matches($serviceText, '(?m)^\s*DeviceAllow=(\S+)(?:\s+(\S+))?\s*$')) {
        $val = $m.Groups[1].Value
        $perm = $m.Groups[2].Value
        if ($val -match '^(char|block)-(.+)$') {
            $allowGroups += [pscustomobject]@{ Kind = $Matches[1]; Name = $Matches[2]; Perm = $perm }
        }
        else {
            $allowPaths += [pscustomobject]@{ Path = $val; Perm = $perm }
        }
    }
}
$allowPaths = @($allowPaths | Sort-Object Path -Unique)
$allowGroups = @($allowGroups | Sort-Object Kind, Name -Unique)

if ($null -eq $serviceText) {
    Warn "找不到 gateway.service（已在 C2 报告），跳过 C9"
}
else {
    # 收集生产代码里的设备路径
    $codeDevices = @{}
    $rsFiles = Get-ChildItem $repoRoot -Recurse -File -Filter *.rs -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -notmatch '[\\/](target|node_modules|\.git)[\\/]' }
    foreach ($f in $rsFiles) {
        $rel = $f.FullName.Substring($repoRoot.Length + 1).Replace('\', '/')
        # 跳过测试文件（tests/ 目录、*_tests.rs）
        if ($rel -match '(^|/)tests/' -or $rel -match '(^|/)[^/]*tests\.rs$') { continue }
        $text = Get-Content $f.FullName -Raw -Encoding UTF8 -ErrorAction SilentlyContinue
        if ($null -eq $text -or $text -notmatch '/dev/') { continue }
        $lines = $text -split "`r?`n"
        Get-TestLineSet $lines
        $testLines = $script:testLineSet
        # 去掉注释（保留换行，行号与原文对齐）
        $cleanLines = (Remove-RustComments $text) -split "`r?`n"
        for ($i = 0; $i -lt $cleanLines.Count; $i++) {
            if ($testLines.Contains($i)) { continue }
            foreach ($m in [regex]::Matches($cleanLines[$i], '"(/dev/[A-Za-z0-9_.\-/]+)"')) {
                $p = $m.Groups[1].Value
                if (-not $codeDevices.ContainsKey($p)) {
                    $codeDevices[$p] = New-Object System.Collections.Generic.List[string]
                }
                $codeDevices[$p].Add("$rel`:$($i + 1)")
            }
        }
    }
    if ($codeDevices.Count -eq 0) {
        Fail "未从 Rust 源码中解析到任何设备路径，C9 失效（正则或扫描范围需要更新）"
    }
    foreach ($p in ($codeDevices.Keys | Sort-Object)) {
        $base = [System.IO.Path]::GetFileName($p)
        $covered = $false
        $how = ""
        # ① 精确路径形式
        foreach ($a in $allowPaths) {
            if ($a.Path -eq $p) {
                $covered = $true; $how = "路径形式 $p"
                if ($a.Perm -notmatch 'w') { Fail "DeviceAllow=$p 未授予写权限（代码需要读写该设备）" }
                break
            }
        }
        # ② 设备组形式：basename 去掉尾部编号 == 设备组名
        if (-not $covered -and $base -match '^(.*?)(\d+)$') {
            $grp = $Matches[1]
            foreach ($g in $allowGroups) {
                if ($g.Name -eq $grp) {
                    $covered = $true; $how = "$($g.Kind)-$grp 设备组"
                    if ($g.Perm -notmatch 'w') { Fail "DeviceAllow=$($g.Kind)-$grp 未授予写权限（代码需要读写该设备）" }
                    break
                }
            }
        }
        if ($covered) {
            Ok "$p 被 DeviceAllow 覆盖（$how）"
        }
        else {
            $src = (($codeDevices[$p] | Sort-Object -Unique) -join ', ')
            Fail "代码会 open 设备 $p（来源：$src），但 gateway.service 的 DeviceAllow 未覆盖它 —— systemd 会在 open() 之前 EPERM 拒绝，该设备静默失效"
        }
    }
}
Write-Host ""

# ══════════════════════════════════════════════════════════
# C10  ReadWritePaths 覆盖配置目录
# ══════════════════════════════════════════════════════════
# ProtectSystem=strict 会把 /etc 整体挂成只读，ReadWritePaths= 是唯一例外通道。
# 配置写回走 ConfigLoader::save() → atomic_write()（临时文件建在目标同目录
# 再 rename 覆盖）→ 缺该目录时所有在线改配置端点 500「运行时已改、磁盘未改」。
Write-Host "[C10] gateway.service 的 ReadWritePaths 覆盖配置目录" -ForegroundColor Cyan
$cfgDirForRwp = ""
if ($null -ne $pathsText) {
    $pm = [regex]::Match($pathsText, '(?s)default_config_path,\s*"[^"]*",\s*"([^"]+)"')
    if ($pm.Success) {
        $cfgPath = $pm.Groups[1].Value
        $cfgDirForRwp = $cfgPath.Substring(0, $cfgPath.LastIndexOf('/'))
    }
}
if (-not $cfgDirForRwp) {
    Warn "无法从 paths.rs 解析 default_config_path（跳过 C10）"
}
elseif ($null -eq $serviceText) {
    Warn "找不到 gateway.service（已在 C2 报告），跳过 C10"
}
else {
    $rwp = @()
    foreach ($m in [regex]::Matches($serviceText, '(?m)^\s*ReadWritePaths=(.*)$')) {
        $rwp += ($m.Groups[1].Value -split '\s+' | Where-Object { $_ })
    }
    if ($rwp.Count -eq 0) {
        Fail "gateway.service 没有 ReadWritePaths= —— ProtectSystem=strict 下配置目录不可写，在线改配置会失败"
    }
    elseif ($rwp -contains $cfgDirForRwp) {
        Ok "ReadWritePaths 包含配置目录 $cfgDirForRwp"
    }
    else {
        Fail "ReadWritePaths=($($rwp -join ' ')) 不包含配置目录 $cfgDirForRwp —— 配置写回（atomic_write 临时文件建在同目录）会被拒，在线改配置端点 500"
    }
}
Write-Host ""

# ══════════════════════════════════════════════════════════
# C11  install.sh 的配置目录权限 ⇔ gateway.service 的写权限
# ══════════════════════════════════════════════════════════
# ReadWritePaths= 只把路径 bind 成可写，目录内的文件仍受属主/权限约束 →
# unit 与 install.sh 必须同时改，否则「路径可写但文件组不可写」依旧写不进去。
Write-Host "[C11] install.sh 配置目录权限与 service 写权限一致" -ForegroundColor Cyan
$installText = ReadText "gateway-core/deploy/install.sh"
if ($null -eq $installText) {
    Warn "找不到 gateway-core/deploy/install.sh（跳过 C11）"
}
elseif ($null -eq $serviceText) {
    Warn "找不到 gateway.service（已在 C2 报告），跳过 C11"
}
else {
    # install.sh 里的配置目录变量
    $cm = [regex]::Match($installText, '(?m)^\s*CONFIG_DIR="([^"]+)"')
    $installCfgDir = if ($cm.Success) { $cm.Groups[1].Value } else { "" }
    if (-not $installCfgDir) {
        Warn "无法从 install.sh 解析 CONFIG_DIR（跳过 C11）"
    }
    else {
        # 该目录的属组（chown -R <user>:<group> "$CONFIG_DIR"）
        $ownGroup = ""
        foreach ($m in [regex]::Matches($installText, '(?m)chown\s+-R\s+\S+:(\S+)\s+"\$CONFIG_DIR"')) {
            $ownGroup = $m.Groups[1].Value
        }
        # 该目录的组可写（chmod g+rwx / g+w 作用在 $CONFIG_DIR 上）
        $groupWritable = $false
        foreach ($line in ($installText -split "`n")) {
            if ($line -match '\$CONFIG_DIR' -and $line -match 'chmod' -and $line -match 'g\+[rwx]*w') {
                $groupWritable = $true
            }
        }
        # service 侧的组与可写路径
        $svcGroup = ([regex]::Match($serviceText, '(?m)^Group=(\S+)')).Groups[1].Value
        $svcSupplementary = @()
        foreach ($m in [regex]::Matches($serviceText, '(?m)^SupplementaryGroups=(\S+)')) {
            $svcSupplementary += ($m.Groups[1].Value -split '\s+' | Where-Object { $_ })
        }
        $rwp2 = @()
        foreach ($m in [regex]::Matches($serviceText, '(?m)^\s*ReadWritePaths=(.*)$')) {
            $rwp2 += ($m.Groups[1].Value -split '\s+' | Where-Object { $_ })
        }
        $rwpHasCfg = $rwp2 -contains $installCfgDir

        Ok "install.sh 的配置目录 = $installCfgDir"
        if ($ownGroup) {
            if ($ownGroup -eq $svcGroup -or $svcSupplementary -contains $ownGroup) {
                Ok "配置目录属组 $ownGroup 与 service 的运行组一致（Group=$svcGroup / SupplementaryGroups=$($svcSupplementary -join ',')）"
            }
            else {
                Fail "install.sh 把配置目录设为 $ownGroup 组，但 service 以 Group=$svcGroup 运行 —— 服务进程没有该组权限，配置目录不可写"
            }
        }
        else {
            Warn "install.sh 里未找到对 \$CONFIG_DIR 的 chown（跳过属组一致性断言）"
        }
        # 组可写 ⇔ ReadWritePaths 含该目录
        if ($groupWritable -and -not $rwpHasCfg) {
            Fail "install.sh 给了配置目录 $installCfgDir 组可写，但 gateway.service 的 ReadWritePaths 不含它 —— systemd 层仍是只读，配置写回失败"
        }
        elseif ($rwpHasCfg -and -not $groupWritable) {
            Fail "gateway.service 的 ReadWritePaths 含 $installCfgDir，但 install.sh 没给该目录组可写 —— 目录内文件仍不可写，配置写回失败"
        }
        elseif ($groupWritable) {
            Ok "配置目录的组可写权限与 ReadWritePaths 一致（$installCfgDir）"
        }
        else {
            Warn "install.sh 未给配置目录组可写，且 ReadWritePaths 也不含它（若本就不需要在线改配置可忽略）"
        }
    }
}
Write-Host ""

# ══════════════════════════════════════════════════════════
# C12  DeviceAllow 里不得有「路径形式但非真实节点」的裸设备名
# ══════════════════════════════════════════════════════════
# /dev/ttyUSB 这类写法在 systemd v255 里语法合法，但应用期 stat 失败后
# 整条被丢弃（"Couldn't stat device %s: %m"）→ 白名单形同虚设。
# 判定：路径形式的 basename 不带编号 → 它要么是真实存在的单一节点
# （/dev/watchdog、/dev/null …），要么是被误当成路径的设备组名。
Write-Host "[C12] DeviceAllow 无「裸设备名」伪路径" -ForegroundColor Cyan
$realSingleNodes = @(
    "watchdog", "softdog", "null", "zero", "full", "random", "urandom",
    "tty", "ptmx", "console", "kmsg", "stdin", "stdout", "stderr", "mem", "port"
)
if ($null -eq $serviceText) {
    Warn "找不到 gateway.service（已在 C2 报告），跳过 C12"
}
else {
    $bad = @()
    foreach ($a in $allowPaths) {
        if ($a.Path -notmatch '^/dev/') { continue }
        $base = [System.IO.Path]::GetFileName($a.Path)
        if ($base -match '\d') { continue }                     # 带编号 = 具体节点（gpiochip0 / watchdog0）
        if ($realSingleNodes -contains $base) { continue }     # 真实存在的单一节点
        $bad += $a.Path
    }
    if ($bad.Count -eq 0) {
        Ok "DeviceAllow 的路径形式均为真实节点（无裸设备名）"
    }
    else {
        foreach ($b in $bad) {
            $bn = [System.IO.Path]::GetFileName($b)
            Fail "DeviceAllow=$b 是「路径形式但非真实节点」—— systemd v255 解析时 stat 失败会**整条丢弃**该条目（Couldn't stat device），串口/设备在 open() 前被 EPERM 拒绝。应改用设备组形式（如 DeviceAllow=char-$bn rw）"
        }
    }
}
Write-Host ""

# ══════════════════════════════════════════════════════════
# C13  install.sh 的 OTA 改写：幂等 + 每个变量恰好一条赋值
# ══════════════════════════════════════════════════════════
# install.sh 用 awk 把 OTA 分区同时改写进 DeviceAllow 与 Environment。
# 这段 awk 的输入是 unit 原文、输出是 unit 新文，两条路径（注释示例就地替换 /
# RUST_LOG 之后补写）若同时命中就会把同一变量写两遍 —— systemd 取最后一行，
# 表面能跑，实际生效值与现场介质可能对不上；重复执行 install.sh 也会叠加。
# 这里把 install.sh 里那段 awk 抽出来实跑，验证：
#   1) 幂等：对已改写的 unit 再改写一次，结果不变
#   2) 每个 SLOT 变量恰好一条赋值，且 DeviceAllow 与之一致
Write-Host "[C13] install.sh 的 OTA 改写幂等且赋值唯一" -ForegroundColor Cyan
if ($null -eq $installText) {
    Warn "找不到 gateway-core/deploy/install.sh（已在 C11 报告），跳过 C13"
}
elseif ($null -eq $serviceText) {
    Warn "找不到 gateway.service（已在 C2 报告），跳过 C13"
}
elseif ($null -eq $bash) {
    Warn "未找到 bash，无法实跑 install.sh 的改写块（跳过 C13）"
}
else {
    # 从 install.sh 里抽出改写块：从 HAVE_SLOT_ENV 探测语句到 awk 结束行
    $blockMatch = [regex]::Match(
        $installText,
        '(?s)(if printf ''%s\\n'' "\$SERVICE_CONTENT" \| grep -q?E.*?\n''\)"\n)')
    if (-not $blockMatch.Success) {
        Fail "无法从 install.sh 解析 OTA 改写块（脚本结构已变）—— C13 失效，请同步更新正则"
    }
    else {
        $block = $blockMatch.Groups[1].Value
        # 改写块通过 base64 传给 bash：避开 Windows/Linux 的路径与引号差异，
        # 也避开「中文注释 + 引号」在 shell 里的转义地狱。
        $blockB64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($block))
        $unitB64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($serviceText))
        # runner 脚本：解码 unit 与改写块 → 模拟 install.sh 的改写 → 输出结果
        $runnerSh = @'
set -u
d="$(mktemp -d)"
trap 'rm -rf "$d"' EXIT
# base64 解码：GNU coreutils 用 -d，macOS/BSD 用 -D，两个都试
b64dec() { base64 -d 2>/dev/null || base64 -D 2>/dev/null || base64 --decode; }
printf '%s' "$GW_UNIT_B64" | b64dec > "$d/unit.service"
printf '%s' "$GW_BLOCK_B64" | b64dec > "$d/block.sh"
run_rewrite() {
  SERVICE_CONTENT="$(cat "$d/unit.service")"
  OTA_SLOT_A="$1"
  OTA_SLOT_B="$2"
  # shellcheck disable=SC1090
  . "$d/block.sh"
  printf '%s\n' "$SERVICE_CONTENT"
}
run_rewrite "$1" "$2" > "$d/out1.service"
cp "$d/out1.service" "$d/unit.service"
run_rewrite "$1" "$2" > "$d/out2.service"
echo "=====OUT1====="
cat "$d/out1.service"
echo "=====OUT2====="
cat "$d/out2.service"
'@
        $runner = Join-Path ([System.IO.Path]::GetTempPath()) ("deploy_lint_c13_" + [guid]::NewGuid().ToString("N") + ".sh")
        try {
            # 用 UTF-8 无 BOM 写：bash 不认 BOM（会被当成命令的一部分）
            [System.IO.File]::WriteAllText($runner, $runnerSh, (New-Object System.Text.UTF8Encoding($false)))
            $env:GW_UNIT_B64 = $unitB64
            $env:GW_BLOCK_B64 = $blockB64
            $out = & $bash $runner "/dev/sda2" "/dev/sda3" 2>&1
            $raw = ($out -join "`n")
            if ($LASTEXITCODE -ne 0) {
                Fail "install.sh 的 OTA 改写块实跑失败: $raw"
            }
            else {
                $m = [regex]::Match($raw, '(?s)=====OUT1=====\r?\n(.*?)\r?\n=====OUT2=====\r?\n(.*)$')
                if (-not $m.Success) {
                    Fail "install.sh 的 OTA 改写块输出格式异常，C13 失效"
                }
                else {
                    $r1 = $m.Groups[1].Value
                    $r2 = $m.Groups[2].Value.TrimEnd("`r", "`n")
                    if ($r1 -eq $r2) {
                        Ok "install.sh 的 OTA 改写幂等（对已改写 unit 再跑一次结果不变）"
                    }
                    else {
                        Fail "install.sh 的 OTA 改写**不幂等** —— 重复部署会叠加 Environment/DeviceAllow 行（改写块有多条路径同时命中）"
                    }
                    # 每个 SLOT 变量恰好一条赋值
                    $cntA = ([regex]::Matches($r1, '(?m)^Environment=GATEWAY_OTA_SLOT_A=')).Count
                    $cntB = ([regex]::Matches($r1, '(?m)^Environment=GATEWAY_OTA_SLOT_B=')).Count
                    if ($cntA -eq 1 -and $cntB -eq 1) {
                        Ok "改写后每个 OTA SLOT 变量恰好一条 Environment 赋值（A=$cntA B=$cntB）"
                    }
                    else {
                        Fail "改写后 Environment=GATEWAY_OTA_SLOT_A 出现 $cntA 次、_B 出现 $cntB 次（各应为 1 次）—— systemd 取最后一行，生效值可能与实际介质不符"
                    }
                    # DeviceAllow 与 Environment 必须指向同一分区
                    $envA = [regex]::Match($r1, '(?m)^Environment=GATEWAY_OTA_SLOT_A=(\S+)').Groups[1].Value
                    $envB = [regex]::Match($r1, '(?m)^Environment=GATEWAY_OTA_SLOT_B=(\S+)').Groups[1].Value
                    $allowA = [regex]::Match($r1, "(?m)^DeviceAllow=$([regex]::Escape($envA)) rw\s*$").Success
                    $allowB = [regex]::Match($r1, "(?m)^DeviceAllow=$([regex]::Escape($envB)) rw\s*$").Success
                    if ($allowA -and $allowB) {
                        Ok "DeviceAllow 与 Environment 指向同一对分区（$envA / $envB）"
                    }
                    else {
                        Fail "改写后 DeviceAllow 与 Environment=GATEWAY_OTA_SLOT_A/B（$envA / $envB）不一致 —— OTA 会写错分区"
                    }
                    # 不得残留旧默认分区
                    if ($r1 -match '(?m)^DeviceAllow=/dev/mmcblk0p[23] rw\s*$') {
                        Fail "改写后仍残留默认分区 DeviceAllow=/dev/mmcblk0p2|p3 —— 现场介质非 eMMC 时 OTA 仍写错分区"
                    }
                    else {
                        Ok "改写后无残留的默认 eMMC 分区白名单"
                    }
                }
            }
        }
        finally {
            Remove-Item Env:\GW_UNIT_B64 -ErrorAction SilentlyContinue
            Remove-Item Env:\GW_BLOCK_B64 -ErrorAction SilentlyContinue
            Remove-Item $runner -ErrorAction SilentlyContinue
        }
    }
}
Write-Host ""

# ══════════════════════════════════════════════════════════
# C14  串口稳定名链路（udev 规则 → config.json → 采集）
# ══════════════════════════════════════════════════════════
# P3-2：USB 串口的 /dev/ttyUSB* 编号由枚举顺序决定，重启一次就可能变成
# /dev/ttyUSB1。config.json 的 buses[].port_name 是裸字符串，直接进
# tokio_serial::new()，名字对不上 → open() 失败 → 采集全线静默停摆，
# 现场表现是「昨天还好好的，今天全采不上」，极易误判为硬件故障。
#
# 修法是给它一个跨重启稳定的名字 /dev/gateway-serialN，由 udev 规则维护。
# 这条链路有四个环节，缺一个就白做：
#   1) gateway-core/deploy/serial-rules.sh —— 生成器本体
#   2) install.sh 的 4.6 步 —— 部署时真的调用它并 reload udev
#   3) build-release.sh —— 必须把生成器打进交付包，否则现场根本没这脚本
#   4) verify-deploy.sh —— 交付检查单要核对 port_name 的存在性与稳定性
# 少任何一环，现场就会退回「人工保证 /dev/ttyUSB0 不变」的老办法，
# 而那正是本项要消灭的失效形态。
Write-Host "[C14] 串口稳定名链路完整（生成器/安装/打包/校验）" -ForegroundColor Cyan
$serialRulesText = ReadText "gateway-core/deploy/serial-rules.sh"
$verifyText = ReadText "deploy/verify-deploy.sh"
$buildText = ReadText "deploy/build-release.sh"
if ($null -eq $serialRulesText) {
    Fail "找不到 gateway-core/deploy/serial-rules.sh（串口稳定名生成器缺失）"
}
elseif ($null -eq $installText) {
    Warn "找不到 gateway-core/deploy/install.sh（已在 C11 报告），跳过 C14"
}
else {
    # 1) 生成器本体：必须用稳定标识（序列号 / 物理路径），不能用裸设备名
    #
    #    这里刻意断言的是 ATTRS{serial} / ATTRS{bInterfaceNumber} / ENV{DEVPATH}，
    #    而不是 ENV{ID_SERIAL} / ENV{ID_PATH}。原因有两个，都踩过：
    #      a) 导入时机 —— systemd 的 60-serial.rules 才是导入 ID_SERIAL/ID_PATH 的
    #         地方，而我方生成的 60-gateway-serial.rules 文件名字典序在它之前
    #         （g < s）→ 我方规则执行时属性还没被导入 → 匹配永不命中 → 软链
    #         永不生成，整套机制静默失效。
    #      b) ID_SERIAL 是「厂商_型号[_序列号]」的复合串，设备没有序列号时
    #         依然非空 → 两块同型号无序列号的转换器得到同一个 ID_SERIAL →
    #         抢同一个软链 → 配置指向错误设备。
    #    ATTRS{} 直接匹配 sysfs 属性、沿父链向上查找，不依赖任何 builtin 导入、
    #    不依赖规则文件顺序；ENV{DEVPATH} 是内核事件自带属性，同样零依赖。
    if ($serialRulesText -match 'ATTRS\{serial\}' -and $serialRulesText -match 'ENV\{DEVPATH\}' -and $serialRulesText -match 'ATTRS\{bInterfaceNumber\}') {
        Ok "serial-rules.sh 按稳定标识匹配（ATTRS{serial} / ATTRS{bInterfaceNumber} / ENV{DEVPATH}），不依赖 builtin 导入顺序"
    }
    else {
        Fail "serial-rules.sh 未使用 ATTRS{serial} / ATTRS{bInterfaceNumber} / ENV{DEVPATH} 等稳定标识 —— 生成的规则无法跨重启保持"
    }
    # 反向断言：不得回退到会漂移/依赖导入顺序的属性
    if ($serialRulesText -match 'ENV\{ID_SERIAL\}==' -or $serialRulesText -match 'ENV\{ID_PATH\}==') {
        Fail "serial-rules.sh 仍用 ENV{ID_SERIAL}== / ENV{ID_PATH}== 匹配 —— 我方规则文件名字典序在 systemd 60-serial.rules 之前，属性未导入，匹配永不命中"
    }
    else {
        Ok "serial-rules.sh 未使用 ENV{ID_SERIAL}== / ENV{ID_PATH}== —— 不受规则文件处理顺序影响"
    }
    # 生成器必须保留已有绑定（设备改名后序号不能被洗牌）
    if ($serialRulesText -match 'serial-bindings\.tsv' -and $serialRulesText -match 'lookup_index') {
        Ok "serial-rules.sh 维护绑定状态（serial-bindings.tsv + lookup_index），设备改名不洗牌"
    }
    else {
        Fail "serial-rules.sh 未保留已有绑定 —— 设备换名后序号被重新分配，配置里的 port_name 会指向别的设备"
    }

    # 2) install.sh 接入：必须在安装流程里真的调用，且失败不阻断安装
    if ($installText -match 'serial-rules\.sh') {
        Ok "install.sh 调用了 serial-rules.sh"
    }
    else {
        Fail "install.sh 未调用 serial-rules.sh —— 部署时不会生成 udev 规则"
    }
    if ($serialRulesText -match 'udevadm\s+control\s+--reload-rules' -and $serialRulesText -match 'udevadm\s+trigger') {
        Ok "serial-rules.sh 重载并触发 udev 规则（规则写盘即生效，无需重启）"
    }
    else {
        Fail "serial-rules.sh 未重载/触发 udev 规则 —— 规则写盘但不生效，需重启才建软链"
    }

    # 3) build-release.sh 打包：生成器不在包里 = 现场没脚本可用
    if ($null -eq $buildText) {
        Warn "找不到 deploy/build-release.sh（跳过打包断言）"
    }
    elseif ($buildText -match 'serial-rules\.sh') {
        Ok "build-release.sh 把 serial-rules.sh 打进交付包"
    }
    else {
        Fail "build-release.sh 未把 serial-rules.sh 打进交付包 —— 现场无从生成稳定名"
    }

    # 4) verify-deploy.sh 校验 port_name
    if ($null -eq $verifyText) {
        Warn "找不到 deploy/verify-deploy.sh（跳过 port_name 校验断言）"
    }
    elseif ($verifyText -match 'port_name' -and $verifyText -match 'gateway-serial') {
        Ok "verify-deploy.sh 核对 buses[].port_name 的存在性与稳定性"
    }
    else {
        Fail "verify-deploy.sh 未核对 buses[].port_name —— 交付检查单看不出串口配置是否会失效"
    }

    # 5) systemd 放行：稳定名是软链，最终落到 /dev/ttyUSB* 这个字符设备。
    #    设备组形式（char-ttyUSB）按设备本身放行，与进程打开的是软链还是
    #    真实节点无关 → 现有 gateway.service 无需改动。这里反向断言：
    #    万一有人把 DeviceAllow 改成 /dev/gateway-serial0 路径形式，
    #    systemd v255 会 stat 失败后整条丢弃该条目 → 串口被 EPERM 拒绝。
    if ($null -ne $serviceText) {
        if ($serviceText -match '(?m)^\s*DeviceAllow=char-ttyUSB\s+rw\s*$') {
            Ok "gateway.service 用设备组形式放行串口（char-ttyUSB）—— udev 软链落到 ttyUSB 设备时同样生效"
        }
        else {
            Fail "gateway.service 未以 char-ttyUSB 设备组形式放行串口 —— 稳定名软链会被 systemd 在 open() 之前 EPERM 拒绝"
        }
        if ($serviceText -match '(?m)^\s*DeviceAllow=/dev/gateway-serial') {
            Fail "gateway.service 出现了 DeviceAllow=/dev/gateway-serial* 路径形式 —— systemd v255 会 stat 失败后整条丢弃该条目"
        }
    }
}
Write-Host ""

# ══════════════════════════════════════════════════════════
# C15  发布门禁链路（重作业 → heavy.yml → release-gate.yml）
# ══════════════════════════════════════════════════════════
# P1-3：7 个重作业（cross-arm / static-musl / coverage / chaos / bench /
# soak-smoke / sku-size-matrix）原先长在 ci.yml 里，门禁是
# `if: github.event_name == 'workflow_dispatch' || vars.RUN_HEAVY == 'true'`。
# 也就是说：**默认 push 不跑、仓库没有 schedule、没有 release workflow**
# ——「发布前必跑」根本不存在，全靠人记得点那个按钮。后果是 P1-2 刚扩到整个
# lib 的 1382 条 cross-arm 测试只在手动触发时才生效，coverage 不跑则
# check job 的覆盖率地板门禁形同虚设，发版时才发现某些重作业从没跑过。
#
# 修法：重作业抽到可复用工作流 heavy.yml，release-gate.yml 无条件调用它，
# 由「推 v* tag / 手动触发 / 每周定时」三条路径驱动。
#
# 这里最要防的回归是**假门禁**：门禁若写在 heavy.yml（被调用方）内部，
# 被 workflow_call 调用时 github.event_name 是**调用方**的事件名（tag push
# 是 'push'），于是重作业被静默全部跳过 —— 门禁看起来在跑，实际一个作业
# 都没执行。所以门禁只能放在调用方，heavy.yml 内部必须干净。
Write-Host "[C15] 发布门禁链路（重作业可复用 + release-gate 无条件触发）" -ForegroundColor Cyan

# 取出一个 job 的正文（顶层 job，缩进 2 空格）
function Get-JobBlock([string]$yaml, [string]$jobName) {
    $pat = "(?ms)^  " + [regex]::Escape($jobName) + ":\s*\r?\n(.*?)(?=^  [A-Za-z0-9_-]+:\s*\r?\n|\z)"
    $m = [regex]::Match($yaml, $pat)
    if ($m.Success) { return $m.Groups[1].Value }
    return $null
}

$heavyJobs = @("chaos", "coverage", "cross-arm", "static-musl", "bench", "soak-smoke", "sku-size-matrix", "release-package")
$ciText = ReadText ".github/workflows/ci.yml"
$heavyText = ReadText ".github/workflows/heavy.yml"
$gateText = ReadText ".github/workflows/release-gate.yml"
$gatePsText = ReadText "scripts/release_gate.ps1"

if ($null -eq $ciText) {
    Warn "找不到 .github/workflows/ci.yml（跳过 C15）"
}
elseif ($null -eq $heavyText) {
    Fail "找不到 .github/workflows/heavy.yml —— 重作业抽离后必须有独立可复用工作流，否则 release-gate 无从调用"
}
elseif ($null -eq $gateText) {
    Fail "找不到 .github/workflows/release-gate.yml —— 没有发布门禁工作流，重作业仍然是「靠人记得点按钮」"
}
else {
    # 1) heavy.yml 必须含全部重作业，且必须是可复用工作流
    $missing = @()
    foreach ($j in $heavyJobs) {
        if ($null -eq (Get-JobBlock $heavyText $j)) { $missing += $j }
    }
    if ($missing.Count -eq 0) {
        Ok "heavy.yml 含全部 $($heavyJobs.Count) 个重作业（$($heavyJobs -join ' / ')）"
    }
    else {
        Fail "heavy.yml 缺少重作业：$($missing -join ' / ')"
    }
    if ($heavyText -match '(?m)^\s*workflow_call:\s*$') {
        Ok "heavy.yml 声明 workflow_call —— 可被 ci.yml 与 release-gate.yml 复用"
    }
    else {
        Fail "heavy.yml 未声明 workflow_call —— 无法作为可复用工作流被调用"
    }

    # 2) 关键反向断言：heavy.yml 内部不得带 job 级 if 门禁
    #    （只看 4 空格缩进的 job 级键；step 级的 if（如 if: always()、
    #     if: matrix.emu != ''）是步骤自身的正常条件，必须保留）
    $gated = @()
    foreach ($j in $heavyJobs) {
        $blk = Get-JobBlock $heavyText $j
        if ($null -ne $blk -and $blk -match '(?m)^ {4}if:') { $gated += $j }
    }
    # 只看 YAML 代码行（去掉注释行）——注释里引用原门禁写法是正常的说明文字
    $heavyCode = (($heavyText -split "`r?`n") | Where-Object { $_ -notmatch '^\s*#' }) -join "`n"
    if ($gated.Count -eq 0 -and $heavyCode -notmatch 'vars\.RUN_HEAVY\s*==') {
        Ok "heavy.yml 内部无 if 门禁 —— 被 workflow_call 调用时事件名是调用方的，门禁留在里面会被静默跳过（假门禁）"
    }
    else {
        $why = if ($gated.Count -gt 0) { "作业：$($gated -join ' / ')" } else { "代码里出现了 vars.RUN_HEAVY == 形式的门禁" }
        Fail "heavy.yml 内部存在 if 门禁（$why）—— 被 workflow_call 调用时会按调用方的事件名判定，重作业会被静默跳过"
    }

    # 3) ci.yml 仍以原条件调用 heavy.yml（手动触发 / RUN_HEAVY）
    $ciHeavy = Get-JobBlock $ciText "heavy"
    if ($null -ne $ciHeavy -and $ciHeavy -match 'uses:\s*\./\.github/workflows/heavy\.yml') {
        Ok "ci.yml 通过 uses 调用 heavy.yml（重作业仍可手动触发，行为不变）"
    }
    else {
        Fail "ci.yml 未调用 heavy.yml —— 重作业被抽走后 ci.yml 失去这 7 项检查"
    }
    if ($null -ne $ciHeavy -and $ciHeavy -match "github\.event_name == 'workflow_dispatch'" -and $ciHeavy -match 'vars\.RUN_HEAVY') {
        Ok "ci.yml 调用方保留原触发条件（workflow_dispatch 或 RUN_HEAVY=true）"
    }
    else {
        Fail "ci.yml 调用方的触发条件被改动 —— 原本手动触发即可跑全量重作业的行为丢失"
    }
    if ($null -ne $ciHeavy -and $ciHeavy -match '(?m)^\s*needs:\s*check\s*$') {
        Ok "ci.yml 的 heavy 调用仍依赖 check（重作业在快门禁通过后才起跑）"
    }
    else {
        Fail "ci.yml 的 heavy 调用未依赖 check —— 重作业会在 fmt/clippy/test 之前烧额度"
    }

    # 4) release-gate.yml：三条触发路径 + 无条件调用 + 不可取消
    if ($gateText -match "(?m)^\s*workflow_dispatch:\s*$" -and $gateText -match "(?m)^\s*schedule:\s*$" -and $gateText -match "'v\*'") {
        Ok "release-gate.yml 三条触发路径齐全（手动 / 推 v* tag / 每周定时）"
    }
    else {
        Fail "release-gate.yml 触发路径不全 —— 至少要保留「推 v* tag」和「每周定时」两条，否则重作业又是靠人记得点按钮"
    }
    $gateHeavy = Get-JobBlock $gateText "heavy"
    if ($null -ne $gateHeavy -and $gateHeavy -match 'uses:\s*\./\.github/workflows/heavy\.yml' -and $gateHeavy -notmatch '(?m)^ {4}if:') {
        Ok "release-gate.yml 无条件调用 heavy.yml（发布门禁不设额外条件）"
    }
    else {
        Fail "release-gate.yml 调用 heavy.yml 的方式不对 —— 必须是无 if 的无条件调用"
    }
    if ($gateText -match '(?m)^\s*cancel-in-progress:\s*false\s*$') {
        Ok "release-gate.yml 不取消进行中的运行 —— 避免「门禁跑到一半被顶掉」造成假通过"
    }
    else {
        Fail "release-gate.yml 的 cancel-in-progress 不是 false —— 后一次触发会取消前一次门禁，出现无结论的假通过"
    }
    if ($gateText -match 'needs\.heavy\.result' -and $gateText -match 'HEAVY_RESULT') {
        Ok "release-gate.yml 用 always() + needs.heavy.result 判定真实结论（重作业失败时门禁明确失败）"
    }
    else {
        Fail "release-gate.yml 未按 needs.heavy.result 判定结论 —— 重作业失败时门禁可能被跳过而非失败"
    }

    # 5) 本地门禁能触发远端门禁（发版前先跑起来并等到出结论）
    if ($null -eq $gatePsText) {
        Warn "找不到 scripts/release_gate.ps1（跳过本地触发断言）"
    }
    elseif ($gatePsText -match 'remote-gate' -and $gatePsText -match 'workflow run' -and $gatePsText -match 'run view') {
        Ok "release_gate.ps1 会触发 release-gate.yml 并轮询到出结论（发版前不必打完 tag 才发现门禁没过）"
    }
    else {
        Fail "release_gate.ps1 未接入远端发布门禁 —— 本地门禁全绿也不代表远端那套重作业过了"
    }
}
Write-Host ""

# ══════════════════════════════════════════════════════════
# C16  交付脚本进 CI：feature→协议注册名映射 与「不允许平行实现」
# ══════════════════════════════════════════════════════════
# 两件事：
#
# 1) **映射表不能漂移**。build-release.sh 的 PROTO_NAME_MAP 把 cargo feature
#    映射成南向协议**注册名**（entries.rs 的 ProtocolEntry.name）。
#    两者不是一一对应（opcua-client → opcua；bacnet-mstp → bacnet_mstp）。
#    BUILD-INFO.protocols 写的是注册名 —— 写错的话 verify-deploy.sh 的
#    「协议裁剪一致性」会拿它跟线上 /api/protocols 比对，必然失败。
#    映射表与 entries.rs 是两份来源，靠这里逐条锁死。
#
# 2) **不允许再出现平行实现**。CI 的 deploy-smoke 曾经手工复刻包布局、
#    自造 BUILD-INFO.txt（build_time=ci-smoke）—— 交付脚本的产出从未被验证，
#    交付脚本有 bug CI 永远发现不了。这里断言：CI 必须真的调用
#    deploy/build-release.sh，且不得再出现自造的 build_time=ci-smoke。
Write-Host "[C16] 交付脚本进 CI（协议名映射一致 + 无平行实现）" -ForegroundColor Cyan

$entriesText = ReadText "gateway-core/src/south/protocol/registry/entries.rs"
if ($null -eq $buildSh) {
    Fail "找不到 deploy/build-release.sh（已在 C5 报告）"
}
elseif ($null -eq $entriesText) {
    Warn "找不到 gateway-core/src/south/protocol/registry/entries.rs（跳过协议名映射断言）"
}
else {
    # 1a) 从 entries.rs 抽出 feature → 注册名
    #     只认「#[cfg(feature=..)] 紧跟 ProtocolEntry { name: ".." }」的注册项；
    #     文件头部的 `use` 语句同样带 cfg，但形态不同，不会被误匹配。
    $regMap = @{}
    foreach ($em in [regex]::Matches(
            $entriesText,
            '#\[cfg\(feature\s*=\s*"([^"]+)"\)\]\s*\r?\n\s*ProtocolEntry\s*\{[^{}]*?name:\s*"([^"]+)"')) {
        $regMap[$em.Groups[1].Value] = $em.Groups[2].Value
    }
    if ($regMap.Count -eq 0) {
        Fail "无法从 entries.rs 解析任何 ProtocolEntry（正则或文件结构变了）—— C16 的映射断言已失效"
    }

    # 1b) 从 build-release.sh 抽出 PROTO_NAME_MAP
    $pm = [regex]::Match($buildSh, '(?m)^PROTO_NAME_MAP="([^"]+)"')
    if (-not $pm.Success) {
        Fail "build-release.sh 里找不到 PROTO_NAME_MAP —— BUILD-INFO.protocols 无从推导（协议名映射会缺失）"
    }
    else {
        $scriptMap = @{}
        foreach ($pair in ($pm.Groups[1].Value -split '\s+' | Where-Object { $_ })) {
            $kv = $pair -split '='
            if ($kv.Count -eq 2) { $scriptMap[$kv[0]] = $kv[1] }
        }
        foreach ($f in $regMap.Keys) {
            $expected = $regMap[$f]
            if (-not $scriptMap.ContainsKey($f)) {
                Fail "entries.rs 注册了 feature '$f'（注册名 '$expected'），但 build-release.sh 的 PROTO_NAME_MAP 没有它 —— BUILD-INFO.protocols 会漏写该协议，verify-deploy.sh 的协议裁剪一致性必然失败"
            }
            elseif ($scriptMap[$f] -ne $expected) {
                Fail "协议名映射漂移：entries.rs 中 feature '$f' 的注册名是 '$expected'，build-release.sh 写的是 '$($scriptMap[$f])' —— BUILD-INFO.protocols 与线上 /api/protocols 口径不一致"
            }
            else {
                Ok "协议名映射一致：$f → $expected"
            }
        }
        foreach ($f in $scriptMap.Keys) {
            if (-not $regMap.ContainsKey($f)) {
                Fail "build-release.sh 的 PROTO_NAME_MAP 里有 entries.rs 未注册的 feature '$f' —— 映射表已过期"
            }
        }
        if (($scriptMap.Keys | Where-Object { $regMap.ContainsKey($_) }).Count -eq $regMap.Count) {
            Ok "PROTO_NAME_MAP 与 entries.rs 的注册项集合完全一致（$($regMap.Count) 项）"
        }

        # 1c) BUILD-INFO 必须同时写注册名与 feature 名两个字段
        if ($buildSh -match '(?m)^protocols=\$PROTO_NAMES\s*$' -and $buildSh -match '(?m)^protocol_features=\$PROTO_FEATURES\s*$') {
            Ok "BUILD-INFO 同时写 protocols（注册名）与 protocol_features（cargo feature 名）"
        }
        else {
            Fail "BUILD-INFO.txt 没有同时写 protocols= 与 protocol_features= —— 两种口径混在一个字段里，verify-deploy.sh 与排障都会判错"
        }
    }
}

# 2) CI 必须真的调用交付脚本，且不得再有自造元信息的平行实现
if ($null -eq $ciText) {
    Warn "找不到 .github/workflows/ci.yml（跳过 CI 调用断言）"
}
else {
    $smokeBlock = Get-JobBlock $ciText "deploy-smoke"
    if ($null -eq $smokeBlock) {
        Fail "ci.yml 里找不到 deploy-smoke 作业 —— 交付冒烟门禁没了"
    }
    else {
        if ($smokeBlock -match 'deploy/build-release\.sh') {
            Ok "ci.yml 的 deploy-smoke 直接调用 deploy/build-release.sh（交付脚本的产出被真实验证）"
        }
        else {
            Fail "ci.yml 的 deploy-smoke 没有调用 deploy/build-release.sh —— 交付脚本的产出仍未进 CI，脚本有 bug 发现不了"
        }
        # 只看 YAML 代码行（去掉注释行）——注释里说明「以前是自造的」是正常的
        $smokeCode = (($smokeBlock -split "`r?`n") | Where-Object { $_ -notmatch '^\s*#' }) -join "`n"
        if ($smokeCode -match 'build_time=ci-smoke') {
            Fail "ci.yml 的 deploy-smoke 仍在自造 BUILD-INFO.txt（build_time=ci-smoke）—— 这是平行实现，必须删除"
        }
        else {
            Ok "ci.yml 的 deploy-smoke 不再自造 BUILD-INFO.txt"
        }
    }
    if ($null -eq $heavyText) {
        Warn "找不到 .github/workflows/heavy.yml（跳过 ARM 交付包断言）"
    }
    else {
        $rpBlock = Get-JobBlock $heavyText "release-package"
        if ($null -eq $rpBlock) {
            Fail "heavy.yml 里找不到 release-package 作业 —— ARM64 真交付包无人产出，交叉架构的交付布局从未验证"
        }
        elseif ($rpBlock -match 'deploy/build-release\.sh') {
            Ok "heavy.yml 的 release-package 调用 deploy/build-release.sh（ARM64 交付包由真脚本产出）"
        }
        else {
            Fail "heavy.yml 的 release-package 没有调用 deploy/build-release.sh —— ARM64 交付包仍是手工拼装"
        }
    }
}

# ══════════════════════════════════════════════════════════
# C17  交付脚本的执行位
#
# 真实发生过：heavy 的 release-package 报
#   `./deploy/build-release.sh: Permission denied`（exit 126）
# 根因是本仓库在 Windows 上开发（core.filemode=false），git 里把脚本记成
# 100644（无执行位）。Linux 上 checkout 出来就没有 x 位，直接 ./ 执行必然失败。
#
# 这不只是 CI 的事：交付包里的 deliver.sh / install.sh 也是同一批文件，
# 现场文档写的正是 `./deliver.sh` —— 包做出来了但现场跑不起来。
#
# 两道防线：
#   ① git 索引里的 mode 必须是 100755（用 git ls-files -s 查，不是看磁盘属性：
#      Windows 磁盘上没有 x 位，看磁盘永远查不出问题）
#   ② CI 里不依赖执行位调用交付脚本（`bash xxx.sh`），这样即便有人在
#      Windows 上新建脚本忘了加执行位，CI 也不会莫名其妙地挂在这上面
Write-Host "[C17] 交付脚本执行位（git 索引 mode + CI 不依赖 x 位）" -ForegroundColor Cyan

$execScripts = @(
    "deploy/build-release.sh",
    "deploy/deliver.sh",
    "deploy/verify-deploy.sh",
    "gateway-core/deploy/install.sh",
    "gateway-core/deploy/serial-rules.sh",
    "scripts/test_serial_rules.sh"
)

$gitBin = Get-Command git -ErrorAction SilentlyContinue
if ($null -eq $gitBin) {
    Warn "找不到 git 命令（跳过执行位断言）"
}
else {
    $lsOut = & git -C $repoRoot ls-files -s -- $execScripts 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $lsOut) {
        Warn "git ls-files 执行失败（跳过执行位断言）"
    }
    else {
        $modeMap = @{}
        foreach ($line in $lsOut) {
            if ($line -match '^(\d+)\s+\S+\s+\d+\t(.+)$') { $modeMap[$Matches[2]] = $Matches[1] }
        }
        foreach ($s in $execScripts) {
            if (-not $modeMap.ContainsKey($s)) {
                Fail "交付脚本 '$s' 不在 git 索引里（路径写错了？）"
                continue
            }
            $mode = $modeMap[$s]
            if ($mode -eq "100755") {
                Ok "执行位正确：$s（100755）"
            }
            else {
                Fail "交付脚本 '$s' 在 git 索引里的 mode 是 $mode（应为 100755）—— Windows 上开发时 core.filemode=false，执行位不会被自动记录，Linux 上 ./ 执行会 Permission denied。用 'git update-index --chmod=+x $s' 修正"
            }
        }
    }
}

# CI 里调用交付脚本必须用 `bash <脚本>`，不能是 `./<脚本>`
foreach ($wf in @(@{ Name = "ci.yml"; Text = $ciText }, @{ Name = "heavy.yml"; Text = $heavyText })) {
    if ($null -eq $wf.Text) { continue }
    if ($wf.Text -match '(?m)^\s*run:\s*\./deploy/build-release\.sh') {
        Fail "$($wf.Name) 里用 './deploy/build-release.sh' 调用交付脚本 —— 依赖 git 执行位，Windows 上开发的新脚本必然挂在这里。改用 'bash deploy/build-release.sh'"
    }
    else {
        Ok "$($wf.Name) 用 'bash deploy/build-release.sh' 调用交付脚本（不依赖执行位）"
    }
}

# 交付脚本必须给包内脚本显式补执行位：git 的 mode 只影响仓库，
# build-release.sh 拷贝出来的文件权限取决于构建机，chmod 才管得住。
if ($null -ne $buildSh) {
    if ($buildSh -match '(?m)chmod \+x[^\n]*\$OUT/install\.sh') {
        Ok "build-release.sh 给包内脚本显式补执行位（chmod +x）"
    }
    else {
        Fail "build-release.sh 没有给包内脚本补执行位（chmod +x）—— 交付包在 Linux 上解出来可能不可执行，现场 ./deliver.sh 会 Permission denied"
    }
}
else {
    Warn "找不到 deploy/build-release.sh（跳过 chmod 断言）"
}

# ══════════════════════════════════════════════════════════
# C18  工作流 shell 里的变量名笔误
#
# 真实发生过：heavy 的 release-package 报
#   `line 48: PROTOCOLS: unbound variable`
# 根因是同一个脚本块里赋值用 `PROTOS=`，引用却写成 `$PROTOCOLS`。
# 在 `set -u` 下这类笔误不是「取到空值」，而是整步直接崩 ——
# 而崩掉的位置往往是**断言已经跑完之后**（包已经构建好了、协议名也对了），
# 于是失败信息完全指不到真正的问题：日志里只剩一句 unbound variable。
#
# 检查法：把脚本块里「被赋值的变量名」与「被引用的变量名」各自收集，
# 若某个引用名与某个赋值名只差极少的字符（编辑距离 ≤ 2），就判定为笔误。
# 这样只报「长得极像、极可能是手滑」的情形，不会把正常的
# `$OUT` / `$f` / `$1` 之类报成错。
Write-Host "[C18] 工作流 shell 变量名笔误（赋值名与引用名形近）" -ForegroundColor Cyan

function Get-Levenshtein([string]$a, [string]$b) {
    if ($a -eq $b) { return 0 }
    $la = $a.Length; $lb = $b.Length
    if ($la -eq 0) { return $lb }
    if ($lb -eq 0) { return $la }
    $prev = New-Object int[] ($lb + 1)
    $cur = New-Object int[] ($lb + 1)
    for ($j = 0; $j -le $lb; $j++) { $prev[$j] = $j }
    for ($i = 1; $i -le $la; $i++) {
        $cur[0] = $i
        for ($j = 1; $j -le $lb; $j++) {
            $cost = if ($a[$i - 1] -eq $b[$j - 1]) { 0 } else { 1 }
            $m = $prev[$j] + 1
            $n = $cur[$j - 1] + 1
            $o = $prev[$j - 1] + $cost
            $cur[$j] = [Math]::Min([Math]::Min($m, $n), $o)
        }
        $tmp = $prev; $prev = $cur; $cur = $tmp
    }
    return [int]$prev[$lb]
}

# GitHub 提供的上下文变量与 shell 内建：不是本脚本块定义的，不参与比对
$ctxVars = @(
    "PATH", "HOME", "USER", "PWD", "OLDPWD", "SHELL", "TMPDIR", "RANDOM", "SECONDS", "LINENO", "IFS",
    "CI", "GITHUB_WORKSPACE", "GITHUB_ENV", "GITHUB_OUTPUT", "GITHUB_STEP_SUMMARY", "GITHUB_PATH",
    "GITHUB_RUN_ID", "GITHUB_RUN_NUMBER", "GITHUB_SHA", "GITHUB_REF", "GITHUB_REPOSITORY",
    "GITHUB_WORKFLOW", "GITHUB_HEAD_REF", "GITHUB_BASE_REF", "GITHUB_TOKEN", "GITHUB_ACTOR",
    "GITHUB_EVENT_NAME", "GITHUB_EVENT_PATH", "GITHUB_API_URL", "GITHUB_SERVER_URL",
    "RUNNER_OS", "RUNNER_ARCH", "RUNNER_TEMP", "RUNNER_TOOL_CACHE", "RUNNER_DEBUG",
    "CARGO_HOME", "CARGO_TERM_COLOR", "CARGO_INCREMENTAL", "RUSTFLAGS", "RUSTUP_HOME",
    "NODE_OPTIONS", "NPM_CONFIG_PREFIX", "http_proxy", "https_proxy", "no_proxy",
    "DEBIAN_FRONTEND", "BASH_ENV", "SHLVL", "PIPESTATUS", "REPLY", "UID", "EUID", "GROUPS"
)

# 只看这些作业的 run 块：交付链路的重作业（交付包 + 交付冒烟）
$watchJobs = @(
    @{ Wf = "ci.yml"; Text = $ciText; Job = "deploy-smoke" },
    @{ Wf = "heavy.yml"; Text = $heavyText; Job = "release-package" }
)

foreach ($wj in $watchJobs) {
    if ($null -eq $wj.Text) {
        Warn "找不到 .github/workflows/$($wj.Wf)（跳过 $($wj.Job) 的变量断言）"
        continue
    }
    $block = Get-JobBlock $wj.Text $wj.Job
    if ($null -eq $block) {
        Warn "$($wj.Wf) 里找不到 $($wj.Job) 作业（跳过变量断言）"
        continue
    }
    # 去掉注释行：注释里出现 $VAR 是正常的说明文字
    $code = (($block -split "`r?`n") | Where-Object { $_ -notmatch '^\s*#' }) -join "`n"

    # 赋值：VAR= / VAR = / export VAR=
    $assigned = @{}
    foreach ($am in [regex]::Matches($code, '(?m)(?:^|\s|;)(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=')) {
        $assigned[$am.Groups[1].Value] = $true
    }
    # 引用：$VAR 或 ${VAR}
    $referenced = @{}
    foreach ($rm in [regex]::Matches($code, '\$\{?([A-Za-z_][A-Za-z0-9_]*)\}?')) {
        $referenced[$rm.Groups[1].Value] = $true
    }

    $hit = $false
    foreach ($r in $referenced.Keys) {
        if ($assigned.ContainsKey($r)) { continue }
        if ($ctxVars -contains $r) { continue }
        if ($r -match '^(BASH_|^[0-9])') { continue }
        # 找形近的赋值名：
        #   ① 编辑距离 ≤ 2（改错一两个字母）
        #   ② 纯增删关系：编辑距离恰等于长度差（多打了几个字母）
        # 第②条是本次真实缺陷的形态，编辑距离为 3，单靠①抓不到。
        foreach ($a in $assigned.Keys) {
            if ($a.Length -lt 5) { continue }
            if ($r.Length -lt 5) { continue }
            $d = Get-Levenshtein $r $a
            # 「纯增删」判据：编辑距离恰好等于两名字长度差 → 说明较长的那个
            # 只是被多打了几个字符（PROTOS → PROTOCOLS 多打了 COLS）。
            # 这是本次真实缺陷的形态；编辑距离为 3，靠「≤2」抓不到。
            $lenDiff = [Math]::Abs($r.Length - $a.Length)
            $isInsertionOnly = ($lenDiff -ge 1 -and $d -eq $lenDiff)
            if (($d -ge 1 -and $d -le 2) -or $isInsertionOnly) {
                $why = if ($isInsertionOnly) { "较长者只是多打了 $lenDiff 个字符" } else { "相差 $d 个字符" }
                Fail "$($wj.Wf) 的 $($wj.Job) 作业里引用了 '" + '$' + $r + "'，但本块内只赋值为 '$a'（$why）—— 疑似变量名笔误。set -u 下这类错误会在断言跑完之后才崩，失败信息指不到真正的问题"
                $hit = $true
                break
            }
        }
    }
    if (-not $hit) {
        Ok "$($wj.Wf) 的 $($wj.Job) 作业里没有形近的变量名笔误（赋值 $($assigned.Count) 个 / 引用 $($referenced.Count) 个）"
    }
}

# ══════════════════════════════════════════════════════════
# C19  sudo 复制出的文件必须能被 runner 自己读
#
# 真实发生过：deploy smoke 报
#   grep: /tmp/cfg_disk_after.json: Permission denied
#   磁盘 config.json 里没有 realtime_buffer=4096（新配置没落盘）
# 根因：`sudo cp "$CONF" /tmp/x.json` 由 root 创建文件，属主 root、
# 权限 0600 → 后面以 runner 身份跑的 grep/cmp 读不到。
# 内容其实完全正确（文件里就有 "realtime_buffer": 4096），
# 断言在「结果为真」的情况下报了「结果为假」，排查方向被带偏。
#
# 正确写法：`sudo cat "$CONF" > /tmp/x.json`
# —— 重定向由当前 shell 执行，文件归 runner 自己，权限 0644。
Write-Host "[C19] sudo 产出的临时文件必须可被 runner 读取" -ForegroundColor Cyan

foreach ($wj in $watchJobs) {
    if ($null -eq $wj.Text) { continue }
    $block = Get-JobBlock $wj.Text $wj.Job
    if ($null -eq $block) { continue }
    $code = (($block -split "`r?`n") | Where-Object { $_ -notmatch '^\s*#' }) -join "`n"

    # 找出所有 `sudo cp <src> <dst>`：dst 由 root 创建，后续读取会 Permission denied
    $bad = $false
    foreach ($cm in [regex]::Matches($code, '(?m)^\s*sudo\s+cp\s+(\S+)\s+(\S+)\s*$')) {
        $dst = $cm.Groups[2].Value
        Fail "$($wj.Wf) 的 $($wj.Job) 作业里用了 'sudo cp ... $dst' —— 目标文件属主是 root、权限 0600，后续以 runner 身份 grep/cmp 会 Permission denied，断言会在内容正确时反而失败。改用 'sudo cat <src> > $dst'"
        $bad = $true
    }
    if (-not $bad) {
        Ok "$($wj.Wf) 的 $($wj.Job) 作业里没有 'sudo cp' 产出的 root 属主临时文件"
    }
}
Write-Host ""

# ══════════════════════════════════════════════════════════
# C20  出厂完整配置模板（P3-1b）
#
# 背景：gateway-core/config.json.example 是「配置片段示例」——缺 buses/points/mqtt
# 等必填段，文件自身也写明不会被网关加载；交付方若拿它当整份配置，网关必然起不来。
# 现场需要的是一份**开箱可加载**的初值，于是随包提供
# gateway-core/deploy/config.factory.json。
#
# 模板坏掉的方式都很安静：打包时漏拷一行 → 包里没这个文件；改 JSON 时手滑
# → 语法错；删掉某个字段 → serde 反序列化失败；配额档位丢了 → license 未带
# quota 时查不到档位。这些都要等交付方在设备上复制过去才发现，排查成本极高。
# 所以这里三重钉死：
#   ① 仓库里存在该文件且是合法 JSON
#   ② 结构完整：buses/points 非空、mqtt/web/license 齐备、
#      license.tier_quotas 四档 core→ultra 且 max_points 严格递增
#   ③ 打包与安装两条链路真的用它：build-release.sh 拷进包、install.sh 兜底落盘
Write-Host "[C20] 出厂完整配置模板（存在性 + 结构完整性 + 打包/安装链路）" -ForegroundColor Cyan

$factoryRel = "gateway-core/deploy/config.factory.json"
$factoryText = ReadText $factoryRel
if ($null -eq $factoryText) {
    Fail "找不到 $factoryRel —— 出厂完整配置模板缺失，交付包只能给片段示例，现场无从下手"
}
else {
    $factory = $null
    try {
        $factory = $factoryText | ConvertFrom-Json -ErrorAction Stop
        Ok "$factoryRel 是合法 JSON"
    }
#
# 另加一道**可启动性**断言：模板的 web 组合不得触发启动期 fail-closed。
# 「能加载」与「能启动」是两件事——历史上模板正因为两者混淆，
# 让走 install.sh 兜底路径装机的网关开箱即被拒绝启动。
    catch {
        Fail "$factoryRel 不是合法 JSON（$($_.Exception.Message)）—— 交付方在设备上复制过去会直接起不来"
    }

    if ($null -ne $factory) {
        # 顶层必填段：缺任何一段，ConfigLoader 的 serde 反序列化就会失败
        $requiredSections = @("buses", "points", "mqtt", "web", "license")
        $missing = @()
        foreach ($sec in $requiredSections) {
            if ($null -eq $factory.PSObject.Properties[$sec]) { $missing += $sec }
        }
        if ($missing.Count -gt 0) {
            Fail "$factoryRel 缺必填段: $($missing -join ', ') —— 网关加载会因字段缺失直接失败"
        }
        else {
            Ok "$factoryRel 含全部必填段: $($requiredSections -join ', ')"
        }

        # 示例采集配置必须非空：空 buses/points 虽然能加载，但开箱验不了采集链路
        $busCount = 0; $pointCount = 0
        if ($null -ne $factory.PSObject.Properties["buses"]) { $busCount = @($factory.buses).Count }
        if ($null -ne $factory.PSObject.Properties["points"]) { $pointCount = @($factory.points).Count }
        if ($busCount -lt 1 -or $pointCount -lt 1) {
            Fail "$factoryRel 的 buses=$busCount / points=$pointCount —— 出厂模板至少要有一条示例总线和点位，否则开箱无法验证采集链路"
        }
        else {
            Ok "$factoryRel 带示例采集配置（buses=$busCount, points=$pointCount）"
        }

        # license 四档配额：core→ultra，且 max_points 严格递增
        $quotas = @()
        if ($null -ne $factory.PSObject.Properties["license"] -and
            $null -ne $factory.license.PSObject.Properties["tier_quotas"]) {
            $quotas = @($factory.license.tier_quotas)
        }
        if ($quotas.Count -ne 4) {
            Fail "$factoryRel 的 license.tier_quotas 应有四档（core/standard/pro/ultra），实际 $($quotas.Count) 档 —— license 文件未带 quota 时查不到档位"
        }
        else {
            $tiers = @($quotas | ForEach-Object { [string]$_.tier })
            $expectTiers = @("core", "standard", "pro", "ultra")
            if (($tiers -join ',') -ne ($expectTiers -join ',')) {
                Fail "$factoryRel 的档位顺序为 [$($tiers -join ', ')]，应为 [core, standard, pro, ultra]"
            }
            else {
                Ok "$factoryRel 的 tier_quotas 覆盖四档且顺序正确"
            }

            $prevPoints = -1.0
            $monotonic = $true
            foreach ($q in $quotas) {
                $mp = $null
                if ($null -ne $q.PSObject.Properties["quota"] -and
                    $null -ne $q.quota.PSObject.Properties["max_points"]) {
                    $mp = $q.quota.max_points
                }
                if ($null -eq $mp) {
                    Fail "$factoryRel 的档位 '$($q.tier)' 没有 quota.max_points —— 该档位无法限制点位数"
                    $monotonic = $false
                    break
                }
                if ([double]$mp -le $prevPoints) {
                    Fail "$factoryRel 的 max_points 未严格递增（'$($q.tier)' = $mp，前一档 = $prevPoints）—— 升级档位在配额上没有意义"
                    $monotonic = $false
                    break
                }
                $prevPoints = [double]$mp
            }
            if ($monotonic) {
                Ok "$factoryRel 的四档 max_points 严格递增"
            }
        }
    }
}

# 打包链路：build-release.sh 必须真的把它拷进交付包
if ($null -eq $buildSh) {
    Warn "找不到 deploy/build-release.sh（跳过出厂模板打包断言）"
}
elseif ($buildSh -match '(?m)^\s*cp\s+[^\n]*config\.factory\.json[^\n]*"\$OUT/') {
    Ok "build-release.sh 把出厂模板拷进交付包"
}
else {
    Fail "build-release.sh 没有把 $factoryRel 拷进 `$OUT —— 交付包里只有片段示例，现场拿不到可加载的初值"
}

        # ★ 出厂模板必须**能启动**，而不只是能被加载。
        #
        # 历史事故：模板写死 listen_addr=0.0.0.0:8080 + 空 password_hash，
        # 而 bootstrap 的 fail-closed 判定是「绑定所有网卡 + 默认凭据 = 硬性违规 →
        # 拒绝启动」。走 install.sh 兜底路径装机的网关开箱起不来，模板注释却写着
        # 「启动会告警」。Rust 侧已有单测（factory_template_in_package_is_loadable）
        # 用 WebConfig::security_violations() 钉住同一判据；这里再钉一道静态检查，
        # 目的是**改坏了当场就能看到**，不必等 cargo test。
        $factoryListen = ""
        if ($null -ne $factory.PSObject.Properties["web"] -and
            $null -ne $factory.web.PSObject.Properties["listen_addr"]) {
            $factoryListen = [string]$factory.web.listen_addr
        }
        $factoryPw = ""
        if ($null -ne $factory.PSObject.Properties["web"] -and
            $null -ne $factory.web.PSObject.Properties["password_hash"]) {
            $factoryPw = [string]$factory.web.password_hash
        }
        $bindsAll = ($factoryListen.StartsWith("0.0.0.0") -or $factoryListen.StartsWith("[::]"))
        if ($bindsAll -and [string]::IsNullOrEmpty($factoryPw)) {
            Fail "$factoryRel 的 web 组合为 listen_addr=$factoryListen + 空 password_hash —— 这会触发启动期 fail-closed（网关直接拒绝启动，不是告警）。出厂模板必须先能起来，LAN 访问由现场改密后再放开"
        }
        elseif ($bindsAll) {
            Ok "$factoryRel 的 web 组合不触发 fail-closed（全网卡监听 + 已设 password_hash）"
        }
        else {
            Ok "$factoryRel 的 web 组合不触发 fail-closed（listen_addr=$factoryListen，回环监听）"
        }

        # password_hash 留空时必须显式保留 allow_default_token=true，
        # 否则 bootstrap 的另一条 bail（无密码可用）同样会让网关起不来。
        if ([string]::IsNullOrEmpty($factoryPw)) {
            $factoryAllowDefault = $null
            if ($null -ne $factory.PSObject.Properties["web"] -and
                $null -ne $factory.web.PSObject.Properties["allow_default_token"]) {
                $factoryAllowDefault = $factory.web.allow_default_token
            }
            if ($factoryAllowDefault -ne $true) {
                Fail "$factoryRel 的 password_hash 为空但 allow_default_token 不为 true —— 模板会让网关因「无密码可用」拒绝启动"
            }
            else {
                Ok "$factoryRel 保留 allow_default_token=true（password_hash 为空时的必要条件）"
            }
        }

# 安装链路：install.sh 在包内没有现场 config.json 时要能用模板兜底落盘，
# 且必须保证「包内 config.json（现场定制）优先于模板」。
$installText = ReadText "gateway-core/deploy/install.sh"
if ($null -eq $installText) {
    Fail "找不到 gateway-core/deploy/install.sh"
}
else {
    $iCustom = $installText.IndexOf('if [ -f "$SCRIPT_DIR/config.json" ]')
    $iFactory = $installText.IndexOf('if [ -f "$SCRIPT_DIR/config.factory.json" ]')
    if ($iFactory -lt 0) {
        Fail "install.sh 没有用 config.factory.json 兜底 —— 包内无 config.json 时交付方只能从零手写配置"
    }
    elseif ($iCustom -lt 0 -or $iCustom -gt $iFactory) {
        Fail "install.sh 里出厂模板的判断出现在包内 config.json 之前（或缺失）—— 现场定制的 config.json 会被模板覆盖"
    }
    else {
        Ok "install.sh 用出厂模板兜底，且包内 config.json 优先于模板"
    }
}
Write-Host ""
Write-Host ""
Write-Host ""

# ══════════════════════════════════════════════════════════
# C21  形态边界声明（数据唯一副本 + 非目标清单）
#
# 背景：本产品形态是**单机工业边缘网关**（无云平台、无双机热备）。
# 这些不是"还没来得及做"，而是权衡后的产品边界——但边界只存在于
# 口头共识里时，客户默认会以为"系统有备份""能不能两台互备"。
# 现场最贵的纠纷不是 bug，是预期落差。
#
# 边界声明如果只写在文档里而没有落到**客户看得见的地方**（交付检查单），
# 等于没写。所以这里钉死三件事：
#   ① 文档层：data-flow.md 有「数据留存与恢复边界」，constraints.md 有「非目标声明」
#   ② 交付层：verify-deploy.sh 真的检查数据留存、真的把边界写进导出的检查单
#   ③ 一致性：文档里写的路径 / 导出端点与代码、路由真实存在
#
# 删掉任一处都会被本段抓住——这是防止"边界被静默抹掉"的保险丝。
Write-Host "[C21] 形态边界声明（数据唯一副本 + 非目标清单）" -ForegroundColor Cyan

$dataFlowText = ReadText "docs/architecture/data-flow.md"
if ($null -eq $dataFlowText) {
    Fail "找不到 docs/architecture/data-flow.md —— 数据留存边界无处声明"
}
else {
    if ($dataFlowText -match '(?m)^##\s*7\.3\s+数据留存与恢复边界') {
        Ok "data-flow.md 有「数据留存与恢复边界」章节"
    }
    else {
        Fail "data-flow.md 缺「7.3 数据留存与恢复边界」章节 —— 历史数据唯一副本的边界无处声明"
    }
    # 必须讲清「唯一副本」与「不可恢复」，否则声明等于没说
    $needPhrases = @(
        @{ p = "唯一副本"; why = "客户必须知道历史数据没有第二个副本" },
        @{ p = "不可恢复"; why = "物理损坏/整机丢失的后果必须写明" },
        @{ p = "data_queue.db"; why = "必须点名实际存放历史数据的库" }
    )
    foreach ($np in $needPhrases) {
        if ($dataFlowText.Contains($np.p)) {
            Ok "data-flow.md 的边界声明含「$($np.p)」"
        }
        else {
            Fail "data-flow.md 的边界声明缺「$($np.p)」—— $($np.why)"
        }
    }
}

$constraintsText = ReadText "docs/architecture/constraints.md"
if ($null -eq $constraintsText) {
    Fail "找不到 docs/architecture/constraints.md —— 非目标声明无处安放"
}
else {
    if ($constraintsText -match '(?m)^##\s*11\.\s*非目标声明') {
        Ok "constraints.md 有「非目标声明」章节"
    }
    else {
        Fail "constraints.md 缺「11. 非目标声明」章节 —— 形态边界只存在于口头共识里"
    }
    # 四条形态边界必须齐全：漏一条就等于该边界没写
    $nonGoals = @("云平台", "双机热备", "集群", "多租户")
    $missingGoals = @()
    foreach ($g in $nonGoals) {
        if (-not $constraintsText.Contains($g)) { $missingGoals += $g }
    }
    if ($missingGoals.Count -gt 0) {
        Fail "constraints.md 的非目标声明缺: $($missingGoals -join ', ') —— 这些边界漏写就会在交付时变成预期落差"
    }
    else {
        Ok "constraints.md 的非目标声明覆盖 $($nonGoals -join ', ')"
    }
    # 每条非目标必须带「重新评估触发条件」，否则会被当成待补的缺陷
    if ($constraintsText -match '重新评估触发条件') {
        Ok "constraints.md 的非目标声明带「重新评估触发条件」列"
    }
    else {
        Fail "constraints.md 的非目标声明没有「重新评估触发条件」列 —— 边界会退化成「还没做」而非设计决策"
    }
}

# 交付层：verify-deploy.sh 必须真的检查数据留存，且把边界写进导出的检查单
$verifyText = ReadText "deploy/verify-deploy.sh"
if ($null -eq $verifyText) {
    Fail "找不到 deploy/verify-deploy.sh"
}
else {
    if ($verifyText -match '(?m)^\s*echo\s+"\$\{BOLD\}\[1g\]') {
        Ok "verify-deploy.sh 有 [1g] 数据留存检查组"
    }
    else {
        Fail "verify-deploy.sh 缺 [1g] 数据留存检查组 —— 客户签字的检查单上没有数据留存结论"
    }
    if ($verifyText.Contains('数据留存与恢复边界')) {
        Ok "verify-deploy.sh 的导出检查单含「数据留存与恢复边界」段落"
    }
    else {
        Fail "verify-deploy.sh 的导出检查单缺「数据留存与恢复边界」段落 —— 边界只写在内部文档，客户看不到"
    }
    # 检查单里必须点名导出端点，审计场景才知道往哪儿导
    if ($verifyText.Contains('/api/history/export')) {
        Ok "verify-deploy.sh 的边界段落点名了导出端点 /api/history/export"
    }
    else {
        Fail "verify-deploy.sh 的边界段落没提导出端点 —— 有审计要求的客户不知道该往哪儿导数据"
    }
}

# 一致性：文档与脚本里的数据路径必须等于代码里的默认值
$pathsText = ReadText "gateway-core/src/support/paths.rs"
if ($null -eq $pathsText) {
    Fail "找不到 gateway-core/src/support/paths.rs —— 无法核对数据路径"
}
else {
    $dbDefault = $null
    if ($pathsText -match '(?s)default_db_path.*?"(/[^\"]*data_queue\.db)"') {
        $dbDefault = $Matches[1]
    }
    if ($null -eq $dbDefault) {
        Fail "paths.rs 里解析不出 default_db_path 的 Linux 默认值"
    }
    else {
        if ($verifyText -and $verifyText.Contains($dbDefault)) {
            Ok "verify-deploy.sh 的数据路径与代码 default_db_path 一致（$dbDefault）"
        }
        else {
            Fail "verify-deploy.sh 的数据路径与代码 default_db_path（$dbDefault）不一致 —— 脚本会检查到一个不存在的库"
        }
        if ($dataFlowText -and $dataFlowText.Contains($dbDefault)) {
            Ok "data-flow.md 的边界声明与代码 default_db_path 一致（$dbDefault）"
        }
        else {
            Fail "data-flow.md 的边界声明里的库路径与代码 default_db_path（$dbDefault）不一致 —— 文档指向的库不是实际存数据的库"
        }
    }
}

# 一致性：导出的端点必须真实存在于路由表
$histRoutes = ReadText "gateway-api/src/web/api/history/routes.rs"
if ($null -eq $histRoutes) {
    Fail "找不到 gateway-api/src/web/api/history/routes.rs"
}
elseif ($histRoutes.Contains('"/api/history/export"')) {
    Ok "导出端点 /api/history/export 在路由表中真实存在"
}
else {
    Fail "路由表里没有 /api/history/export —— 文档与检查单承诺的导出入口不存在"
}

# ══════════════════════════════════════════════════════════
# C22  engine 层不得反向依赖业务层（依赖倒置守护）
#
# 背景：engine 是基础设施层（数据模型 / 指标 / 报警判定 / 磁盘监控），
# 北向输出(north)、数据队列(queue)、处理管道(pipeline) 都是它的**下游**。
# 但三者都需要 engine 的核心模型，于是历史上形成了三处成环：
#   engine ↔ north    （报警引擎直接持有 MqttClient 上报）
#   engine ↔ queue    （磁盘监控直接持有 DataQueue 开关入队）
#   engine ↔ pipeline （报警引擎订阅定义在 pipeline 里的 RealtimeBus）
#
# O-4 用依赖倒置消除这三处：契约抽到 engine::ports（AlarmPublisher /
# DiskHealthSink）与 engine::bus（RealtimeBus），由 north / queue 实现，
# pipeline 侧保留重导出以兼容既有调用点。改造后 engine 只允许依赖 config。
#
# 本段是保险丝：Rust 编译器**不会**因为依赖方向回退而报警——环会悄悄长回来。
# 所以必须用门禁把方向钉死：任何人再把 `crate::north|queue|pipeline`
# 写进 engine/，本段立刻转红。
Write-Host "[C22] engine 层无反向依赖（依赖倒置守护）" -ForegroundColor Cyan

$engineRoot = Join-Repo "gateway-core/src/engine"
if (-not (Test-Path $engineRoot)) {
    Fail "找不到 gateway-core/src/engine —— 无法核对依赖方向"
}
else {
    # 去掉注释后扫描：文档里的 rustdoc 链接（如 `[crate::pipeline::vformula]`）
    # 不是代码依赖，不该被判违规。
    $violations = @()
    Get-ChildItem $engineRoot -Recurse -Filter "*.rs" -File | ForEach-Object {
        $rel = $_.FullName.Substring($repoRoot.Length + 1)
        $code = Remove-RustComments (Get-Content $_.FullName -Raw)
        $lineNo = 0
        foreach ($line in ($code -split "`r?`n")) {
            $lineNo++
            if ($line -match 'crate::(north|queue|pipeline)\b') {
                $violations += ("{0}:{1}: {2}" -f $rel, $lineNo, $line.Trim())
            }
        }
    }
    if ($violations.Count -eq 0) {
        Ok "engine/ 下无 crate::north|queue|pipeline 反向依赖"
    }
    else {
        Fail ("engine/ 出现反向依赖（已消除的环会重新长回来）：" + ($violations -join " ; "))
    }
}

# 契约层必须真实存在（否则上面那条"无反向依赖"可能是因为把实现整个删了）
$portsText = ReadText "gateway-core/src/engine/ports.rs"
if ($null -eq $portsText) {
    Fail "找不到 gateway-core/src/engine/ports.rs —— 依赖倒置的契约层没了"
}
else {
    foreach ($tn in @("AlarmPublisher", "DiskHealthSink")) {
        if ($portsText -match ("pub\s+trait\s+" + $tn + "\b")) {
            Ok "engine/ports.rs 定义了 trait $tn"
        }
        else {
            Fail "engine/ports.rs 缺 trait $tn —— 契约层被掏空，engine 与下游只能重新直接耦合"
        }
    }
}

# 契约必须有人实现（trait 存在但无人实现 = 抽象悬空）
$queueText = ReadText "gateway-core/src/queue/mod.rs"
if ($queueText -match 'impl\s+crate::engine::ports::DiskHealthSink\s+for\s+DataQueue\b') {
    Ok "queue 侧实现了 DiskHealthSink（engine ↔ queue 的环被切断）"
}
else {
    Fail "queue 侧没有实现 DiskHealthSink —— engine 的磁盘监控只能直接依赖 DataQueue"
}

$mqttPubText = ReadText "gateway-core/src/north/mqtt/alarm_publisher.rs"
if ($null -eq $mqttPubText) {
    Fail "找不到 gateway-core/src/north/mqtt/alarm_publisher.rs —— north 侧的报警上报契约实现没了"
}
elseif ($mqttPubText -match 'impl\s+AlarmPublisher\s+for\s+MqttClient') {
    Ok "north/mqtt 侧实现了 AlarmPublisher（engine ↔ north 的环被切断）"
}
else {
    Fail "north/mqtt 侧没有实现 AlarmPublisher —— 报警引擎只能直接依赖 MqttClient"
}

# 广播总线的归属：定义必须在 engine/bus.rs，pipeline 只保留重导出
$busText = ReadText "gateway-core/src/engine/bus.rs"
if ($null -eq $busText) {
    Fail "找不到 gateway-core/src/engine/bus.rs —— RealtimeBus 的中立归属没了"
}
elseif ($busText -match 'pub\s+struct\s+RealtimeBus') {
    Ok "RealtimeBus 定义在 engine/bus.rs（engine ↔ pipeline 的环被切断）"
}
else {
    Fail "engine/bus.rs 里没有 RealtimeBus 定义 —— 总线归属回退到业务层"
}

$pipelineText = ReadText "gateway-core/src/pipeline/mod.rs"
if ($null -eq $pipelineText) {
    Fail "找不到 gateway-core/src/pipeline/mod.rs"
}
elseif ($pipelineText -match 'pub\s+use\s+crate::engine::bus::') {
    Ok "pipeline 侧重导出 engine/bus 的总线契约（既有调用点保持兼容）"
}
else {
    Fail "pipeline 侧没有重导出 engine/bus 的总线契约 —— RealtimeBus 的归属又回到了 pipeline"
}

# ── C23  巨型文件拆分守护（O-6） ──
#
# 背景：engine 下有三个 800~1000 行的巨型文件——
#   engine/metrics/scoped.rs       （944 行）
#   engine/monitor/remote_diag.rs  （999 行）
#   engine/monitor/host.rs         （902 行）
# 单文件过千行时，改一个函数要在几百行里定位上下文；review 时也看不出
# 「哪些是业务逻辑、哪些是平台差异、哪些是测试」，回归面靠肉眼兜不住。
#
# O-6 把它们按关注点拆成模块目录（对外 API 与行为零变化），
# 这里用门禁钉死拆分结果不被悄悄合回去：
#   ① 三个主文件行数均 < 500
#   ② 拆出的子模块文件真实存在（防止"删了子模块、把代码塞回主文件"）
#   ③ 主文件确实以 mod 声明引用子模块（防止子模块变成孤儿文件）
Write-Host "[C23] 巨型文件拆分守护（三个 800+ 行文件均 < 500 行且子模块在位）" -ForegroundColor Cyan

# 统计真实行数：
#   - 不用 Measure-Object -Line（它会漏算空行，口径偏小）；
#   - 也不用 Get-Content（Windows PowerShell 5.1 默认按 ANSI/GBK 解码 UTF-8，
#     中文注释会被误解码成含 0x0A 的字节序列，行数被低估 10% 以上）；
#   - 用 [System.IO.File]::ReadAllLines：按 UTF-8 解码，行数与编辑器一致，
#     且 Windows PowerShell 5.1 / PowerShell 7 行为相同（门禁口径不随机器变）。
function Get-RustLineCount([string]$relPath) {
    $full = Join-Repo $relPath
    if (-not (Test-Path $full)) { return -1 }
    return [System.IO.File]::ReadAllLines($full).Count
}

$splitTargets = @(
    @{ Main = "gateway-core/src/engine/metrics/scoped.rs";      Limit = 500 },
    @{ Main = "gateway-core/src/engine/monitor/remote_diag.rs"; Limit = 500 },
    @{ Main = "gateway-core/src/engine/monitor/host.rs";        Limit = 500 }
)
foreach ($t in $splitTargets) {
    $n = Get-RustLineCount $t.Main
    if ($n -lt 0) {
        Fail ("找不到 " + $t.Main + " —— 拆分目标文件缺失")
    }
    elseif ($n -ge $t.Limit) {
        Fail ($t.Main + " 有 $n 行（≥ $($t.Limit)）—— 巨型文件拆分被合回去了")
    }
    else {
        Ok ($t.Main + " 共 $n 行（< $($t.Limit)）")
    }
}

# 拆出的子模块必须真实存在：只查主文件行数不够——把代码塞回主文件、
# 同时删掉子模块，也能让行数检查通过（但主文件就超了）。
# 所以这里逐个核对子模块文件在位。
$splitSubmodules = @(
    @{ Main = "gateway-core/src/engine/metrics/scoped.rs";      Subs = @("scoped/tests.rs") },
    @{ Main = "gateway-core/src/engine/monitor/remote_diag.rs"; Subs = @("remote_diag/log_archive.rs", "remote_diag/tail.rs", "remote_diag/shell.rs", "remote_diag/tests.rs") },
    @{ Main = "gateway-core/src/engine/monitor/host.rs";        Subs = @("host/parsers.rs", "host/linux.rs", "host/windows.rs", "host/other.rs", "host/tests.rs") }
)
foreach ($s in $splitSubmodules) {
    $baseDir = Split-Path -Parent $s.Main
    $missing = @()
    foreach ($sub in $s.Subs) {
        $subPath = Join-Repo ($baseDir + "/" + $sub)
        if (-not (Test-Path $subPath)) { $missing += $sub }
    }
    if ($missing.Count -gt 0) {
        Fail ($s.Main + " 的子模块缺失: $($missing -join ', ') —— 拆分结果不完整")
    }
    else {
        Ok ($s.Main + " 的 $($s.Subs.Count) 个子模块均在位")
    }
}

# 主文件必须以 mod 声明真正引用子模块：否则子模块会变成没人用的孤儿文件
# （Rust 会因未使用的 mod 报警，但若有人把它塞进别的模块，孤儿就静默了）。
$splitModDecls = @(
    @{ Main = "gateway-core/src/engine/metrics/scoped.rs";      Mods = @("tests") },
    @{ Main = "gateway-core/src/engine/monitor/remote_diag.rs"; Mods = @("log_archive", "tail", "shell", "tests") },
    @{ Main = "gateway-core/src/engine/monitor/host.rs";        Mods = @("parsers", "linux", "windows", "other", "tests") }
)
foreach ($m in $splitModDecls) {
    $text = ReadText $m.Main
    if ($null -eq $text) {
        Fail ("找不到 " + $m.Main)
        continue
    }
    $absent = @()
    foreach ($modName in $m.Mods) {
        if ($text -notmatch ("(?m)^\s*(pub\s+)?mod\s+" + $modName + "\s*;")) { $absent += $modName }
    }
    if ($absent.Count -gt 0) {
        Fail ($m.Main + " 未声明 mod: $($absent -join ', ') —— 子模块成了孤儿文件")
    }
    else {
        Ok ($m.Main + " 正确声明了 $($m.Mods.Count) 个子模块")
    }
}

# 平台分发别名：host.rs 用 platform:: 统一调用点，三个平台实现签名必须一致。
# 缺了别名，host.rs 就得在每个调用点写 #[cfg]，跨平台分支很容易漏。
$hostText = ReadText "gateway-core/src/engine/monitor/host.rs"
if ($null -eq $hostText) {
    Fail "找不到 gateway-core/src/engine/monitor/host.rs"
}
else {
    $aliasMissing = @()
    foreach ($alias in @("use self::linux as platform;", "use self::windows as platform;", "use other as platform;")) {
        if ($hostText -notmatch [regex]::Escape($alias)) { $aliasMissing += $alias }
    }
    if ($aliasMissing.Count -gt 0) {
        Fail ("host.rs 缺平台分发别名: " + ($aliasMissing -join ', ') + " —— 跨平台调用点会散落成零散 cfg")
    }
    else {
        Ok "host.rs 用 platform:: 别名统一分发三平台实现"
    }
}

Write-Host ""
Write-Host ""
Write-Host ""

# ── C24  行数预算：不允许新增巨型文件（O-8） ──
#
# 前端已经由 eslint 的 max-lines 强制（gateway-web/eslint.config.js）：
# 未登记的新文件超过 500 行即告警，CI 用 `--max-warnings 0` 跑 lint → 等于阻断。
# 后端历史上巨型文件较多，一上来全报会把门禁变成噪音，
# 所以这里做「只减不增」的棘轮（基线每行带单文件行数上限 = 登记时行数 + 余量）：
#   ① 未登记且 > 500 行                → FAIL（新债，必须拆）
#   ② 已登记但超过基线记录的行数       → FAIL（存量只能瘦身，不许继续长）
#   ③ 已登记但已降到 ≤ 500 行          → OK（提示可以从基线移除）
#   ④ 已登记但文件不存在（改名/搬迁）  → WARN（基线腐烂）
Write-Host "[C24] 行数预算：不允许新增巨型文件（> 500 行），且存量只减不增" -ForegroundColor Cyan

$lineBudget = 500
# 基线上限 = 登记时行数 + 该余量（生成基线时用同一数值）。
# 判定「已瘦身」必须扣掉余量，否则每个文件都会常驻低于上限 20 行而被误报。
$lineBudgetMargin = 20
$frozenRel = "scripts/line_budget_frozen.txt"
$frozenFull = Join-Repo $frozenRel
$frozen = @{}
if (-not (Test-Path $frozenFull)) {
    Fail ("缺少行数预算基线 " + $frozenRel + " —— 没有基线就无法区分新旧债务")
}
else {
    $malformed = @()
    foreach ($line in [System.IO.File]::ReadAllLines($frozenFull)) {
        $t = $line.Trim()
        if ($t -eq "" -or $t.StartsWith("#")) { continue }
        # 格式：<仓库相对路径><TAB><行数上限>。只登记路径不给上限，
        # 等于给这个文件永久豁免——存量可以悄悄长回巨型文件而门禁全绿。
        $sep = $t.LastIndexOf("`t")
        if ($sep -le 0) { $malformed += $t; continue }
        $rel = $t.Substring(0, $sep).Trim()
        $capText = $t.Substring($sep + 1).Trim()
        if ($capText -notmatch '^\d+$') { $malformed += $t; continue }
        $frozen[$rel] = [int]$capText
    }
    if ($malformed.Count -gt 0) {
        Fail ("行数预算基线格式错误 $($malformed.Count) 行（应为 路径<TAB>行数上限）：" + ($malformed -join ", "))
    }
    Ok ("行数预算基线已载入：$($frozen.Count) 个已知巨型文件（带单文件上限）")
}

# 扫描范围：三个后端 crate 的业务源码。
# 测试文件不计——测试本来就该长，而且不进产品二进制。
$oversized = @()
foreach ($crate in @("gateway-core", "gateway-api", "gatewayd")) {
    $srcRoot = Join-Repo ($crate + "/src")
    if (-not (Test-Path $srcRoot)) { continue }
    foreach ($f in Get-ChildItem -Recurse -Path $srcRoot -Filter *.rs) {
        $rel = $crate + "/src/" + ($f.FullName.Substring($srcRoot.Length + 1) -replace '\\', '/')
        if ($rel -match '/tests?/') { continue }
        if ($f.Name -match '(^|_)tests?\.rs$') { continue }
        $n = [System.IO.File]::ReadAllLines($f.FullName).Count
        if ($n -gt $lineBudget) {
            $oversized += [pscustomobject]@{ Rel = $rel; Lines = $n }
        }
    }
}

$newGiants = @($oversized | Where-Object { -not $frozen.ContainsKey($_.Rel) })
if ($newGiants.Count -gt 0) {
    $detail = ($newGiants | ForEach-Object { "$($_.Rel)($($_.Lines) 行)" }) -join ", "
    Fail ("新增巨型文件 $($newGiants.Count) 个（> $lineBudget 行）：$detail —— 请拆分；确属必要的登记进 $frozenRel")
}
else {
    Ok "行数预算：没有新增巨型文件"
}

# 存量棘轮：基线里的文件只能瘦身。少了这条，「已登记」就变成永久豁免。
$grown = @()
foreach ($rel in ($frozen.Keys | Sort-Object)) {
    $full = Join-Repo $rel
    if (-not (Test-Path $full)) { continue }
    $n = [System.IO.File]::ReadAllLines($full).Count
    if ($n -gt $frozen[$rel]) { $grown += "$rel($n 行 > 上限 $($frozen[$rel]) 行)" }
}
if ($grown.Count -gt 0) {
    Fail ("存量巨型文件继续增长 $($grown.Count) 个：" + ($grown -join ", ") + " —— 先拆再提交，不要上调基线")
}
else {
    Ok "行数预算：存量巨型文件未增长"
}

# 基线核对：拆够了要提示移除，文件没了要提示失效——否则基线会腐烂成一份假账。
$shrunk = @()
$tighten = @()
$stale = @()
foreach ($rel in ($frozen.Keys | Sort-Object)) {
    $full = Join-Repo $rel
    if (-not (Test-Path $full)) { $stale += $rel; continue }
    $n = [System.IO.File]::ReadAllLines($full).Count
    if ($n -le $lineBudget) { $shrunk += "$rel($n 行)" }
    elseif ($n -lt ($frozen[$rel] - $lineBudgetMargin)) {
        $tighten += "$rel(上限 $($frozen[$rel]) → 当前 $n 行)"
    }
}
if ($stale.Count -gt 0) {
    Warn ("行数预算基线条目失效 $($stale.Count) 个（文件不存在）：" + ($stale -join ", "))
}
if ($shrunk.Count -gt 0) {
    Ok ("行数预算：$($shrunk.Count) 个文件已降到 $lineBudget 行以内，可从基线移除：" + ($shrunk -join ", "))
}
if ($tighten.Count -gt 0) {
    Ok ("行数预算：$($tighten.Count) 个文件已瘦身，可把基线上限下调到当前行数：" + ($tighten -join ", "))
}

Write-Host ""
Write-Host ""


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
