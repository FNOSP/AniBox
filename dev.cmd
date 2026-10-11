@echo off
cd /d "%~dp0"
REM 与主仓库 dev.cmd 同款的一键开发启动：先清残留，再起 Tauri dev。
REM 杀掉上次的 Tauri dev 实例，避免 vite 端口/IPC 管道被旧进程占用
taskkill /f /im fntv-plus-tauri2.exe >nul 2>&1
REM 杀掉 proxy.exe：避免与安装版 Fntv-Plus / 主仓库 dev 实例争抢 22346 代理端口
REM （代价同主仓库：如果安装版正在跑会被断代理，重开即可）
taskkill /f /im proxy.exe >nul 2>&1
REM 故意不杀 mpv.exe：可能与正在观看的实例冲突；上次内嵌的 mpv 在宿主窗口销毁时会自行退出
timeout /t 1 >nul

REM rustup 新装时本 shell 的 PATH 可能未刷新，兜底前插
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

REM 清理占用 vite 固定端口 1420 的残留进程（strictPort，占用即启动失败）
for /f "tokens=5" %%p in ('netstat -ano -p tcp ^| findstr /c:":1420 " ^| findstr /c:"LISTENING"') do (
  echo [dev] Killing stale listener on port 1420 ^(pid=%%p^)
  taskkill /f /pid %%p >nul 2>&1
)

echo [2/2] Starting Tauri dev (first Rust compile takes 5-15 min; F12 for DevTools)...
call npm run tauri dev
