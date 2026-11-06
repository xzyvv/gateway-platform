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

    三种验证深度（互斥选用）：

      - 默认      `cargo check --all-targets` —— 只做类型检查，最快
      - -Release  `cargo build --release` + ELF/静态链接/GLIBC 校验
      - -TestNoRun `cargo test --no-run` —— 编译**测试二进制**并校验 ELF。
        check 阶段不产出可执行文件，测试代码里的架构假设（指针宽度、
        libc 结构体布局、`#[cfg(target_arch = "aarch64")]` 分支）
        直到链接阶段才暴露。

    与 linux_cross_check.ps1 的区别：那边验证「Linux 内核接口」，
    这边验证「ARM 架构差异」——两者互补，都不可省略。

    缺少 zig / 未安装对应 rust target 时**跳过**（exit 0），不阻塞其它门禁；
    用 -Strict 可改成失败。

.PARAMETER Crates
    要验证的 crate 列表。默认 gateway-core / gateway-api / gatewayd。

.PARAMETER MuslOnly
    只验证静态 musl 产物（现场首选，免 glibc 版本耦合）。

.PARAMETER Release
    构建 release 二进制并校验 ELF 头 / 静态链接 / GLIBC 引用。
    比 cargo check 更重（完整链接），但能证明「产物真的出得来」。

.PARAMETER TestNoRun
    编译测试二进制（`cargo test --no-run`）并校验 ELF 头 / 静态链接。
    `cargo check --all-targets` 只做类型检查，**不产出可执行文件**——测试代码里
    的架构假设（指针宽度、`libc` 结构体布局、`#[cfg(target_arch = "aarch64")]`
    分支）在链接阶段才暴露。本开关把「测试代码能否在 ARM 上链接出可执行文件」
    纳入本地门禁。

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
    powershell -File scripts/arm_cross_check.ps1 -MuslOnly -Release
    powershell -File scripts/arm_cross_check.ps1 -MuslOnly -TestNoRun
