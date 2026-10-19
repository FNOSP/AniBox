; AniBox NSIS installer hooks
; ---------------------------------------------------------------
; Tauri includes this file before MUI pages are inserted, so the
; MUI_* defines below override the welcome / finish page copy.
; Keep this file UTF-8 with BOM (NSIS 3 + makensis -INPUTCHARSET UTF8).
; ---------------------------------------------------------------

!define MUI_WELCOMEPAGE_TITLE "欢迎安装 AniBox"
!define MUI_WELCOMEPAGE_TEXT "AniBox 是为飞牛影视（fnOS）定制的第三方 Windows 桌面客户端。$\r$\nReact 自绘界面、独立 mpv 播放、Bangumi / TMDB 元数据聚合。$\r$\n$\r$\n• 首页 Hero 轮播 · TMDB / Bangumi 双面板$\r$\n• 沉浸式详情页 · 观影记录与年度报告$\r$\n• mpv 三源弹幕 · PotPlayer · 演员信息入库检测"

!define MUI_FINISHPAGE_TITLE "AniBox 安装完成"
!define MUI_FINISHPAGE_TEXT "AniBox 已安装完成。$\r$\n$\r$\n首次启动前请确认已安装 mpv（或使用本机 PotPlayer），并在设置中登录飞牛影视账号。$\r$\n$\r$\n感谢使用 AniBox。"

; ---------------------------------------------------------------
; [upgrade-4.0] 旧版清理
;
; 为什么不能只靠 installer.nsi 里的 PageReinstall：
;   自绘安装器（SetupUi.cs）在交互模式下只负责收集选项，随后用 /S 静默
;   重新拉起安装器本体。NSIS 的**静默模式会跳过所有页面**（包括 custom
;   page），所以 Tauri 默认的「检测已安装旧版本 → 卸载」页面永远不会执行。
;   结果就是旧版（Electron Fntv-Plus / 旧 Tauri AniBox）留在机器上，
;   新版本另装一处，出现两个程序。
;
; 本钩子在 Section Install 里、写文件之前执行，因此对交互安装与静默安装
; 都生效。覆盖范围：
;   · Electron Fntv-Plus：%LOCALAPPDATA%\Programs\Fntv-Plus、
;     %LOCALAPPDATA%\Fntv-Plus、%PROGRAMFILES%\Fntv-Plus、
;     %PROGRAMFILES64%\Fntv-Plus（注册表卸载项 + 文件系统双路探测）
;   · 旧 Tauri AniBox：HKCU / HKLM64 / HKLM32 下 DisplayName 含 AniBox 的
;     卸载项（含 %LOCALAPPDATA%\AniBox 与 %PROGRAMFILES%\AniBox 两种安装）
;
; 原则：
;   1. 只删程序文件与卸载项/快捷方式，**不动用户数据**
;      （%APPDATA%\fntv、%APPDATA%\com.fntv.plus.tauri2 留给 Rust 迁移）；
;   2. 新旧安装目录相同（原地升级）时跳过卸载，直接覆盖，避免多余卸载；
;   3. 卸载失败不阻断安装，只写日志（%TEMP%\anibox-legacy-uninstall.log）；
;   4. 静默卸载参数统一用 /S；electron-builder 与 Tauri NSIS 都支持。
;
; 仅安装器需要这些函数；卸载器构建时 Tauri 会定义 BUILD_UNINSTALLER。
; ---------------------------------------------------------------
!ifndef BUILD_UNINSTALLER

Var AniBoxLegacyIndex
Var AniBoxLegacyGuard
Var AniBoxLegacyKeyName
Var AniBoxLegacyKeyPath
Var AniBoxLegacyDisplayName
Var AniBoxLegacyLower
Var AniBoxLegacyUninstall
Var AniBoxLegacyDir
Var AniBoxLegacyRc
Var AniBoxLegacyDeleteKey
Var AniBoxLegacyElectronSeen
Var AniBoxLegacyMsg

