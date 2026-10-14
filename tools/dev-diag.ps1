# dev-diag.ps1 - environment snapshot printed by dev.cmd before `tauri dev`.
# Pure ASCII on purpose: a .cmd/.ps1 with non-ASCII text gets mangled by the OEM codepage.
$ErrorActionPreference = 'SilentlyContinue'

function Line($k, $v) { "{0,-22}: {1}" -f $k, $v }

Write-Host "  OS                 : $((Get-CimInstance Win32_OperatingSystem).Caption) build $((Get-CimInstance Win32_OperatingSystem).BuildNumber)"
Line "cargo" ((cargo --version) -join ' ')
Line "node" (node --version)
Line "npm" (npm --version)
Line "rustc" ((rustc --version) -join ' ')

# WebView2 runtime is the usual suspect for "blank page" / "custom protocol dead".
$wvExe = @(
  Get-ChildItem -Path "$env:ProgramFiles\Microsoft\EdgeWebView\Application\*\msedgewebview2.exe" -ErrorAction SilentlyContinue
  Get-ChildItem -Path "${env:ProgramFiles(x86)}\Microsoft\EdgeWebView\Application\*\msedgewebview2.exe" -ErrorAction SilentlyContinue
) | Select-Object -First 1
Line "WebView2 runtime" ($(if ($wvExe) { $wvExe.Directory.Name + '  (' + $wvExe.FullName + ')' } else { 'NOT FOUND - Edge WebView2 runtime missing' }))

# config: only non-secret fields; whether a password/token exist is what matters
$cfgPath = Join-Path $env:APPDATA 'com.anibox.app\config.json'
if (Test-Path $cfgPath) {
    try {
        $c = Get-Content -Raw -LiteralPath $cfgPath | ConvertFrom-Json
        Line "config server" $c.server
        Line "config user" $c.username
        Line "token present" ([bool]$c.token)
        Line "password saved" ([bool]$c.password)
        Line "auto_login" $c.auto_login
        Line "mpv path" $c.mpv_path
        Line "proxy path" $c.proxy_path
    } catch { Line "config" "unreadable ($_)" }
} else {
    Line "config" "MISSING ($cfgPath) - first run will ask for login"
}

# do the referenced binaries exist?
$cfg = $null
if (Test-Path $cfgPath) { try { $cfg = Get-Content -Raw -LiteralPath $cfgPath | ConvertFrom-Json } catch {} }
if ($cfg) {
    if ($cfg.mpv_path)  { Line "mpv exists"   (Test-Path -LiteralPath $cfg.mpv_path) }
    if ($cfg.proxy_path) { Line "proxy exists" (Test-Path -LiteralPath $cfg.proxy_path) }
}

# ports: 1420 = vite, 22346 = Go proxy
foreach ($p in 1420, 22346) {
    $hit = netstat -ano -p tcp | Select-String ":$p " | Select-Object -First 1
    Line "port $p" ($(if ($hit) { $hit.Line.Trim() } else { 'free' }))
}

# relevant env knobs currently set
foreach ($k in 'FNTV_DEVTOOLS', 'FNTV_CUTOUT', 'FNTV_AUTOPLAY_TEST', 'FNTV_EVAL_JS', 'RUST_LOG') {
    $v = [Environment]::GetEnvironmentVariable($k)
    if ($v) { Line "env $k" $v }
}

# logs that will be produced
foreach ($f in 'anibox.log', 'anibox-mpv.log') {
    $p = Join-Path $env:TEMP $f
    $info = Get-Item -LiteralPath $p -ErrorAction SilentlyContinue
    Line "log $f" ($(if ($info) { "$($info.Length) bytes, modified $($info.LastWriteTime)" } else { 'not created yet' }))
}