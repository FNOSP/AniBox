#!/usr/bin/env pwsh
#Requires -Version 7.0
<#
.SYNOPSIS
    下载并校验 Microsoft Edge WebView2 Evergreen Standalone（离线全量）安装包。

.DESCRIPTION
    AniBox 的 NSIS 安装包在 `webviewInstallMode = offlineInstaller` 下会把
    MicrosoftEdgeWebView2RuntimeInstallerX64.exe 内嵌进安装包，用户端安装时
    无需联网。Tauri 在 `tauri build` 时会自动把该文件下载到自己的缓存
    （Windows 为 %LOCALAPPDATA%\tauri），本脚本用于：

      1. 在**离线构建机**上预先取得安装包并留档；
      2. 校验微软官方下载链接是否可用（-HeadOnly，不发正文）；
      3. 记录 / 校验 SHA-256，便于在发布说明中给出可复核的指纹。

    下载文件默认落在 `src-tauri/webview2/`（已在 .gitignore 中忽略，
    **不要**把约 200MB 的二进制提交进仓库）。

    官方短链始终指向“最新版”，因此每次下载到的版本会随时间变化；SHA-256
    需以当次下载结果为准（用本脚本打印的值，或从微软官方页面核对）。

.PARAMETER Arch
    x64（默认，项目当前唯一发布目标）、x86 或 arm64。
    已实测的官方短链（2026-10-20，HEAD 校验）：
      x64   linkid=2124701 → MicrosoftEdgeWebView2RuntimeInstallerX64.exe（约 214MB）
      x86   linkid=2099617 → MicrosoftEdgeWebView2RuntimeInstallerX86.exe（约 190MB）
      arm64 微软未给出稳定短链；注意 linkid=2124703 是约 1.8MB 的 Bootstrapper，
            不是 ARM64 离线包。ARM64 必须从微软官方下载页取得 Standalone
            链接后用 -Url 显式传入。

.PARAMETER Url
    覆盖下载地址。缺省时 x64/x86 使用上表的官方短链。

.PARAMETER Destination
    输出目录，默认 <仓库根>/src-tauri/webview2。

.PARAMETER ExpectedSha256
    期望的 SHA-256（十六进制）。提供后会与下载结果比对，不一致则退出码 2。

.PARAMETER HeadOnly
    只发 HTTP HEAD 检查链接可用性与文件大小，不下载正文。

.PARAMETER Force
    即使目标文件已存在也重新下载。

.PARAMETER Retries
    下载失败重试次数（默认 3）。

.EXAMPLE
    # 只检查链接（不下载 200MB）
    pwsh -NoProfile -File .\tools\fetch-webview2-offline.ps1 -HeadOnly

.EXAMPLE
    # 下载 x64 离线包到 src-tauri/webview2 并打印 SHA-256
    pwsh -NoProfile -File .\tools\fetch-webview2-offline.ps1

.EXAMPLE
    # 下载并校验固定指纹
    pwsh -NoProfile -File .\tools\fetch-webview2-offline.ps1 -ExpectedSha256 <hex>
#>
[CmdletBinding()]
param(
    [ValidateSet('x64', 'x86', 'arm64')]
    [string]$Arch = 'x64',

    [string]$Url,

    [string]$Destination,

    [string]$ExpectedSha256,

    [switch]$HeadOnly,

    [switch]$Force,

    [int]$Retries = 3,

    [int]$RetryDelaySeconds = 3
)

$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent $PSScriptRoot
if ([string]::IsNullOrWhiteSpace($Destination)) {
    $Destination = Join-Path $RepoRoot 'src-tauri/webview2'
}

# 官方 Evergreen Standalone Installer 短链（始终指向最新版）。
# 说明：微软未为 ARM64 提供稳定短链（2124703 是 Bootstrapper），需用 -Url 传入。
$DefaultUrls = @{
    x64 = 'https://go.microsoft.com/fwlink/?linkid=2124701'
    x86 = 'https://go.microsoft.com/fwlink/?linkid=2099617'
}

