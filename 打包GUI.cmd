@echo off
REM AniBox GUI packer (lc-1807): version / overwrite / output dir with state memory.
REM ASCII-only + CRLF per repo convention. Local build only, no upload.
pwsh -NoProfile -ExecutionPolicy Bypass -File "%~dp0tools\packager-gui.ps1" %*
