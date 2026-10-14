@echo off
cd /d "%~dp0"
REM One-click dev launcher, mirrors Fntv-Plus dev.cmd.
REM Kill stale Tauri dev instance (frees vite port / IPC pipes)
taskkill /f /im anibox.exe >nul 2>&1
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

REM ---- mirror admission secrets: RUNTIME env only -- nothing injects them at COMPILE time ----
REM No .github/ workflow, no release script and no tauri.conf.json build hook exists in this repo,
REM and build.rs only calls tauri_build::build(), so nothing bakes FNTV_BGM_SECRET /
REM FNTV_TMDB_SECRET into the binary. `tauri build` is not wired to anything here => a packaged
REM build has NEITHER a compile-time secret NOR (for end users) a runtime one, and therefore always
REM takes the fallback paths described below. Baking one in is possible (set the var in the shell
REM that runs `cargo build` / `tauri build`; bgm.rs and tmdb.rs read option_env!) and equals
REM publishing it (Kazumi does the same) -- a deliberate trade-off, not an accident.
REM => missing secrets MUST stay non-fatal. See the two blocks below.

REM ---- [lc-119] Bangumi mirror admission secret (this process only, never written into the repo) ----
REM bgm.rs prefers our mirror (bgm.690075.xyz). A MISSING FNTV_BGM_SECRET is not fatal: it is treated
REM as "mirror unavailable" -> one WARN line in %TEMP%\anibox.log -> straight to api.bgm.tv
REM (next.bgm.tv for /p1/*). Bangumi needs no credentials, so that fallback is fully functional;
REM the mirror only adds edge cache + abuse protection for our own domain. A secret that is PRESENT
REM BUT WRONG still fails loudly (401 bad_signature) -- deterministic rejection, never downgraded.
REM Dev convenience: if the var is unset, read it from the main repo's bgm-mirror/.secret.local
REM (already gitignored there). The value stays in this cmd process env; nothing is written into
REM this repo.
REM NOTE: keep this flattened (single-line ifs, no nested blocks). cmd misparses a nested
REM       "if exist (...) else (...)" inside an outer parenthesized block -- the outer branch is
REM       silently skipped. Verified the hard way on this machine.
set "FNTV_BGM_SECRET_FILE=%~dp0..\Fntv-Plus\bgm-mirror\.secret.local"
if defined FNTV_BGM_SECRET echo [dev] FNTV_BGM_SECRET already set in this shell -- keeping it
if not defined FNTV_BGM_SECRET if exist "%FNTV_BGM_SECRET_FILE%" set /p FNTV_BGM_SECRET=<"%FNTV_BGM_SECRET_FILE%" && set "FNTV_BGM_LOADED=1"
if defined FNTV_BGM_LOADED echo [dev] FNTV_BGM_SECRET loaded from %FNTV_BGM_SECRET_FILE% -- in-process only, not persisted
if not defined FNTV_BGM_SECRET echo [dev] WARN: %FNTV_BGM_SECRET_FILE% missing or empty -- will degrade to direct api.bgm.tv/next.bgm.tv (no mirror cache/abuse layer). Set FNTV_BGM_SECRET to use the mirror.

REM ---- [lc-122] TMDB mirror admission secret (independent key; this process only) ----
REM tmdb.rs prefers the TMDB mirror (tmdb.690075.xyz, signed with FNTV_TMDB_SECRET). A MISSING
REM FNTV_TMDB_SECRET is not fatal: same "mirror unavailable" path as 429 / unreachable / 5xx -> one
REM WARN line in %TEMP%\anibox.log -> direct api.themoviedb.org with the user's own key. A key
REM that is PRESENT BUT WRONG (401 bad_signature) and all other 4xx stay hard failures by design.
REM Dev convenience: if the var is unset, parse it from the main repo's tmdb-mirror/.secret.local.
REM NOTE: that file is "NAME=value" format (unlike bgm's raw-value file), so parse with
REM       for /f "tokens=1,* delims==" -- set /p would hand the whole "FNTV_TMDB_SECRET=..." line
REM       to the process. tokens=1,* keeps every '=' after the first one (base64 padding safe).
set "FNTV_TMDB_SECRET_FILE=%~dp0..\Fntv-Plus\tmdb-mirror\.secret.local"
if not defined FNTV_TMDB_SECRET set "FNTV_TMDB_WAS_UNSET=1"
if defined FNTV_TMDB_SECRET echo [dev] FNTV_TMDB_SECRET already set in this shell -- keeping it
if not defined FNTV_TMDB_SECRET if exist "%FNTV_TMDB_SECRET_FILE%" for /f "usebackq tokens=1,* delims==" %%a in ("%FNTV_TMDB_SECRET_FILE%") do if /i "%%a"=="FNTV_TMDB_SECRET" if not defined FNTV_TMDB_SECRET set "FNTV_TMDB_SECRET=%%b"
if defined FNTV_TMDB_WAS_UNSET if defined FNTV_TMDB_SECRET echo [dev] FNTV_TMDB_SECRET loaded from %FNTV_TMDB_SECRET_FILE% -- in-process only, not persisted
if not defined FNTV_TMDB_SECRET echo [dev] WARN: %FNTV_TMDB_SECRET_FILE% missing or unparsable -- mirror is skipped (no WARN-free path); direct fallback needs your own tmdbApiKey in %APPDATA%\fntv\config.json or FNTV_TMDB_KEY. Set FNTV_TMDB_SECRET to use the mirror.

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
pwsh -NoProfile -ExecutionPolicy Bypass -File "%~dp0tools\dev-diag.ps1"

echo.
echo [3/3] Starting Tauri dev (first Rust compile takes 5-15 min)...
echo       DevTools  : F12 / Ctrl+Shift+I inside the app, or set FNTV_DEVTOOLS=1 before dev.cmd
echo       App log   : %%TEMP%%\anibox.log      (ms timestamps, [win]/[cutout]/[probe]/[play/info]/[imgproto])
echo       mpv log   : %%TEMP%%\anibox-mpv.log
echo       Go proxy  : %%TEMP%%\proxy.log  (only when started by hand)
echo       -> on exit this window prints the tail of both logs.
echo.
call npm run tauri dev
echo.
echo [dev] tauri dev exited with code %errorlevel%.
echo.
echo ======================== last 60 lines: anibox.log ========================
rem 用 pwsh 7 + 显式 -Encoding utf8：日志是 UTF-8，而 powershell 5.1 的 Get-Content
rem 默认按 ANSI(GBK 936) 读 → 中文全乱码。pwsh 也是本仓库约定的唯一 PowerShell。
pwsh -NoProfile -ExecutionPolicy Bypass -Command "Get-Content -LiteralPath $env:TEMP\anibox.log -Tail 60 -Encoding utf8 -ErrorAction SilentlyContinue"
echo ================================================================================
echo ======================== last 20 lines: anibox-mpv.log =====================
pwsh -NoProfile -ExecutionPolicy Bypass -Command "Get-Content -LiteralPath $env:TEMP\anibox-mpv.log -Tail 20 -Encoding utf8 -ErrorAction SilentlyContinue"
echo ================================================================================
pause