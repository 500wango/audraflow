!include LogicLib.nsh
!include FileFunc.nsh

; ── Stop running AudraFlow processes before overwriting files ──────────
!macro NSIS_HOOK_PREINSTALL
  DetailPrint "Stopping running AudraFlow processes..."
  nsExec::ExecToLog 'taskkill /f /im AudraFlow.exe 2>nul'
  nsExec::ExecToLog 'taskkill /f /im audraflow-orchestrator.exe 2>nul'
  nsExec::ExecToLog 'taskkill /f /im audraflow-asr-runtime.exe 2>nul'
  nsExec::ExecToLog 'taskkill /f /im audraflow-whisper-cli.exe 2>nul'
  nsExec::ExecToLog 'taskkill /f /im whisper-cli.exe 2>nul'
  Sleep 1500
!macroend

!macro AUDRAFLOW_INSTALL_VC_REDIST_X64
  SetRegView 64
  ClearErrors
  ReadRegDWORD $R0 HKLM "SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64" "Installed"
  ${If} ${Errors}
  ${OrIf} $R0 != 1
    DetailPrint "Microsoft Visual C++ Runtime x64 was not detected."
    ; Prefer a VC redist shipped by Tauri bundleVCRuntime, if present.
    ${If} ${FileExists} "$INSTDIR\vc_redist.x64.exe"
      DetailPrint "Installing bundled Microsoft Visual C++ Redistributable x64..."
      ExecWait `"$INSTDIR\vc_redist.x64.exe" /install /quiet /norestart` $R2
    ${ElseIf} ${FileExists} "$INSTDIR\resources\vc_redist.x64.exe"
      DetailPrint "Installing bundled Microsoft Visual C++ Redistributable x64..."
      ExecWait `"$INSTDIR\resources\vc_redist.x64.exe" /install /quiet /norestart` $R2
    ${Else}
      DetailPrint "Downloading Microsoft Visual C++ Redistributable x64..."
      Delete "$TEMP\audraflow-vc_redist.x64.exe"
      ExecWait `powershell.exe -NoProfile -ExecutionPolicy Bypass -Command "try { [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12; Invoke-WebRequest -UseBasicParsing -Uri 'https://aka.ms/vc14/vc_redist.x64.exe' -OutFile '$TEMP\audraflow-vc_redist.x64.exe'; exit 0 } catch { exit 1 }"` $R1
      ${If} $R1 == 0
        DetailPrint "Installing Microsoft Visual C++ Redistributable x64..."
        ExecWait `"$TEMP\audraflow-vc_redist.x64.exe" /install /quiet /norestart` $R2
        Delete "$TEMP\audraflow-vc_redist.x64.exe"
      ${Else}
        DetailPrint "Could not download Microsoft Visual C++ Redistributable x64. Runtime components can still be installed from Settings after VC++ is available."
        StrCpy $R2 1
      ${EndIf}
    ${EndIf}
    ${If} $R2 == 0
      DetailPrint "Microsoft Visual C++ Redistributable x64 installed."
    ${ElseIf} $R2 == 3010
      DetailPrint "Microsoft Visual C++ Redistributable x64 installed. A restart may be required."
    ${ElseIf} $R2 == 1638
      DetailPrint "Microsoft Visual C++ Redistributable x64 is already present (newer or same version)."
    ${Else}
      DetailPrint "Microsoft Visual C++ Redistributable x64 installer exited with code $R2."
    ${EndIf}
  ${Else}
    DetailPrint "Microsoft Visual C++ Runtime x64 is already installed."
  ${EndIf}
  SetRegView lastused
!macroend

; Resolve the first existing source path among known install layouts into $R9.
; Callers set candidate list via sequential checks.
!macro AUDRAFLOW_FIND_FILE _outvar _a _b _c _d
  StrCpy ${_outvar} ""
  ${If} ${FileExists} `${_a}`
    StrCpy ${_outvar} `${_a}`
  ${ElseIf} ${FileExists} `${_b}`
    StrCpy ${_outvar} `${_b}`
  ${ElseIf} ${FileExists} `${_c}`
    StrCpy ${_outvar} `${_c}`
  ${ElseIf} ${FileExists} `${_d}`
    StrCpy ${_outvar} `${_d}`
  ${EndIf}
!macroend

