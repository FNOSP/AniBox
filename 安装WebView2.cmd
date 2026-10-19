@echo off
REM ============================================================================
REM  AniBox - WebView2 Runtime bootstrap (double-click this file)
REM
REM  Use this when AniBox cannot start / shows a blank window because the
REM  Microsoft WebView2 Runtime is missing on this Windows machine.
REM
REM  It only checks the registry / install folders and, when missing, downloads
REM  the official Microsoft Evergreen Bootstrapper and runs it.
REM  Nothing is bundled in this repository.
REM
REM  Options are forwarded to tools\ensure-webview2.ps1, e.g.:
REM      install-webview2.cmd -CheckOnly
REM      install-webview2.cmd -Silent
REM
REM  Keep this file ASCII-ONLY and CRLF (a .cmd with non-ASCII bytes is split
REM  into broken fragments by the console - same rule as release.cmd).
REM ============================================================================
setlocal enableextensions

set "SCRIPT=%~dp0tools\ensure-webview2.ps1"
if not exist "%SCRIPT%" (
  echo [ERROR] Missing "%SCRIPT%".
  echo         Run this file from the AniBox repository root.
  pause
  exit /b 1
)

set "PS_EXE=powershell"
where pwsh >nul 2>&1
if %errorlevel% equ 0 set "PS_EXE=pwsh"

"%PS_EXE%" -NoProfile -ExecutionPolicy Bypass -File "%SCRIPT%" %*
set "RC=%errorlevel%"

echo.
if %RC% neq 0 echo [webview2] exit code %RC% - runtime still missing or install failed.
if %RC% equ 0 echo [webview2] OK - you can start AniBox now.
pause
exit /b %RC%
