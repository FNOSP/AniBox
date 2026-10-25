#!/usr/bin/env pwsh
#Requires -Version 7.0
<#
.SYNOPSIS
    AniBox 打包 GUI（lc-1807 / lc-1814 死锁修复 / lc-1815 美化+实时日志）。
.DESCRIPTION
    WinForms 扁平化前端套 tools/build-installer.ps1（密钥注入/重试/校验逻辑不变）：
      · UI：玻璃蓝紫 accent（与主程序同色系）、卡片分组、终端风日志区、窗口可调大小；
      · 实时进度：构建中 stdout 异步读 + **stderr 增量轮询**（cargo 编译行全走 stderr，
        落盘 %TEMP% 后按字节偏移增量回放进日志区，\r 刷新的进度行也拆行显示）；
      · cmd 全隐藏：外层 cmd 用 start -WindowStyle Hidden 闪没；构建子进程 CreateNoWindow。
    仅本地构建，无任何上传动作。
#>
[CmdletBinding()]
param([switch]$StopApp)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

$RepoRoot = Split-Path -Parent $PSScriptRoot
$StateFile = Join-Path $env:APPDATA 'com.anibox.app\packager-gui.json'

# ── 主题色（对齐主程序 tokens） ──
$C_BG      = [System.Drawing.Color]::FromArgb(244, 246, 251)   # 窗口底
$C_CARD    = [System.Drawing.Color]::FromArgb(252, 253, 255)   # 卡片
$C_LINE    = [System.Drawing.Color]::FromArgb(224, 229, 240)   # 卡片描边
$C_ACCENT  = [System.Drawing.Color]::FromArgb(108, 155, 255)   # #6c9bff
$C_ACCENT2 = [System.Drawing.Color]::FromArgb(143, 108, 255)   # #8f6cff
$C_ACCENT_DK = [System.Drawing.Color]::FromArgb(90, 138, 240)  # hover 深
$C_TEXT    = [System.Drawing.Color]::FromArgb(34, 43, 64)
$C_MUTED   = [System.Drawing.Color]::FromArgb(120, 130, 150)
$C_TERM_BG = [System.Drawing.Color]::FromArgb(30, 30, 46)      # 日志终端底
$C_TERM_FG = [System.Drawing.Color]::FromArgb(205, 214, 244)   # 日志字
$F_MAIN = New-Object System.Drawing.Font('Segoe UI', 9.5)
$F_BOLD = New-Object System.Drawing.Font('Segoe UI', 9.5, [System.Drawing.FontStyle]::Bold)
$F_TITLE = New-Object System.Drawing.Font('Segoe UI', 14, [System.Drawing.FontStyle]::Bold)
$F_MONO = New-Object System.Drawing.Font('Consolas', 9)

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

$curVersion = '4.1.0'
try {
    $curVersion = (Get-Content -LiteralPath (Join-Path $RepoRoot 'src-tauri/tauri.conf.json') -Raw |
        ConvertFrom-Json).version
} catch {}

# ── 小工具：圆角卡片 / 扁平按钮 ──
function New-RoundPath([int]$w, [int]$h, [int]$r) {
    $p = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = 2 * $r
    $p.AddArc(0, 0, $d, $d, 180, 90); $p.AddArc($w - $d, 0, $d, $d, 270, 90)
    $p.AddArc($w - $d, $h - $d, $d, $d, 0, 90); $p.AddArc(0, $h - $d, $d, $d, 90, 90)
    $p.CloseFigure(); $p
}
function New-Card([int]$x, [int]$y, [int]$w, [int]$h) {
    $pn = New-Object System.Windows.Forms.Panel
    $pn.Location = New-Object System.Drawing.Point($x, $y)
    $pn.Size = New-Object System.Drawing.Size($w, $h)
    $pn.BackColor = $C_CARD
    $pn.Region = New-Object System.Drawing.Region((New-RoundPath $w $h 10))
    $pn.Add_Paint({
        param($sender, $e)
        $e.Graphics.SmoothingMode = 'AntiAlias'
        $e.Graphics.DrawPath((New-Object System.Drawing.Pen($script:C_LINE, 1)), (New-RoundPath $sender.Width $sender.Height 10))
    })
    $pn
}
function Set-FlatButton($btn, [System.Drawing.Color]$bg, [System.Drawing.Color]$fg, [System.Drawing.Color]$hoverBg) {
    $btn.FlatStyle = 'Flat'
    $btn.BackColor = $bg
    $btn.ForeColor = $fg
    $btn.Font = $F_BOLD
    $btn.Cursor = 'Hand'
    $btn.FlatAppearance.BorderSize = 0
    $btn.FlatAppearance.MouseOverBackColor = $hoverBg
    $btn.Region = New-Object System.Drawing.Region((New-RoundPath $btn.Width $btn.Height 8))
}