; 日志：$AniBoxLegacyMsg → DetailPrint + %TEMP%\anibox-legacy-uninstall.log
Function AniBoxLegacyLog
  DetailPrint "$AniBoxLegacyMsg"
  FileOpen $R3 "$TEMP\anibox-legacy-uninstall.log" a
  ${If} $R3 != ""
    FileWrite $R3 "$AniBoxLegacyMsg$\r$\n"
    FileClose $R3
  ${EndIf}
FunctionEnd

; 去掉 InstallLocation 可能带的引号 / 尾反斜杠（输入输出：$AniBoxLegacyDir）
Function AniBoxLegacyTrimDir
  StrCpy $R0 $AniBoxLegacyDir 1
  ${If} $R0 == '"'
    StrCpy $AniBoxLegacyDir $AniBoxLegacyDir "" 1
  ${EndIf}
  StrLen $R0 $AniBoxLegacyDir
  ${If} $R0 > 0
    IntOp $R0 $R0 - 1
    StrCpy $R1 $AniBoxLegacyDir 1 $R0
    ${If} $R1 == '"'
      StrCpy $AniBoxLegacyDir $AniBoxLegacyDir $R0
    ${ElseIf} $R1 == "\"
      StrCpy $AniBoxLegacyDir $AniBoxLegacyDir $R0
    ${EndIf}
  ${EndIf}
FunctionEnd

