#!/usr/bin/env pwsh
#Requires -Version 7.0
<#
.SYNOPSIS
    AniBox 稳定打包脚本（PowerShell 7）。

.DESCRIPTION
    根治 `tauri build` 偶发 "os error 32 / 另一个程序正在使用此文件" 的问题：

    1. 从 secrets/ 读取并注入构建期密钥（已设置的保留，绝不打印值）。
    2. 构建前清理残留进程：anibox.exe（打包期间它会锁住构建产物，
       [用户要求 2026-11-27] 检测到直接自动结束，不再提示人工关闭）+ 构建工具
       残留（cargo / rustc / makensis / link）。
    3. 失败时自动清理残留进程后重试，最多 3 次；占用类错误必重试，
       其它错误若连续两次文本相同（确定性错误）才放弃。
    4. 成功后把 NSIS 安装包复制到 release/，打印路径、大小、SHA-256。
    5. 全程追加写入 release/build.log（带时间戳，不覆盖历史）。

.PARAMETER StopApp
    已废弃（保留兼容 GUI 旧调用）：现在默认就会自动结束 anibox.exe。

.PARAMETER MaxAttempts
    最大构建尝试次数，默认 3。

.PARAMETER RetryDelaySeconds
    文件占用类失败后等待的秒数，默认 15。
#>
[CmdletBinding()]
param(
    [switch]$StopApp,
    [int]$MaxAttempts = 3,
    [int]$RetryDelaySeconds = 15
)

$ErrorActionPreference = 'Stop'

$RepoRoot   = Split-Path -Parent $PSScriptRoot          # tools/ -> 仓库根
$ReleaseDir = Join-Path $RepoRoot 'release'
$LogFile    = Join-Path $ReleaseDir 'build.log'

New-Item -ItemType Directory -Force -Path $ReleaseDir | Out-Null
Set-Location -LiteralPath $RepoRoot

# ------------------------------------------------------------------ logging
function Write-Log {
    param(
        [Parameter(Mandatory = $true)][string]$Message,
        [ValidateSet('INFO', 'WARN', 'ERROR', 'OK')][string]$Level = 'INFO'
    )
    $ts   = Get-Date -Format 'yyyy-MM-dd HH:mm:ss'
    $line = "[$ts] [$Level] $Message"
    $color = switch ($Level) {
        'WARN'  { 'Yellow' }
        'ERROR' { 'Red' }
        'OK'    { 'Green' }
        default { 'Gray' }
    }
    Write-Host $line -ForegroundColor $color
    # [lc-1817] build.log 被别的进程占用（编辑器/资源管理器预览/上次构建残留）时
    # 重试一次再放弃 —— 日志只是佐证，绝不让它中断构建本身。
    try {
        Add-Content -LiteralPath $LogFile -Value $line -Encoding utf8 -ErrorAction Stop
    } catch {
        Start-Sleep -Milliseconds 150
        try { Add-Content -LiteralPath $LogFile -Value $line -Encoding utf8 -ErrorAction Stop } catch {}
    }
}

function Write-Section {
    param([string]$Title)
    Write-Log ('-' * 72)
    Write-Log $Title
    Write-Log ('-' * 72)
}

# ------------------------------------------------------------- secret loading
function Import-SecretFile {
    <#
      读取单个密钥并注入到当前进程环境变量。
      - RawValue : 文件本身就是裸值（bgm-mirror/.secret.local，base64 结尾带 '='）
      否则按 NAME=value 解析，取第一个 '=' 之后的全部内容（保留 base64 padding）。
      已存在的环境变量一律保留，不覆盖、不打印值。
    #>
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Name,
        [switch]$RawValue
    )

    if (-not [string]::IsNullOrEmpty([Environment]::GetEnvironmentVariable($Name, 'Process'))) {
        Write-Log "$Name 已存在于环境中，保留现有值（不覆盖、不打印）。"
        return
    }

    if (-not (Test-Path -LiteralPath $Path)) {
        Write-Log "$Name 未找到密钥文件：$Path" 'WARN'
        return
    }

    $content = Get-Content -LiteralPath $Path -Raw
    if ($null -eq $content) {
        Write-Log "$Name 密钥文件为空：$Path" 'WARN'
        return
    }

    $value = $null
    if ($RawValue) {
        $value = $content.Trim()
    }
    else {
        foreach ($line in ($content -split "`r?`n")) {
            if ($line -match '^\s*([^=]+?)\s*=(.*)$') {
                $key = $matches[1].Trim()
                if ($key -ieq $Name) {
                    $value = $matches[2].Trim()
                    break
                }
            }
        }
    }

    if ([string]::IsNullOrWhiteSpace($value)) {
        Write-Log "$Name 密钥文件存在但没有可用值：$Path" 'WARN'
        return
    }

    [Environment]::SetEnvironmentVariable($Name, $value, 'Process')
    Write-Log "$Name 已加载 <- $Path（值已隐藏）。"
}

