<#
.SYNOPSIS
    Linux 交叉编译门禁：在 Windows 开发机上验证 Linux 专属代码仍可编译。

.DESCRIPTION
    产物部署到 Linux 嵌入式设备，而开发机是 Windows → 全仓
    `#[cfg(target_os = "linux")]` 代码块在常规 `cargo check` 中**根本不会被编译**。
    一旦这些代码块被改坏（如 API 变更、类型不匹配），要到真机编译时才会暴露，
    属于「现场才发现」的典型风险。

    本脚本用 zig 作为交叉 C 编译器/链接器，对三个**实际部署**的 crate 做
    `cargo check --target x86_64-unknown-linux-gnu --all-targets`。

    不包含 gateway-desktop（已冻结；其 Tauri 依赖的 libdbus-sys 需要 Linux sysroot，
    Windows 侧无交叉 sysroot）。

    缺少 zig 或未安装 linux target 时**跳过**（exit 0），不阻塞其它门禁。

.PARAMETER Crates
    要验证的 crate 列表。默认 gateway-core / gateway-api / gatewayd。

.PARAMETER Features
    传给 cargo 的 features。默认 fins（与本地其它门禁口径一致）。

.PARAMETER Probe
    只做能力探测，不执行编译。退出码 0 = 就绪，3 = 不可用（并打印原因）。
    供 release_gate.ps1 决定该步骤是「运行」还是「SKIP」。

.EXAMPLE
    powershell -File scripts/linux_cross_check.ps1
#>
param(
    [string[]]$Crates = @("gateway-core", "gateway-api", "gatewayd"),
    [string]$Features = "fins",
    [switch]$Strict,
    [switch]$Probe
)

$ErrorActionPreference = "Continue"
$repoRoot = Split-Path -Parent $PSScriptRoot
$target = "x86_64-unknown-linux-gnu"

# ===== 能力探测 =====
function Find-Zig {
    $cmd = Get-Command zig -ErrorAction SilentlyContinue
    if ($null -ne $cmd) { return $cmd.Source }

    # 已知布局：<root>/zig.exe、<root>/zig-<ver>/zig.exe、<root>/zig/zig-<ver>/zig.exe
    $roots = @(
        $env:LOCALAPPDATA,
        (Join-Path $env:LOCALAPPDATA "Programs"),
        "C:\Program Files",
        "C:\Program Files (x86)",
        $env:TEMP,
        $env:USERPROFILE
    ) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }

    foreach ($root in $roots) {
        # 深度 2 内的 zig.exe（限定目录名含 zig，避免命中无关同名文件）
        $hit = Get-ChildItem -Path $root -Filter "zig.exe" -Recurse -Depth 2 -ErrorAction SilentlyContinue |
            Where-Object { $_.DirectoryName -match "zig" } |
            Select-Object -First 1
        if ($hit) { return $hit.FullName }
    }
    return $null
}

$zig = Find-Zig
if ($null -eq $zig) {
    Write-Host "[linux-check] 未检测到 zig —— 跳过 Linux 交叉编译验证。" -ForegroundColor Yellow
    Write-Host "[linux-check] 安装：https://ziglang.org/download/ （或 winget install ziglang.zig）" -ForegroundColor Yellow
    if ($Probe) { exit 3 }
    if ($Strict) { exit 1 }
    exit 0
}

$installed = & rustup target list --installed 2>&1
if (@($installed) -notcontains $target) {
    Write-Host "[linux-check] 未安装 rust target $target —— 跳过。" -ForegroundColor Yellow
    Write-Host "[linux-check] 安装：rustup target add $target" -ForegroundColor Yellow
    if ($Probe) { exit 3 }
    if ($Strict) { exit 1 }
    exit 0
}

if ($Probe) {
    Write-Host "[linux-check] 就绪（zig: $zig）" -ForegroundColor Green
    exit 0
}

# ===== 编译包装器（规避 cmd.exe 8191 字符上限）=====
$workDir = Join-Path $repoRoot "target\linux-cross-check"
New-Item -ItemType Directory -Force -Path $workDir | Out-Null
$wrapperExe = Join-Path $workDir "zigwrap.exe"
$wrapperSrc = Join-Path $PSScriptRoot "linux\zigwrap.rs"

if (-not (Test-Path $wrapperExe) -or
    (Get-Item $wrapperExe).LastWriteTime -lt (Get-Item $wrapperSrc).LastWriteTime) {
    Write-Host "[linux-check] 编译 zig 包装器 ..." -ForegroundColor Cyan
    & rustc -O $wrapperSrc -o $wrapperExe
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[linux-check] 包装器编译失败" -ForegroundColor Red
        exit 1
    }
}

# ===== 执行交叉编译 =====
$env:ZIG_EXE = $zig
# 链接器模式必须显式告知目标：rustc 只在**编译阶段**传 `--target=`，
# 链接阶段不传。缺了它，zig 会按宿主(Windows)找 `util.dll`/`rt.dll`/`dl.dll`，
# 报 "unable to find dynamic system library 'util'" —— 看起来像缺依赖，
# 实际是目标三元组丢失。
$env:ZIG_TARGET = $target
# cc-rs 约定：CC_<target> / AR_<target>（target 用连字符形式）
Set-Item -Path "Env:CC_$target" -Value $wrapperExe
Set-Item -Path "Env:AR_$target" -Value $wrapperExe
# cargo 约定：CARGO_TARGET_<TRIPLE>_LINKER，三元组的连字符必须换成下划线，
# 否则 cargo 会忽略该变量（并在 stderr 给出 warning），链接会回落到默认 linker 而失败。
$linkerEnvKey = "CARGO_TARGET_$($target.Replace('-', '_').ToUpper())_LINKER"
Set-Item -Path "Env:$linkerEnvKey" -Value $wrapperExe

Write-Host "[linux-check] cargo check --target $target （crate: $($Crates -join ', ')）" -ForegroundColor Cyan
$cargoArgs = @("check", "--target", $target, "--all-targets", "--features", $Features)
foreach ($c in $Crates) { $cargoArgs += @("-p", $c) }

Push-Location $repoRoot
try {
    & cargo @cargoArgs
    $code = $LASTEXITCODE
}
finally {
    Pop-Location
}

if ($code -eq 0) {
    Write-Host "[linux-check] 通过" -ForegroundColor Green
}
else {
    Write-Host "[linux-check] 失败（exit $code）" -ForegroundColor Red
}
exit $code