; 枚举一个注册表视图下的卸载项，匹配 DisplayName 含 Fntv-Plus / AniBox 的项。
; ROOT 必须是字面量（EnumRegKey 不接受变量）：HKCU / HKLM64 / HKLM32。
!macro ANIBOX_SCAN_LEGACY_UNINSTALL ROOT
  !define AniBoxScanID ${__LINE__}
  StrCpy $AniBoxLegacyIndex 0
  StrCpy $AniBoxLegacyGuard 0
  anibox_scan_loop_${AniBoxScanID}:
    IntOp $AniBoxLegacyGuard $AniBoxLegacyGuard + 1
    IntCmp $AniBoxLegacyGuard 1000 anibox_scan_done_${AniBoxScanID} anibox_scan_guard_ok_${AniBoxScanID} anibox_scan_done_${AniBoxScanID}
    anibox_scan_guard_ok_${AniBoxScanID}:
    EnumRegKey $AniBoxLegacyKeyName "${ROOT}" "Software\Microsoft\Windows\CurrentVersion\Uninstall" $AniBoxLegacyIndex
    StrCmp $AniBoxLegacyKeyName "" anibox_scan_done_${AniBoxScanID}
    StrCpy $AniBoxLegacyKeyPath "Software\Microsoft\Windows\CurrentVersion\Uninstall\$AniBoxLegacyKeyName"
    ReadRegStr $AniBoxLegacyDisplayName "${ROOT}" "$AniBoxLegacyKeyPath" "DisplayName"
    StrCmp $AniBoxLegacyDisplayName "" anibox_scan_next_${AniBoxScanID}

    ; 大小写无关匹配：fntv-plus / anibox
    ; 注意：StrLoc 未找到时返回空字符串，找到时返回 0 基位置。
    ; 这里必须先判断“是否为空”，绝不能拿结果和 0 比较 —— 否则会把
    ; 系统里**所有**卸载项都误判成本项目旧版并逐个静默卸载。
    ${StrCase} $AniBoxLegacyLower "$AniBoxLegacyDisplayName" "L"
    ${StrLoc} $R0 "$AniBoxLegacyLower" "fntv-plus" ">"
    ${If} $R0 != ""
      StrCpy $AniBoxLegacyElectronSeen 1
    ${Else}
      ${StrLoc} $R0 "$AniBoxLegacyLower" "anibox" ">"
      ${If} $R0 == ""
        Goto anibox_scan_next_${AniBoxScanID}
      ${EndIf}
    ${EndIf}

    ReadRegStr $AniBoxLegacyUninstall "${ROOT}" "$AniBoxLegacyKeyPath" "UninstallString"
    ReadRegStr $AniBoxLegacyDir "${ROOT}" "$AniBoxLegacyKeyPath" "InstallLocation"
    Call AniBoxLegacyTrimDir

    ; InstallLocation 缺失 → DisplayIcon("...exe,0") 反推
    ${If} $AniBoxLegacyDir == ""
      ReadRegStr $R0 "${ROOT}" "$AniBoxLegacyKeyPath" "DisplayIcon"
      ${If} $R0 != ""
        ${StrLoc} $R1 "$R0" "," ">"
        ${If} $R1 != 0
          IntOp $R1 $R1 - 1
          StrCpy $R0 $R0 $R1
        ${EndIf}
        StrCpy $AniBoxLegacyDir $R0
        Call AniBoxLegacyTrimDir
        ${GetParent} "$AniBoxLegacyDir" $AniBoxLegacyDir
      ${EndIf}
    ${EndIf}

    ; 仍缺失 → 从 UninstallString 的可执行文件路径反推
    ${If} $AniBoxLegacyDir == ""
    ${AndIf} $AniBoxLegacyUninstall != ""
      StrCpy $R0 $AniBoxLegacyUninstall 1
      ${If} $R0 == '"'
        StrCpy $R0 $AniBoxLegacyUninstall "" 1
        ${StrLoc} $R1 "$R0" '"' ">"
        ${If} $R1 != 0
          IntOp $R1 $R1 - 1
          StrCpy $R0 $R0 $R1
        ${EndIf}
      ${Else}
        ${StrLoc} $R1 "$R0" " " ">"
        ${If} $R1 != 0
          IntOp $R1 $R1 - 1
          StrCpy $R0 $R0 $R1
        ${EndIf}
      ${EndIf}
      ${GetParent} "$R0" $AniBoxLegacyDir
      Call AniBoxLegacyTrimDir
    ${EndIf}

    StrCpy $AniBoxLegacyDeleteKey 0

    ; 新旧目录相同 = 原地升级：不卸载，直接覆盖（避免多跑一次卸载器）
    ${If} $AniBoxLegacyDir != ""
      ${StrCase} $R0 "$AniBoxLegacyDir" "L"
      ${StrCase} $R1 "$INSTDIR" "L"
      ${If} $R0 == $R1
        StrCpy $AniBoxLegacyMsg "legacy: same install dir, skip uninstall ($AniBoxLegacyDisplayName)"
        Call AniBoxLegacyLog
        Goto anibox_scan_next_${AniBoxScanID}
      ${EndIf}
    ${EndIf}

    ; 二次安全校验：卸载器路径必须属于 AniBox / Fntv-Plus，否则绝不执行。
    ; 这是防止注册表 DisplayName 误匹配导致卸载无关软件的最后一道闸门。
    ${If} $AniBoxLegacyUninstall != ""
      ${StrCase} $R2 "$AniBoxLegacyUninstall" "L"
      ${StrLoc} $R0 "$R2" "anibox" ">"
      ${If} $R0 == ""
        ${StrLoc} $R0 "$R2" "fntv-plus" ">"
        ${If} $R0 == ""
          StrCpy $AniBoxLegacyMsg "legacy: SKIP suspicious uninstaller ($AniBoxLegacyDisplayName)"
          Call AniBoxLegacyLog
          Goto anibox_scan_next_${AniBoxScanID}
        ${EndIf}
      ${EndIf}
    ${EndIf}

    ${If} $AniBoxLegacyUninstall != ""
      StrCpy $AniBoxLegacyMsg "legacy: uninstalling $AniBoxLegacyDisplayName"
      Call AniBoxLegacyLog
      ${If} $AniBoxLegacyDir != ""
        ExecWait '$AniBoxLegacyUninstall /S _?=$AniBoxLegacyDir' $AniBoxLegacyRc
      ${Else}
        ExecWait '$AniBoxLegacyUninstall /S' $AniBoxLegacyRc
      ${EndIf}
      ${If} $AniBoxLegacyRc == 0
        StrCpy $AniBoxLegacyDeleteKey 1
        StrCpy $AniBoxLegacyMsg "legacy: uninstall OK ($AniBoxLegacyDisplayName)"
      ${Else}
        StrCpy $AniBoxLegacyMsg "legacy: uninstall FAILED rc=$AniBoxLegacyRc ($AniBoxLegacyDisplayName), continue"
      ${EndIf}
      Call AniBoxLegacyLog
    ${Else}
      ; 没有卸载器 → 至少清掉陈旧卸载项
      StrCpy $AniBoxLegacyDeleteKey 1
      StrCpy $AniBoxLegacyMsg "legacy: no UninstallString, remove stale key ($AniBoxLegacyDisplayName)"
      Call AniBoxLegacyLog
    ${EndIf}

    ${If} $AniBoxLegacyDeleteKey == 1
      DeleteRegKey "${ROOT}" "$AniBoxLegacyKeyPath"
      ; 删除后枚举下标会前移，从头再扫一遍
      StrCpy $AniBoxLegacyIndex 0
      Goto anibox_scan_loop_${AniBoxScanID}
    ${EndIf}

  anibox_scan_next_${AniBoxScanID}:
    IntOp $AniBoxLegacyIndex $AniBoxLegacyIndex + 1
    Goto anibox_scan_loop_${AniBoxScanID}
  anibox_scan_done_${AniBoxScanID}:
  !undef AniBoxScanID