; ── Pre-install bundled whisper runtime to managed component path ──────
; App first-run seed also performs this; NSIS preinstall makes first launch faster
; and works even if the user never opens Settings.
!macro AUDRAFLOW_PREINSTALL_WHISPER_RUNTIME
  !insertmacro AUDRAFLOW_FIND_FILE $R9 \
    "$INSTDIR\windows-runtime\whisper-cli.exe" \
    "$INSTDIR\resources\windows-runtime\whisper-cli.exe" \
    "$INSTDIR\whisper-cli.exe" \
    "$INSTDIR\audraflow-whisper-cli.exe"

  ${If} $R9 != ""
    DetailPrint "Pre-installing bundled Whisper runtime component..."
    CreateDirectory "$APPDATA\com.audraflow.app\runtime\components\whisper\bin"

    ; Prefer full windows-runtime directory when available.
    ${If} ${FileExists} "$INSTDIR\windows-runtime\whisper-cli.exe"
      CopyFiles /SILENT "$INSTDIR\windows-runtime\*.*" "$APPDATA\com.audraflow.app\runtime\components\whisper\bin"
    ${ElseIf} ${FileExists} "$INSTDIR\resources\windows-runtime\whisper-cli.exe"
      CopyFiles /SILENT "$INSTDIR\resources\windows-runtime\*.*" "$APPDATA\com.audraflow.app\runtime\components\whisper\bin"
    ${Else}
      ; Fallback: copy the resolved CLI and any co-located DLLs from $INSTDIR.
      CopyFiles /SILENT "$R9" "$APPDATA\com.audraflow.app\runtime\components\whisper\bin\whisper-cli.exe"
      ${If} ${FileExists} "$INSTDIR\whisper.dll"
        CopyFiles /SILENT "$INSTDIR\whisper.dll" "$APPDATA\com.audraflow.app\runtime\components\whisper\bin\whisper.dll"
      ${EndIf}
      ${If} ${FileExists} "$INSTDIR\ggml.dll"
        CopyFiles /SILENT "$INSTDIR\ggml.dll" "$APPDATA\com.audraflow.app\runtime\components\whisper\bin\ggml.dll"
      ${EndIf}
      ${If} ${FileExists} "$INSTDIR\ggml-base.dll"
        CopyFiles /SILENT "$INSTDIR\ggml-base.dll" "$APPDATA\com.audraflow.app\runtime\components\whisper\bin\ggml-base.dll"
      ${EndIf}
      ${If} ${FileExists} "$INSTDIR\ggml-cpu.dll"
        CopyFiles /SILENT "$INSTDIR\ggml-cpu.dll" "$APPDATA\com.audraflow.app\runtime\components\whisper\bin\ggml-cpu.dll"
      ${EndIf}
    ${EndIf}

    ; Normalize CLI name expected by the app.
    ${If} ${FileExists} "$APPDATA\com.audraflow.app\runtime\components\whisper\bin\audraflow-whisper-cli.exe"
      ${IfNot} ${FileExists} "$APPDATA\com.audraflow.app\runtime\components\whisper\bin\whisper-cli.exe"
        CopyFiles /SILENT "$APPDATA\com.audraflow.app\runtime\components\whisper\bin\audraflow-whisper-cli.exe" "$APPDATA\com.audraflow.app\runtime\components\whisper\bin\whisper-cli.exe"
      ${EndIf}
    ${EndIf}
    DetailPrint "Whisper runtime component pre-installed."
  ${Else}
    DetailPrint "Bundled Whisper runtime not found; install from Settings after launch."
  ${EndIf}
!macroend

!macro AUDRAFLOW_PREINSTALL_FFMPEG_RUNTIME
  !insertmacro AUDRAFLOW_FIND_FILE $R8 \
    "$INSTDIR\audraflow-ffmpeg.exe" \
    "$INSTDIR\ffmpeg.exe" \
    "$INSTDIR\windows-runtime\ffmpeg.exe" \
    "$INSTDIR\resources\windows-runtime\ffmpeg.exe"

  !insertmacro AUDRAFLOW_FIND_FILE $R7 \
    "$INSTDIR\audraflow-ffprobe.exe" \
    "$INSTDIR\ffprobe.exe" \
    "$INSTDIR\windows-runtime\ffprobe.exe" \
    "$INSTDIR\resources\windows-runtime\ffprobe.exe"

  ${If} $R8 != ""
  ${AndIf} $R7 != ""
    DetailPrint "Pre-installing bundled FFmpeg runtime component..."
    CreateDirectory "$APPDATA\com.audraflow.app\runtime\components\ffmpeg\bin"
    CopyFiles /SILENT "$R8" "$APPDATA\com.audraflow.app\runtime\components\ffmpeg\bin\ffmpeg.exe"
    CopyFiles /SILENT "$R7" "$APPDATA\com.audraflow.app\runtime\components\ffmpeg\bin\ffprobe.exe"
    DetailPrint "FFmpeg runtime component pre-installed."
  ${Else}
    DetailPrint "Bundled FFmpeg tools not found; install from Settings after launch."
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro AUDRAFLOW_INSTALL_VC_REDIST_X64
  !insertmacro AUDRAFLOW_PREINSTALL_WHISPER_RUNTIME
  !insertmacro AUDRAFLOW_PREINSTALL_FFMPEG_RUNTIME
!macroend
