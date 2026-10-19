@echo off
REM ==================================================================
REM  AniBox - double-click packaging entry
REM
REM  Delegates to tools\build-installer.ps1 which:
REM    * injects mirror secrets from secrets\
REM    * cleans up leftover build tools (cargo/rustc/makensis/link)
REM    * retries on "os error 32 / file in use" errors (up to 3x)
REM    * copies the NSIS installer to release\ and prints its SHA-256
REM
REM  Keep this file ASCII-ONLY and CRLF (see release.cmd for why).
REM ==================================================================

setlocal
cd /d "%~dp0"

pwsh -NoProfile -ExecutionPolicy Bypass -File "%~dp0tools\build-installer.ps1" %*
set "RC=%errorlevel%"

echo.
if %RC% neq 0 echo [pack] build FAILED with code %RC%
if %RC% equ 0 echo [pack] done.
pause
exit /b %RC%