!macroend

; 文件系统兜底：注册表项丢失 / 安装器未登记时，仍按已知目录卸载 Electron 版。
Function AniBoxTryElectronUninstall
  ; 输入 $R0 = 安装目录
  ; 与 INSTDIR 相同 = 原地覆盖，不跑卸载器
  ${If} $R0 == $INSTDIR
    Return
  ${EndIf}
  ${If} ${FileExists} "$R0\Uninstall Fntv-Plus.exe"
    StrCpy $AniBoxLegacyMsg "legacy: found Fntv-Plus uninstaller in $R0"
    Call AniBoxLegacyLog
    ExecWait '"$R0\Uninstall Fntv-Plus.exe" /S _?=$R0' $AniBoxLegacyRc
    ${If} $AniBoxLegacyRc == 0
      StrCpy $AniBoxLegacyMsg "legacy: file-based uninstall OK ($R0)"
    ${Else}
      StrCpy $AniBoxLegacyMsg "legacy: file-based uninstall FAILED rc=$AniBoxLegacyRc ($R0), continue"
    ${EndIf}
    Call AniBoxLegacyLog
  ${EndIf}
FunctionEnd

; 文件系统兜底：旧 Tauri AniBox（注册表项丢失时仍能清理）
Function AniBoxTryTauriUninstall
  ; 输入 $R0 = 安装目录；与 INSTDIR 相同则跳过（原地覆盖）
  ${If} $R0 == $INSTDIR
    Return
  ${EndIf}
  ${If} ${FileExists} "$R0\uninstall.exe"
    StrCpy $AniBoxLegacyMsg "legacy: found AniBox uninstaller in $R0"
    Call AniBoxLegacyLog
    ; 不加 _?= : 让卸载器复制到 TEMP 运行，才能完整删除 INSTDIR
    ExecWait '"$R0\uninstall.exe" /S' $AniBoxLegacyRc
    ${If} $AniBoxLegacyRc == 0
      StrCpy $AniBoxLegacyMsg "legacy: file-based Tauri uninstall OK ($R0)"
    ${Else}
      StrCpy $AniBoxLegacyMsg "legacy: file-based Tauri uninstall FAILED rc=$AniBoxLegacyRc ($R0), continue"
    ${EndIf}
    Call AniBoxLegacyLog
  ${EndIf}