# ── 窗体（可调大小；双缓冲防闪） ──
$f = New-Object System.Windows.Forms.Form
$f.Text = 'AniBox 打包'
$f.Font = $F_MAIN
$f.Size = New-Object System.Drawing.Size(620, 640)
$f.MinimumSize = New-Object System.Drawing.Size(620, 560)
$f.StartPosition = 'CenterScreen'
$f.BackColor = $C_BG
$f.FormBorderStyle = 'Sizable'
$f.MaximizeBox = $true
# DoubleBuffered 是 protected 属性，PowerShell 直接赋值会报错——走反射开双缓冲防闪
$f.GetType().GetProperty('DoubleBuffered', [System.Reflection.BindingFlags]'Instance,NonPublic').SetValue($f, $true)

# ── 标题区：渐变 logo 方块 + 大标题 ──
$logo = New-Object System.Windows.Forms.Panel
$logo.Location = New-Object System.Drawing.Point(20, 18)
$logo.Size = New-Object System.Drawing.Size(40, 40)
$logo.Region = New-Object System.Drawing.Region((New-RoundPath 40 40 11))
$logo.Add_Paint({
    param($sender, $e)
    $rect = New-Object System.Drawing.Rectangle(0, 0, $sender.Width, $sender.Height)
    $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush($rect, $script:C_ACCENT, $script:C_ACCENT2, 35)
    $e.Graphics.FillRectangle($brush, $rect)
    $fmt = New-Object System.Drawing.StringFormat
    $fmt.Alignment = 'Center'; $fmt.LineAlignment = 'Center'
    $e.Graphics.DrawString('A', (New-Object System.Drawing.Font('Segoe UI', 15, [System.Drawing.FontStyle]::Bold)), [System.Drawing.Brushes]::White, $rect, $fmt)
})
$f.Controls.Add($logo)

$lbTitle = New-Object System.Windows.Forms.Label
$lbTitle.Text = 'AniBox 打包'
$lbTitle.Font = $F_TITLE
$lbTitle.ForeColor = $C_TEXT
$lbTitle.Location = New-Object System.Drawing.Point(72, 18)
$lbTitle.AutoSize = $true
$f.Controls.Add($lbTitle)
$lbSub = New-Object System.Windows.Forms.Label
$lbSub.Text = "本地构建 · 密钥注入 · 当前版本 $curVersion"
$lbSub.ForeColor = $C_MUTED
$lbSub.Location = New-Object System.Drawing.Point(74, 46)
$lbSub.AutoSize = $true
$f.Controls.Add($lbSub)

# ── 输入卡片 ──
$card = New-Card 20 74 ($f.ClientSize.Width - 40) 118
$f.Controls.Add($card)

$lb1 = New-Object System.Windows.Forms.Label
$lb1.Text = '版本号'; $lb1.Location = '18,16'; $lb1.AutoSize = $true; $lb1.ForeColor = $C_TEXT
$card.Controls.Add($lb1)
$tbVer = New-Object System.Windows.Forms.TextBox
$tbVer.Location = New-Object System.Drawing.Point(90, 12)
$tbVer.Size = New-Object System.Drawing.Size(150, 26)
$tbVer.Font = $F_MAIN
if ($state.LastVersion) { $tbVer.Text = $state.LastVersion } else { $tbVer.Text = $curVersion }
$card.Controls.Add($tbVer)
$lbCur = New-Object System.Windows.Forms.Label
$lbCur.Text = "tauri.conf.json：$curVersion"
$lbCur.Location = New-Object System.Drawing.Point(252, 16); $lbCur.AutoSize = $true; $lbCur.ForeColor = $C_MUTED
$card.Controls.Add($lbCur)

$cbOver = New-Object System.Windows.Forms.CheckBox
$cbOver.Text = '覆盖同名安装包（不勾 = 自动加时间戳后缀）'
$cbOver.Location = New-Object System.Drawing.Point(18, 46)
$cbOver.AutoSize = $true; $cbOver.ForeColor = $C_TEXT
$cbOver.Checked = [bool]$state.Overwrite
$card.Controls.Add($cbOver)

