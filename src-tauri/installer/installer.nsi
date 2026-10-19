; installer.nsi — AniBox 自绘安装器模板（Tauri NSIS 自定义模板）
; ─────────────────────────────────────────────────────────────────────────────
; 【架构】NSIS 只当打包内核：
;   ▍交互安装：用户双击安装包（非静默）→ .onInit 解压 UI 四件套
;     (AniBoxSetupUi.exe + WebView2 DLL + www.zip + app-meta/totalsize)
;     到 %TEMP%\anibox-setup-<pid> → 启动 UI 进程 → 本进程 Quit（一个页面都不建）。
;     UI 用 HTML/CSS 绘制四页（欢迎/许可 → 安装选项 → 进度 → 完成），选定模式与
;     路径后以 /S 静默参数重新拉起本安装器本体 → 走 Tauri NSIS 原生安装链路
;     （卸载器 / 注册表 / 快捷方式 全部零改动）。
;   ▍静默启动(/S)：自动更新、命令行部署 → 不拉 UI，原生静默安装。
; 【进度协议】UI 以 /_IPC=<dir> 传 IPC 目录（带引号，兼容带空格 TEMP 路径），
;   本进程在钩子写 started.flag / extract.flag / done.flag；百分比由 UI 轮询
;   INSTDIR 字节数、以 www 内 totalsize.txt 为分母换算。
; 【安装模式】currentUser（默认，%LOCALAPPDATA%\AniBox）与 allUsers
;   （%PROGRAMFILES%\AniBox，需要管理员）。UI 对 allUsers 用 runas 拉起安装器。
; 【可回退】本模板与资源都在 src-tauri/installer/；删除 tauri.conf.json 的
;   bundle.windows.nsis.template 即回到 Tauri 默认向导。
; ⚠ 本文件必须带 UTF-8 BOM（否则中文注释按 ANSI 读编译失败）。
; ─────────────────────────────────────────────────────────────────────────────
Unicode true
ManifestDPIAware true
ManifestDPIAwareness PerMonitorV2

!if "{{compression}}" == "none"
  SetCompress off
!else
  SetCompressor /SOLID "{{compression}}"
!endif