# ------------------------------------------------------------ process helpers
$BuildToolNames = @('cargo', 'rustc', 'makensis', 'link')

function Get-BuildToolProcesses {
    $result = @()
    foreach ($name in $BuildToolNames) {
        $result += @(Get-Process -Name $name -ErrorAction SilentlyContinue)
    }
    return @($result)
}

function Stop-BuildToolProcesses {
    param([string]$Reason)
    $procs = Get-BuildToolProcesses
    if ($procs.Count -eq 0) {
        return
    }
    foreach ($p in $procs) {
        try {
            Write-Log "结束构建工具进程 $($p.ProcessName) (PID $($p.Id)) —— $Reason" 'WARN'
            Stop-Process -Id $p.Id -Force -ErrorAction Stop
        }
        catch {
            Write-Log "无法结束 PID $($p.Id)：$($_.Exception.Message)" 'WARN'
        }
    }
    Start-Sleep -Seconds 2
}

function Assert-AppNotRunning {
    # [用户要求 2026-11-27] 打包期间 anibox.exe 会锁住构建产物：检测到直接自动
    # 结束，不再要求人工关闭（此前「检测到只提示」的策略已废弃）。
    $app = @(Get-Process -Name 'anibox' -ErrorAction SilentlyContinue)
    if ($app.Count -eq 0) {
        return
    }
    $pids = ($app | ForEach-Object { $_.Id }) -join ', '
    Write-Log "检测到 anibox.exe (PID $pids) 正在运行，会锁住 src-tauri\target 构建产物，自动结束。" 'WARN'
    $app | Stop-Process -Force
    Start-Sleep -Seconds 2
}

# --------------------------------------------------------------- build logic
$RetryablePatterns = @(
    'os error 32',
    'os error 5',
    '另一个程序正在使用此文件',
    '拒绝访问',
    'Access is denied'
)

function Test-RetryableError {
    param([string]$Text)
    if ([string]::IsNullOrEmpty($Text)) { return $false }
    foreach ($pattern in $RetryablePatterns) {
        if ($Text -match [regex]::Escape($pattern)) {
            return $true
        }
    }
    return $false
}

# ================================================================== main flow
$startedAt = Get-Date

Write-Section "AniBox build-installer 启动  ($($startedAt.ToString('yyyy-MM-dd HH:mm:ss')))"
Write-Log "仓库根目录：$RepoRoot"
Write-Log "日志文件：$LogFile"
Write-Log "提示：若频繁出现文件被锁，可把 src-tauri\target 加入 Windows Defender 排除项（需管理员权限，本脚本不自动修改）。"

# --- 1. 密钥注入 -----------------------------------------------------------
Write-Section '步骤 1/4：注入构建期密钥'
Import-SecretFile -Path (Join-Path $RepoRoot 'secrets/bgm-mirror/.secret.local')  -Name 'FNTV_BGM_SECRET'  -RawValue
Import-SecretFile -Path (Join-Path $RepoRoot 'secrets/tmdb-mirror/.secret.local') -Name 'FNTV_TMDB_SECRET'
Import-SecretFile -Path (Join-Path $RepoRoot 'secrets/gh-mirror/.secret.local')   -Name 'FNTV_GH_SECRET'

$missing = @()
foreach ($n in @('FNTV_BGM_SECRET', 'FNTV_TMDB_SECRET', 'FNTV_GH_SECRET')) {
    if ([string]::IsNullOrEmpty([Environment]::GetEnvironmentVariable($n, 'Process'))) {
        $missing += $n
    }
}
if ($missing.Count -gt 0) {
    Write-Log ("缺少密钥：" + ($missing -join ', ') + ' —— 打包仍可继续，但对应镜像功能会降级。') 'WARN'
}
else {
    Write-Log '三个密钥均已就绪。' 'OK'
}

# --- 2. 构建前进程检查 -----------------------------------------------------
Write-Section '步骤 2/4：构建前进程检查'
Assert-AppNotRunning
Stop-BuildToolProcesses -Reason '构建前清理残留'

# --- 3. 构建（带重试）-----------------------------------------------------
Write-Section '步骤 3/4：执行构建（最多重试）'
Write-Log '命令：npm run tauri build -- --bundles nsis'

$buildSucceeded = $false
$attempt        = 0
$lastErrorText  = ''

