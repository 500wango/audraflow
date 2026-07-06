!include LogicLib.nsh

!macro AUDRAFLOW_INSTALL_VC_REDIST_X64
  SetRegView 64
  ClearErrors
  ReadRegDWORD $R0 HKLM "SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64" "Installed"
  ${If} ${Errors}
  ${OrIf} $R0 != 1
    DetailPrint "Microsoft Visual C++ Runtime x64 was not detected."
    DetailPrint "Starting Microsoft Visual C++ Redistributable x64 background install..."
    Delete "$TEMP\audraflow-vc_redist.x64.exe"
    Exec `powershell.exe -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -Command "try { [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; Invoke-WebRequest -UseBasicParsing -Uri 'https://aka.ms/vc14/vc_redist.x64.exe' -OutFile '$TEMP\audraflow-vc_redist.x64.exe'; Start-Process -FilePath '$TEMP\audraflow-vc_redist.x64.exe' -ArgumentList '/install','/quiet','/norestart' -Wait; Remove-Item -LiteralPath '$TEMP\audraflow-vc_redist.x64.exe' -Force -ErrorAction SilentlyContinue } catch { }"`
    DetailPrint "AudraFlow will use bundled runtime DLLs while the background installer runs."
  ${Else}
    DetailPrint "Microsoft Visual C++ Runtime x64 is already installed."
  ${EndIf}
  SetRegView lastused
!macroend

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro AUDRAFLOW_INSTALL_VC_REDIST_X64
!macroend
