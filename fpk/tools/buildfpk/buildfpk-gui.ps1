# buildfpk-gui.ps1 — Fntv-Plus fpk 打包 GUI（PowerShell WinForms 原生窗口）
#
# 设计要点（替代旧版网页 GUI「卡死无反应」）：
#   - 打包引擎 build-fpk.exe 以子进程异步执行，stdout/stderr 事件流实时追加到日志框；
#   - UI 线程从不阻塞（BeginInvoke 跨线程刷新），引擎跑多久界面都不卡；
#   - 产物自动输出到 <仓库根>/release（默认 D:\GitHub\Fntv-Plus\release）。
# 启动：双击 fpk/打包.bat（powershell -File 本脚本）。
# 自检（不弹窗）：powershell -File 本脚本 -SelfTest

param([switch]$SelfTest)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
[System.Windows.Forms.Application]::EnableVisualStyles()

$fpkDir     = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)  # fpk/（脚本位于 tools/buildfpk/）
$engineExe  = Join-Path $fpkDir 'build-fpk.exe'
$releaseDir = Join-Path (Split-Path -Parent $fpkDir) 'release'       # <仓库根>/release

# ── 窗体 ──
$form                 = New-Object System.Windows.Forms.Form
$form.Text            = 'Fntv-Plus fpk 打包工具'
$form.Size            = New-Object System.Drawing.Size(760, 692)
$form.StartPosition   = 'CenterScreen'
$form.FormBorderStyle = 'FixedSingle'
$form.MaximizeBox     = $false

# ── 模式选择 ──
$gbMode             = New-Object System.Windows.Forms.GroupBox
$gbMode.Text        = '打包模式'
$gbMode.Location    = New-Object System.Drawing.Point(16, 12)
$gbMode.Size        = New-Object System.Drawing.Size(710, 128)

$rbDev             = New-Object System.Windows.Forms.RadioButton
$rbDev.Text        = '测试包（版号不动，仅追加 -N 序号；包名 Fntv-Plus-NNN.fpk）'
$rbDev.Location    = New-Object System.Drawing.Point(14, 24)
$rbDev.Size        = New-Object System.Drawing.Size(680, 20)
$rbDev.Checked     = $true

$rbRel             = New-Object System.Windows.Forms.RadioButton
$rbRel.Text        = '正式发布包（需指定版号，包名 Fntv-Plus-X.Y.Z.fpk）'
$rbRel.Location    = New-Object System.Drawing.Point(14, 52)
$rbRel.Size        = New-Object System.Drawing.Size(680, 20)

$lblName           = New-Object System.Windows.Forms.Label
$lblName.Text      = '显示名：'
$lblName.Location  = New-Object System.Drawing.Point(34, 82)
$lblName.Size      = New-Object System.Drawing.Size(60, 20)
$txtName           = New-Object System.Windows.Forms.TextBox
$txtName.Text      = 'Fntv-Plus'
$txtName.Location  = New-Object System.Drawing.Point(98, 79)
$txtName.Size      = New-Object System.Drawing.Size(180, 22)
$txtName.Enabled   = $false

$lblVer            = New-Object System.Windows.Forms.Label
$lblVer.Text       = '正式版号：'
$lblVer.Location   = New-Object System.Drawing.Point(292, 82)
$lblVer.Size       = New-Object System.Drawing.Size(70, 20)
$txtVer            = New-Object System.Windows.Forms.TextBox
$txtVer.Location   = New-Object System.Drawing.Point(366, 79)
$txtVer.Size       = New-Object System.Drawing.Size(120, 22)
$txtVer.Enabled    = $false

$gbMode.Controls.AddRange(@($rbDev, $rbRel, $lblName, $txtName, $lblVer, $txtVer))

# ── 目标架构（lc-1387：x86 / ARM64 / 两者连打；对应引擎 --arch amd64|arm64） ──
$gbArch             = New-Object System.Windows.Forms.GroupBox
$gbArch.Text        = '目标架构'
$gbArch.Location    = New-Object System.Drawing.Point(16, 146)
$gbArch.Size        = New-Object System.Drawing.Size(710, 56)

$rbX86             = New-Object System.Windows.Forms.RadioButton
$rbX86.Text        = 'x86（默认，飞牛 x86 主机）'
$rbX86.Location    = New-Object System.Drawing.Point(14, 22)
$rbX86.Size        = New-Object System.Drawing.Size(196, 20)
$rbX86.Checked     = $true

$rbArm             = New-Object System.Windows.Forms.RadioButton
$rbArm.Text        = 'ARM64（ARM 主机）'
$rbArm.Location    = New-Object System.Drawing.Point(214, 22)
$rbArm.Size        = New-Object System.Drawing.Size(180, 20)

$rbBoth            = New-Object System.Windows.Forms.RadioButton
$rbBoth.Text       = 'x86 + ARM64（依次打两个包）'
$rbBoth.Location   = New-Object System.Drawing.Point(398, 22)
$rbBoth.Size       = New-Object System.Drawing.Size(300, 20)

$gbArch.Controls.AddRange(@($rbX86, $rbArm, $rbBoth))

