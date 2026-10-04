; qsketch Windows installer (NSIS, Modern UI 2)
;
; Build with:
;   makensis /DVERSION=1.2.3 installer\qsketch.nsi
;
; Expects scripts/build-windows.sh to have already staged the release build into
; dist/windows/ (qsketch.exe, LICENSE-MIT, LICENSE-APACHE, README.md) relative to the
; repo root -- see STAGE_DIR below. Produces dist\qsketch-<version>-setup.exe.
;
; Supports silent installation (installer /S) and silent uninstallation
; (uninstall.exe /S), which NSIS/MUI2 provide out of the box as long as no custom
; nsDialogs pages are used (this script only uses stock MUI2 + Components pages).

Unicode true

!ifndef VERSION
  !define VERSION "0.0.0"
!endif

!define APP_NAME "qsketch"
!define APP_EXE "qsketch.exe"
!define COMPANY_NAME "Cabbit-Labs"
!define APP_URL "https://github.com/Cabbit-Labs/qsketch"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_NAME}"
!define STAGE_DIR "..\dist\windows"
!define ICON_FILE "..\assets\icon\icon.ico"
; Explorer thumbnail provider (crates/qsketch-thumb). THUMB_CLSID must match
; the CLSID in that crate; SHELLEX_THUMB is the shell's IThumbnailProvider
; handler category.
!define THUMB_DLL "qsketch_thumb.dll"
!define THUMB_CLSID "{9B1F2A6E-5C3D-4E7A-8F41-2D6C0B7E3A55}"
!define SHELLEX_THUMB "{E357FCCD-A995-4576-B01F-234630154E96}"
!define APPROVED_KEY "Software\Microsoft\Windows\CurrentVersion\Shell Extensions\Approved"

Name "${APP_NAME}"
OutFile "..\dist\qsketch-${VERSION}-setup.exe"
InstallDir "$PROGRAMFILES64\${APP_NAME}"
InstallDirRegKey HKLM "${UNINST_KEY}" "InstallLocation"
RequestExecutionLevel admin
ShowInstDetails show
ShowUnInstDetails show

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "${APP_NAME}"
VIAddVersionKey "CompanyName" "${COMPANY_NAME}"
VIAddVersionKey "FileDescription" "${APP_NAME} installer"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "Copyright (C) 2026 ${COMPANY_NAME} and qsketch contributors"

;--------------------------------
; Modern UI 2

!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "x64.nsh"

!define MUI_ABORTWARNING
!define MUI_ICON "${ICON_FILE}"
!define MUI_UNICON "${ICON_FILE}"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "${STAGE_DIR}\LICENSE-MIT"
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_RUN "$INSTDIR\${APP_EXE}"
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

;--------------------------------
; Init

Function .onInit
  ; The app is 64-bit only; make sure registry writes go to the native 64-bit view
  ; instead of the 32-bit Wow6432Node redirection, and bail out on 32-bit Windows.
  ${IfNot} ${RunningX64}
    MessageBox MB_OK|MB_ICONSTOP "${APP_NAME} requires 64-bit Windows."
    Abort
  ${EndIf}
  SetRegView 64
FunctionEnd

Function un.onInit
  SetRegView 64
FunctionEnd

;--------------------------------
; Install