while ($attempt -lt $MaxAttempts -and -not $buildSucceeded) {
    $attempt++
    Write-Section "构建尝试 $attempt / $MaxAttempts"
    Assert-AppNotRunning
    Stop-BuildToolProcesses -Reason "第 $attempt 次构建前清理"

    $attemptStart = Get-Date
    # [lc-1818] 实时日志：纯 `$output = … | Tee-Object` 会把整条管道吞进变量，stdout
    # 一行不出（GUI 日志区全程空白，打包完成才见分晓）。ForEach-Object 每行**到达时**
    # Write-Host 直出（GUI 的 stdout 管道逐行实时拿到），再传下游写文件 + 存变量。
    $output = & npm run tauri build -- --bundles nsis 2>&1 | ForEach-Object {
        Write-Host $_.ToString()
        $_
    } | Tee-Object -FilePath $LogFile -Append
    $exitCode = $LASTEXITCODE
    $elapsed  = (Get-Date) - $attemptStart
    $text     = ($output | Out-String)

    if ($exitCode -eq 0) {
        $buildSucceeded = $true
        Write-Log "第 $attempt 次构建成功，用时 $([int]$elapsed.TotalSeconds) 秒。" 'OK'
        break
    }

    Write-Log "第 $attempt 次构建失败（exit $exitCode），用时 $([int]$elapsed.TotalSeconds) 秒。" 'ERROR'

    # [用户要求 2026-11-27] 失败一律自动清理残留进程后重试，不再按错误类别「立即失败」；
    # 唯一例外是连续两次错误文本完全相同且非占用类 —— 那是确定性错误（如代码 bug），
    # 重试不会好，及时停下。
    $retryable  = Test-RetryableError $text
    $deterministic = (-not $retryable) -and ($text -eq $lastErrorText)
    $lastErrorText = $text

    if ($deterministic) {
        Write-Log '连续两次相同且非占用类错误，判定为确定性错误，不再重试。' 'ERROR'
        break
    }

    if ($attempt -lt $MaxAttempts) {
        if ($retryable) {
            Write-Log "检测到文件占用类错误，等待 $RetryDelaySeconds 秒后自动清理残留进程并重试。" 'WARN'
        }
        else {
            Write-Log "非占用类错误，仍自动清理残留进程并重试（最多 $MaxAttempts 次）。" 'WARN'
        }
        Start-Sleep -Seconds $RetryDelaySeconds
        Assert-AppNotRunning
        Stop-BuildToolProcesses -Reason "第 $attempt 次失败后重试清理"
    }
}

if (-not $buildSucceeded) {
    Write-Log "构建最终失败（共尝试 $attempt 次）。详见日志：$LogFile" 'ERROR'
    exit 1
}

# --- 4. 收集安装包 ---------------------------------------------------------
Write-Section '步骤 4/4：收集安装包'
$confPath = Join-Path $RepoRoot 'src-tauri/tauri.conf.json'
$version  = 'unknown'
try {
    $version = (Get-Content -LiteralPath $confPath -Raw | ConvertFrom-Json).version
}
catch {
    Write-Log "无法从 tauri.conf.json 读取版本：$($_.Exception.Message)" 'WARN'
}

$bundleDir = Join-Path $RepoRoot 'src-tauri/target/release/bundle/nsis'
$expected  = Join-Path $bundleDir "AniBox_${version}_x64-setup.exe"

if (-not (Test-Path -LiteralPath $expected)) {
    Write-Log "预期安装包不存在，回退为搜索 *setup.exe：$expected" 'WARN'
    $candidate = Get-ChildItem -LiteralPath $bundleDir -Filter '*setup.exe' -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if ($candidate) { $expected = $candidate.FullName }
}

if (-not (Test-Path -LiteralPath $expected)) {
    Write-Log "未找到任何安装包：$bundleDir" 'ERROR'
    exit 1
}

$destName = Split-Path -Leaf $expected
$dest     = Join-Path $ReleaseDir $destName
Copy-Item -LiteralPath $expected -Destination $dest -Force

$sizeBytes = (Get-Item -LiteralPath $dest).Length
$sizeMB    = [math]::Round($sizeBytes / 1MB, 2)
$sha256    = (Get-FileHash -LiteralPath $dest -Algorithm SHA256).Hash
$total     = (Get-Date) - $startedAt

Write-Section '构建完成'
Write-Log "安装包路径：$dest" 'OK'
Write-Log "安装包大小：$sizeMB MB ($sizeBytes bytes)"
Write-Log "SHA-256：$sha256"
Write-Log "总耗时：$([int]$total.TotalMinutes) 分 $($total.Seconds) 秒（尝试 $attempt 次）"
Write-Log "日志：$LogFile"
Write-Host ''
Write-Host "打包完成 -> $dest" -ForegroundColor Green
Write-Host "SHA-256: $sha256" -ForegroundColor Green
exit 0
