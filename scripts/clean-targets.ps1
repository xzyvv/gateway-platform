<#
.SYNOPSIS
    清理 Rust 构建产物：仓库 target/ 与 Trae 智能体会话沙箱里散落的 target 目录。

.DESCRIPTION
    背景：Trae 智能体在会话沙箱（%APPDATA%\Trae CN\ModularData\ai-agent\work-mode-projects\<会话ID>\）
    里执行 Rust 构建时，常会设置 CARGO_TARGET_DIR 指向 gateway_target / gcore_target / gw_target /
    linux_target 之类的自定义目录。这些目录既不在 .gitignore 的 target/ 规则内，也不受 cargo 管理，
    会持续堆积到几十 GB。

    本脚本清理两类目标：
      1) 仓库根的 target/（-KeepRelease 可保留 release 产物）
      2) Trae 会话沙箱里名字为 target 或 *_target 的目录（-SandboxOnly 可只清这一类）

    删除走 .NET 长路径接口（\\?\ 前缀），规避 Windows MAX_PATH 260 字符限制。

.PARAMETER KeepRelease
    保留仓库 target/release 下的产物，只清 target/ 里的其它内容。

.PARAMETER SandboxOnly
    只清理会话沙箱里的散落 target 目录，不动仓库自己的 target/。

.PARAMETER DryRun
    只统计各目标占用，不实际删除。

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\clean-targets.ps1 -DryRun
    查看将要清理的目标与体积，不做删除。

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\clean-targets.ps1 -KeepRelease
    清理仓库 target/（保留 release）与沙箱里的散落 target 目录。

.NOTES
    仅支持 Windows（依赖 %APPDATA% 下的 Trae 沙箱路径）。
#>
[CmdletBinding()]
param(
    [switch]$KeepRelease,
    [switch]$SandboxOnly,
    [switch]$DryRun
)

$ErrorActionPreference = 'Continue'
$repoRoot = Split-Path -Parent $PSScriptRoot

function Get-DirSize([string]$p) {
    if (-not (Test-Path -LiteralPath $p)) { return 0 }
    $s = Get-ChildItem -LiteralPath $p -Recurse -Force -File -ErrorAction SilentlyContinue | Measure-Object Length -Sum
    if ($null -eq $s.Sum) { return 0 }
    return [int64]$s.Sum
}

function Remove-DirLongPath([string]$p) {
    if (-not (Test-Path -LiteralPath $p)) { return }
    Get-ChildItem -LiteralPath $p -Recurse -Force -File -ErrorAction SilentlyContinue | ForEach-Object {
        try { [System.IO.File]::SetAttributes("\\?\$($_.FullName)", 'Normal') } catch {}
        try { [System.IO.File]::Delete("\\?\$($_.FullName)") } catch {}
    }
    Get-ChildItem -LiteralPath $p -Recurse -Force -Directory -ErrorAction SilentlyContinue |
        Sort-Object { $_.FullName.Length } -Descending | ForEach-Object {
            try { [System.IO.Directory]::Delete("\\?\$($_.FullName)", $true) } catch {}
        }
    try { [System.IO.Directory]::Delete("\\?\$p", $true) } catch {}
}

function Format-Size([int64]$bytes) {
    if ($bytes -ge 1GB) { return ("{0:N2} GB" -f ($bytes / 1GB)) }
    if ($bytes -ge 1MB) { return ("{0:N1} MB" -f ($bytes / 1MB)) }
    return ("{0:N0} B" -f $bytes)
}

# ---------------------------------------------------------------------------
# 目标收集
# ---------------------------------------------------------------------------
$repoTarget = Join-Path $repoRoot 'target'
$sandboxRoots = @(
    (Join-Path $env:APPDATA 'Trae CN\ModularData\ai-agent\work-mode-projects'),
    (Join-Path $env:APPDATA 'TRAE SOLO CN\ModularData\ai-agent\work-mode-projects')
)

$sandboxTargets = New-Object System.Collections.Generic.List[string]
foreach ($root in $sandboxRoots) {
    if (-not (Test-Path -LiteralPath $root)) { continue }
    Get-ChildItem -LiteralPath $root -Recurse -Force -Directory -ErrorAction SilentlyContinue |
        Where-Object {
            ($_.Name -eq 'target' -or $_.Name -like '*_target') -and
            $_.FullName -notmatch '[\\/]node_modules[\\/]'
        } |
        ForEach-Object { $sandboxTargets.Add($_.FullName) }
}

Write-Host "=== Trae 构建产物清理 ==="
Write-Host ("仓库 target/: {0}" -f $repoTarget)
Write-Host ("会话沙箱 target 目录: {0} 个" -f $sandboxTargets.Count)
if ($DryRun) { Write-Host "[DryRun] 只统计，不删除" }
Write-Host ""

$totalFreed = [int64]0

# ---------------------------------------------------------------------------
# 1. 仓库 target/
# ---------------------------------------------------------------------------
if (-not $SandboxOnly -and (Test-Path -LiteralPath $repoTarget)) {
    $targets = @()
    if ($KeepRelease) {
        $targets = Get-ChildItem -LiteralPath $repoTarget -Force -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -ne 'release' } |
            ForEach-Object { $_.FullName }
    } else {
        $targets = @($repoTarget)
    }
    foreach ($t in $targets) {
        $size = Get-DirSize $t
        Write-Host ("  [仓库] {0}  {1}" -f (Format-Size $size), $t)
        if (-not $DryRun) { Remove-DirLongPath $t }
        $totalFreed += $size
    }
}

# ---------------------------------------------------------------------------
# 2. 会话沙箱里的散落 target 目录
# ---------------------------------------------------------------------------
foreach ($t in $sandboxTargets) {
    if (-not (Test-Path -LiteralPath $t)) { continue }
    $size = Get-DirSize $t
    Write-Host ("  [沙箱] {0}  {1}" -f (Format-Size $size), $t)
    if (-not $DryRun) { Remove-DirLongPath $t }
    $totalFreed += $size
}

Write-Host ""
Write-Host ("合计释放: {0}" -f (Format-Size $totalFreed))
if ($KeepRelease) { Write-Host "（仓库 target/release 已保留）" }
