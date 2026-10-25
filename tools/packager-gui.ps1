#!/usr/bin/env pwsh
#Requires -Version 7.0
<#
.SYNOPSIS
    AniBox 打包 GUI（lc-1807）：选版本号 / 是否覆盖同名安装包 / 出包位置（状态记忆）。
.DESCRIPTION
    WinForms 前端套 tools/build-installer.ps1（全部密钥注入/重试/校验逻辑不变）：
      · 版本号：预填 tauri.conf.json 当前值，可改（改后即时改写 tauri.conf.json +
        src-tauri/Cargo.toml 的 version，保证 exe 元数据与安装包名一致）；
      · 覆盖同名：勾选 = 出包到目标目录时覆盖同名文件；不勾 = 同名存在则自动加时间戳后缀；
      · 出包位置：默认 release\，可改任意目录；目录、覆盖开关、上次版本号存
        %APPDATA%\com.anibox.app\packager-gui.json（状态记忆，跨启动）。
    仅本地构建，无任何上传动作。
#>
[CmdletBinding()]
param([switch]$StopApp)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

$RepoRoot = Split-Path -Parent $PSScriptRoot
$StateFile = Join-Path $env:APPDATA 'com.anibox.app\packager-gui.json'

# ── 状态读写 ──
$state = @{ OutDir = Join-Path $RepoRoot 'release'; Overwrite = $true; LastVersion = '' }
if (Test-Path -LiteralPath $StateFile) {
    try {
        $j = Get-Content -LiteralPath $StateFile -Raw | ConvertFrom-Json
        if ($j.outDir) { $state.OutDir = $j.outDir }
        if ($null -ne $j.overwrite) { $state.Overwrite = [bool]$j.overwrite }
        if ($j.lastVersion) { $state.LastVersion = $j.lastVersion }
    } catch {}
}
function Save-State {
    try {
        $dir = Split-Path -Parent $StateFile
        if (-not (Test-Path -LiteralPath $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
        @{ outDir = $state.OutDir; overwrite = $state.Overwrite; lastVersion = $state.LastVersion } |
            ConvertTo-Json | Set-Content -LiteralPath $StateFile -Encoding utf8
    } catch {}
}

# ── 当前版本预填 ──
$curVersion = '4.1.0'
try {
    $curVersion = (Get-Content -LiteralPath (Join-Path $RepoRoot 'src-tauri/tauri.conf.json') -Raw |
        ConvertFrom-Json).version
} catch {}

# ── GUI ──
$f = New-Object System.Windows.Forms.Form
$f.Text = 'AniBox 打包'
$f.Size = New-Object System.Drawing.Size(560, 330)
$f.FormBorderStyle = 'FixedDialog'
$f.StartPosition = 'CenterScreen'
$f.MaximizeBox = $false

$lb1 = New-Object System.Windows.Forms.Label
$lb1.Text = '版本号'; $lb1.Location = '20,22'; $lb1.AutoSize = $true
$f.Controls.Add($lb1)
$tbVer = New-Object System.Windows.Forms.TextBox
$tbVer.Location = '110,18'; $tbVer.Size = '160,24'
if ($state.LastVersion) { $tbVer.Text = $state.LastVersion } else { $tbVer.Text = $curVersion }
$f.Controls.Add($tbVer)
$lbCur = New-Object System.Windows.Forms.Label
$lbCur.Text = "tauri.conf.json 当前：$curVersion"
$lbCur.Location = '285,22'; $lbCur.AutoSize = $true; $lbCur.ForeColor = 'Gray'
$f.Controls.Add($lbCur)

$cbOver = New-Object System.Windows.Forms.CheckBox
$cbOver.Text = '覆盖同名安装包'
$cbOver.Location = '20,60'; $cbOver.AutoSize = $true
$cbOver.Checked = [bool]$state.Overwrite
$f.Controls.Add($cbOver)

$lb2 = New-Object System.Windows.Forms.Label
$lb2.Text = '出包位置'; $lb2.Location = '20,100'; $lb2.AutoSize = $true
$f.Controls.Add($lb2)
$tbOut = New-Object System.Windows.Forms.TextBox
$tbOut.Location = '110,96'; $tbOut.Size = '330,24'; $tbOut.Text = $state.OutDir
$f.Controls.Add($tbOut)
$btnPick = New-Object System.Windows.Forms.Button
$btnPick.Text = '选择…'; $btnPick.Location = '448,95'; $btnPick.Size = '80,26'
$f.Controls.Add($btnPick)
$btnPick.Add_Click({
    $fb = New-Object System.Windows.Forms.FolderBrowserDialog
    $fb.Description = '选择安装包输出目录'
    if ($fb.ShowDialog() -eq 'OK') { $tbOut.Text = $fb.SelectedPath }
})

$btnBuild = New-Object System.Windows.Forms.Button
$btnBuild.Text = '开始打包'
$btnBuild.Location = '20,140'; $btnBuild.Size = '140,36'
$f.Controls.Add($btnBuild)

$lbState = New-Object System.Windows.Forms.Label
$lbState.Text = '就绪'
$lbState.Location = '175,148'; $lbState.AutoSize = $true; $lbState.ForeColor = 'Gray'
$f.Controls.Add($lbState)
$tbLog = New-Object System.Windows.Forms.TextBox
$tbLog.Multiline = $true; $tbLog.ReadOnly = $true; $tbLog.ScrollBars = 'Vertical'
$tbLog.Font = New-Object System.Drawing.Font('Consolas', 9)
$tbLog.Location = '20,190'; $tbLog.Size = '508,90'
$f.Controls.Add($tbLog)

function Log-Line($m) { $tbLog.AppendText("$m`r`n"); $tbLog.ScrollToCaret(); [System.Windows.Forms.Application]::DoEvents() }

$btnBuild.Add_Click({
    $ver = $tbVer.Text.Trim()
    if ($ver -notmatch '^\d+(\.\d+){1,3}$') {
        [System.Windows.Forms.MessageBox]::Show("版本号格式：数字点分（如 4.1.0）", 'AniBox 打包') | Out-Null
        return
    }
    $out = $tbOut.Text.Trim()
    if (-not $out) { [System.Windows.Forms.MessageBox]::Show('出包位置不能为空') | Out-Null; return }
    $btnBuild.Enabled = $false
    $tbLog.Clear()
    try {
        Log-Line "1/4 写入版本号 $ver（tauri.conf.json + Cargo.toml）…"
        $confP = Join-Path $RepoRoot 'src-tauri/tauri.conf.json'
        $conf = Get-Content -LiteralPath $confP -Raw | ConvertFrom-Json
        $conf.version = $ver
        $conf | ConvertTo-Json -Depth 30 | Set-Content -LiteralPath $confP -Encoding utf8
        $cargoP = Join-Path $RepoRoot 'src-tauri/Cargo.toml'
        $cargo = Get-Content -LiteralPath $cargoP -Raw
        $cargo = [regex]::Replace($cargo, '(?m)^(version\s*=\s*)"[^"]+"', "`$1`"$ver`"", 1)
        Set-Content -LiteralPath $cargoP -Value $cargo -Encoding utf8

        Log-Line "2/4 运行 tools\build-installer.ps1（密钥注入+重试+SHA-256）…"
        $psi = New-Object System.Diagnostics.ProcessStartInfo
        $psi.FileName = 'pwsh'
        $psi.Arguments = "-NoProfile -ExecutionPolicy Bypass -File `"$RepoRoot\tools\build-installer.ps1`"$(if ($StopApp) { ' -StopApp' })"
        $psi.WorkingDirectory = $RepoRoot
        $psi.UseShellExecute = $false
        $psi.RedirectStandardOutput = $true
        $psi.RedirectStandardError = $true
        $psi.CreateNoWindow = $true
        $proc = [System.Diagnostics.Process]::Start($psi)
        while (-not $proc.HasExited) {
            $line = $proc.StandardOutput.ReadLine()
            if ($line) { Log-Line $line.TrimEnd() }
            [System.Windows.Forms.Application]::DoEvents()
        }
        $errTail = $proc.StandardError.ReadToEnd()
        if ($proc.ExitCode -ne 0) {
            Log-Line "构建失败（code $($proc.ExitCode)）$errTail"
            [System.Windows.Forms.MessageBox]::Show('构建失败，详见日志区', 'AniBox 打包') | Out-Null
            return
        }

        Log-Line "3/4 收集安装包到 $out …"
        if (-not (Test-Path -LiteralPath $out)) { New-Item -ItemType Directory -Force -Path $out | Out-Null }
        $name = "AniBox_${ver}_x64-setup.exe"
        $src = Join-Path $RepoRoot "src-tauri\target\release\bundle\nsis\$name"
        if (-not (Test-Path -LiteralPath $src)) {
            $src = Get-ChildItem -LiteralPath (Join-Path $RepoRoot 'src-tauri\target\release\bundle\nsis') -Filter '*setup.exe' |
                Sort-Object LastWriteTime -Descending | Select-Object -First 1 -ExpandProperty FullName
        }
        if (-not $src -or -not (Test-Path -LiteralPath $src)) {
            Log-Line '未找到安装包产物'; return
        }
        $dest = Join-Path $out $name
        if ((Test-Path -LiteralPath $dest) -and -not $cbOver.Checked) {
            $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
            $dest = Join-Path $out "AniBox_${ver}_x64-setup.$stamp.exe"
            Log-Line '同名安装包存在：改用时间戳文件名'
        }
        Copy-Item -LiteralPath $src -Destination $dest -Force
        $sha = (Get-FileHash -LiteralPath $dest -Algorithm SHA256).Hash
        $mb = [math]::Round((Get-Item -LiteralPath $dest).Length / 1MB, 2)
        Log-Line "4/4 完成：$dest"
        Log-Line ("大小 {0} MB · SHA-256 {1}" -f $mb, $sha)

        $state.OutDir = $out; $state.Overwrite = [bool]$cbOver.Checked; $state.LastVersion = $ver
        Save-State
        [System.Windows.Forms.MessageBox]::Show("打包完成`n$dest", 'AniBox 打包') | Out-Null
    } catch {
        Log-Line "异常：$($_.Exception.Message)"
        [System.Windows.Forms.MessageBox]::Show("打包异常：$($_.Exception.Message)", 'AniBox 打包') | Out-Null
    } finally {
        $btnBuild.Enabled = $true
        $lbState.Text = '就绪'
    }
})

[void]$f.ShowDialog()