$FileNames = @{
    x64   = 'MicrosoftEdgeWebView2RuntimeInstallerX64.exe'
    x86   = 'MicrosoftEdgeWebView2RuntimeInstallerX86.exe'
    arm64 = 'MicrosoftEdgeWebView2RuntimeInstallerARM64.exe'
}

$effectiveUrl = $null
if (-not [string]::IsNullOrWhiteSpace($Url)) {
    $effectiveUrl = $Url.Trim()
}
elseif ($DefaultUrls.ContainsKey($Arch)) {
    $effectiveUrl = $DefaultUrls[$Arch]
}

if (-not $effectiveUrl) {
    throw ("未提供 {0} 的下载地址。请从微软官方 WebView2 下载页取得 ARM64 Standalone Installer 链接后用 -Url 传入。" -f $Arch)
}

function Get-CurlPath {
    $cmd = Get-Command curl.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    return $null
}

function Format-Bytes([long]$Bytes) {
    if ($Bytes -ge 1GB) { return ('{0:N2} GB' -f ($Bytes / 1GB)) }
    if ($Bytes -ge 1MB) { return ('{0:N2} MB' -f ($Bytes / 1MB)) }
    if ($Bytes -ge 1KB) { return ('{0:N2} KB' -f ($Bytes / 1KB)) }
    return ("$Bytes bytes")
}

Write-Host '=============================================='
Write-Host (' AniBox - WebView2 离线安装包获取 ({0})' -f $Arch)
Write-Host '=============================================='
Write-Host ('[webview2] 下载地址: {0}' -f $effectiveUrl)

# ---------------------------------------------------------------- HEAD 模式
if ($HeadOnly) {
    $curl = Get-CurlPath
    try {
        if ($curl) {
            Write-Host '[webview2] HEAD 检查中（curl -I）...'
            $head = & $curl -sIL --max-time 30 $effectiveUrl 2>&1
            $lines = @($head)
            foreach ($line in $lines) {
                if ($line -match '^(HTTP/|Location:|Content-Length:|Content-Type:)') {
                    Write-Host ("    {0}" -f $line.Trim())
                }
            }
            $lenLine = $lines | Where-Object { $_ -match '^Content-Length:\s*(\d+)' } | Select-Object -Last 1
            if ($lenLine -and $lenLine -match '^Content-Length:\s*(\d+)') {
                Write-Host ('[webview2] 远端文件大小: {0}' -f (Format-Bytes ([long]$matches[1])))
            }
            exit 0
        }
        else {
            Write-Host '[webview2] 未找到 curl.exe，改用 Invoke-WebRequest -Method Head ...'
            $resp = Invoke-WebRequest -Uri $effectiveUrl -Method Head -MaximumRedirection 10 -UseBasicParsing
            Write-Host ('[webview2] 状态码: {0}' -f [int]$resp.StatusCode)
            if ($resp.Headers['Content-Length']) {
                Write-Host ('[webview2] 远端文件大小: {0}' -f (Format-Bytes ([long]$resp.Headers['Content-Length'])))
            }
            exit 0
        }
    }
    catch {
        Write-Error ("HEAD 检查失败: {0}" -f $_.Exception.Message) -ErrorAction Continue
        exit 1
    }
}

# ---------------------------------------------------------------- 下载模式
New-Item -ItemType Directory -Force -Path $Destination | Out-Null
$destPath = Join-Path $Destination $FileNames[$Arch]
$expected = if ($ExpectedSha256) { $ExpectedSha256.Trim().ToUpperInvariant() } else { $null }

if ((Test-Path -LiteralPath $destPath) -and -not $Force) {
    $existing = (Get-FileHash -LiteralPath $destPath -Algorithm SHA256).Hash.ToUpperInvariant()
    if ($expected) {
        if ($existing -eq $expected) {
            Write-Host ('[webview2] 已存在且 SHA-256 匹配，跳过下载: {0}' -f $destPath) -ForegroundColor Green
            exit 0
        }
        Write-Warning ('已存在的文件 SHA-256 与期望不符，将重新下载: {0}' -f $destPath)
    }
    else {
        Write-Host ('[webview2] 已存在: {0}' -f $destPath)
        Write-Host ('[webview2]   SHA-256: {0}' -f $existing)
        Write-Host '[webview2] 如需重新下载请加 -Force。'
        exit 0
    }
}

