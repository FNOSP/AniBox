#!/usr/bin/env node
/**
 * [lc-1350] 生成「用户一键热补丁」单文件 cmd：
 *   patches/hotfix-lc1350/patcher.js  --(base64 内嵌)-->  patches/Fntv-Plus热补丁-lc1350-PotPlayer僵尸流修复.cmd
 *
 * cmd 结构：头部为纯批处理逻辑（定位安装目录 → 自解压内嵌补丁器到 %TEMP% →
 * 用应用自带 Electron 以 ELECTRON_RUN_AS_NODE 方式执行补丁器 → 回显结果），
 * 尾部 exit /b 之后的 base64 载荷永不执行。
 *
 * 用法: node scripts/make-hotpatch-cmd.mjs
 * 测试辅助（不用于生产）: 环境变量 FNTV_ELECTRON_EXE=可执行文件、
 *   FNTV_PATCH_DIR_OVERRIDE=补丁目录，可在测试环境跑通全流程而不碰真实安装。
 */
import * as fs from 'node:fs';
import * as path from 'node:path';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const patcherPath = path.join(root, 'patches', 'hotfix-lc1350', 'patcher.js');
const outPath = path.join(root, 'patches', 'Fntv-Plus热补丁-lc1350-PotPlayer僵尸流修复.cmd');

// 0) 语法自检
execFileSync(process.execPath, ['--check', patcherPath], { stdio: 'inherit' });

// 1) base64（76 列折行，certutil 可直接解码）
const patcherSrc = fs.readFileSync(patcherPath);
const b64 = patcherSrc.toString('base64').replace(/(.{76})/g, '$1\n').trimEnd();
const b64Lines = b64.split('\n');
if (b64Lines.some((l) => /[^A-Za-z0-9+/=]/.test(l))) {
    throw new Error('base64 行含非法字符，cmd 提取逻辑会破坏载荷');
}