FunctionEnd

Function AniBoxRemoveLegacyElectronFiles
  ; 注册表里已经命中过 Fntv-Plus 就不再重复执行（避免二次 UAC 弹窗）
  ${If} $AniBoxLegacyElectronSeen != 1
    StrCpy $R0 "$LOCALAPPDATA\Programs\Fntv-Plus"
    Call AniBoxTryElectronUninstall
    StrCpy $R0 "$LOCALAPPDATA\Fntv-Plus"
    Call AniBoxTryElectronUninstall
    StrCpy $R0 "$PROGRAMFILES\Fntv-Plus"
    Call AniBoxTryElectronUninstall
    StrCpy $R0 "$PROGRAMFILES64\Fntv-Plus"
    Call AniBoxTryElectronUninstall
  ${EndIf}

  ; 旧 Tauri AniBox：与 UI 的 InstallDetect 文件系统兜底保持一致
  StrCpy $R0 "$LOCALAPPDATA\AniBox"
  Call AniBoxTryTauriUninstall
  StrCpy $R0 "$LOCALAPPDATA\Programs\AniBox"
  Call AniBoxTryTauriUninstall
  StrCpy $R0 "$PROGRAMFILES\AniBox"
  Call AniBoxTryTauriUninstall
  StrCpy $R0 "$PROGRAMFILES64\AniBox"
  Call AniBoxTryTauriUninstall
FunctionEnd

; 清掉旧版开始菜单 / 桌面快捷方式（升级模式 /UPDATE 不动，避免误删）
Function AniBoxCleanLegacyShortcuts
  ${GetOptions} $CMDLINE "/UPDATE" $R0
  ${IfNot} ${Errors}
    Return
  ${EndIf}

  SetShellVarContext current
  Delete "$SMPROGRAMS\Fntv-Plus.lnk"
  Delete "$SMPROGRAMS\Fntv-Plus\Fntv-Plus.lnk"
  RMDir "$SMPROGRAMS\Fntv-Plus"
  Delete "$DESKTOP\Fntv-Plus.lnk"

  SetShellVarContext all
  Delete "$SMPROGRAMS\Fntv-Plus.lnk"
  Delete "$SMPROGRAMS\Fntv-Plus\Fntv-Plus.lnk"
  RMDir "$SMPROGRAMS\Fntv-Plus"
  Delete "$DESKTOP\Fntv-Plus.lnk"

  ; 恢复安装模式对应的 shell 上下文（后续写快捷方式依赖它）
  ${GetOptions} $CMDLINE "/currentuser" $R0
  ${IfNot} ${Errors}
    SetShellVarContext current
    Return
  ${EndIf}
  ${GetOptions} $CMDLINE "/allusers" $R0
  ${IfNot} ${Errors}
    SetShellVarContext all
    Return
  ${EndIf}
  SetShellVarContext current
FunctionEnd

Function AniBoxRemoveLegacyInstalls
  StrCpy $AniBoxLegacyElectronSeen 0
  !insertmacro ANIBOX_SCAN_LEGACY_UNINSTALL "HKCU"
  !insertmacro ANIBOX_SCAN_LEGACY_UNINSTALL "HKLM64"
  !insertmacro ANIBOX_SCAN_LEGACY_UNINSTALL "HKLM32"
  Call AniBoxRemoveLegacyElectronFiles
  Call AniBoxCleanLegacyShortcuts
  ; 旧目录可能已被卸载器删除，重新落回安装目录
  CreateDirectory "$INSTDIR"
  SetOutPath "$INSTDIR"
FunctionEnd