{{#if signed_plugins_path}}
!addplugindir "{{signed_plugins_path}}"
{{/if}}

!include MUI2.nsh
!include LogicLib.nsh
!include FileFunc.nsh
!include x64.nsh
!include WordFunc.nsh
!include "utils.nsh"
!include "FileAssociation.nsh"
!include "Win\COM.nsh"
!include "Win\Propkey.nsh"
!include "Win\RestartManager.nsh"
!include "StrFunc.nsh"
${StrCase}
${StrLoc}

{{#if installer_hooks}}
!include "{{installer_hooks}}"
{{/if}}

; 自绘 UI 资材目录（由 tools/build-setup-ui.mjs 在 beforeBundleCommand 生成）。
; 渲染后的 .nsi 位于 <target>\release\nsis\<arch>\，向上四级即 src-tauri。
!define ANIBOX_STAGE "${__FILEDIR__}\..\..\..\..\installer\staged"

!define WEBVIEW2APPGUID "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"

!define MANUFACTURER "{{manufacturer}}"
!define PRODUCTNAME "{{product_name}}"
!define VERSION "{{version}}"
!define VERSIONWITHBUILD "{{version_with_build}}"
!define HOMEPAGE "{{homepage}}"
!define INSTALLMODE "{{install_mode}}"
!define LICENSE "{{license}}"
!define INSTALLERICON "{{installer_icon}}"
!define SIDEBARIMAGE "{{sidebar_image}}"
!define HEADERIMAGE "{{header_image}}"
!define UNINSTALLERICON "{{uninstaller_icon}}"
!define UNINSTALLERHEADERIMAGE "{{uninstaller_header_image}}"
!define MAINBINARYNAME "{{main_binary_name}}"
!define MAINBINARYSRCPATH "{{main_binary_path}}"
!define BUNDLEID "{{bundle_id}}"
!define COPYRIGHT "{{copyright}}"
!define OUTFILE "{{out_file}}"
!define ARCH "{{arch}}"
!define ADDITIONALPLUGINSPATH "{{additional_plugins_path}}"
!define ALLOWDOWNGRADES "{{allow_downgrades}}"
!define DISPLAYLANGUAGESELECTOR "{{display_language_selector}}"
!define INSTALLWEBVIEW2MODE "{{install_webview2_mode}}"
!define WEBVIEW2INSTALLERARGS "{{webview2_installer_args}}"
!define WEBVIEW2BOOTSTRAPPERPATH "{{webview2_bootstrapper_path}}"
!define WEBVIEW2INSTALLERPATH "{{webview2_installer_path}}"
!define MINIMUMWEBVIEW2VERSION "{{minimum_webview2_version}}"
!define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCTNAME}"
!define MANUKEY "Software\${MANUFACTURER}"
!define MANUPRODUCTKEY "${MANUKEY}\${PRODUCTNAME}"
!define UNINSTALLERSIGNCOMMAND "{{uninstaller_sign_cmd}}"
!define ESTIMATEDSIZE "{{estimated_size}}"
!define STARTMENUFOLDER "{{start_menu_folder}}"

Var PassiveMode
Var UpdateMode
Var NoShortcutMode
Var WixMode
Var OldMainBinaryName
; 安装模式：0=currentUser（默认）, 1=allUsers
Var InstallModeAll
!ifndef BUILD_UNINSTALLER
  Var ipcDir
  Var uiBase
!endif
; ── 自绘卸载 UI（un.onInit 解压 → 启动 → 本进程退出）──
Var UnIpcDir
Var UnPurgeData
Var UnUiBase

Name "${PRODUCTNAME}"
BrandingText "${COPYRIGHT}"
OutFile "${OUTFILE}"

; 自定义模板自行处理提权：交互 UI 对 allUsers 用 runas；这里 manifest 保持 asInvoker。
RequestExecutionLevel user

; 仅供 NSIS 目录选择页追加产品名；实际默认目录在 .onInit 里按模式设置。
!define PLACEHOLDER_INSTALL_DIR "placeholder\${PRODUCTNAME}"
InstallDir "${PLACEHOLDER_INSTALL_DIR}"

VIProductVersion "${VERSIONWITHBUILD}"
VIAddVersionKey "ProductName" "${PRODUCTNAME}"
VIAddVersionKey "FileDescription" "${PRODUCTNAME}"
VIAddVersionKey "LegalCopyright" "${COPYRIGHT}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"

!addplugindir "${ADDITIONALPLUGINSPATH}"

!if "${UNINSTALLERSIGNCOMMAND}" != ""
  !uninstfinalize '${UNINSTALLERSIGNCOMMAND}'
!endif

; Installer icon
!if "${INSTALLERICON}" != ""
  !define MUI_ICON "${INSTALLERICON}"
!endif

; Installer sidebar image
!if "${SIDEBARIMAGE}" != ""
  !define MUI_WELCOMEFINISHPAGE_BITMAP "${SIDEBARIMAGE}"
!endif

; Header images
!if "${HEADERIMAGE}" != ""
  !define MUI_HEADERIMAGE
!else if "${UNINSTALLERHEADERIMAGE}" != ""
  !define MUI_HEADERIMAGE
!endif
!if "${HEADERIMAGE}" != ""
  !define MUI_HEADERIMAGE_BITMAP "${HEADERIMAGE}"
!endif
!if "${UNINSTALLERHEADERIMAGE}" != ""
  !define MUI_HEADERIMAGE_UNBITMAP "${UNINSTALLERHEADERIMAGE}"
!endif
!if "${UNINSTALLERICON}" != ""
  !define MUI_UNICON "${UNINSTALLERICON}"
!endif

!define MUI_LANGDLL_REGISTRY_ROOT "HKCU"
!define MUI_LANGDLL_REGISTRY_KEY "${MANUPRODUCTKEY}"
!define MUI_LANGDLL_REGISTRY_VALUENAME "Installer Language"

; ── 原生向导页（交互安装会在 .onInit 提前 Quit，实际不会显示）──
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
!insertmacro MUI_PAGE_WELCOME

!if "${LICENSE}" != ""
  !define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
  !insertmacro MUI_PAGE_LICENSE "${LICENSE}"
!endif

; Reinstall / uninstall page (only if previous install detected)
Var ReinstallPageCheck
Page custom PageReinstall PageLeaveReinstall
Function PageReinstall
  StrCpy $0 0
  wix_loop:
    EnumRegKey $1 HKLM "SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall" $0
    StrCmp $1 "" wix_loop_done
    IntOp $0 $0 + 1
    ReadRegStr $R0 HKLM "SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\$1" "DisplayName"
    ReadRegStr $R1 HKLM "SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\$1" "Publisher"
    StrCmp "$R0$R1" "${PRODUCTNAME}${MANUFACTURER}" 0 wix_loop
    ReadRegStr $R0 HKLM "SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\$1" "UninstallString"
    ${StrCase} $R1 $R0 "L"
    ${StrLoc} $R0 $R1 "msiexec" ">"
    StrCmp $R0 0 0 wix_loop_done
    StrCpy $WixMode 1
    StrCpy $R6 "SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\$1"
    Goto compare_version
  wix_loop_done:

  ReadRegStr $R0 SHCTX "${UNINSTKEY}" ""
  ReadRegStr $R1 SHCTX "${UNINSTKEY}" "UninstallString"
  ${IfThen} "$R0$R1" == "" ${|} Abort ${|}

  compare_version:
  StrCpy $R4 "$(older)"
  ${If} $WixMode = 1
    ReadRegStr $R0 HKLM "$R6" "DisplayVersion"
  ${Else}
    ReadRegStr $R0 SHCTX "${UNINSTKEY}" "DisplayVersion"
  ${EndIf}
  ${IfThen} $R0 == "" ${|} StrCpy $R4 "$(unknown)" ${|}

  nsis_tauri_utils::SemverCompare "${VERSION}" $R0
  Pop $R0
  ${If} $R0 = 0
    StrCpy $R1 "$(alreadyInstalledLong)"
    StrCpy $R2 "$(addOrReinstall)"
    StrCpy $R3 "$(uninstallApp)"
    !insertmacro MUI_HEADER_TEXT "$(alreadyInstalled)" "$(chooseMaintenanceOption)"
  ${ElseIf} $R0 = 1
    StrCpy $R1 "$(olderOrUnknownVersionInstalled)"
    StrCpy $R2 "$(uninstallBeforeInstalling)"
    StrCpy $R3 "$(dontUninstall)"
    !insertmacro MUI_HEADER_TEXT "$(alreadyInstalled)" "$(choowHowToInstall)"
  ${ElseIf} $R0 = -1
    StrCpy $R1 "$(newerVersionInstalled)"
    StrCpy $R2 "$(uninstallBeforeInstalling)"
    !if "${ALLOWDOWNGRADES}" == "true"
      StrCpy $R3 "$(dontUninstall)"
    !else
      StrCpy $R3 "$(dontUninstallDowngrade)"
    !endif
    !insertmacro MUI_HEADER_TEXT "$(alreadyInstalled)" "$(choowHowToInstall)"
  ${Else}
    Abort
  ${EndIf}

  ${If} $PassiveMode = 1
    Call PageLeaveReinstall
  ${Else}
    nsDialogs::Create 1018
    Pop $R4
    ${IfThen} $(^RTL) = 1 ${|} nsDialogs::SetRTL $(^RTL) ${|}
    ${NSD_CreateLabel} 0 0 100% 24u $R1
    Pop $R1
    ${NSD_CreateRadioButton} 30u 50u -30u 8u $R2
    Pop $R2
    ${NSD_OnClick} $R2 PageReinstallUpdateSelection
    ${NSD_CreateRadioButton} 30u 70u -30u 8u $R3
    Pop $R3
    !if "${ALLOWDOWNGRADES}" == "false"
      ${IfThen} $R0 = -1 ${|} EnableWindow $R3 0 ${|}
    !endif
    ${NSD_OnClick} $R3 PageReinstallUpdateSelection
    ${If} $ReinstallPageCheck <> 2
      SendMessage $R2 ${BM_SETCHECK} ${BST_CHECKED} 0
    ${Else}
      SendMessage $R3 ${BM_SETCHECK} ${BST_CHECKED} 0
    ${EndIf}
    ${NSD_SetFocus} $R2
    nsDialogs::Show
  ${EndIf}
FunctionEnd
Function PageReinstallUpdateSelection
  ${NSD_GetState} $R2 $R1
  ${If} $R1 == ${BST_CHECKED}
    StrCpy $ReinstallPageCheck 1
  ${Else}
    StrCpy $ReinstallPageCheck 2
  ${EndIf}
FunctionEnd
Function PageLeaveReinstall
  ${NSD_GetState} $R2 $R1

  ${If} $WixMode = 1
    Goto reinst_uninstall
  ${EndIf}
  ${If} $UpdateMode = 1
    Goto reinst_done
  ${EndIf}

  ${If} $R0 = 0
    ${If} $R1 = 1
      Goto reinst_done
    ${Else}
      Goto reinst_uninstall
    ${EndIf}
  ${ElseIf} $R0 = 1
    ${If} $R1 = 1
      Goto reinst_uninstall
    ${Else}
      Goto reinst_done
    ${EndIf}
  ${ElseIf} $R0 = -1
    ${If} $R1 = 1
      Goto reinst_uninstall
    ${Else}
      Goto reinst_done
    ${EndIf}
  ${EndIf}

  reinst_uninstall:
    HideWindow
    ClearErrors
    ${If} $WixMode = 1
      ReadRegStr $R1 HKLM "$R6" "UninstallString"
      ExecWait '$R1' $0
    ${Else}
      ReadRegStr $4 SHCTX "${MANUPRODUCTKEY}" ""
      ReadRegStr $R1 SHCTX "${UNINSTKEY}" "UninstallString"
      ${IfThen} $UpdateMode = 1 ${|} StrCpy $R1 "$R1 /UPDATE" ${|}
      ${IfThen} $PassiveMode = 1 ${|} StrCpy $R1 "$R1 /P" ${|}
      StrCpy $R1 "$R1 _?=$4"
      ExecWait '$R1' $0
    ${EndIf}
    BringToFront
    ${IfThen} ${Errors} ${|} StrCpy $0 2 ${|}
    ${If} $0 <> 0
    ${OrIf} ${FileExists} "$INSTDIR\${MAINBINARYNAME}.exe"
      ${If} $WixMode = 1
      ${AndIf} $0 = 1602
        Abort
      ${EndIf}
      ${If} $0 = 1
        Abort
      ${EndIf}
      MessageBox MB_ICONEXCLAMATION "$(unableToUninstall)"
      Abort
    ${EndIf}
  reinst_done:
FunctionEnd

!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
!insertmacro MUI_PAGE_DIRECTORY

Var AppStartMenuFolder
!if "${STARTMENUFOLDER}" != ""
  !define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
  !define MUI_STARTMENUPAGE_DEFAULTFOLDER "${STARTMENUFOLDER}"
!else
  !define MUI_PAGE_CUSTOMFUNCTION_PRE Skip
!endif
!insertmacro MUI_PAGE_STARTMENU Application $AppStartMenuFolder

!insertmacro MUI_PAGE_INSTFILES

!define MUI_FINISHPAGE_NOAUTOCLOSE
!define MUI_FINISHPAGE_SHOWREADME
!define MUI_FINISHPAGE_SHOWREADME_TEXT "$(createDesktop)"
!define MUI_FINISHPAGE_SHOWREADME_FUNCTION CreateOrUpdateDesktopShortcut
!define MUI_FINISHPAGE_RUN
!define MUI_FINISHPAGE_RUN_FUNCTION RunMainBinary
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipIfPassive
!insertmacro MUI_PAGE_FINISH

Function RunMainBinary
  nsis_tauri_utils::RunAsUser "$INSTDIR\${MAINBINARYNAME}.exe" ""
FunctionEnd

; Uninstaller pages
Var DeleteAppDataCheckbox
Var DeleteAppDataCheckboxState
!define /ifndef WS_EX_LAYOUTRTL         0x00400000
!define MUI_PAGE_CUSTOMFUNCTION_SHOW un.ConfirmShow
Function un.ConfirmShow
  FindWindow $1 "#32770" "" $HWNDPARENT
  System::Call "user32::GetDpiForWindow(p r1) i .r2"
  ${If} $(^RTL) = 1
    StrCpy $3 "${__NSD_CheckBox_EXSTYLE} | ${WS_EX_LAYOUTRTL}"
    IntOp $4 50 * $2
  ${Else}
    StrCpy $3 "${__NSD_CheckBox_EXSTYLE}"
    IntOp $4 0 * $2
  ${EndIf}
  IntOp $5 100 * $2
  IntOp $6 400 * $2
  IntOp $7 25 * $2
  IntOp $4 $4 / 96
  IntOp $5 $5 / 96
  IntOp $6 $6 / 96
  IntOp $7 $7 / 96
  System::Call 'user32::CreateWindowEx(i r3, w "${__NSD_CheckBox_CLASS}", w "$(deleteAppData)", i ${__NSD_CheckBox_STYLE}, i r4, i r5, i r6, i r7, p r1, i0, i0, i0) i .s'
  Pop $DeleteAppDataCheckbox
  SendMessage $HWNDPARENT ${WM_GETFONT} 0 0 $1
  SendMessage $DeleteAppDataCheckbox ${WM_SETFONT} $1 1
FunctionEnd
!define MUI_PAGE_CUSTOMFUNCTION_LEAVE un.ConfirmLeave
Function un.ConfirmLeave
  SendMessage $DeleteAppDataCheckbox ${BM_GETCHECK} 0 0 $DeleteAppDataCheckboxState
  ; 原生确认页（兜底路径）勾选状态同步给自绘协议变量
  StrCpy $UnPurgeData $DeleteAppDataCheckboxState
FunctionEnd
!define MUI_PAGE_CUSTOMFUNCTION_PRE un.SkipIfPassive
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

; Languages
{{#each languages}}
!insertmacro MUI_LANGUAGE "{{this}}"
{{/each}}
!insertmacro MUI_RESERVEFILE_LANGDLL
{{#each language_files}}
  !include "{{this}}"
{{/each}}

; ── 静默安装 IPC 标记 ──
!macro AniBoxIpcFlag NAME
  !ifndef BUILD_UNINSTALLER
    ${If} $ipcDir != ""
      FileOpen $0 "$ipcDir\${NAME}" w
      FileClose $0
    ${EndIf}
  !endif
!macroend

; ── 静默卸载 IPC 标记（卸载器与安装器共用 flag 协议）──
; 只会在 Section Uninstall / un.* 函数里展开，因此 FileOpen 写的是卸载遍的 IPC 目录。
!macro AniBoxUnIpcFlag NAME
  ${If} $UnIpcDir != ""
    FileOpen $0 "$UnIpcDir\${NAME}" w
    FileClose $0
  ${EndIf}
!macroend

Function .onInit
  !ifndef BUILD_UNINSTALLER
    ; 交互安装：解压自绘 UI 资材并移交，本进程立即退出。
    ; 不能用 $PLUGINSDIR（NSIS 退出会清空它，运行中的 UI exe 被锁残留）。
    ${GetOptions} $CMDLINE "/_IPC=" $ipcDir
    ${IfNot} ${Silent}
      System::Call 'kernel32::GetCurrentProcessId() i.r0'
      StrCpy $uiBase "$TEMP\anibox-setup-$0"
      CreateDirectory "$uiBase"
      File /oname=$uiBase\AniBoxSetupUi.exe "${ANIBOX_STAGE}\AniBoxSetupUi.exe"
      File /oname=$uiBase\WebView2Loader.dll "${ANIBOX_STAGE}\WebView2Loader.dll"
      File /oname=$uiBase\Microsoft.Web.WebView2.Core.dll "${ANIBOX_STAGE}\Microsoft.Web.WebView2.Core.dll"
      File /oname=$uiBase\Microsoft.Web.WebView2.Wpf.dll "${ANIBOX_STAGE}\Microsoft.Web.WebView2.Wpf.dll"
      File /oname=$uiBase\icon.ico "${ANIBOX_STAGE}\icon.ico"
      File /oname=$uiBase\www.zip "${ANIBOX_STAGE}\www.zip"
      File /oname=$uiBase\app-meta.json "${ANIBOX_STAGE}\app-meta.json"
      File /oname=$uiBase\totalsize.txt "${ANIBOX_STAGE}\totalsize.txt"
      Exec '"$uiBase\AniBoxSetupUi.exe" --setup "$EXEPATH" --www "$uiBase"'
      Quit
    ${EndIf}
    ; 静默遍（被 UI 以 /S 拉起）：记 started 标记
    !insertmacro AniBoxIpcFlag "started.flag"
  !endif

  ${GetOptions} $CMDLINE "/P" $PassiveMode
  ${IfNot} ${Errors}
    StrCpy $PassiveMode 1
  ${EndIf}

  ${GetOptions} $CMDLINE "/NS" $NoShortcutMode
  ${IfNot} ${Errors}
    StrCpy $NoShortcutMode 1
  ${EndIf}

  ${GetOptions} $CMDLINE "/UPDATE" $UpdateMode
  ${IfNot} ${Errors}
    StrCpy $UpdateMode 1
  ${EndIf}

  ; 安装模式：/allusers → 1；/currentuser 或未指定 → 0
  StrCpy $InstallModeAll 0
  ${GetOptions} $CMDLINE "/allusers" $0
  ${IfNot} ${Errors}
    StrCpy $InstallModeAll 1
  ${EndIf}
  ${GetOptions} $CMDLINE "/currentuser" $0
  ${IfNot} ${Errors}
    StrCpy $InstallModeAll 0
  ${EndIf}

  ${If} $InstallModeAll = 1
    UserInfo::GetAccountType
    Pop $0
    ${If} $0 != "Admin"
      MessageBox MB_ICONSTOP "为所有用户安装需要管理员权限。$\r$\n请右键安装程序并选择「以管理员身份运行」。"
      SetErrorLevel 1
      Quit
    ${EndIf}
    SetShellVarContext all
  ${Else}
    SetShellVarContext current
  ${EndIf}

  !if "${DISPLAYLANGUAGESELECTOR}" == "true"
    !insertmacro MUI_LANGDLL_DISPLAY
  !endif

  !insertmacro SetContext

  ${If} $INSTDIR == "${PLACEHOLDER_INSTALL_DIR}"
    ${If} $InstallModeAll = 1
      ${If} ${RunningX64}
        !if "${ARCH}" == "x64"
          StrCpy $INSTDIR "$PROGRAMFILES64\${PRODUCTNAME}"
        !else if "${ARCH}" == "arm64"
          StrCpy $INSTDIR "$PROGRAMFILES64\${PRODUCTNAME}"
        !else
          StrCpy $INSTDIR "$PROGRAMFILES\${PRODUCTNAME}"
        !endif
      ${Else}
        StrCpy $INSTDIR "$PROGRAMFILES\${PRODUCTNAME}"
      ${EndIf}
    ${Else}
      StrCpy $INSTDIR "$LOCALAPPDATA\${PRODUCTNAME}"
    ${EndIf}
    Call RestorePreviousInstallLocation
  ${EndIf}
FunctionEnd

Section EarlyChecks
  !if "${ALLOWDOWNGRADES}" == "false"
  ${If} ${Silent}
    ${If} $R0 = -1
      System::Call 'kernel32::AttachConsole(i -1)i.r0'
      ${If} $0 <> 0
        System::Call 'kernel32::GetStdHandle(i -11)i.r0'
        System::call 'kernel32::SetConsoleTextAttribute(i r0, i 0x0004)'
        FileWrite $0 "$(silentDowngrades)"
      ${EndIf}
      Abort
    ${EndIf}
  ${EndIf}
  !endif
SectionEnd

Section WebView2
  ${If} ${RunningX64}
    ReadRegStr $4 HKLM "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\${WEBVIEW2APPGUID}" "pv"
  ${Else}
    ReadRegStr $4 HKLM "SOFTWARE\Microsoft\EdgeUpdate\Clients\${WEBVIEW2APPGUID}" "pv"
  ${EndIf}
  ${If} $4 == ""
    ReadRegStr $4 HKCU "SOFTWARE\Microsoft\EdgeUpdate\Clients\${WEBVIEW2APPGUID}" "pv"
  ${EndIf}

  ${If} $4 == ""
    ${If} $UpdateMode <> 1
      !if "${INSTALLWEBVIEW2MODE}" == "downloadBootstrapper"
        Delete "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        DetailPrint "$(webview2Downloading)"
        NSISdl::download "https://go.microsoft.com/fwlink/p/?LinkId=2124703" "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        Pop $0
        ${If} $0 == "success"
          DetailPrint "$(webview2DownloadSuccess)"
        ${Else}
          DetailPrint "$(webview2DownloadError)"
          Abort "$(webview2AbortError)"
        ${EndIf}
        StrCpy $6 "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        Goto install_webview2
      !endif
      !if "${INSTALLWEBVIEW2MODE}" == "embedBootstrapper"
        Delete "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        File "/oname=$TEMP\MicrosoftEdgeWebview2Setup.exe" "${WEBVIEW2BOOTSTRAPPERPATH}"
        DetailPrint "$(installingWebview2)"
        StrCpy $6 "$TEMP\MicrosoftEdgeWebview2Setup.exe"
        Goto install_webview2
      !endif
      !if "${INSTALLWEBVIEW2MODE}" == "offlineInstaller"
        Delete "$TEMP\MicrosoftEdgeWebView2RuntimeInstaller.exe"
        File "/oname=$TEMP\MicrosoftEdgeWebView2RuntimeInstaller.exe" "${WEBVIEW2INSTALLERPATH}"
        DetailPrint "$(installingWebview2)"
        StrCpy $6 "$TEMP\MicrosoftEdgeWebView2RuntimeInstaller.exe"
        Goto install_webview2
      !endif
      Goto webview2_done
      install_webview2:
        DetailPrint "$(installingWebview2)"
        ExecWait "$6 ${WEBVIEW2INSTALLERARGS} /install" $1
        ${If} $1 = 0
          DetailPrint "$(webview2InstallSuccess)"
        ${Else}
          DetailPrint "$(webview2InstallError)"
          Abort "$(webview2AbortError)"
        ${EndIf}
      webview2_done:
    ${EndIf}
  ${Else}
    !if "${MINIMUMWEBVIEW2VERSION}" != ""
      ${VersionCompare} "${MINIMUMWEBVIEW2VERSION}" "$4" $R0
      ${If} $R0 = 1
        update_webview:
          DetailPrint "$(installingWebview2)"
          ${If} ${RunningX64}
            ReadRegStr $R1 HKLM "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate" "path"
          ${Else}
            ReadRegStr $R1 HKLM "SOFTWARE\Microsoft\EdgeUpdate" "path"
          ${EndIf}
          ${If} $R1 == ""
            ReadRegStr $R1 HKCU "SOFTWARE\Microsoft\EdgeUpdate" "path"
          ${EndIf}
          ${If} $R1 != ""
            ExecWait `"$R1" /install appguid=${WEBVIEW2APPGUID}&needsadmin=true` $1
            ${If} $1 = 0
              DetailPrint "$(webview2InstallSuccess)"
            ${Else}
              MessageBox MB_ICONEXCLAMATION|MB_ABORTRETRYIGNORE "$(webview2InstallError)" IDIGNORE ignore IDRETRY update_webview
              Quit
              ignore:
            ${EndIf}
          ${EndIf}
      ${EndIf}
    !endif
  ${EndIf}
SectionEnd

Section Install
  SetOutPath $INSTDIR

  !ifmacrodef NSIS_HOOK_PREINSTALL
    !insertmacro NSIS_HOOK_PREINSTALL
  !endif

  !insertmacro CheckIfAppIsRunning "$INSTDIR\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"

  ; 开始写文件：UI 把阶段从 prepare 切到 extract
  !insertmacro AniBoxIpcFlag "extract.flag"

  File "${MAINBINARYSRCPATH}"

  {{#each resources_dirs}}
    CreateDirectory "$INSTDIR\\{{this}}"
  {{/each}}
  {{#each resources}}
    File /a "/oname={{this.[1]}}" "{{no-escape @key}}"
  {{/each}}

  {{#each binaries}}
    File /a "/oname={{this}}" "{{no-escape @key}}"
  {{/each}}

  {{#each file_associations as |association| ~}}
    {{#each association.ext as |ext| ~}}
       !insertmacro APP_ASSOCIATE "{{ext}}" "{{or association.name ext}}" "{{association-description association.description ext}}" "$INSTDIR\${MAINBINARYNAME}.exe,0" "Open with ${PRODUCTNAME}" "$INSTDIR\${MAINBINARYNAME}.exe $\"%1$\""
    {{/each}}
  {{/each}}

  {{#each deep_link_protocols as |protocol| ~}}
    WriteRegStr SHCTX "Software\Classes\\{{protocol}}" "URL Protocol" ""
    WriteRegStr SHCTX "Software\Classes\\{{protocol}}" "" "URL:${BUNDLEID} protocol"
    WriteRegStr SHCTX "Software\Classes\\{{protocol}}\DefaultIcon" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\",0"
    WriteRegStr SHCTX "Software\Classes\\{{protocol}}\shell\open\command" "" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\""
  {{/each}}

  WriteUninstaller "$INSTDIR\uninstall.exe"

  WriteRegStr SHCTX "${MANUPRODUCTKEY}" "" $INSTDIR

  ; 记录安装模式供卸载器使用（1=currentUser, 0=allUsers）
  ${If} $InstallModeAll = 1
    WriteRegDWORD SHCTX "${UNINSTKEY}" "CurrentUser" 0
  ${Else}
    WriteRegDWORD SHCTX "${UNINSTKEY}" "CurrentUser" 1
  ${EndIf}

  ReadRegStr $OldMainBinaryName SHCTX "${UNINSTKEY}" "MainBinaryName"
  ${If} $OldMainBinaryName != ""
  ${AndIf} $OldMainBinaryName != "${MAINBINARYNAME}.exe"
    Delete "$INSTDIR\$OldMainBinaryName"
  ${EndIf}
  WriteRegStr SHCTX "${UNINSTKEY}" "MainBinaryName" "${MAINBINARYNAME}.exe"

  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayName" "${PRODUCTNAME}"
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayIcon" "$\"$INSTDIR\${MAINBINARYNAME}.exe$\""
  WriteRegStr SHCTX "${UNINSTKEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr SHCTX "${UNINSTKEY}" "Publisher" "${MANUFACTURER}"
  WriteRegStr SHCTX "${UNINSTKEY}" "InstallLocation" "$\"$INSTDIR$\""
  WriteRegStr SHCTX "${UNINSTKEY}" "UninstallString" "$\"$INSTDIR\uninstall.exe$\""
  WriteRegDWORD SHCTX "${UNINSTKEY}" "NoModify" "1"
  WriteRegDWORD SHCTX "${UNINSTKEY}" "NoRepair" "1"

  ${GetSize} "$INSTDIR" "/M=uninstall.exe /S=0K /G=0" $0 $1 $2
  IntOp $0 $0 + ${ESTIMATEDSIZE}
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD SHCTX "${UNINSTKEY}" "EstimatedSize" "$0"

  !if "${HOMEPAGE}" != ""
    WriteRegStr SHCTX "${UNINSTKEY}" "URLInfoAbout" "${HOMEPAGE}"
    WriteRegStr SHCTX "${UNINSTKEY}" "URLUpdateInfo" "${HOMEPAGE}"
    WriteRegStr SHCTX "${UNINSTKEY}" "HelpLink" "${HOMEPAGE}"
  !endif

  !insertmacro MUI_STARTMENU_WRITE_BEGIN Application
    Call CreateOrUpdateStartMenuShortcut
  !insertmacro MUI_STARTMENU_WRITE_END

  ${If} $PassiveMode = 1
  ${OrIf} ${Silent}
    Call CreateOrUpdateDesktopShortcut
  ${EndIf}

  !ifmacrodef NSIS_HOOK_POSTINSTALL
    !insertmacro NSIS_HOOK_POSTINSTALL
  !endif

  ; 安装收尾：UI 据此判成功
  !insertmacro AniBoxIpcFlag "done.flag"

  ${If} $PassiveMode = 1
    SetAutoClose true
  ${EndIf}
SectionEnd

Function .onInstSuccess
  ${If} $PassiveMode = 1
  ${OrIf} ${Silent}
    ${GetOptions} $CMDLINE "/R" $R0
    ${IfNot} ${Errors}
      ${GetOptions} $CMDLINE "/ARGS" $R0
      nsis_tauri_utils::RunAsUser "$INSTDIR\${MAINBINARYNAME}.exe" "$R0"
    ${EndIf}
  ${EndIf}
FunctionEnd

Function un.onInit
  ; 先按卸载器自身目录判断安装模式（避免 HKCU/HKLM 同名键互相干扰），
  ; 目录不在默认位置时再回退到注册表 CurrentUser 值。
  StrCpy $InstallModeAll 0
  ${If} ${RunningX64}
    StrLen $1 "$PROGRAMFILES64"
    StrCpy $2 "$INSTDIR" $1
    ${If} $2 == "$PROGRAMFILES64"
      StrCpy $InstallModeAll 1
      Goto anibox_mode_done
    ${EndIf}
  ${EndIf}
  StrLen $1 "$PROGRAMFILES"
  StrCpy $2 "$INSTDIR" $1
  ${If} $2 == "$PROGRAMFILES"
    StrCpy $InstallModeAll 1
    Goto anibox_mode_done
  ${EndIf}
  StrLen $1 "$LOCALAPPDATA"
  StrCpy $2 "$INSTDIR" $1
  ${If} $2 == "$LOCALAPPDATA"
    StrCpy $InstallModeAll 0
    Goto anibox_mode_done
  ${EndIf}

  ClearErrors
  ReadRegDWORD $0 HKCU "${UNINSTKEY}" "CurrentUser"
  ${If} ${Errors}
    ClearErrors
    ReadRegDWORD $0 HKLM "${UNINSTKEY}" "CurrentUser"
  ${EndIf}
  ${If} $0 == 0
    StrCpy $InstallModeAll 1
  ${EndIf}

  anibox_mode_done:
  ${If} $InstallModeAll = 1
    SetShellVarContext all
  ${Else}
    SetShellVarContext current
  ${EndIf}

  !insertmacro SetContext
  !insertmacro MUI_UNGETLANGUAGE

  ${GetOptions} $CMDLINE "/P" $PassiveMode
  ${IfNot} ${Errors}
    StrCpy $PassiveMode 1
  ${EndIf}

  ${GetOptions} $CMDLINE "/UPDATE" $UpdateMode
  ${IfNot} ${Errors}
    StrCpy $UpdateMode 1
  ${EndIf}

  ; ── 自绘卸载 UI ─────────────────────────────────────────────────────────────
  ; 参数协议（由 SetupUi 在确认后以 /S 重新拉起时携带）：
  ;   /_UNINST=1          本遍是自绘 UI 发起的静默卸载
  ;   /_PURGE_DATA=0|1    0=保留用户数据（默认）, 1=删除 %APPDATA%/%LOCALAPPDATA%
  ;   /_IPC=<dir>         进度 flag 目录（extract.flag / purge.flag / done.flag）
  ${GetOptions} $CMDLINE "/_IPC=" $UnIpcDir
  StrCpy $UnPurgeData 0
  ClearErrors
  ${GetOptions} $CMDLINE "/_PURGE_DATA=" $0
  ${IfNot} ${Errors}
    StrCpy $UnPurgeData $0
  ${EndIf}

  ; 静默遍：先写 started 标记，供 UI 从 prepare 切到真实删除阶段
  ${If} ${Silent}
    !insertmacro AniBoxUnIpcFlag "started.flag"
  ${EndIf}

  ; 交互卸载：解压自绘 UI 到 %TEMP%\anibox-uninst-<pid>，启动后本进程立即退出，
  ; 一个原生页面都不建。升级（/UPDATE）、被动（/P）与静默（/S）不拉 UI。
  ; UI 不在 INSTDIR 内运行，因此卸载过程删除安装目录不会锁住自己。
  ${IfNot} ${Silent}
  ${AndIf} $UpdateMode <> 1
  ${AndIf} $PassiveMode <> 1
    ClearErrors
    ${GetOptions} $CMDLINE "/_UNINST=" $0
    ${If} ${Errors}
      System::Call 'kernel32::GetCurrentProcessId() i.r0'
      StrCpy $UnUiBase "$TEMP\anibox-uninst-$0"
      CreateDirectory "$UnUiBase"
      File /oname=$UnUiBase\AniBoxSetupUi.exe "${ANIBOX_STAGE}\AniBoxSetupUi.exe"
      File /oname=$UnUiBase\WebView2Loader.dll "${ANIBOX_STAGE}\WebView2Loader.dll"
      File /oname=$UnUiBase\Microsoft.Web.WebView2.Core.dll "${ANIBOX_STAGE}\Microsoft.Web.WebView2.Core.dll"
      File /oname=$UnUiBase\Microsoft.Web.WebView2.Wpf.dll "${ANIBOX_STAGE}\Microsoft.Web.WebView2.Wpf.dll"
      File /oname=$UnUiBase\icon.ico "${ANIBOX_STAGE}\icon.ico"
      File /oname=$UnUiBase\www.zip "${ANIBOX_STAGE}\www.zip"
      File /oname=$UnUiBase\app-meta.json "${ANIBOX_STAGE}\app-meta.json"

      ; E2E 钩子：把截图/自动卸载参数透传给 UI（普通用户不会带这些参数）
      StrCpy $1 ""
      ClearErrors
      ${GetOptions} $CMDLINE "/_SHOTS=" $0
      ${IfNot} ${Errors}
        StrCpy $1 '$\"--shots=$0$\"'
      ${EndIf}
      ClearErrors
      ${GetOptions} $CMDLINE "/_AUTO_UNINST=" $0
      ${IfNot} ${Errors}
        StrCpy $1 "$1 --auto-uninstall"
      ${EndIf}
      ClearErrors
      ${GetOptions} $CMDLINE "/_AUTO_EXIT=" $0
      ${IfNot} ${Errors}
        StrCpy $1 "$1 --auto-exit"
      ${EndIf}

      Exec '"$UnUiBase\AniBoxSetupUi.exe" --uninstall --exe "$INSTDIR\uninstall.exe" --dir "$INSTDIR" --www "$UnUiBase" --version "${VERSION}" --purge=$UnPurgeData $1'
      Quit
    ${EndIf}
  ${EndIf}
FunctionEnd

Section Uninstall
  !ifmacrodef NSIS_HOOK_PREUNINSTALL
    !insertmacro NSIS_HOOK_PREUNINSTALL
  !endif

  ; 自绘 UI 据此把阶段从 prepare 切到「正在删除应用文件」
  !insertmacro AniBoxUnIpcFlag "extract.flag"

  !insertmacro CheckIfAppIsRunning "$INSTDIR\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"

  Delete "$INSTDIR\${MAINBINARYNAME}.exe"

  {{#each resources}}
    Delete "$INSTDIR\\{{this.[1]}}"
  {{/each}}

  {{#each binaries}}
    Delete "$INSTDIR\\{{this}}"
  {{/each}}

  {{#each file_associations as |association| ~}}
    {{#each association.ext as |ext| ~}}
      !insertmacro APP_UNASSOCIATE "{{ext}}" "{{or association.name ext}}"
    {{/each}}
  {{/each}}

  {{#each deep_link_protocols as |protocol| ~}}
    ReadRegStr $R7 SHCTX "Software\Classes\\{{protocol}}\shell\open\command" ""
    ${If} $R7 == "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\""
      DeleteRegKey SHCTX "Software\Classes\\{{protocol}}"
    ${EndIf}
  {{/each}}

  Delete "$INSTDIR\uninstall.exe"

  {{#each resources_ancestors}}
  RMDir /REBOOTOK "$INSTDIR\\{{this}}"
  {{/each}}
  RMDir "$INSTDIR"

  ${If} $UpdateMode <> 1
    !insertmacro DeleteAppUserModelId

    !insertmacro MUI_STARTMENU_GETFOLDER Application $AppStartMenuFolder
    !insertmacro IsShortcutTarget "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Pop $0
    ${If} $0 = 1
      !insertmacro UnpinShortcut "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk"
      Delete "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk"
      RMDir "$SMPROGRAMS\$AppStartMenuFolder"
    ${EndIf}
    !insertmacro IsShortcutTarget "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Pop $0
    ${If} $0 = 1
      !insertmacro UnpinShortcut "$SMPROGRAMS\${PRODUCTNAME}.lnk"
      Delete "$SMPROGRAMS\${PRODUCTNAME}.lnk"
    ${EndIf}
    !insertmacro IsShortcutTarget "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Pop $0
    ${If} $0 = 1
      !insertmacro UnpinShortcut "$DESKTOP\${PRODUCTNAME}.lnk"
      Delete "$DESKTOP\${PRODUCTNAME}.lnk"
    ${EndIf}
  ${EndIf}

  DeleteRegKey SHCTX "${UNINSTKEY}"

  ${If} $UpdateMode <> 1
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${PRODUCTNAME}"
  ${EndIf}

  ${If} $UnPurgeData = 1
  ${AndIf} $UpdateMode <> 1
    ; UI 在删除用户数据前切到「正在删除用户数据」阶段
    !insertmacro AniBoxUnIpcFlag "purge.flag"
    DeleteRegKey SHCTX "${MANUPRODUCTKEY}"
    DeleteRegKey /ifempty SHCTX "${MANUKEY}"

    DeleteRegValue HKCU "${MANUPRODUCTKEY}" "Installer Language"
    DeleteRegKey /ifempty HKCU "${MANUPRODUCTKEY}"
    DeleteRegKey /ifempty HKCU "${MANUKEY}"

    ; 用户数据始终属于当前用户，不能跟随 allUsers 的 $APPDATA
    SetShellVarContext current
    RmDir /r "$APPDATA\${BUNDLEID}"
    RmDir /r "$LOCALAPPDATA\${BUNDLEID}"
  ${EndIf}

  !ifmacrodef NSIS_HOOK_POSTUNINSTALL
    !insertmacro NSIS_HOOK_POSTUNINSTALL
  !endif

  ; 卸载收尾：UI 据此判成功（必须在所有删除动作之后）
  !insertmacro AniBoxUnIpcFlag "done.flag"

  ${If} $PassiveMode = 1
  ${OrIf} $UpdateMode = 1
    SetAutoClose true
  ${EndIf}
SectionEnd

Function RestorePreviousInstallLocation
  ReadRegStr $4 SHCTX "${MANUPRODUCTKEY}" ""
  StrCmp $4 "" +2 0
    StrCpy $INSTDIR $4
FunctionEnd

Function Skip
  Abort
FunctionEnd

Function SkipIfPassive
  ${IfThen} $PassiveMode = 1  ${|} Abort ${|}
FunctionEnd
Function un.SkipIfPassive
  ${IfThen} $PassiveMode = 1  ${|} Abort ${|}
FunctionEnd

Function CreateOrUpdateStartMenuShortcut
  StrCpy $R0 0

  !insertmacro IsShortcutTarget "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk" "$INSTDIR\$OldMainBinaryName"
  Pop $0
  ${If} $0 = 1
    !insertmacro SetShortcutTarget "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    StrCpy $R0 1
  ${EndIf}

  !insertmacro IsShortcutTarget "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\$OldMainBinaryName"
  Pop $0
  ${If} $0 = 1
    !insertmacro SetShortcutTarget "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    StrCpy $R0 1
  ${EndIf}

  ${If} $R0 = 1
    Return
  ${EndIf}

  ${If} $WixMode = 0
    ${If} $UpdateMode = 1
    ${OrIf} $NoShortcutMode = 1
      Return
    ${EndIf}
  ${EndIf}

  !if "${STARTMENUFOLDER}" != ""
    CreateDirectory "$SMPROGRAMS\$AppStartMenuFolder"
    CreateShortcut "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    !insertmacro SetLnkAppUserModelId "$SMPROGRAMS\$AppStartMenuFolder\${PRODUCTNAME}.lnk"
  !else
    CreateShortcut "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    !insertmacro SetLnkAppUserModelId "$SMPROGRAMS\${PRODUCTNAME}.lnk"
  !endif
FunctionEnd

Function CreateOrUpdateDesktopShortcut
  !insertmacro IsShortcutTarget "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\$OldMainBinaryName"
  Pop $0
  ${If} $0 = 1
    !insertmacro SetShortcutTarget "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Return
  ${EndIf}

  ${If} $WixMode = 0
    ${If} $UpdateMode = 1
    ${OrIf} $NoShortcutMode = 1
      Return
    ${EndIf}
  ${EndIf}

  CreateShortcut "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
  !insertmacro SetLnkAppUserModelId "$DESKTOP\${PRODUCTNAME}.lnk"
FunctionEnd
