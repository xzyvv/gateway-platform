<#
.SYNOPSIS
    ARM 交叉编译门禁：在 Windows 开发机上验证 ARM 产物能编译。

.DESCRIPTION
    产物部署到 ARM 工业设备，但 CI 的 cross-arm 作业是「手动触发 / RUN_HEAVY」
    的重作业，且 GitHub Actions 额度已耗尽 → **ARM 代码路径长期没有被编译验证过**。
    ARM 与 x86_64 的差异不是「同一份代码换个编译参数」：

      - `cfg(target_arch = "aarch64")` 代码块在 x86_64 编译中根本不参与；
      - `usize`/`isize` 宽度、原子指令可用性、指针宽度相关 API 会报错；
      - 部分 C 依赖（cdivsufsort、aws-lc-sys 等）的 build.rs 只在 ARM 上走
        不同的源码分支，x86_64 编译不覆盖。

    本脚本用 zig 作为交叉 C 编译器/链接器，对 **实际部署的两个 ARM 架构**
    做 `cargo check --all-targets`：

      - aarch64-unknown-linux-gnu（64 位 ARM，真机主流）
      - armv7-unknown-linux-gnueabihf（32 位 ARM，老设备）

    与 linux_cross_check.ps1 的区别：那边验证「Linux 内核接口」，
    这边验证「ARM 架构差异」——两者互补，都不可省略。

    缺少 zig / 未安装对应 rust target 时**跳过**（exit 0），不阻塞其它门禁；
    用 -Strict 可改成失败。

.PARAMETER Crates
    要验证的 crate 列表。默认 gateway-core / gateway-api / gatewayd。

.PARAMETER Features
    传给 cargo 的 features。默认 fins（与本地其它门禁口径一致）。

.PARAMETER Targets
    要验证的 target 列表。默认两个 ARM target。

.PARAMETER Strict
    缺能力时返回 1（失败）而不是 0（跳过）。

.PARAMETER Probe
    只做能力探测，不执行编译。退出码 0 = 就绪，3 = 不可用（并打印原因）。
    供 release_gate.ps1 决定该步骤是「运行」还是「SKIP」。

.EXAMPLE
    powershell -File scripts/arm_cross_check.ps1
    powershell -File scripts/arm_cross_check.ps1 -Targets aarch64-unknown-linux-gnu
#>
param(
    [string[]]$Crates = @("gateway-core", "gateway-api", "gatewayd"),
    [string]$Features = "fins",
    [string[]]$Targets = @("aarch64-unknown-linux-gnu", "armv7-unknown-linux-gnueabihf"),
    [switch]$Strict,
    [switch]$Probe
)

$ErrorActionPreference = "Continue"
$repoRoot = Split-Path -Parent $PSScriptRoot

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

# 仓库用 rust-toolchain.toml 锁定了版本（如 1.98.1），该版本与本机 `stable`
# 是**两个不同的 toolchain 目录**。rustup target add 不带 --toolchain 时装到
# default（stable）上，编译时 rustc 仍按 rust-toolchain.toml 找 1.98.1 的
# sysroot → 报 "can't find crate for `core`"。
# 所以必须把 target 装到**仓库实际使用的那个 toolchain** 上。
function Get-RepoToolchain {
    $tcFile = Join-Path $repoRoot "rust-toolchain.toml"
    if (Test-Path $tcFile) {
        $line = Select-String -Path $tcFile -Pattern '^\s*channel\s*=\s*"([^"]+)"' |
            Select-Object -First 1
        if ($line) { return $line.Matches[0].Groups[1].Value }
    }
    return "stable"
}

$zig = Find-Zig
if ($null -eq $zig) {
    Write-Host "[arm-check] 未检测到 zig —— 跳过 ARM 交叉编译验证。" -ForegroundColor Yellow
    Write-Host "[arm-check] 安装：https://ziglang.org/download/ （或 winget install ziglang.zig）" -ForegroundColor Yellow
    if ($Probe) { exit 3 }
    if ($Strict) { exit 1 }
    exit 0
}

$toolchain = Get-RepoToolchain
$installed = & rustup target list --toolchain $toolchain --installed 2>&1
$missing = @($Targets | Where-Object { @($installed) -notcontains $_ })

if ($missing.Count -gt 0) {
    Write-Host "[arm-check] toolchain '$toolchain' 缺少 target: $($missing -join ', ') —— 跳过。" -ForegroundColor Yellow
    Write-Host "[arm-check] 安装：rustup target add --toolchain $toolchain $($missing -join ' ')" -ForegroundColor Yellow
    if ($Probe) { exit 3 }
    if ($Strict) { exit 1 }
    exit 0
}

if ($Probe) {
    Write-Host "[arm-check] 就绪（zig: $zig, toolchain: $toolchain, targets: $($Targets -join ', ')）" -ForegroundColor Green
    exit 0
}

# ===== 编译包装器（规避 cmd.exe 8191 字符上限）=====
$workDir = Join-Path $repoRoot "target\arm-cross-check"
New-Item -ItemType Directory -Force -Path $workDir | Out-Null
$wrapperExe = Join-Path $workDir "zigwrap.exe"
$wrapperSrc = Join-Path $PSScriptRoot "linux\zigwrap.rs"

if (-not (Test-Path $wrapperExe) -or
    (Get-Item $wrapperExe).LastWriteTime -lt (Get-Item $wrapperSrc).LastWriteTime) {
    Write-Host "[arm-check] 编译 zig 包装器 ..." -ForegroundColor Cyan
    & rustc -O $wrapperSrc -o $wrapperExe
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[arm-check] 包装器编译失败" -ForegroundColor Red
        exit 1
    }
}

# cc-rs 用相对路径调用会失败（工作目录随 crate 变化），必须给绝对路径
$wrapperAbs = (Resolve-Path $wrapperExe).Path

$failed = @()
Push-Location $repoRoot
try {
    foreach ($target in $Targets) {
        Write-Host "[arm-check] cargo check --target $target （crate: $($Crates -join ', ')）" -ForegroundColor Cyan

        $env:ZIG_EXE = $zig
        # cc-rs 约定：CC_<target> / AR_<target>（target 用连字符形式）
        Set-Item -Path "Env:CC_$target" -Value $wrapperAbs
        Set-Item -Path "Env:AR_$target" -Value $wrapperAbs
        # cargo 约定：CARGO_TARGET_<TRIPLE>_LINKER，三元组的连字符必须换成下划线，
        # 否则 cargo 会忽略该变量（并在 stderr 给出 warning），链接会回落到默认 linker 而失败。
        $linkerEnvKey = "CARGO_TARGET_$($target.Replace('-', '_').ToUpper())_LINKER"
        Set-Item -Path "Env:$linkerEnvKey" -Value $wrapperAbs

        $cargoArgs = @("check", "--target", $target, "--all-targets", "--features", $Features)
        foreach ($c in $Crates) { $cargoArgs += @("-p", $c) }

        & cargo @cargoArgs
        $code = $LASTEXITCODE
        if ($code -eq 0) {
            Write-Host "[arm-check] $target 通过" -ForegroundColor Green
        }
        else {
            Write-Host "[arm-check] $target 失败（exit $code）" -ForegroundColor Red
            $failed += $target
        }
    }
}
finally {
    Pop-Location
}

if ($failed.Count -gt 0) {
    Write-Host "[arm-check] 失败 target: $($failed -join ', ')" -ForegroundColor Red
    exit 1
}
Write-Host "[arm-check] 全部通过" -ForegroundColor Green
exit 0