# ── 输出目录 ──
$lblOut           = New-Object System.Windows.Forms.Label
$lblOut.Text      = '输出目录：'
$lblOut.Location  = New-Object System.Drawing.Point(16, 214)
$lblOut.Size      = New-Object System.Drawing.Size(80, 20)
$txtOut           = New-Object System.Windows.Forms.TextBox
$txtOut.Text      = $releaseDir
$txtOut.ReadOnly  = $true
$txtOut.Location  = New-Object System.Drawing.Point(100, 211)
$txtOut.Size      = New-Object System.Drawing.Size(626, 22)

# ── 操作按钮 ──
$btnStart           = New-Object System.Windows.Forms.Button
$btnStart.Text      = '开始打包'
$btnStart.Location  = New-Object System.Drawing.Point(16, 246)
$btnStart.Size      = New-Object System.Drawing.Size(140, 34)

$btnOpen            = New-Object System.Windows.Forms.Button
$btnOpen.Text       = '打开输出目录'
$btnOpen.Location   = New-Object System.Drawing.Point(170, 246)
$btnOpen.Size       = New-Object System.Drawing.Size(140, 34)

# ── 日志框 ──
$logBox                       = New-Object System.Windows.Forms.TextBox
$logBox.Multiline             = $true
$logBox.ReadOnly              = $true
$logBox.ScrollBars            = 'Vertical'
$logBox.Font                  = New-Object System.Drawing.Font('Consolas', 9)
$logBox.Location              = New-Object System.Drawing.Point(16, 292)
$logBox.Size                  = New-Object System.Drawing.Size(710, 340)
$logBox.BackColor             = [System.Drawing.Color]::FromArgb(24, 22, 32)
$logBox.ForeColor             = [System.Drawing.Color]::Gainsboro

$form.Controls.AddRange(@($gbMode, $gbArch, $lblOut, $txtOut, $btnStart, $btnOpen, $logBox))

$script:running = $false
$script:pending = @()     # [lc-1387] 待执行的架构队列（'' = amd64/x86、'--arch arm64' = ARM64）
$script:isRel   = $false
$script:relName = ''
$script:relVer  = ''

# 日志追加：句柄未创建（ShowDialog 之前）走直接写入；创建后走 BeginInvoke 跨线程刷新
function Append-Log {
    param([string]$Line)
    if ($form.IsHandleCreated) {
        $form.BeginInvoke([System.Action]{
            $logBox.AppendText($Line + [Environment]::NewLine)
            $logBox.SelectionStart = $logBox.Text.Length
            $logBox.ScrollToCaret()
        }) | Out-Null
    } else {
        $logBox.AppendText($Line + [Environment]::NewLine)
    }
}

function Set-UiRunning {
    param([bool]$Busy)
    $form.BeginInvoke([System.Action]{
        $script:running = $Busy
        $btnStart.Enabled = -not $Busy
        $rbDev.Enabled = -not $Busy
        $rbRel.Enabled = -not $Busy
        $rbX86.Enabled = -not $Busy
        $rbArm.Enabled = -not $Busy
        $rbBoth.Enabled = -not $Busy
        if ($Busy) { $btnStart.Text = '打包中…' } else { $btnStart.Text = '开始打包' }
    }) | Out-Null
}

# 引擎缺失时自动用 go 编译（同步执行，编译很快）
function Ensure-Engine {
    if (Test-Path $engineExe) { return $true }
    Append-Log '未找到打包引擎 build-fpk.exe，正在用 go 编译…'
    $go = Get-Command go -ErrorAction SilentlyContinue
    if (-not $go) { Append-Log '[X] 未安装 Go 工具链，无法编译引擎。请安装 Go 1.23+ 后重试。'; return $false }
    Push-Location (Join-Path $fpkDir 'tools\buildfpk')
    try {
        & go build -o $engineExe . 2>&1 | ForEach-Object { Append-Log ("    " + $_) }
    } finally {
        Pop-Location
    }
    if (Test-Path $engineExe) { Append-Log '引擎编译完成。'; return $true }
    Append-Log '[X] 引擎编译失败，详见上方输出。'
    return $false
}

# [lc-1387] 目标架构 → 引擎参数队列（'' = amd64/x86；'--arch arm64' = ARM64）
function Get-ArchTasks {
    if ($rbArm.Checked)  { return @('--arch arm64') }
    if ($rbBoth.Checked) { return @('', '--arch arm64') }
    return @('')
}

function Build-EngineArgs {
    param([string]$ArchFlag)
    if ($script:isRel) {
        $a = 'release --name "' + $script:relName + '" --version "' + $script:relVer + '" --out "' + $releaseDir + '"'
    } else {
        $a = 'build --out "' + $releaseDir + '"'
    }
    if ($ArchFlag -ne '') { $a = $a + ' ' + $ArchFlag }
    return $a
}