#>
param(
    [string[]]$Crates = @("gateway-core", "gateway-api", "gatewayd"),
    [string]$Features = "fins",
    [string[]]$Targets = @("aarch64-unknown-linux-gnu", "armv7-unknown-linux-gnueabihf"),
    [switch]$MuslOnly,
    [switch]$Release,
    [switch]$TestNoRun,
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

# musl 静态产物：现场设备 glibc 版本千差万别（Debian10=2.28 / Debian11=2.31 /
# Yocto 常见 musl），gnu 动态产物带着构建机的 glibc 版本要求，在旧系统上直接
# 报 "GLIBC_2.xx not found" —— 这是现场最典型的「装上去跑不起来」。
# deploy/build-release.sh --target linux-arm64-musl 就是为免掉这层耦合。
if ($MuslOnly) {
    $Targets = @($Targets | Where-Object { $_ -match "musl" })
    if ($Targets.Count -eq 0) {
        Write-Host "[arm-check] -MuslOnly 但没有 musl target —— 跳过。" -ForegroundColor Yellow
        if ($Probe) { exit 3 }
        if ($Strict) { exit 1 }
        exit 0
    }
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

# rust 自带的 rust-lld（GNU flavor）。musl 静态链接需要它：
# rustc 在 aarch64 musl target 上会传 `-Wl,--fix-cortex-a53-843419`
# （Cortex-A53 erratum 缓解），zig cc 的 `-Wl,` 白名单不认这个参数会直接失败，
# 而 zig 内嵌的 lld 与 rust-lld 都支持它。改用 ld.lld flavor 直接驱动
# rust-lld 即可绕开白名单。
$rustLld = $null
$sysroot = (& rustc --print sysroot).Trim()
foreach ($cand in @(
        "$sysroot\lib\rustlib\x86_64-pc-windows-msvc\bin\rust-lld.exe",
        "$sysroot\lib\rustlib\x86_64-pc-windows-gnu\bin\rust-lld",
        "$sysroot\lib\rustlib\aarch64-unknown-linux-gnu\bin\rust-lld"
    )) {
    if (Test-Path $cand) { $rustLld = $cand; break }
}

$failed = @()
Push-Location $repoRoot
try {
    foreach ($target in $Targets) {
        $isMusl = $target -match "musl"
        $cargoVerb = if ($Release) { "build --release" } elseif ($TestNoRun) { "test --no-run" } else { "check --all-targets" }
        Write-Host "[arm-check] cargo $cargoVerb --target $target （crate: $($Crates -join ', ')）" -ForegroundColor Cyan

        $env:ZIG_EXE = $zig
        # cc-rs 约定：CC_<target> / AR_<target>（target 用连字符形式）
        Set-Item -Path "Env:CC_$target" -Value $wrapperAbs
        Set-Item -Path "Env:AR_$target" -Value $wrapperAbs

        if ($isMusl -and $null -ne $rustLld) {
            # musl：链接器用 rust-lld（C 编译仍用 zig cc）
            $linkerEnvKey = "CARGO_TARGET_$($target.Replace('-', '_').ToUpper())_LINKER"
            Set-Item -Path "Env:$linkerEnvKey" -Value $rustLld
            $env:RUSTFLAGS = "-C linker-flavor=ld.lld -C link-self-contained=yes"
        }
        else {
            # cargo 约定：CARGO_TARGET_<TRIPLE>_LINKER，三元组的连字符必须换成下划线，
            # 否则 cargo 会忽略该变量（并在 stderr 给出 warning），链接会回落到默认 linker 而失败。
            $linkerEnvKey = "CARGO_TARGET_$($target.Replace('-', '_').ToUpper())_LINKER"
            Set-Item -Path "Env:$linkerEnvKey" -Value $wrapperAbs
            $env:RUSTFLAGS = ""
        }

        if ($Release) {
            $cargoArgs = @("build", "--target", $target, "--release", "--features", $Features)
        }
        elseif ($TestNoRun) {
            # 编译测试二进制但不运行：`cargo check --all-targets` 只做类型检查，
            # 不产出可执行文件 → 测试代码里的架构假设（指针宽度、libc 结构体布局）
            # 直到链接阶段才暴露。
            $cargoArgs = @("test", "--target", $target, "--no-run", "--features", $Features)
        }
        else {
            $cargoArgs = @("check", "--target", $target, "--all-targets", "--features", $Features)
        }
        foreach ($c in $Crates) { $cargoArgs += @("-p", $c) }

        & cargo @cargoArgs
        $code = $LASTEXITCODE
        if ($code -ne 0) {
            Write-Host "[arm-check] $target 失败（exit $code）" -ForegroundColor Red
            $failed += $target
            continue
        }

        if ($TestNoRun) {
            # 产物校验：测试二进制必须真的存在、是 ARM ELF、musl 下必须静态链接。
            # cargo test --no-run 成功但没产物 = 门禁没生效，等于白跑。
            $depsDir = Join-Path $repoRoot "target\$target\debug\deps"
            if (-not (Test-Path $depsDir)) {
                Write-Host "[arm-check] $target 测试二进制目录缺失：$depsDir" -ForegroundColor Red
                $failed += $target
                continue
            }
            # Linux target 下可执行文件没有扩展名；deps 目录里的 .d / .rlib / .rmeta
            # 都带扩展名。因此「无扩展名 + ELF magic」= 测试可执行文件。
            # 这里刻意**全量校验**（含 chaos / soak / load 等集成测试二进制）：
            # 集成测试里的架构假设（指针宽度、libc 结构体布局）同样是 ARM 风险点。
            $testBins = Get-ChildItem $depsDir -File -ErrorAction SilentlyContinue |
                Where-Object { $_.Extension -eq "" -and $_.Length -gt 0 }
            if ($testBins.Count -eq 0) {
                Write-Host "[arm-check] $target 未产出测试二进制（$depsDir）—— cargo test --no-run 未真正链接" -ForegroundColor Red
                $failed += $target
                continue
            }

            $expected = if ($target -match "aarch64") { 0xB7 } else { 0x28 }
            $badBins = @()
            foreach ($bin in $testBins) {
                $bytes = [System.IO.File]::ReadAllBytes($bin.FullName)
                if ($bytes.Length -lt 0x18 -or
                    -not ($bytes[0] -eq 0x7F -and $bytes[1] -eq 0x45 -and $bytes[2] -eq 0x4C -and $bytes[3] -eq 0x46)) {
                    $badBins += "$($bin.Name)（非 ELF）"
                    continue
                }
                $machine = [BitConverter]::ToUInt16($bytes, 0x12)
                if ($machine -ne $expected) {
                    $badBins += "$($bin.Name)（machine=0x$('{0:X}' -f $machine)，期望 0x$('{0:X}' -f $expected)）"
                    continue
                }
                if ($isMusl) {
                    $text = [System.Text.Encoding]::ASCII.GetString($bytes)
                    if ($text.Contains(".dynamic")) {
                        $badBins += "$($bin.Name)（含 .dynamic 段，静态链接未生效）"
                        continue
                    }
                    $glibcRefs = [regex]::Matches($text, "GLIBC_2\.\d+") | ForEach-Object { $_.Value } | Sort-Object -Unique
                    if ($glibcRefs.Count -gt 0) {
                        $badBins += "$($bin.Name)（含 GLIBC 引用：$($glibcRefs -join ', ')）"
                        continue
                    }
                }
            }
            if ($badBins.Count -gt 0) {
                Write-Host "[arm-check] $target 测试二进制校验失败：" -ForegroundColor Red
                $badBins | ForEach-Object { Write-Host "    $_" -ForegroundColor Red }
                $failed += $target
                continue
            }
            $totalMB = [math]::Round((($testBins | Measure-Object -Property Length -Sum).Sum) / 1MB, 1)
            Write-Host "[arm-check] $target 测试二进制：$($testBins.Count) 个（合计 $totalMB MB），架构/静态链接校验通过" -ForegroundColor Cyan
        }

        if ($Release) {
            # 产物校验：架构要对、静态链接要成立、不能带 GLIBC 版本引用
            $binName = if ($Crates -contains "gatewayd") { "gatewayd" } else { $Crates[0] }
            $binPath = Join-Path $repoRoot "target\$target\release\$binName"
            if (-not (Test-Path $binPath)) {
                Write-Host "[arm-check] $target 产物缺失：$binPath" -ForegroundColor Red
                $failed += $target
                continue
            }
            $sizeMB = [math]::Round((Get-Item $binPath).Length / 1MB, 1)
            Write-Host "[arm-check] $target 产物：$binPath （$sizeMB MB）" -ForegroundColor Cyan

            $bytes = [System.IO.File]::ReadAllBytes($binPath)
            # ELF magic
            if (-not ($bytes[0] -eq 0x7F -and $bytes[1] -eq 0x45 -and $bytes[2] -eq 0x4C -and $bytes[3] -eq 0x46)) {
                Write-Host "[arm-check] $target 产物不是 ELF 文件" -ForegroundColor Red
                $failed += $target
                continue
            }
            # e_machine @ 0x12 (小端) / 0x28 (大端)
            $little = ($bytes[5] -eq 1)
            $machine = if ($little) { [BitConverter]::ToUInt16($bytes, 0x12) } else { [BitConverter]::ToUInt16($bytes, 0x12) }
            $expected = if ($target -match "aarch64") { 0xB7 } else { 0x28 }
            if ($machine -ne $expected) {
                Write-Host "[arm-check] $target 产物架构不符（machine=0x$('{0:X}' -f $machine)，期望 0x$('{0:X}' -f $expected)）" -ForegroundColor Red
                $failed += $target
                continue
            }
            Write-Host "[arm-check] $target 架构正确（machine=0x$('{0:X}' -f $machine)）" -ForegroundColor Green

            if ($isMusl) {
                # 静态链接：不应有 .dynamic 段
                $text = [System.Text.Encoding]::ASCII.GetString($bytes)
                $hasDynamic = $text.Contains(".dynamic")
                if ($hasDynamic) {
                    Write-Host "[arm-check] $target 产物含 .dynamic 段 —— 静态链接未生效" -ForegroundColor Red
                    $failed += $target
                    continue
                }
                # 静态 musl 产物不应带 GLIBC 版本引用
                $glibcRefs = [regex]::Matches($text, "GLIBC_2\.\d+") | ForEach-Object { $_.Value } | Sort-Object -Unique
                if ($glibcRefs.Count -gt 0) {
                    Write-Host "[arm-check] $target 产物含 GLIBC 版本引用：$($glibcRefs -join ', ') —— 静态 musl 链接未生效" -ForegroundColor Red
                    $failed += $target
                    continue
                }
                Write-Host "[arm-check] $target 静态链接确认（无 .dynamic 段、无 GLIBC 版本引用）" -ForegroundColor Green
            }
        }

        Write-Host "[arm-check] $target 通过" -ForegroundColor Green
    }
}
finally {
    Pop-Location
    $env:RUSTFLAGS = ""
}

if ($failed.Count -gt 0) {
    Write-Host "[arm-check] 失败 target: $($failed -join ', ')" -ForegroundColor Red
    exit 1
}
Write-Host "[arm-check] 全部通过" -ForegroundColor Green
exit 0
