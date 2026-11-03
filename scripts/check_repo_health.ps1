# 仓库健康度扫描脚本（PowerShell 版本，本地 dev 用）
#
# 用途：与 check_repo_health.sh 相同的 4 项扫描，本地 Windows 跑方便
# 接入：手动执行 `pwsh scripts/check_repo_health.ps1` 或加入 pre-commit hook
#
# 退出码：0 = 通过 / 1 = 有阻断性失败

param(
    [switch]$Verbose
)

$ErrorActionPreference = "Stop"
Set-Location -Path (Resolve-Path "$PSScriptRoot/..")

$fail = $false
$warn = $false

Write-Host "=== 1. bare CR 检测（\r\r\n 损坏换行符）==="
$bareCrFiles = @()
$scanDirs = @("gateway-core/src", "gateway-api/src", "gateway-web/src")
foreach ($dir in $scanDirs) {
    if (Test-Path $dir) {
        Get-ChildItem -Path $dir -Recurse -File -Include "*.rs","*.ts","*.vue" -ErrorAction SilentlyContinue | ForEach-Object {
            $bytes = [System.IO.File]::ReadAllBytes($_.FullName)
            # 检测 \r\r\n
            for ($i = 0; $i -lt $bytes.Length - 2; $i++) {
                if ($bytes[$i] -eq 0x0D -and $bytes[$i+1] -eq 0x0D -and $bytes[$i+2] -eq 0x0A) {
                    $bareCrFiles += $_.FullName
                    break
                }
            }
        }
    }
}
if ($bareCrFiles.Count -gt 0) {
    Write-Host "FAIL: 检测到 \r\r\n 损坏换行符" -ForegroundColor Red
    $bareCrFiles | ForEach-Object { Write-Host "  $_" }
    $fail = $true
} else {
    Write-Host "OK: 无 bare CR" -ForegroundColor Green
}

Write-Host ""
Write-Host "=== 2. rust-toolchain 版本一致性 ==="
$toolchainPath = "rust-toolchain.toml"
if (-not (Test-Path $toolchainPath)) {
    Write-Host "FAIL: rust-toolchain.toml 不存在" -ForegroundColor Red
    $fail = $true
} else {
    $tcContent = Get-Content $toolchainPath -Raw
    if ($tcContent -match 'channel\s*=\s*"([^"]+)"') {
        $toolchainVer = $matches[1]
    } else {
        $toolchainVer = ""
        Write-Host "FAIL: rust-toolchain.toml 未找到 channel 配置" -ForegroundColor Red
        $fail = $true
    }
}
$ciPath = ".github/workflows/ci.yml"
if (Test-Path $ciPath) {
    $ciContent = Get-Content $ciPath -Raw
    $ciVers = @([regex]::Matches($ciContent, 'dtolnay/rust-toolchain@([0-9.]+)') | ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique)
    if ($ciVers.Count -gt 1) {
        Write-Host "FAIL: ci.yml 里存在多个不同的 toolchain 版本" -ForegroundColor Red
        $ciVers | ForEach-Object { Write-Host "  $_" }
        $fail = $true
    } elseif ($toolchainVer -and $toolchainVer -ne $ciVers[0]) {
        Write-Host "FAIL: rust-toolchain.toml channel=$toolchainVer != ci.yml @$($ciVers[0])" -ForegroundColor Red
        $fail = $true
    } else {
        Write-Host "OK: 都是 $toolchainVer" -ForegroundColor Green
    }
}

Write-Host ""
Write-Host "=== 3. ts-rs generated 完整性检测 ==="
$genDir = "gateway-web/src/api/generated"
if (Test-Path $genDir) {
    $genFiles = Get-ChildItem -Path $genDir -Filter "*.ts" -ErrorAction SilentlyContinue
    if ($genFiles -and $genFiles.Count -gt 0) {
        Write-Host "OK: generated/ 有 $($genFiles.Count) 个 .ts 文件" -ForegroundColor Green
    } else {
        Write-Host "WARN: gateway-web/src/api/generated/ 为空，运行 cargo test export_ts_types 生成" -ForegroundColor Yellow
        $warn = $true
    }
} else {
    Write-Host "WARN: gateway-web/src/api/generated/ 不存在，运行 cargo test export_ts_types 生成" -ForegroundColor Yellow
    $warn = $true
}