# 逐架构串行执行（x86 + ARM64 时连打两个包；任一失败则停止队列）
function Start-NextBuild {
    if ($script:pending.Count -eq 0) { Set-UiRunning $false; Append-Log '全部打包任务结束。'; return }
    $archFlag = $script:pending[0]
    $script:pending = @($script:pending | Select-Object -Skip 1)
    if ($archFlag -eq '') { $label = 'x86 (amd64)' } else { $label = 'ARM64' }
    Append-Log ''
    Append-Log ('---- 开始打包：' + $label + ' ----')

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName               = $engineExe
    $psi.Arguments              = (Build-EngineArgs $archFlag)
    $psi.WorkingDirectory       = $fpkDir
    $psi.UseShellExecute        = $false
    $psi.CreateNoWindow         = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError  = $true
    $psi.StandardOutputEncoding = [System.Text.Encoding]::UTF8
    $psi.StandardErrorEncoding  = [System.Text.Encoding]::UTF8

    $proc = New-Object System.Diagnostics.Process
    $proc.StartInfo = $psi
    $proc.EnableRaisingEvents = $true

    # 显式委托 + param 绑定（PS 5.1 下 add_* 直接传 scriptblock 时 $_ 不可靠）
    $outHandler = [System.Diagnostics.DataReceivedEventHandler]{
        param($sender, $e)
        if ($e -and $e.Data) { Append-Log $e.Data }
    }
    $errHandler = [System.Diagnostics.DataReceivedEventHandler]{
        param($sender, $e)
        if ($e -and $e.Data) { Append-Log ('[stderr] ' + $e.Data) }
    }
    $exitHandler = [System.EventHandler]{
        param($sender, $e)
        $code = -1
        try { $code = $sender.ExitCode } catch { }
        Append-Log ''
        if ($code -eq 0) {
            Append-Log ('[完成] ' + $label + ' 打包成功，产物已输出到: ' + $releaseDir)
            # 队列里还有下一个架构 → 回到 UI 线程继续
            $form.BeginInvoke([System.Action]{
                if ($script:pending.Count -gt 0) {
                    Start-NextBuild
                } else {
                    Set-UiRunning $false
                    Append-Log '全部打包任务结束。'
                }
            }) | Out-Null
        } else {
            Append-Log ('[失败] ' + $label + ' 打包未成功（退出码 ' + $code + '），详见上方日志；队列已停止。')
            Set-UiRunning $false
        }
    }
    $proc.add_OutputDataReceived($outHandler)
    $proc.add_ErrorDataReceived($errHandler)
    $proc.add_Exited($exitHandler)

    try {
        $null = $proc.Start()
        $proc.BeginOutputReadLine()
        $proc.BeginErrorReadLine()
    } catch {
        Append-Log ('[X] 启动引擎失败: ' + $_.Exception.Message)
        Set-UiRunning $false
    }
}

$btnStart.Add_Click({
    if ($script:running) { return }
    $isRel = $rbRel.Checked
    $ver = $txtVer.Text.Trim()
    if ($isRel -and ($txtName.Text.Trim() -eq '')) { Append-Log '[X] 正式发布需要填写显示名。'; return }
    if ($isRel -and ($ver -notmatch '^\d+\.\d+\.\d+$')) { Append-Log '[X] 正式版号须为 x.y.z 三段数字（如 1.3.1）。'; return }
    if (-not (Ensure-Engine)) { return }

    Set-UiRunning $true
    $logBox.Clear()
    Append-Log ('启动打包… ' + (Get-Date -Format 'HH:mm:ss'))
    $script:isRel   = $isRel
    $script:relName = $txtName.Text.Trim()
    $script:relVer  = $ver
    $script:pending = @(Get-ArchTasks)
    Start-NextBuild
})

$btnOpen.Add_Click({
    if (-not (Test-Path $releaseDir)) { New-Item -ItemType Directory -Path $releaseDir | Out-Null }
    Start-Process explorer.exe $releaseDir
})

$rbDev.Add_CheckedChanged({ if ($rbDev.Checked) { $txtName.Enabled = $false; $txtVer.Enabled = $false } })
$rbRel.Add_CheckedChanged({ if ($rbRel.Checked) { $txtName.Enabled = $true; $txtVer.Enabled = $true; $txtVer.Focus() } })

Append-Log ('Fntv-Plus fpk 打包工具就绪。')
$engineState = if (Test-Path $engineExe) { ' [已就绪]' } else { ' [缺失，首次打包时自动编译]' }
Append-Log ('引擎: ' + $engineExe + $engineState)
Append-Log ('输出目录: ' + $releaseDir)
Append-Log '选好模式与目标架构后点「开始打包」，日志会实时滚动在下方。'

if ($SelfTest) {
    # 自检模式：验证 UI 构建/日志写入后直接退出（不弹窗）
    Write-Host 'SELF-TEST-OK'
    $form.Dispose()
    exit 0
}

try {
    [void]$form.ShowDialog()
} catch {
    # 任何未捕获异常弹窗提示（避免窗口闪退无提示）
    $msg = $_.Exception.Message + [Environment]::NewLine + $_.InvocationInfo.PositionMessage
    try { [System.Windows.Forms.MessageBox]::Show($msg, '打包工具异常') | Out-Null } catch { Write-Host $msg }
    throw
}