; ---------------------------------------------------------------
; [upgrade-3.9] 旧版数据保留
;
; 弹幕 Cookie 在旧安装目录的 uosc_danmaku 脚本目录里；先备份，避免随旧程序
; 一起被卸载。用户数据目录（%APPDATA%\fntv、%APPDATA%\com.fntv.plus.tauri2）
; 不在这里处理，由 Rust（src-tauri/src/migrate.rs）在首次启动时迁移。
; ---------------------------------------------------------------
Function AniBoxPreserveLegacyData
  StrCpy $R0 ""
  ${If} ${FileExists} "$LOCALAPPDATA\Programs\Fntv-Plus\third_party\fntv-mpv\portable_config\scripts\uosc_danmaku\bili_cookie.txt"
    StrCpy $R0 "$LOCALAPPDATA\Programs\Fntv-Plus\third_party\fntv-mpv\portable_config\scripts\uosc_danmaku\bili_cookie.txt"
  ${ElseIf} ${FileExists} "$LOCALAPPDATA\Fntv-Plus\third_party\fntv-mpv\portable_config\scripts\uosc_danmaku\bili_cookie.txt"
    StrCpy $R0 "$LOCALAPPDATA\Fntv-Plus\third_party\fntv-mpv\portable_config\scripts\uosc_danmaku\bili_cookie.txt"
  ${ElseIf} ${FileExists} "$PROGRAMFILES\Fntv-Plus\third_party\fntv-mpv\portable_config\scripts\uosc_danmaku\bili_cookie.txt"
    StrCpy $R0 "$PROGRAMFILES\Fntv-Plus\third_party\fntv-mpv\portable_config\scripts\uosc_danmaku\bili_cookie.txt"
  ${ElseIf} ${FileExists} "$PROGRAMFILES64\Fntv-Plus\third_party\fntv-mpv\portable_config\scripts\uosc_danmaku\bili_cookie.txt"
    StrCpy $R0 "$PROGRAMFILES64\Fntv-Plus\third_party\fntv-mpv\portable_config\scripts\uosc_danmaku\bili_cookie.txt"
  ${ElseIf} ${FileExists} "$LOCALAPPDATA\AniBox\third_party\fntv-mpv\portable_config\scripts\uosc_danmaku\bili_cookie.txt"
    StrCpy $R0 "$LOCALAPPDATA\AniBox\third_party\fntv-mpv\portable_config\scripts\uosc_danmaku\bili_cookie.txt"
  ${ElseIf} ${FileExists} "$PROGRAMFILES\AniBox\third_party\fntv-mpv\portable_config\scripts\uosc_danmaku\bili_cookie.txt"
    StrCpy $R0 "$PROGRAMFILES\AniBox\third_party\fntv-mpv\portable_config\scripts\uosc_danmaku\bili_cookie.txt"
  ${ElseIf} ${FileExists} "$PROGRAMFILES64\AniBox\third_party\fntv-mpv\portable_config\scripts\uosc_danmaku\bili_cookie.txt"
    StrCpy $R0 "$PROGRAMFILES64\AniBox\third_party\fntv-mpv\portable_config\scripts\uosc_danmaku\bili_cookie.txt"
  ${EndIf}

  ${If} $R0 != ""
    ; $APPDATA 受 SetShellVarContext 影响：allUsers 模式下会变成 ProgramData，
    ; 这里临时切回当前用户，保证写到 %APPDATA%\com.anibox.app。
    SetShellVarContext current
    CreateDirectory "$APPDATA\com.anibox.app"
    ${IfNot} ${FileExists} "$APPDATA\com.anibox.app\bili_cookie.txt"
      CopyFiles /SILENT "$R0" "$APPDATA\com.anibox.app"
    ${EndIf}
    ; 恢复安装模式上下文
    ${GetOptions} $CMDLINE "/allusers" $R1
    ${IfNot} ${Errors}
      SetShellVarContext all
    ${EndIf}
  ${EndIf}
FunctionEnd

!endif

!macro NSIS_HOOK_PREINSTALL
  Call AniBoxPreserveLegacyData
  Call AniBoxRemoveLegacyInstalls
!macroend

!macro NSIS_HOOK_POSTINSTALL
!macroend

!macro NSIS_HOOK_PREUNINSTALL
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
!macroend
