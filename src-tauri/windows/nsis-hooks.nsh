!include LogicLib.nsh

!macro AUDRAFLOW_INSTALL_VC_REDIST_X64
  SetRegView 64
  ClearErrors
  ReadRegDWORD $R0 HKLM "SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64" "Installed"
  ${If} ${Errors}
  ${OrIf} $R0 != 1
    DetailPrint "Microsoft Visual C++ Runtime x64 was not detected."
    DetailPrint "Downloading Microsoft Visual C++ Redistributable x64..."
    Delete "$TEMP\audraflow-vc_redist.x64.exe"
    ExecWait 'powershell.exe -NoProfile -ExecutionPolicy Bypass -Command "try { [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; Invoke-WebRequest -UseBasicParsing -Uri ''https://aka.ms/vc14/vc_redist.x64.exe'' -OutFile ''$TEMP\audraflow-vc_redist.x64.exe''; exit 0 } catch { exit 1 }"' $R1
    ${If} $R1 == 0
      DetailPrint "Installing Microsoft Visual C++ Redistributable x64..."
      ExecWait '"$TEMP\audraflow-vc_redist.x64.exe" /install /quiet /norestart' $R2
      ${If} $R2 == 0
        DetailPrint "Microsoft Visual C++ Redistributable x64 installed."
      ${ElseIf} $R2 == 3010
        DetailPrint "Microsoft Visual C++ Redistributable x64 installed. A restart may be required."
      ${Else}
        DetailPrint "Microsoft Visual C++ Redistributable x64 installer exited with code $R2. AudraFlow will use bundled runtime DLLs."
      ${EndIf}
      Delete "$TEMP\audraflow-vc_redist.x64.exe"
    ${Else}
      DetailPrint "Could not download Microsoft Visual C++ Redistributable x64. AudraFlow will use bundled runtime DLLs."
    ${EndIf}
  ${Else}
    DetailPrint "Microsoft Visual C++ Runtime x64 is already installed."
  ${EndIf}
  SetRegView lastused
!macroend

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro AUDRAFLOW_INSTALL_VC_REDIST_X64
!macroend
