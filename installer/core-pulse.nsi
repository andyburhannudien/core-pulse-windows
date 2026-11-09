; core-pulse per-user installer.
;
; Design rules:
;   - RequestExecutionLevel user: no UAC, ever.
;   - Install under %LOCALAPPDATA%\Programs\core-pulse.
;   - Uninstall entry under HKCU, visible in Settings > Apps.
;   - User data in %LOCALAPPDATA%\core-pulse is preserved on uninstall.
;
; Build with scripts\make-installer.ps1, which passes APP_VERSION, OUTFILE,
; ICON, SRC_DIR, DOCS_DIR and README as /D defines.

Unicode true

!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "LogicLib.nsh"

!ifndef APP_VERSION
    !define APP_VERSION "0.1.0"
!endif

!ifndef OUTFILE
    !define OUTFILE "..\artifacts\core-pulse-${APP_VERSION}-windows-x64-setup.exe"
!endif

!ifndef SRC_DIR
    !define SRC_DIR "..\target\release"
!endif

!ifndef DOCS_DIR
    !define DOCS_DIR "..\docs"
!endif

!ifndef README
    !define README "..\README.md"
!endif

Name "core-pulse ${APP_VERSION}"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Programs\core-pulse"
InstallDirRegKey HKCU "Software\core-pulse" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma
SetCompressorDictSize 64
BrandingText "core-pulse"

VIProductVersion "${APP_VERSION}.0"
VIAddVersionKey /LANG=1033 "ProductName" "core-pulse"
VIAddVersionKey /LANG=1033 "FileDescription" "core-pulse system monitor installer"
VIAddVersionKey /LANG=1033 "FileVersion" "${APP_VERSION}"
VIAddVersionKey /LANG=1033 "ProductVersion" "${APP_VERSION}"
VIAddVersionKey /LANG=1033 "LegalCopyright" "MIT OR Apache-2.0"

!define MUI_ABORTWARNING
!ifdef ICON
    !define MUI_ICON "${ICON}"
    !define MUI_UNICON "${ICON}"
!endif

!define MUI_FINISHPAGE_RUN "$INSTDIR\core-pulse.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Run core-pulse"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Function .onInit
    SetShellVarContext current

    FindWindow $0 "" "core-pulse"
    StrCmp $0 0 done
    MessageBox MB_OK|MB_ICONEXCLAMATION "core-pulse is running. Close it before installing."
    Abort

done:
FunctionEnd

Function un.onInit
    SetShellVarContext current

    FindWindow $0 "" "core-pulse"
    StrCmp $0 0 done
    MessageBox MB_OK|MB_ICONEXCLAMATION "core-pulse is running. Close it before uninstalling."
    Abort

done:
FunctionEnd

Section "core-pulse (required)" SEC_MAIN
    SectionIn RO
    SetShellVarContext current

    SetOutPath "$INSTDIR"
    File "${SRC_DIR}\core-pulse.exe"
    File "${SRC_DIR}\core-pulse-cli.exe"
    File "${SRC_DIR}\core-pulse-helper.exe"
    File "${README}"
    !ifdef ICON
        File "${ICON}"
    !endif

    SetOutPath "$INSTDIR\docs"
    File /r "${DOCS_DIR}\*.*"
    SetOutPath "$INSTDIR"

    WriteUninstaller "$INSTDIR\uninstall.exe"

    CreateDirectory "$SMPROGRAMS\core-pulse"
    !ifdef ICON
        CreateShortcut "$SMPROGRAMS\core-pulse\core-pulse.lnk" "$INSTDIR\core-pulse.exe" "" "$INSTDIR\core-pulse.ico"
    !else
        CreateShortcut "$SMPROGRAMS\core-pulse\core-pulse.lnk" "$INSTDIR\core-pulse.exe"
    !endif
    CreateShortcut "$SMPROGRAMS\core-pulse\core-pulse CLI.lnk" "$INSTDIR\core-pulse-cli.exe"
    CreateShortcut "$SMPROGRAMS\core-pulse\Uninstall core-pulse.lnk" "$INSTDIR\uninstall.exe"

    WriteRegStr HKCU "Software\core-pulse" "InstallDir" "$INSTDIR"

    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\core-pulse" "DisplayName" "core-pulse"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\core-pulse" "DisplayVersion" "${APP_VERSION}"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\core-pulse" "Publisher" "core-pulse"
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\core-pulse" "InstallLocation" "$INSTDIR"
    !ifdef ICON
        WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\core-pulse" "DisplayIcon" "$INSTDIR\core-pulse.ico"
    !else
        WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\core-pulse" "DisplayIcon" "$INSTDIR\core-pulse.exe"
    !endif
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\core-pulse" "UninstallString" '"$INSTDIR\uninstall.exe"'
    WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\core-pulse" "NoModify" 1
    WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\core-pulse" "NoRepair" 1

    ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
    IntFmt $0 "0x%08X" $0
    WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\core-pulse" "EstimatedSize" "$0"
SectionEnd

Section /o "Desktop shortcut" SEC_DESKTOP
    SetShellVarContext current
    !ifdef ICON
        CreateShortcut "$DESKTOP\core-pulse.lnk" "$INSTDIR\core-pulse.exe" "" "$INSTDIR\core-pulse.ico"
    !else
        CreateShortcut "$DESKTOP\core-pulse.lnk" "$INSTDIR\core-pulse.exe"
    !endif
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
    !insertmacro MUI_DESCRIPTION_TEXT ${SEC_MAIN} "The application, CLI, elevated helper, and documentation."
    !insertmacro MUI_DESCRIPTION_TEXT ${SEC_DESKTOP} "Create a shortcut on the desktop."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section "Uninstall"
    SetShellVarContext current

    Delete "$INSTDIR\core-pulse.exe"
    Delete "$INSTDIR\core-pulse-cli.exe"
    Delete "$INSTDIR\core-pulse-helper.exe"
    Delete "$INSTDIR\core-pulse.ico"
    Delete "$INSTDIR\README.md"
    Delete "$INSTDIR\uninstall.exe"
    RMDir /r "$INSTDIR\docs"
    RMDir "$INSTDIR"

    Delete "$SMPROGRAMS\core-pulse\core-pulse.lnk"
    Delete "$SMPROGRAMS\core-pulse\core-pulse CLI.lnk"
    Delete "$SMPROGRAMS\core-pulse\Uninstall core-pulse.lnk"
    RMDir "$SMPROGRAMS\core-pulse"

    Delete "$DESKTOP\core-pulse.lnk"

    DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\core-pulse"
    DeleteRegKey HKCU "Software\core-pulse"

    ; %LOCALAPPDATA%\core-pulse (logs and history) is intentionally preserved.
SectionEnd