$lb2 = New-Object System.Windows.Forms.Label
$lb2.Text = '出包位置'; $lb2.Location = '18,80'; $lb2.AutoSize = $true; $lb2.ForeColor = $C_TEXT
$card.Controls.Add($lb2)
$tbOut = New-Object System.Windows.Forms.TextBox
$tbOut.Location = New-Object System.Drawing.Point(90, 76)
$tbOut.Size = New-Object System.Drawing.Size(330, 26)
$tbOut.Font = $F_MAIN
$tbOut.Text = $state.OutDir
$card.Controls.Add($tbOut)
$btnPick = New-Object System.Windows.Forms.Button
$btnPick.Text = '浏览…'
$btnPick.Location = New-Object System.Drawing.Point(430, 74)
$btnPick.Size = New-Object System.Drawing.Size(78, 30)
Set-FlatButton $btnPick $C_CARD $C_ACCENT $C_BG
$card.Controls.Add($btnPick)
$btnPick.Add_Click({
    $fb = New-Object System.Windows.Forms.FolderBrowserDialog
    $fb.Description = '选择安装包输出目录'
    if ($fb.ShowDialog() -eq 'OK') { $tbOut.Text = $fb.SelectedPath }
})

# ── 操作行：开始打包 + 步骤进度 ──
$btnBuild = New-Object System.Windows.Forms.Button
$btnBuild.Text = '开始打包'
$btnBuild.Location = New-Object System.Drawing.Point(20, 206)
$btnBuild.Size = New-Object System.Drawing.Size(132, 40)
Set-FlatButton $btnBuild $C_ACCENT ([System.Drawing.Color]::White) $C_ACCENT_DK
$f.Controls.Add($btnBuild)

$lbStep = New-Object System.Windows.Forms.Label
$lbStep.Text = '就绪'
$lbStep.ForeColor = $C_MUTED
$lbStep.Location = New-Object System.Drawing.Point(168, 210)
$lbStep.AutoSize = $true
$f.Controls.Add($lbStep)
$lbElapsed = New-Object System.Windows.Forms.Label
$lbElapsed.Text = ''
$lbElapsed.ForeColor = $C_MUTED
$lbElapsed.Location = New-Object System.Drawing.Point(168, 230)
$lbElapsed.AutoSize = $true
$f.Controls.Add($lbElapsed)

$pb = New-Object System.Windows.Forms.ProgressBar
$pb.Location = New-Object System.Drawing.Point(($f.ClientSize.Width - 240), 216)
$pb.Size = New-Object System.Drawing.Size(220, 18)
$pb.Style = 'Continuous'
$pb.Value = 0
$pb.Anchor = 'Top,Right'
$f.Controls.Add($pb)

# ── 日志区（终端风，窗口拉伸时跟随） ──
$lbLogTitle = New-Object System.Windows.Forms.Label
$lbLogTitle.Text = '构建日志（实时）'
$lbLogTitle.ForeColor = $C_MUTED
$lbLogTitle.Location = New-Object System.Drawing.Point(22, 258)
$lbLogTitle.AutoSize = $true
$f.Controls.Add($lbLogTitle)
$tbLog = New-Object System.Windows.Forms.TextBox
$tbLog.Multiline = $true; $tbLog.ReadOnly = $true
$tbLog.ScrollBars = 'Vertical'
$tbLog.BackColor = $C_TERM_BG
$tbLog.ForeColor = $C_TERM_FG
$tbLog.Font = $F_MONO
$tbLog.BorderStyle = 'None'
$tbLog.Location = New-Object System.Drawing.Point(20, 280)
$tbLog.Size = New-Object System.Drawing.Size(($f.ClientSize.Width - 40), ($f.ClientSize.Height - 320))
$tbLog.Anchor = 'Top,Bottom,Left,Right'
$f.Controls.Add($tbLog)

function Log-Line($m) {
    if ($tbLog.TextLength -gt 400000) { $tbLog.Text = $tbLog.Text.Substring(200000) }  # 防无限增长
    $tbLog.AppendText("$m`r`n")
    $tbLog.SelectionStart = $tbLog.TextLength
    $tbLog.ScrollToCaret()
    [System.Windows.Forms.Application]::DoEvents()
}
function Set-Step([string]$text, [int]$pct) {
    $lbStep.Text = $text
    if ($pct -lt 0) {
        $pb.Style = 'Marquee'; $pb.MarqueeAnimationSpeed = 25   # 构建中不确定进度
    } else {
        $pb.Style = 'Continuous'; $pb.Value = [Math]::Min(100, [Math]::Max(0, $pct))
    }
    [System.Windows.Forms.Application]::DoEvents()
}