Section "qsketch (required)" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"
  File "${STAGE_DIR}\${APP_EXE}"
  File /nonfatal "${STAGE_DIR}\LICENSE-MIT"
  File /nonfatal "${STAGE_DIR}\LICENSE-APACHE"
  File /nonfatal "${STAGE_DIR}\THIRD_PARTY_NOTICES.md"
  File /nonfatal "${STAGE_DIR}\README.md"

  ; The thumbnail DLL may be loaded by Explorer's surrogate process during an
  ; upgrade; a loaded DLL can be renamed but not overwritten, so move the old
  ; copy aside first and let it go at the next reboot.
  IfFileExists "$INSTDIR\${THUMB_DLL}" 0 +3
    Delete "$INSTDIR\${THUMB_DLL}.old"
    Rename "$INSTDIR\${THUMB_DLL}" "$INSTDIR\${THUMB_DLL}.old"
  Delete /REBOOTOK "$INSTDIR\${THUMB_DLL}.old"
  File /nonfatal "${STAGE_DIR}\${THUMB_DLL}"

  ; Start Menu shortcuts
  CreateDirectory "$SMPROGRAMS\${APP_NAME}"
  CreateShortCut "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}"
  CreateShortCut "$SMPROGRAMS\${APP_NAME}\Uninstall ${APP_NAME}.lnk" "$INSTDIR\uninstall.exe"

  ; .qsk file association
  WriteRegStr HKLM "Software\Classes\.qsk" "" "qsketch.Document"
  WriteRegStr HKLM "Software\Classes\.qsk" "Content Type" "application/x-qsketch"
  WriteRegStr HKLM "Software\Classes\qsketch.Document" "" "qsketch Document"
  WriteRegStr HKLM "Software\Classes\qsketch.Document\DefaultIcon" "" "$INSTDIR\${APP_EXE},0"
  WriteRegStr HKLM "Software\Classes\qsketch.Document\shell\open\command" "" '"$INSTDIR\${APP_EXE}" "%1"'

  ; Explorer thumbnails for .qsk: the provider DLL draws the preview stored
  ; inside the file. Registered on the extension and on the ProgID, so the
  ; thumbnails survive another program claiming .qsk.
  ${If} ${FileExists} "$INSTDIR\${THUMB_DLL}"
    WriteRegStr HKLM "Software\Classes\CLSID\${THUMB_CLSID}" "" "qsketch Thumbnail Provider"
    WriteRegStr HKLM "Software\Classes\CLSID\${THUMB_CLSID}\InprocServer32" "" "$INSTDIR\${THUMB_DLL}"
    WriteRegStr HKLM "Software\Classes\CLSID\${THUMB_CLSID}\InprocServer32" "ThreadingModel" "Apartment"
    WriteRegStr HKLM "Software\Classes\.qsk\ShellEx\${SHELLEX_THUMB}" "" "${THUMB_CLSID}"
    WriteRegStr HKLM "Software\Classes\qsketch.Document\ShellEx\${SHELLEX_THUMB}" "" "${THUMB_CLSID}"
    WriteRegStr HKLM "${APPROVED_KEY}" "${THUMB_CLSID}" "qsketch Thumbnail Provider"
  ${EndIf}

  ; Add/Remove Programs entry
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${UNINST_KEY}" "Publisher" "${COMPANY_NAME}"
  WriteRegStr HKLM "${UNINST_KEY}" "URLInfoAbout" "${APP_URL}"
  WriteRegStr HKLM "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\${APP_EXE},0"
  WriteRegStr HKLM "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKLM "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  WriteRegDWORD HKLM "${UNINST_KEY}" "EstimatedSize" "$0"

  WriteUninstaller "$INSTDIR\uninstall.exe"

  ; Let the shell know a new file association exists.
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, i 0, i 0)'
SectionEnd

Section "Desktop Shortcut" SecDesktop
  CreateShortCut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}"
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMain} "The ${APP_NAME} application (required)."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} "Add a shortcut to ${APP_NAME} on the desktop."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

;--------------------------------
; Uninstall

Section "Uninstall"
  Delete "$INSTDIR\${APP_EXE}"
  Delete "$INSTDIR\LICENSE-MIT"
  Delete "$INSTDIR\LICENSE-APACHE"
  Delete "$INSTDIR\THIRD_PARTY_NOTICES.md"
  Delete "$INSTDIR\README.md"
  Delete /REBOOTOK "$INSTDIR\${THUMB_DLL}"
  Delete /REBOOTOK "$INSTDIR\${THUMB_DLL}.old"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  Delete "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\${APP_NAME}\Uninstall ${APP_NAME}.lnk"
  RMDir "$SMPROGRAMS\${APP_NAME}"
  Delete "$DESKTOP\${APP_NAME}.lnk"

  DeleteRegKey HKLM "Software\Classes\.qsk"
  DeleteRegKey HKLM "Software\Classes\qsketch.Document"
  DeleteRegKey HKLM "Software\Classes\CLSID\${THUMB_CLSID}"
  DeleteRegValue HKLM "${APPROVED_KEY}" "${THUMB_CLSID}"
  DeleteRegKey HKLM "${UNINST_KEY}"

  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, i 0, i 0)'
SectionEnd