$tmpPath = "$destPath.part"
if (Test-Path -LiteralPath $tmpPath) { Remove-Item -LiteralPath $tmpPath -Force }

$curl = Get-CurlPath
$downloaded = $false

if ($curl) {
    for ($attempt = 1; $attempt -le $Retries -and -not $downloaded; $attempt++) {
        Write-Host ('[webview2] 下载尝试 {0}/{1}（支持断点续传）...' -f $attempt, $Retries)
        # -C - : 若存在 .part 则续传；服务端不支持续传时 curl 返回 33，退化为整包重下。
        & $curl -L --fail --retry 2 --retry-delay $RetryDelaySeconds -C - -o $tmpPath $effectiveUrl
        $code = $LASTEXITCODE
        if ($code -eq 0) {
            $downloaded = $true
            break
        }
        Write-Warning ('curl 退出码 {0}（可能服务端不支持续传），改为整包重下。' -f $code)
        if (Test-Path -LiteralPath $tmpPath) { Remove-Item -LiteralPath $tmpPath -Force }
        & $curl -L --fail -o $tmpPath $effectiveUrl
        if ($LASTEXITCODE -eq 0) {
            $downloaded = $true
            break
        }
        if ($attempt -lt $Retries) {
            Write-Warning ('第 {0} 次下载失败，{1} 秒后重试...' -f $attempt, $RetryDelaySeconds)
            Start-Sleep -Seconds $RetryDelaySeconds
        }
    }
}
else {
    Write-Host '[webview2] 未找到 curl.exe，使用 Invoke-WebRequest（不支持断点续传）...'
    for ($attempt = 1; $attempt -le $Retries -and -not $downloaded; $attempt++) {
        try {
            Invoke-WebRequest -Uri $effectiveUrl -OutFile $tmpPath -UseBasicParsing
            $downloaded = $true
        }
        catch {
            Write-Warning ('第 {0} 次下载失败: {1}' -f $attempt, $_.Exception.Message)
            if ($attempt -lt $Retries) { Start-Sleep -Seconds $RetryDelaySeconds }
        }
    }
}

if (-not $downloaded) {
    Write-Error ('下载失败，已尝试 {0} 次。' -f $Retries) -ErrorAction Continue
    exit 1
}

if (Test-Path -LiteralPath $destPath) { Remove-Item -LiteralPath $destPath -Force }
Move-Item -LiteralPath $tmpPath -Destination $destPath -Force

$sizeBytes = (Get-Item -LiteralPath $destPath).Length
$sha256 = (Get-FileHash -LiteralPath $destPath -Algorithm SHA256).Hash.ToUpperInvariant()

Write-Host ''
Write-Host '[webview2] 下载完成。' -ForegroundColor Green
Write-Host ('[webview2]   文件: {0}' -f $destPath)
Write-Host ('[webview2]   大小: {0} ({1} bytes)' -f (Format-Bytes $sizeBytes), $sizeBytes)
Write-Host ('[webview2]   SHA-256: {0}' -f $sha256)

if ($expected) {
    if ($sha256 -eq $expected) {
        Write-Host '[webview2] SHA-256 校验通过。' -ForegroundColor Green
    }
    else {
        Write-Error ('SHA-256 不匹配！期望 {0}，实际 {1}' -f $expected, $sha256) -ErrorAction Continue
        exit 2
    }
}

Write-Host ''
Write-Host '提示：Tauri 在 `tauri build`（offlineInstaller 模式）时会自行把该文件'
Write-Host '      下载到 %LOCALAPPDATA%\tauri 缓存；本目录用于离线构建机留档与校验。'
Write-Host '      src-tauri/webview2/ 已在 .gitignore 中忽略，请勿提交该二进制。'
exit 0