Write-Host ""
Write-Host "=== 4. CRLF/LF 一致性检测 ==="
$crlfCount = 0
$lfCount = 0
$rsDir = "gateway-core/src"
$rsDirs = @("gateway-core/src", "gateway-api/src")
if (Test-Path $rsDir) {
    Get-ChildItem -Path $rsDirs -Recurse -File -Filter "*.rs" -ErrorAction SilentlyContinue | ForEach-Object {
        $content = [System.IO.File]::ReadAllBytes($_.FullName)
        $hasCrlf = $false
        for ($i = 0; $i -lt $content.Length - 1; $i++) {
            if ($content[$i] -eq 0x0D -and $content[$i+1] -eq 0x0A) {
                $hasCrlf = $true
                break
            }
        }
        if ($hasCrlf) { $script:crlfCount++ } else { $script:lfCount++ }
    }
}
if ($crlfCount -gt 0 -and $lfCount -gt 0) {
    Write-Host "WARN: CRLF 文件 $crlfCount 个，LF 文件 $lfCount 个 —— 建议统一" -ForegroundColor Yellow
    Write-Host "  建议加 .gitattributes 统一换行符："
    Write-Host "    *.rs text eol=lf"
    Write-Host "  然后执行：git add --renormalize ."
    $warn = $true
} elseif ($crlfCount -gt 0) {
    Write-Host "INFO: 全 CRLF ($crlfCount 个 .rs) —— 不影响编译但建议统一为 LF" -ForegroundColor Yellow
    $warn = $true
} elseif ($lfCount -gt 0) {
    Write-Host "OK: 全 LF ($lfCount 个 .rs)" -ForegroundColor Green
} else {
    Write-Host "OK: 无 .rs 文件" -ForegroundColor Green
}

Write-Host ""

Write-Host "=== 5. build artifact leak 检测 ==="
# 检查不应出现在 git 工作树的构建产物 / 自动生成文件
# F-02：原实现用 Test-Path 判工作树里是否存在该路径——只要本地构建过一次，
# target/ 与 dist/ 必然存在，于是 100% 误报，永远 FAIL。正确口径是「是否被 git 跟踪」。
# 无 git 可用（或不在 git 工作树内）时无法判定，显式降级为 SKIP 而不是谎报通过/失败。
$artifactPaths = @(
  "target",
  "gateway-web/dist",
  "gateway-web/auto-imports.d.ts",
  "gateway-web/components.d.ts",
  "gateway-desktop/src-tauri/Cargo.lock"
)
$leaked = $false
$gitAvailable = $false
if (Get-Command git -ErrorAction SilentlyContinue) {
    git rev-parse --is-inside-work-tree *> $null
    $gitAvailable = ($LASTEXITCODE -eq 0)
}
if (-not $gitAvailable) {
    Write-Host "SKIP: 未检测到 git 工作树，无法判定构建产物是否入库（第 5 项跳过）" -ForegroundColor Yellow
} else {
    foreach ($p in $artifactPaths) {
        $tracked = git ls-files -- $p 2>$null
        if ($LASTEXITCODE -eq 0 -and $tracked) {
            Write-Host "LEAK: $p 已被 git 跟踪（应被 .gitignore 排除）" -ForegroundColor Red
            $leaked = $true
        }
    }
    # vue-tsc-errors*.txt glob
    $vueTscTracked = git ls-files -- "gateway-web/vue-tsc-errors*.txt" 2>$null
    foreach ($f in $vueTscTracked) {
        Write-Host "LEAK: $f 已被 git 跟踪（应被 .gitignore 排除）" -ForegroundColor Red
        $leaked = $true
    }
    if (-not $leaked) {
        Write-Host "OK: 无构建产物 / 自动生成文件入库" -ForegroundColor Green
    }
}
if ($leaked) {
  $fail = $true
}
if ($fail) {
    Write-Host "FAILED: 阻断性错误，请修复后重试" -ForegroundColor Red
    exit 1
} elseif ($warn) {
    Write-Host "WARN: 仅有警告（不阻断）" -ForegroundColor Yellow
    exit 0
} else {
    Write-Host "All checks passed" -ForegroundColor Green
    exit 0
}

Write-Host ""