$btnBuild.Add_Click({
    $ver = $tbVer.Text.Trim()
    if ($ver -notmatch '^\d+(\.\d+){1,3}$') {
        [void][System.Windows.Forms.MessageBox]::Show("版本号格式：数字点分（如 4.1.0）", 'AniBox 打包')
        return
    }
    $out = $tbOut.Text.Trim()
    if (-not $out) { [void][System.Windows.Forms.MessageBox]::Show('出包位置不能为空'); return }
    $btnBuild.Enabled = $false
    $tbLog.Clear()
    $swAll = [System.Diagnostics.Stopwatch]::StartNew()
    try {
        if ($ver -ne $curVersion) {
            Set-Step "① 写入版本号 $ver …" 8
            Log-Line "1/4 写入版本号 $ver（tauri.conf.json + Cargo.toml）…"
            $confP = Join-Path $RepoRoot 'src-tauri/tauri.conf.json'
            $conf = Get-Content -LiteralPath $confP -Raw | ConvertFrom-Json
            $conf.version = $ver
            $conf | ConvertTo-Json -Depth 30 | Set-Content -LiteralPath $confP -Encoding utf8
            $cargoP = Join-Path $RepoRoot 'src-tauri/Cargo.toml'
            $cargo = Get-Content -LiteralPath $cargoP -Raw
            $cargo = [regex]::Replace($cargo, '(?m)^(version\s*=\s*)"[^"]+"', "`$1`"$ver`"", 1)
            Set-Content -LiteralPath $cargoP -Value $cargo -Encoding utf8
        } else {
            Set-Step '① 版本一致，跳过改写' 8
            Log-Line "1/4 版本与 tauri.conf.json 一致（$ver），跳过改写"
        }

        Set-Step '② 构建（cargo + tauri + NSIS）…' -1
        Log-Line "2/4 运行 tools\build-installer.ps1（密钥注入+重试+SHA-256）…"
        # [lc-1814] 死锁修法：stderr 用 cmd /c … 2>文件 在 OS 层落盘（cargo 编译行全走
        # stderr，管道版必互等死锁）；stdout 单管道异步读。构建中**增量回放** errlog
        # （字节偏移推进，\r 刷新的 cargo 进度行也拆行显示）→ 日志区实时滚动。
        $errLog = Join-Path $env:TEMP "anibox-packager-err-$PID.log"
        if (Test-Path -LiteralPath $errLog) { Remove-Item -LiteralPath $errLog -Force }
        $psi = New-Object System.Diagnostics.ProcessStartInfo
        $psi.FileName = 'cmd.exe'
        $psi.Arguments = "/c chcp 65001>nul & pwsh -NoProfile -ExecutionPolicy Bypass -File `"$RepoRoot\tools\build-installer.ps1`"$(if ($StopApp) { ' -StopApp' }) 2>`"$errLog`""
        $psi.WorkingDirectory = $RepoRoot
        $psi.UseShellExecute = $false
        $psi.RedirectStandardOutput = $true
        $psi.RedirectStandardError = $true
        $psi.CreateNoWindow = $true
        $proc = [System.Diagnostics.Process]::Start($psi)
        $t = $null
        $errOff = 0L
        $lastPoll = [System.Diagnostics.Stopwatch]::StartNew()
        $lineCount = 0
        while (-not $proc.HasExited) {
            if ($null -eq $t -or $t.IsCompleted) { $t = $proc.StandardOutput.ReadLineAsync() }
            $spin = 0
            while (-not $t.IsCompleted -and -not $proc.HasExited -and $spin -lt 200) {
                [System.Windows.Forms.Application]::DoEvents()
                Start-Sleep -Milliseconds 40
                $spin += 40
            }
            if ($t.IsCompleted -and $t.Result) { Log-Line $t.Result.TrimEnd(); $lineCount++ }
            # stderr 增量回放（节流 ~1.2s）：读字节偏移后的新增段，按 \r/\n 拆行
            if ($lastPoll.ElapsedMilliseconds -gt 1200 -and (Test-Path -LiteralPath $errLog)) {
                $lastPoll.Restart()
                try {
                    $bytes = [System.IO.File]::ReadAllBytes($errLog)
                    if ($bytes.Length -gt $errOff) {
                        $chunk = [System.Text.Encoding]::UTF8.GetString($bytes, [int]$errOff, [int]($bytes.Length - $errOff))
                        # 只消费「完整段」：截到最后一个换行/回车，余量留给下一轮
                        $cut = [Math]::Max($chunk.LastIndexOf("`n"), $chunk.LastIndexOf("`r"))
                        if ($cut -ge 0) {
                            $errOff += $cut + 1
                            $chunk.Substring(0, $cut + 1) -split "`r|`n" | Where-Object { $_ -ne '' } | ForEach-Object {
                                Log-Line $_; $lineCount++
                            }
                        }
                    }
                } catch {}
                $lbElapsed.Text = ("已输出 {0} 行 · 用时 {1:mm\:ss}" -f $lineCount, $swAll.Elapsed)
                [System.Windows.Forms.Application]::DoEvents()
            }
        }
        # 排干：先等在飞的 ReadLineAsync 完成（task 未完成时流被占用，同步 ReadLine 会抛
        # "stream is in use"），再同步排干缓存行；errlog 剩余段最后补一轮。
        $dw = [System.Diagnostics.Stopwatch]::StartNew()
        while ($null -ne $t -and -not $t.IsCompleted -and $dw.ElapsedMilliseconds -lt 3000) {
            [System.Windows.Forms.Application]::DoEvents()
            Start-Sleep -Milliseconds 20
        }
        if ($null -ne $t -and $t.IsCompleted -and $t.Result) { Log-Line $t.Result.TrimEnd() }
        while ($null -ne ($line = $proc.StandardOutput.ReadLine())) { Log-Line $line.TrimEnd() }
        if (Test-Path -LiteralPath $errLog) {
            try {
                $bytes = [System.IO.File]::ReadAllBytes($errLog)
                if ($bytes.Length -gt $errOff) {
                    [System.Text.Encoding]::UTF8.GetString($bytes, [int]$errOff, [int]($bytes.Length - $errOff)) -split "`r|`n" |
                        Where-Object { $_ -ne '' } | ForEach-Object { Log-Line $_ }
                }
            } catch {}
        }
        if ($proc.ExitCode -ne 0) {
            Set-Step '构建失败' 100
            Log-Line "构建失败（code $($proc.ExitCode)，详见上方日志）"
            [void][System.Windows.Forms.MessageBox]::Show('构建失败，详见日志区', 'AniBox 打包')
            return
        }
        Set-Step '③ 收集安装包…' 90
        Log-Line "3/4 收集安装包到 $out …"
        if (-not (Test-Path -LiteralPath $out)) { New-Item -ItemType Directory -Force -Path $out | Out-Null }
        $name = "AniBox_${ver}_x64-setup.exe"
        $src = Join-Path $RepoRoot "src-tauri\target\release\bundle\nsis\$name"
        if (-not (Test-Path -LiteralPath $src)) {
            $src = Get-ChildItem -LiteralPath (Join-Path $RepoRoot 'src-tauri\target\release\bundle\nsis') -Filter '*setup.exe' |
                Sort-Object LastWriteTime -Descending | Select-Object -First 1 -ExpandProperty FullName
        }
        if (-not $src -or -not (Test-Path -LiteralPath $src)) {
            Set-Step '未找到安装包产物' 100
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
        Set-Step '④ 完成' 100
        Log-Line "4/4 完成：$dest"
        Log-Line ("大小 {0} MB · SHA-256 {1}" -f $mb, $sha)
        Log-Line ("总用时 {0:mm\:ss}" -f $swAll.Elapsed)

        if (Test-Path -LiteralPath $errLog) { Remove-Item -LiteralPath $errLog -Force -ErrorAction SilentlyContinue }
        $state.OutDir = $out; $state.Overwrite = [bool]$cbOver.Checked; $state.LastVersion = $ver
        Save-State
        [void][System.Windows.Forms.MessageBox]::Show("打包完成`n$dest", 'AniBox 打包')
    } catch {
        Set-Step '异常' 100
        Log-Line "异常：$($_.Exception.Message)"
        [void][System.Windows.Forms.MessageBox]::Show("打包异常：$($_.Exception.Message)", 'AniBox 打包')
    } finally {
        $btnBuild.Enabled = $true
    }
})

[void]$f.ShowDialog()
