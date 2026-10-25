@echo off
REM AniBox GUI packer (lc-1807; lc-1815 hide console): version / overwrite / output dir with state memory.
REM ASCII-only + CRLF per repo convention. Local build only, no upload.
REM start + -WindowStyle Hidden: the launcher cmd exits immediately (no lingering black box);
REM -WindowStyle hides pwsh's own console only -- the WinForms window still shows normally.
start "" pwsh -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "%~dp0tools\packager-gui.ps1" %*
exit /b
