@echo off
cd /d "%~dp0"
REM One-click dev launcher, mirrors Fntv-Plus dev.cmd.
REM Kill stale Tauri dev instance (frees vite port / IPC pipes)
taskkill /f /im fntv-plus-tauri2.exe >nul 2>&1
REM Kill stale proxy.exe to avoid 22346 port clash with installed Fntv-Plus / main-repo dev
taskkill /f /im proxy.exe >nul 2>&1
REM Intentionally NOT killing mpv.exe (it may be playing something you are watching).
%SystemRoot%\System32\timeout.exe /t 1 >nul

REM rustup fresh install may not have refreshed this shell's PATH - prepend just in case
set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"

REM ---- [lc-115] diagnostics knobs -------------------------------------------------
REM DevTools no longer opens on every start. To get it back:
REM     set FNTV_DEVTOOLS=1  &&  dev.cmd          (or press F12 / Ctrl+Shift+I in the app)
if not defined FNTV_DEVTOOLS set "FNTV_DEVTOOLS=0"
REM Chromium/WebView2 own logging to the console (protocol errors, CSS/JS problems, ...)
set "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--enable-logging=stderr --v=0"
if not defined RUST_LOG set "RUST_LOG=info"
REM Unattended playback smoke test (plays 5s after start, results land in the app log):
REM     set FNTV_AUTOPLAY_TEST=<item-guid>  &&  dev.cmd
REM Inject arbitrary JS into the webview 5s after start:
REM     set FNTV_EVAL_JS=<js>  &&  dev.cmd
REM ---------------------------------------------------------------------------------

where cargo >nul 2>&1
if %errorlevel% neq 0 (
  echo [ERROR] cargo not found. Install Rust first: winget install Rustlang.Rustup
  pause
  exit /b 1
)

if not exist node_modules (
  echo [1/3] Installing npm dependencies...
  call npm install
  if %errorlevel% neq 0 (
    echo npm install FAILED.
    pause
    exit /b %errorlevel%
  )
) else (
  echo [1/3] npm dependencies OK
)

REM Free vite fixed port 1420 (strictPort: startup fails if occupied).
REM NOTE: on Windows a busy port makes bind() fail with EACCES (permission denied)
REM rather than EADDRINUSE -- easy to misread as a rights problem. Also this
REM machine's ephemeral range starts at 1024 (netsh int ipv4 show dynamicport tcp),
REM so ANY process can transiently grab 1420 as a source port. Observed in practice:
REM NetEase UU booster (uu.exe) held 1420 on its 198.18.0.x virtual adapter, which
REM showed up as an intermittent 'vite: listen EACCES 127.0.0.1:1420' failure.
REM So: kill by port in EVERY state (not just LISTENING), skip pid 0 (TIME_WAIT
REM entries belong to already-dead sockets and must not be reported as holders).
for /f "tokens=5" %%p in ('netstat -ano -p tcp ^| findstr /c:":1420 "') do (
  if not "%%p"=="0" (
    echo [dev] Port 1420 held by pid=%%p -- killing
    taskkill /f /pid %%p >nul 2>&1
  )
)
netstat -ano -p tcp | findstr /c:":1420 " | findstr /c:"LISTENING" >nul 2>&1
if not errorlevel 1 (
  echo [dev] WARN: something is still LISTENING on 1420 after cleanup.
  echo [dev]   netstat -ano -p tcp ^| findstr /c:":1420 "
  echo [dev]   If vite then fails with EACCES: quit the UU booster / Clash TUN proxy, or run this elevated:
  echo [dev]   netsh int ipv4 set dynamicport tcp start=49152 num=16384
)

echo [2/3] Environment diagnostics (saves you a round trip when something breaks)...
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0tools\dev-diag.ps1"

echo.
echo [3/3] Starting Tauri dev (first Rust compile takes 5-15 min)...
echo       DevTools  : F12 / Ctrl+Shift+I inside the app, or set FNTV_DEVTOOLS=1 before dev.cmd
echo       App log   : %%TEMP%%\fntv-libmpv.log      (ms timestamps, [win]/[cutout]/[probe]/[play/info]/[imgproto])
echo       mpv log   : %%TEMP%%\fntv-tauri2-mpv.log
echo       Go proxy  : %%TEMP%%\proxy.log  (only when started by hand)
echo       -> on exit this window prints the tail of both logs.
echo.
call npm run tauri dev
echo.
echo [dev] tauri dev exited with code %errorlevel%.
echo.
echo ======================== last 60 lines: fntv-libmpv.log ========================
powershell -NoProfile -ExecutionPolicy Bypass -Command "Get-Content -LiteralPath $env:TEMP\fntv-libmpv.log -Tail 60 -ErrorAction SilentlyContinue"
echo ================================================================================
echo ======================== last 20 lines: fntv-tauri2-mpv.log =====================
powershell -NoProfile -ExecutionPolicy Bypass -Command "Get-Content -LiteralPath $env:TEMP\fntv-tauri2-mpv.log -Tail 20 -ErrorAction SilentlyContinue"
echo ================================================================================
pause