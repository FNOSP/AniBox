@echo off
cd /d "%~dp0"
REM One-click dev launcher, mirrors Fntv-Plus dev.cmd.
REM Kill stale Tauri dev instance (frees vite port / IPC pipes)
taskkill /f /im fntv-plus-tauri2.exe >nul 2>&1
REM Kill stale proxy.exe to avoid 22346 port clash with installed Fntv-Plus / main-repo dev
taskkill /f /im proxy.exe >nul 2>&1
REM Intentionally NOT killing mpv.exe (may kill a session you are watching;
REM the embedded mpv exits by itself once its host window is destroyed)
%SystemRoot%\System32\timeout.exe /t 1 >nul

REM rustup fresh install may not have refreshed this shell's PATH - prepend just in case
set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"

where cargo >nul 2>&1
if %errorlevel% neq 0 (
  echo [ERROR] cargo not found. Install Rust first: winget install Rustlang.Rustup
  pause
  exit /b 1
)

if not exist node_modules (
  echo [1/2] Installing npm dependencies...
  call npm install
  if %errorlevel% neq 0 (
    echo npm install FAILED.
    pause
    exit /b %errorlevel%
  )
) else (
  echo [1/2] npm dependencies OK
)

REM Free vite fixed port 1420 (strictPort: startup fails if occupied)
for /f "tokens=5" %%p in ('netstat -ano -p tcp ^| findstr /c:":1420 " ^| findstr /c:"LISTENING"') do (
  echo [dev] Killing stale listener on port 1420 ^(pid=%%p^)
  taskkill /f /pid %%p >nul 2>&1
)

echo [2/2] Starting Tauri dev (first Rust compile takes 5-15 min; F12 for DevTools)...
call npm run tauri dev
echo.
echo [dev] tauri dev exited with code %errorlevel%.
pause