const L = [
    // ⚠️ 本文件所有输出行必须纯 ASCII：chcp 65001 下 cmd 解析含中文的批处理行存在
    // 字节错位风险（多字节字符被切开、echo 片段被当命令执行）。面向用户的中文说明
    // 一律由 patcher.js（Node 走 WriteConsoleW，任意代码页安全）打印。
    '@echo off',
    'chcp 65001 >nul 2>nul',
    'setlocal EnableExtensions',
    'title Fntv-Plus Hotfix lc-1350 (PotPlayer zombie-stream fix)',
    'echo ============================================================',
    'echo   Fntv-Plus hotfix lc-1350',
    'echo   Fix: PotPlayer zombie streams saturating your NAS gigabit link',
    'echo        (playback stuck at 3-4 MB/s, NAS keeps uploading even',
    'echo         after closing PotPlayer).',
    'echo   The detailed Chinese instructions are printed below.',
    'echo ============================================================',
    'echo.',
    '',
    'rem ---------- args: [install-dir] [restore] ----------',
    'set "RESTORE="',
    'set "INSTALLDIR="',
    'if /i "%~1"=="restore" (set "RESTORE=1") else if not "%~1"=="" set "INSTALLDIR=%~1"',
    'if /i "%~2"=="restore" set "RESTORE=1"',
    '',
    'rem ---------- locate install dir (needed as the JS runtime; restore also needs it) ----------',
    'if defined INSTALLDIR goto :have_dir',
    'if exist "%~dp0resources\\app.asar" set "INSTALLDIR=%~dp0"',
    'if defined INSTALLDIR goto :have_dir',
    'call :regfind "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall"',
    'if defined INSTALLDIR goto :have_dir',
    'call :regfind "HKLM\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall"',
    'if defined INSTALLDIR goto :have_dir',
    'call :regfind "HKLM\\Software\\Wow6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall"',
    'if defined INSTALLDIR goto :have_dir',
    'if exist "%LOCALAPPDATA%\\Programs\\fntv\\resources\\app.asar" set "INSTALLDIR=%LOCALAPPDATA%\\Programs\\fntv"',
    'if defined INSTALLDIR goto :have_dir',
    'if exist "%LOCALAPPDATA%\\Programs\\Fntv-Plus\\resources\\app.asar" set "INSTALLDIR=%LOCALAPPDATA%\\Programs\\Fntv-Plus"',
    ':have_dir',
    'if defined RESTORE goto :run',
    'if not defined INSTALLDIR (',
    '  echo [FAIL] Fntv-Plus install directory not found.',
    '  echo   Option 1: copy this file into the install root -- the folder',
    '  echo            that contains "resources" -- and run it again;',
    '  echo   Option 2: run from a console with the install dir as argument:',
    '  echo            "%~nx0" "D:\\your-install-dir"',
    '  pause',
    '  exit /b 1',
    ')',
    'if not exist "%INSTALLDIR%\\resources\\app.asar" (',
    '  echo [FAIL] resources\\app.asar not found under %INSTALLDIR% .',
    '  pause',
    '  exit /b 1',
    ')',
    'echo [INFO] Install dir: %INSTALLDIR%',
    '',
    ':run',
    'rem ---------- self-extract the patcher ----------',
    'set "WORK=%TEMP%\\fntv-hotfix-lc1350"',
    'if not exist "%WORK%" md "%WORK%" >nul 2>nul',
    'set "B64=%WORK%\\patcher.b64"',
    'set "PATCHER=%WORK%\\patcher.js"',
    'if exist "%B64%" del "%B64%" >nul 2>nul',
    'set "INBLOB="',
    'for /f "tokens=1,* delims=:" %%A in (\'findstr /n "^" "%~f0"\') do (',
    '  if "%%B"=="__PATCHER_B64_END__" set "INBLOB="',
    '  if defined INBLOB >>"%B64%" echo(%%B',
    '  if "%%B"=="__PATCHER_B64_BEGIN__" set "INBLOB=1"',
    ')',
    'if not exist "%B64%" (',
    '  echo [FAIL] self-extract failed - embedded payload not found.',
    '  pause',
    '  exit /b 1',
    ')',
    'certutil -f -decode "%B64%" "%PATCHER%" >nul 2>nul',
    'if errorlevel 1 (',
    '  echo [FAIL] embedded payload base64 decode failed - see certutil above.',
    '  pause',
    '  exit /b 1',
    ')',
    '',
    'rem ---------- runtime: prefer the bundled Electron (ELECTRON_RUN_AS_NODE); FNTV_ELECTRON_EXE overrides for tests ----------',
    'set "EXE="',
    'if defined FNTV_ELECTRON_EXE if exist "%FNTV_ELECTRON_EXE%" set "EXE=%FNTV_ELECTRON_EXE%"',
    'if not defined EXE if defined INSTALLDIR if exist "%INSTALLDIR%\\Fntv-Plus.exe" set "EXE=%INSTALLDIR%\\Fntv-Plus.exe"',
    'if not defined EXE if defined INSTALLDIR for %%F in ("%INSTALLDIR%\\*.exe") do (',
    '  if not defined EXE if /i not "%%~nxF"=="Uninstall Fntv-Plus.exe" if /i not "%%~nxF"=="Uninstall.exe" set "EXE=%%~fF"',
    ')',
    'if not defined EXE (',
    '  if defined RESTORE echo [FAIL] restore could not locate the install dir to use as the JS runtime.',
    '  if defined RESTORE echo   Copy this file into the install root -- the folder that contains "resources" -- and retry.',
    '  if not defined RESTORE echo [FAIL] no runnable exe found in the install dir.',
    '  pause',
    '  exit /b 1',
    ')',
    '',
    'rem ---------- run the patcher ----------',
    'rem ---------- if the app is running, ask the user to exit first, so the',
    'rem ---------- version marker in config.json is not clobbered by write-back ----------',
    'if defined RESTORE goto :exec',
    'tasklist /FI "IMAGENAME eq Fntv-Plus.exe" /NH 2>nul | findstr /i "Fntv-Plus.exe" >nul',
    'if not errorlevel 1 (',
    '  echo [WARN] Fntv-Plus is currently running.',
    '  echo   Please EXIT it now, then press any key here to continue patching...',
    '  if defined FNTV_HOTPATCH_NO_PAUSE goto :exec',
    '  pause >nul',
    ')',
    ':exec',
    'set "ELECTRON_RUN_AS_NODE=1"',
    'set "RARG="',
    'if defined RESTORE set "RARG=--restore"',
    'if defined FNTV_PATCH_DIR_OVERRIDE goto :run_override',
    'if defined INSTALLDIR "%EXE%" "%PATCHER%" --asar "%INSTALLDIR%\\resources\\app.asar" --exe "%EXE%" --patches "%APPDATA%\\fntv\\patches" --patches-alt "%APPDATA%\\Fntv-Plus\\patches" %RARG%',
    'if not defined INSTALLDIR "%EXE%" "%PATCHER%" --patches "%APPDATA%\\fntv\\patches" --patches-alt "%APPDATA%\\Fntv-Plus\\patches" %RARG%',
    'goto :run_done',
    ':run_override',
    'if defined INSTALLDIR "%EXE%" "%PATCHER%" --asar "%INSTALLDIR%\\resources\\app.asar" --exe "%EXE%" --patches "%FNTV_PATCH_DIR_OVERRIDE%" %RARG%',
    'if not defined INSTALLDIR "%EXE%" "%PATCHER%" --patches "%FNTV_PATCH_DIR_OVERRIDE%" %RARG%',
    ':run_done',
    'set "EC=%ERRORLEVEL%"',
    'del "%B64%" "%PATCHER%" >nul 2>nul',
    'echo.',
    'if "%EC%"=="0" (echo [DONE] success. Press any key to close.) else (echo [FAIL] exit code %EC% - please screenshot the full output above.)',
    'if defined FNTV_HOTPATCH_NO_PAUSE exit /b %EC%',
    'pause',
    'exit /b %EC%',
    '',
    'rem ---------- locate install dir from registry (electron-builder leaves InstallLocation empty; derive from DisplayIcon) ----------',
    ':regfind',
    'for /f "usebackq delims=" %%K in (`reg query %1 /s /f "Fntv-Plus.exe" /d 2^>nul ^| findstr /i /c:"HKEY_"`) do (',
    '  if not defined INSTALLDIR call :fromkey "%%K"',
    ')',
    'goto :eof',
    '',
    ':fromkey',
    'set "ICON="',
    'for /f "tokens=2*" %%A in (\'reg query %1 /v DisplayIcon 2^>nul ^| findstr /i "DisplayIcon"\') do set "ICON=%%B"',
    'if not defined ICON goto :eof',
    'set "ICON=%ICON:,0=%"',
    'for %%F in ("%ICON%") do set "CAND=%%~dpF"',
    'if exist "%CAND%resources\\app.asar" set "INSTALLDIR=%CAND%"',
    'goto :eof',
    '',
    '__PATCHER_B64_BEGIN__',
    ...b64Lines,
    '__PATCHER_B64_END__',
    '',
];

fs.writeFileSync(outPath, L.join('\r\n') + '\r\n', 'utf8'); // CRLF + UTF-8(无 BOM)，与仓库既有 cmd 一致
const size = fs.statSync(outPath).size;
console.log(`[make-hotpatch-cmd] 已生成 ${path.relative(root, outPath)} (${size} bytes, patcher ${patcherSrc.length} bytes → base64 ${b64Lines.length} 行)`);
