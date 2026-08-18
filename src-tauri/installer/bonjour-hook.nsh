; Bonjour bootstrap for Mirrwin's NSIS installer.
;
; iPhones find this machine over mDNS, which on Windows means Apple's Bonjour:
; the protocol library LoadLibrary's dnssd.dll and talks to mDNSResponder.exe.
; Without it the receiver starts happily and is simply never discovered, which
; is why the app also reports the missing dependency at runtime (error -3 in
; tools/airplay-dll/Bridge.cpp).
;
; This hook only ever ADDS Bonjour when it is absent, and asks first. There is
; deliberately no uninstall counterpart: Bonjour is a shared system component
; and anything else on the machine — iTunes, printer drivers, other AirPlay
; receivers — may be relying on it.

!include LogicLib.nsh
!include x64.nsh

; Capture this file's own directory NOW, while the file is being included.
; Inside the macro body it would be too late: NSIS expands ${__FILEDIR__} where
; the macro is *inserted*, which is the generated installer.nsi in Tauri's build
; directory, not here.
!ifndef BONJOUR_SRC_DIR
  !define BONJOUR_SRC_DIR "${__FILEDIR__}"
!endif

!macro NSIS_HOOK_POSTINSTALL
  Push $R0
  Push $R1

  ; Look for the 64-bit dnssd.dll, the one the app actually loads. NSIS
  ; installers are 32-bit, so $SYSDIR points at SysWOW64 and file-system
  ; redirection has to come off for the check to mean anything.
  StrCpy $R0 "present"
  ${If} ${RunningX64}
    ${DisableX64FSRedirection}
    ${IfNot} ${FileExists} "$WINDIR\System32\dnssd.dll"
      StrCpy $R0 "missing"
    ${EndIf}
    ${EnableX64FSRedirection}
  ${Else}
    ${IfNot} ${FileExists} "$SYSDIR\dnssd.dll"
      StrCpy $R0 "missing"
    ${EndIf}
  ${EndIf}

  ${If} $R0 == "missing"
    ; /SD IDYES: an unattended install still gets a working receiver.
    MessageBox MB_YESNO|MB_ICONQUESTION \
      "Mirrwin 需要 Apple Bonjour，iPhone 才能在「屏幕镜像」里发现这台电脑。$\r$\n$\r$\n\
       本机未检测到 Bonjour，是否现在安装？（需要管理员权限）" \
      /SD IDYES IDNO bonjour_skip

    ; Extract by its own name rather than /oname: $PLUGINSDIR sits under the
    ; user's TEMP, which contains a space whenever their account name does.
    InitPluginsDir
    SetOutPath $PLUGINSDIR
    File "${BONJOUR_SRC_DIR}\bonjour.msi"
    SetOutPath $INSTDIR
    DetailPrint "正在安装 Apple Bonjour…"

    ; Hand the 64-bit package to the 64-bit msiexec. Reaching it from a 32-bit
    ; installer means naming System32 with redirection switched off. A silent
    ; install of Mirrwin stays silent; otherwise show progress.
    StrCpy $R1 "/passive"
    ${If} ${Silent}
      StrCpy $R1 "/qn"
    ${EndIf}
    ${If} ${RunningX64}
      ${DisableX64FSRedirection}
      ExecWait '"$WINDIR\System32\msiexec.exe" /i "$PLUGINSDIR\bonjour.msi" $R1 /norestart' $R1
      ${EnableX64FSRedirection}
    ${Else}
      ExecWait '"$SYSDIR\msiexec.exe" /i "$PLUGINSDIR\bonjour.msi" $R1 /norestart' $R1
    ${EndIf}

    ; Mirrwin itself installs fine either way, so a refused elevation or a
    ; failed package is a warning, not an aborted install.
    ${If} $R1 != 0
      DetailPrint "Bonjour installer returned $R1"
      MessageBox MB_OK|MB_ICONEXCLAMATION \
        "Bonjour 未安装成功（msiexec 返回代码 $R1）。$\r$\n$\r$\n\
         Mirrwin 已安装完成，但在装好 Bonjour 之前 iPhone 搜索不到本机。$\r$\n\
         可稍后自行安装 Bonjour，或重新运行本安装程序。" \
        /SD IDOK
    ${EndIf}

    bonjour_skip:
  ${EndIf}

  Pop $R1
  Pop $R0
!macroend
