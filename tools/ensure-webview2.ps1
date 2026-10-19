#requires -Version 5.1
<#
.SYNOPSIS
  AniBox WebView2 Runtime 检测 / 安装兜底脚本。

.DESCRIPTION
  Tauri 在 Windows 上依赖 WebView2 Runtime。运行时缺失时 AniBox 会直接退出或白屏，
  连设置面板都打不开 —— 所以必须在启动前用本脚本处理。

  检测口径（任一命中即视为已安装，版本取较大者）：
    * 注册表：HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\*\pv
              HKLM\SOFTWARE\Microsoft\EdgeUpdate\Clients\*\pv
              （只认 name 含 WebView2 或 Client ID 前缀 {F1E7B9A0 的项）
    * 目录：  %ProgramFiles(x86)%\Microsoft\EdgeWebView\Application\<ver>\msedgewebview2.exe
              %ProgramFiles%\Microsoft\EdgeWebView\Application\<ver>\msedgewebview2.exe
              %LOCALAPPDATA%\Microsoft\EdgeWebView\Application\<ver>\msedgewebview2.exe

  缺失或版本低于 Tauri v2 要求（109+）时：
    * 默认：询问用户 → 下载官方 Evergreen Bootstrapper 到临时目录并启动安装；
    * -Silent：不询问，直接静默安装（/silent /install）；
    * -CheckOnly：只检测并打印，不下载（退出码 0=正常，1=缺失/过低）；
    * -OpenDownloadPage：只打开官方下载页，不下载安装器。

  下载地址（官方，短链始终指向最新版）：
    Bootstrapper : https://go.microsoft.com/fwlink/p/?LinkId=2124703
    x64 离线安装包: https://go.microsoft.com/fwlink/?linkid=2124701
  说明：微软没有官方国内镜像；第三方镜像可能过期或被篡改，本脚本不内置。

.EXAMPLE
  powershell -NoProfile -ExecutionPolicy Bypass -File .\tools\ensure-webview2.ps1

.EXAMPLE
  pwsh -NoProfile -File .\tools\ensure-webview2.ps1 -CheckOnly
#>
[CmdletBinding()]
param(
    [switch]$Silent,
    [switch]$CheckOnly,
    [switch]$OpenDownloadPage
)

$ErrorActionPreference = 'Stop'

$MinimumVersion = [version]'109.0.0.0'
$BootstrapperUrl = 'https://go.microsoft.com/fwlink/p/?LinkId=2124703'
$StandaloneUrl = 'https://go.microsoft.com/fwlink/?linkid=2124701'

function Get-WebView2Info {
    $bestReg = $null
    $bestDir = $null

    $roots = @(
        'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients',
        'HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients'
    )
    foreach ($root in $roots) {
        if (-not (Test-Path -LiteralPath $root)) { continue }
        foreach ($client in (Get-ChildItem -LiteralPath $root -ErrorAction SilentlyContinue)) {
            try {
                $props = Get-ItemProperty -LiteralPath $client.PSPath -ErrorAction Stop
                $name = [string]$props.name
                $pv = [string]$props.pv
                if ([string]::IsNullOrWhiteSpace($pv)) { continue }

                $isWv2 = ($name -match '(?i)webview2') -or ($client.PSChildName -match '^\{F1E7B9A0')
                if (-not $isWv2) { continue }

                $ver = $null
                if (-not [version]::TryParse($pv.Trim(), [ref]$ver)) { continue }
                if (($null -eq $bestReg) -or ($ver -gt $bestReg)) { $bestReg = $ver }
            } catch {
                # 单个客户端键读取失败不影响整体检测
            }
        }
    }

    $bases = @()
    if ($env:ProgramFiles) { $bases += (Join-Path $env:ProgramFiles 'Microsoft\EdgeWebView\Application') }
    $pf86 = [Environment]::GetEnvironmentVariable('ProgramFiles(x86)')
    if ($pf86) { $bases += (Join-Path $pf86 'Microsoft\EdgeWebView\Application') }
    if ($env:LOCALAPPDATA) { $bases += (Join-Path $env:LOCALAPPDATA 'Microsoft\EdgeWebView\Application') }

    foreach ($base in $bases) {
        if (-not (Test-Path -LiteralPath $base)) { continue }
        foreach ($dir in (Get-ChildItem -LiteralPath $base -Directory -ErrorAction SilentlyContinue)) {
            $exe = Join-Path $dir.FullName 'msedgewebview2.exe'
            if (-not (Test-Path -LiteralPath $exe)) { continue }
            $ver = $null
            if (-not [version]::TryParse($dir.Name, [ref]$ver)) { continue }
            if (($null -eq $bestDir) -or ($ver -gt $bestDir.Version)) {
                $bestDir = [pscustomobject]@{ Version = $ver; Path = $exe }
            }
        }
    }

    $version = $null
    if ($bestReg -and $bestDir) { $version = if ($bestReg -ge $bestDir.Version) { $bestReg } else { $bestDir.Version } }
    elseif ($bestReg) { $version = $bestReg }
    elseif ($bestDir) { $version = $bestDir.Version }

    $source = 'none'
    if ($bestReg) { $source = 'registry' }
    if ($bestDir) { if ($source -eq 'registry') { $source = 'registry+directory' } else { $source = 'directory' } }

    [pscustomobject]@{
        Installed = ($null -ne $version)
        Version   = $version
        Path      = if ($bestDir) { $bestDir.Path } else { $null }
        Source    = $source
    }
}

function Show-Status([pscustomobject]$Info) {
    if ($Info.Installed) {
        Write-Host ("[webview2] 已检测到 WebView2 Runtime: v{0}  ({1})" -f $Info.Version, $Info.Source)
        if ($Info.Path) { Write-Host ("[webview2] 安装路径: {0}" -f $Info.Path) }
        if ($Info.Version -lt $MinimumVersion) {
            Write-Warning ("[webview2] 版本低于最低要求 v{0}，建议升级。" -f $MinimumVersion)
        }
    } else {
        Write-Warning "[webview2] 未检测到 WebView2 Runtime —— AniBox 可能无法启动或白屏。"
    }
}

Write-Host '=============================================='
Write-Host ' AniBox - WebView2 Runtime 检测'
Write-Host '=============================================='

$info = Get-WebView2Info
Show-Status $info

if ($OpenDownloadPage) {
    Write-Host "[webview2] 正在打开官方下载页: $BootstrapperUrl"
    Start-Process $BootstrapperUrl
    exit 0
}

if ($info.Installed -and $info.Version -ge $MinimumVersion) {
    Write-Host '[webview2] 运行时正常，无需处理。'
    exit 0
}

if ($CheckOnly) {
    Write-Host '[webview2] CheckOnly：未做任何修改。'
    exit 1
}

if (-not $Silent) {
    $answer = Read-Host '是否现在下载并安装 WebView2 Runtime？[Y/n]'
    if ($answer -and $answer.Trim().ToLower() -notin @('y', 'yes', '是')) {
        Write-Host '[webview2] 已取消。可手动安装后重新运行本脚本。'
        exit 1
    }
}

$installer = Join-Path $env:TEMP 'MicrosoftEdgeWebview2Setup.exe'
try {
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    Write-Host "[webview2] 正在下载官方 Bootstrapper ..."
    Write-Host "           $BootstrapperUrl"
    Invoke-WebRequest -Uri $BootstrapperUrl -OutFile $installer -UseBasicParsing
    Write-Host ("[webview2] 下载完成: {0}" -f $installer)
} catch {
    Write-Warning ("[webview2] 下载失败: {0}" -f $_.Exception.Message)
    Write-Host '[webview2] 改为打开官方下载页，请在浏览器中手动下载安装。'
    Start-Process $BootstrapperUrl
    Write-Host "[webview2] 离线安装包（x64）: $StandaloneUrl"
    exit 1
}

try {
    if ($Silent) {
        Write-Host '[webview2] 静默安装中（/silent /install）...'
        Start-Process -FilePath $installer -ArgumentList '/silent', '/install' -Wait
    } else {
        Write-Host '[webview2] 正在启动安装程序，请按提示完成安装 ...'
        Start-Process -FilePath $installer -Wait
    }
} catch {
    Write-Warning ("[webview2] 启动安装程序失败: {0}" -f $_.Exception.Message)
    Write-Host "[webview2] 请手动运行: $installer"
    exit 1
}

Write-Host '[webview2] 安装结束，重新检测 ...'
$after = Get-WebView2Info
Show-Status $after

if ($after.Installed -and $after.Version -ge $MinimumVersion) {
    Write-Host '[webview2] 完成。现在可以启动 AniBox。'
    exit 0
}

Write-Host '[webview2] 仍未检测到可用版本。若安装器已提示成功，请重启后重试。'
exit 1
